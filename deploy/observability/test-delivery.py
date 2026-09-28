#!/usr/bin/env python3
"""Exercise delivery transactions in temporary directories with no host/daemon access."""

import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('delivery', Path(__file__).with_name('delivery.py'))
delivery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(delivery)


class DeliveryTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        base = Path(temporary.name)
        self.source, self.root = base / 'source', base / 'live'
        self.source.mkdir()
        self.root.mkdir()
        for name in delivery.MANAGED:
            for directory in (self.source, self.root):
                path = directory / name
                if name.endswith('.yml'):
                    path.write_text('compose')
                else:
                    path.mkdir()
                    (path / 'config').write_text('old')
        self.secret = self.root / '.env'
        self.secret.write_text('DO_NOT_REPLACE=private')
        self.installed = base / 'installed'
        self.installed.write_text('installed collector')
        self.installed.chmod(0o755)
        (self.source / 'host' / 'clumsies-observability-metrics').write_text('collector')
        (self.root / 'host' / 'clumsies-observability-metrics').write_text('collector')
        self.calls = []
        self.services = {'prometheus': {}, 'alertmanager': {}, 'grafana': {}, 'loki': {}, 'alloy': {}}
        self.addCleanup(patch.stopall)
        patch.object(delivery, 'HOST_FILES', {'clumsies-observability-metrics': self.installed}).start()
        patch.object(delivery, 'run', side_effect=self.command).start()
        self.verify = patch.object(delivery, 'verify').start()

    def command(self, *args):
        args = tuple(map(str, args))
        self.calls.append(args)
        if '--format' in args and 'json' in args:
            return json.dumps({'services': self.services})
        if 'inspect' in args:
            return 'sha256:existing-image'
        if 'ps' in args:
            return 'existing-container'
        return ''

    def deploy(self):
        delivery.deploy(self.source, self.root, 'a' * 40)

    def test_success_preserves_credentials_mounts_and_images(self):
        mounted = (self.root / 'prometheus').stat().st_ino
        (self.source / 'prometheus/config').write_text('new')
        (self.root / 'prometheus/removed').write_text('stale')
        self.deploy()
        self.assertEqual((self.root / 'prometheus/config').read_text(), 'new')
        self.assertEqual((self.root / 'prometheus').stat().st_ino, mounted)
        self.assertFalse((self.root / 'prometheus/removed').exists())
        self.assertEqual(self.secret.read_text(), 'DO_NOT_REPLACE=private')
        self.assertEqual((self.root / '.delivery/current').read_text().strip(), 'a' * 40)
        self.assertEqual((self.root / '.delivery/previous/prometheus/config').read_text(), 'old')
        self.assertTrue(any('SIGHUP' in args for args in self.calls))
        self.assertFalse(any('up' in args or 'pull' in args for args in self.calls))

    def test_failed_verification_restores_old_files_and_returns_failure(self):
        (self.source / 'prometheus/config').write_text('new')
        self.verify.side_effect = [RuntimeError('injected unhealthy targets'), None]
        with self.assertRaisesRegex(RuntimeError, 'injected'):
            self.deploy()
        self.assertEqual((self.root / 'prometheus/config').read_text(), 'old')
        self.assertFalse((self.root / '.delivery/current').exists())
        self.assertEqual(self.verify.call_count, 2)
        self.assertEqual(self.secret.read_text(), 'DO_NOT_REPLACE=private')

    def test_validation_failure_never_changes_live_configuration(self):
        (self.source / 'prometheus/config').write_text('new')
        def fail(*args):
            if 'run' in args:
                raise RuntimeError('invalid config')
            return self.command(*args)
        with patch.object(delivery, 'run', side_effect=fail):
            with self.assertRaisesRegex(RuntimeError, 'invalid config'):
                self.deploy()
        self.assertEqual((self.root / 'prometheus/config').read_text(), 'old')
        self.verify.assert_not_called()

    def test_compose_update_uses_existing_image_ids_without_pulling(self):
        (self.source / 'compose.observability.yml').write_text('changed compose')
        self.deploy()
        args = next(args for args in self.calls if 'up' in args)
        self.assertIn('--no-build', args)
        self.assertEqual(args[args.index('--pull') + 1], 'never')
        images = json.loads((self.root / '.delivery/previous/images.json').read_text())
        self.assertTrue(all(s['image'] == 'sha256:existing-image' for s in images['services'].values()))

    def test_rollback_failure_is_reported_and_backup_remains(self):
        (self.source / 'grafana/config').write_text('new')
        self.verify.side_effect = RuntimeError('unhealthy')
        with self.assertRaisesRegex(RuntimeError, 'unhealthy'):
            self.deploy()
        self.assertEqual((self.root / '.delivery/previous/grafana/config').read_text(), 'old')
        self.assertEqual((self.root / 'grafana/config').read_text(), 'old')

    def test_host_rollback_restores_the_installed_executable_not_just_repo_copy(self):
        (self.source / 'host/clumsies-observability-metrics').write_text('new collector')
        self.verify.side_effect = [RuntimeError('injected'), None]
        with self.assertRaisesRegex(RuntimeError, 'injected'):
            self.deploy()
        self.assertEqual(self.installed.read_text(), 'installed collector')
        self.assertEqual(self.installed.stat().st_mode & 0o777, 0o755)

    def test_restart_failure_also_rolls_back(self):
        (self.source / 'grafana/config').write_text('new')
        failed = False
        def command(*args):
            nonlocal failed
            if 'restart' in args and not failed:
                failed = True
                raise RuntimeError('restart failed')
            return self.command(*args)
        with patch.object(delivery, 'run', side_effect=command):
            with self.assertRaisesRegex(RuntimeError, 'restart failed'):
                self.deploy()
        self.assertEqual((self.root / 'grafana/config').read_text(), 'old')
        self.verify.assert_called_once()

    def test_symlinks_are_rejected_before_live_changes(self):
        (self.source / 'prometheus/config').unlink()
        (self.source / 'prometheus/config').symlink_to(self.secret)
        with self.assertRaisesRegex(ValueError, 'Symlink'):
            self.deploy()
        self.assertFalse(self.calls)

    def test_unchanged_configuration_is_verified_without_restart(self):
        self.deploy()
        self.verify.assert_called_once()
        self.assertFalse(any('restart' in a or 'kill' in a or 'up' in a for a in self.calls))


