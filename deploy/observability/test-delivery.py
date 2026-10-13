#!/usr/bin/env python3
"""Exercise delivery transactions in temporary directories with no host/daemon access."""

import importlib.util
import json
import re
import sqlite3
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
        # These are synthetic fixture values, never host/live credentials.
        self.notification = self.root / 'alertmanager/alertmanager.yml'
        self.notification.parent.mkdir()
        self.notification.write_text('global:\n  smtp_auth_password: fixture-private-password\nroute:\n  receiver: custom-operations\nreceivers:\n  - name: custom-operations\n')
        template = self.source / 'alertmanager'
        template.mkdir()
        (template / 'alertmanager.yml').write_text('smtp_auth_password: REPLACE_GMAIL_APP_PASSWORD\nroute: placeholder\n')
        self.notification_original = self.notification.read_bytes()
        self.secret = self.root / '.env'
        self.secret.write_text('DO_NOT_REPLACE=private')
        self.installed = base / 'installed'
        self.installed.write_text('installed collector')
        self.installed.chmod(0o755)
        (self.source / 'host' / 'clumsies-observability-metrics').write_text('collector')
        (self.root / 'host' / 'clumsies-observability-metrics').write_text('collector')
        self.calls = []
        self.services = {'prometheus': {}, 'alertmanager': {}, 'grafana': {}, 'loki': {}, 'alloy': {}, 'blackbox-exporter': {}}
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

    def test_runtime_notification_credentials_and_routing_are_preserved(self):
        (self.source / 'prometheus/config').write_text('new')
        self.deploy()
        self.assertEqual(self.notification.read_bytes(), self.notification_original)
        self.assertFalse((self.root / '.delivery/previous/alertmanager').exists())
        check = next(args for args in self.calls if '/bin/amtool' in args)
        self.assertIn(f'{self.root / "alertmanager"}:/etc/alertmanager:ro', check)
        self.assertNotIn(f'{self.source / "alertmanager"}:/etc/alertmanager:ro', check)
        self.assertFalse(any('SIGHUP' in args and args[-1] == 'alertmanager' for args in self.calls))

    def test_notification_template_changes_alone_are_not_delivered(self):
        (self.source / 'alertmanager/alertmanager.yml').write_text('different placeholders and routes')
        self.deploy()
        self.assertEqual(self.notification.read_bytes(), self.notification_original)
        self.assertFalse(any('kill' in args or 'restart' in args or 'up' in args for args in self.calls))

    def test_failed_verification_restores_old_files_and_returns_failure(self):
        (self.source / 'prometheus/config').write_text('new')
        self.verify.side_effect = [RuntimeError('injected unhealthy targets'), None]
        with self.assertRaisesRegex(RuntimeError, 'injected'):
            self.deploy()
        self.assertEqual((self.root / 'prometheus/config').read_text(), 'old')
        self.assertFalse((self.root / '.delivery/current').exists())
        self.assertEqual(self.verify.call_count, 2)
        self.assertEqual(self.secret.read_text(), 'DO_NOT_REPLACE=private')
        self.assertEqual(self.notification.read_bytes(), self.notification_original)

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
    def verify(self, *, targets=None, logs='', loki_status='success', probe_success=1, collection=True,
               host_age=0, db_age=0, scrape_age=0, missing_collection_instance=None):
        # Exact synthetic seconds keep ISO microsecond rounding out of boundary tests.
        now = 1_780_000_000.0
        current = now + 1
        fresh = delivery.datetime.datetime.fromtimestamp(current - scrape_age, delivery.datetime.timezone.utc).isoformat()
        root = Path(__file__).resolve().parent
        expected = delivery.expected_targets(root)
        if targets is None:
            targets = [{'health': 'up', 'lastScrape': fresh, 'labels': {'job': job, 'instance': instance}}
                       for job, instance in expected]
        def response(url):
            if '/api/health' in url:
                return json.dumps({'database': 'ok'}).encode()
            if '/api/v1/targets' in url:
                return json.dumps({'data': {'activeTargets': targets}}).encode()
            if '/loki/api/' in url:
                return json.dumps({'status': loki_status, 'data': {'result': []}}).encode()
            return b'ready'
        def query(expression):
            if expression.startswith('probe_success'):
                return [{'metric': {'instance': instance}, 'value': [now, str(probe_success)]}
                        for job, instance in expected if job == 'blackbox-public']
            if expression in ('clumsies_database_collection_success', 'clumsies_observability_collection_timestamp_seconds',
                              'clumsies_database_collection_timestamp_seconds'):
                age = host_age if expression == 'clumsies_observability_collection_timestamp_seconds' else db_age
                if expression == 'clumsies_database_collection_success':
                    if not collection:
                        return []
                    value = 1
                elif age is None:
                    return []
                else:
                    value = current - age
                return [{'metric': {'job': job, 'instance': instance}, 'value': [current, str(value)]}
                        for job, instance in expected if job == 'node' and instance != missing_collection_instance]
            return [{'value': [now, '1']}]
        with patch.object(delivery, 'get', side_effect=response), \
             patch.object(delivery, 'query', side_effect=query), \
             patch.object(delivery, 'verify_dashboards'), \
             patch.object(delivery.time, 'monotonic', side_effect=[0, 1, 181]), \
             patch.object(delivery.time, 'sleep'), \
             patch.object(delivery.time, 'time', return_value=current):
            delivery.verify(now, lambda *args: logs, root)

    def test_healthy_scrapes_and_empty_logs_are_valid(self):
        self.verify()

    def test_empty_missing_stale_or_failed_targets_do_not_count_as_success(self):
        now = delivery.datetime.datetime.now(delivery.datetime.timezone.utc).isoformat()
        partial = [{'health': 'up', 'lastScrape': now,
                    'labels': {'job': 'prometheus', 'instance': 'localhost:9090'}}]
        for targets in ([], partial):
            with self.subTest(targets=targets), self.assertRaisesRegex(RuntimeError, 'missing'):
                self.verify(targets=targets)
        for health in ('up', 'down'):
            targets = [{'health': health, 'lastScrape': '2020-01-01T00:00:00Z',
                        'labels': {'job': job, 'instance': instance}}
                       for job, instance in delivery.expected_targets(Path(__file__).resolve().parent)]
            with self.subTest(health=health), self.assertRaisesRegex(RuntimeError, 'post-deploy scrape'):
                self.verify(targets=targets)

    def test_successful_probe_scrape_does_not_hide_unready_application(self):
        with self.assertRaisesRegex(RuntimeError, 'Public probes'):
            self.verify(probe_success=0)

    def test_failed_or_old_collection_cannot_pass_delivery(self):
        with self.assertRaisesRegex(RuntimeError, 'collection'):
            self.verify(collection=False)

    def test_future_scrape_cannot_pass_delivery(self):
        with self.assertRaisesRegex(RuntimeError, 'post-deploy scrape'):
            self.verify(scrape_age=-1)

    def test_collection_time_must_be_present_after_deploy_and_not_in_future(self):
        for field in ('host_age', 'db_age'):
            for age in (None, -1, 5, float('nan')):
                with self.subTest(field=field, age=age), self.assertRaisesRegex(RuntimeError, 'collection'):
                    self.verify(**{field: age})

    def test_one_valid_node_cannot_hide_an_unmeasured_expected_node(self):
        expected = delivery.expected_targets(Path(__file__).resolve().parent) | {('node', 'extra:9100')}
        with patch.object(delivery, 'expected_targets', return_value=expected):
            with self.assertRaisesRegex(RuntimeError, 'collection'):
                self.verify(missing_collection_instance='extra:9100')

    def test_provisioning_and_query_errors_fail_delivery(self):
        with self.assertRaisesRegex(RuntimeError, 'Grafana provisioning'):
            self.verify(logs='logger=provisioning.dashboard level=error msg=invalid')
        with self.assertRaisesRegex(RuntimeError, 'Loki query'):
            self.verify(loki_status='error')

    def test_static_bootstrap_rollback_uses_configured_targets_not_discovered_ones(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / 'prometheus').mkdir()
            (root / 'prometheus/prometheus.yml').write_text('''scrape_configs:
  - job_name: node
    static_configs:
      - targets: ["node-exporter:9100"]
  - job_name: blackbox-public
    metrics_path: /probe
    params:
      module: [https_follow]
    static_configs:
      - targets:
          - https://app.clumsies.ai/api/v1/admin/health
          - https://docs.clumsies.ai/
    relabel_configs:
      - source_labels: [__address__]
        target_label: __param_target
''')
            self.assertEqual(delivery.expected_targets(root), {
                ('node', 'node-exporter:9100'),
                ('blackbox-public', 'https://app.clumsies.ai/api/v1/admin/health'),
                ('blackbox-public', 'https://docs.clumsies.ai/'),
            })
            (root / 'prometheus/prometheus.yml').write_text('scrape_configs:\n  - job_name: node\n    file_sd_configs: []\n')
            with self.assertRaisesRegex(ValueError, 'unsupported'):
                delivery.expected_targets(root)

    def test_expected_target_rule_matches_file_discovery_catalogs(self):
        root = Path(__file__).resolve().parent
        rules = (root / 'prometheus/rules.yml').read_text()
        declared = set(re.findall(r'"job", "([^"\n]+)"[^\n]+"instance", "([^"\n]+)"', rules))
        self.assertEqual(declared, delivery.expected_targets(root))
        config = (root / 'prometheus/prometheus.yml').read_text()
        for file in ('targets.json', 'probes.json'):
            self.assertIn(f'/etc/prometheus/{file}', config)


class DashboardDatabaseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.dashboards = self.root / 'grafana/dashboards'
        self.dashboards.mkdir(parents=True)
        self.expected = {'uid': 'test', 'title': 'Latency', 'panels': [{'id': 1}],
                         'templating': {'list': []}, 'links': []}
        (self.dashboards / 'test.json').write_text(json.dumps(self.expected))
        self.data = self.root / 'data'
        self.data.mkdir()
        self.database = self.data / 'grafana.db'
        self.mounts = [{'Destination': '/var/lib/grafana', 'Source': str(self.data)},
                       {'Destination': '/var/lib/grafana/dashboards', 'Source': str(self.dashboards)}]

    def verify(self):
        with patch.object(delivery, 'run', return_value=json.dumps(self.mounts)):
            delivery.verify_dashboards(self.root, lambda *args: 'grafana-container')

    def test_grafana_unified_resource_content_is_checked_without_admin_credentials(self):
        with sqlite3.connect(self.database) as conn:
            conn.execute('CREATE TABLE resource (name TEXT,value TEXT,resource TEXT,"group" TEXT,namespace TEXT,folder TEXT)')
            conn.execute('INSERT INTO resource VALUES (?,?,?,?,?,?)',
                         ('test', json.dumps({'spec': self.expected}), 'dashboards', 'dashboard.grafana.app', 'default', 'folder'))
            conn.execute('INSERT INTO resource VALUES (?,?,?,?,?,?)',
                         ('folder', json.dumps({'spec': {'title': 'Clumsies'}}), 'folders', 'folder.grafana.app', 'default', ''))
            conn.execute('CREATE TABLE dashboard (uid TEXT,data TEXT,is_folder INTEGER,org_id INTEGER)')
        self.verify()  # Empty legacy table does not mean the dashboards are missing.
        with sqlite3.connect(self.database) as conn:
            conn.execute('UPDATE resource SET folder=? WHERE name=?', ('wrong-folder', 'test'))
        with self.assertRaisesRegex(RuntimeError, 'outside the Clumsies folder'):
            self.verify()
        with sqlite3.connect(self.database) as conn:
            conn.execute('UPDATE resource SET folder=? WHERE name=?', ('folder', 'test'))
        changed = dict(self.expected, panels=[{'id': 2}])
        with sqlite3.connect(self.database) as conn:
            conn.execute('UPDATE resource SET value=? WHERE name=?', (json.dumps({'spec': changed}), 'test'))
        with self.assertRaisesRegex(RuntimeError, 'content has not been provisioned'):
            self.verify()

    def test_legacy_dashboard_storage_remains_supported(self):
        with sqlite3.connect(self.database) as conn:
            conn.execute('CREATE TABLE dashboard (uid TEXT,data TEXT,is_folder INTEGER,org_id INTEGER)')
            conn.execute('INSERT INTO dashboard VALUES (?,?,?,?)', ('test', json.dumps(self.expected), 0, 1))
            conn.execute('INSERT INTO dashboard VALUES (?,?,?,?)', ('folder', json.dumps({'title': 'Clumsies'}), 1, 1))
        self.verify()
        (self.dashboards / 'other.json').write_text(json.dumps(dict(self.expected, uid='not-provisioned')))
        with self.assertRaisesRegex(RuntimeError, 'not-provisioned'):
            self.verify()

    def test_wrong_mount_does_not_pass_with_matching_database(self):
        other = self.root / 'old-dashboard-files'
        other.mkdir()
        (other / 'test.json').write_text('{}')
        self.mounts[1]['Source'] = str(other)
        with self.assertRaisesRegex(RuntimeError, 'mount is stale'):
            self.verify()


if __name__ == '__main__':
    unittest.main()