class VerificationTests(unittest.TestCase):
    def verify(self, *, targets=None, logs='', loki_status='success'):
        now = delivery.time.time()
        fresh = delivery.datetime.datetime.fromtimestamp(now + 1, delivery.datetime.timezone.utc).isoformat()
        if targets is None:
            targets = [{'health': 'up', 'lastScrape': fresh}]
        def response(url):
            if '/api/health' in url:
                return json.dumps({'database': 'ok'}).encode()
            if '/api/v1/targets' in url:
                return json.dumps({'data': {'activeTargets': targets}}).encode()
            if '/loki/api/' in url:
                return json.dumps({'status': loki_status, 'data': {'result': []}}).encode()
            return b'ready'
        with patch.object(delivery, 'get', side_effect=response), \
             patch.object(delivery, 'query', return_value=[{'value': [now, '1']}]), \
             patch.object(delivery.time, 'monotonic', side_effect=[0, 1, 181]), \
             patch.object(delivery.time, 'sleep'):
            delivery.verify(now, lambda *args: logs)

    def test_healthy_scrapes_and_empty_logs_are_valid(self):
        self.verify()

    def test_empty_stale_or_failed_targets_do_not_count_as_success(self):
        for targets in ([], [{'health': 'down', 'lastScrape': '2020-01-01T00:00:00Z'}],
                        [{'health': 'up', 'lastScrape': '2020-01-01T00:00:00Z'}]):
            with self.subTest(targets=targets), self.assertRaisesRegex(RuntimeError, 'timed out'):
                self.verify(targets=targets)

    def test_provisioning_and_query_errors_fail_delivery(self):
        with self.assertRaisesRegex(RuntimeError, 'Grafana provisioning'):
            self.verify(logs='logger=provisioning.dashboard level=error msg=invalid')
        with self.assertRaisesRegex(RuntimeError, 'Loki query'):
            self.verify(loki_status='error')


if __name__ == '__main__':
    unittest.main()
