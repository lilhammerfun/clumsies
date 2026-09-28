#!/usr/bin/env python3
"""Deploy configuration to an existing stack, preserving images, credentials and volumes."""

import datetime
import fcntl
import json
from pathlib import Path
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request


MANAGED = ('compose.observability.yml', 'prometheus', 'alertmanager', 'grafana',
           'blackbox', 'loki', 'alloy', 'host')
HOST_FILES = {
    'clumsies-observability-metrics': Path('/usr/local/sbin/clumsies-observability-metrics'),
    'clumsies-observability-metrics.service': Path('/etc/systemd/system/clumsies-observability-metrics.service'),
    'clumsies-observability-metrics.timer': Path('/etc/systemd/system/clumsies-observability-metrics.timer'),
}


def run(*args):
    return subprocess.check_output([str(arg) for arg in args], text=True).strip()


def inventory(path):
    """Reject links and special files before copying configuration as root."""
    if path.is_symlink():
        raise ValueError(f'Symlink not permitted in managed configuration: {path}')
    if path.is_file():
        return path.read_bytes()
    if path.is_dir():
        return {p.name: inventory(p) for p in sorted(path.iterdir())}
    raise ValueError(f'Missing or special configuration file: {path}')


def sync(source, target):
    """Keep mounted directory inodes; replace individual files and remove stale entries."""
    if source.is_dir():
        target.mkdir(parents=True, exist_ok=True)
        for path in target.iterdir():
            if not (source / path.name).exists():
                if path.is_dir():
                    shutil.rmtree(path)
                else:
                    path.unlink()
        for path in source.iterdir():
            sync(path, target / path.name)
    else:
        # Grafana polls files: never let it read a partially copied JSON document.
        with tempfile.NamedTemporaryFile(dir=target.parent, delete=False) as temporary:
            replacement = Path(temporary.name)
        try:
            shutil.copy2(source, replacement)
            replacement.replace(target)
        finally:
            replacement.unlink(missing_ok=True)


def get(url):
    with urllib.request.urlopen(url, timeout=5) as response:
        return response.read()


def query(expression):
    result = json.loads(get('http://127.0.0.1:9090/api/v1/query?' +
                            urllib.parse.urlencode({'query': expression})))
    if result['status'] != 'success':
        raise RuntimeError('Prometheus query failed')
    return result['data']['result']


def verify(started, compose):
    """Allow scrape/provisioning convergence, but never accept empty target discovery."""
    deadline = time.monotonic() + 180
    last = 'not ready'
    while time.monotonic() < deadline:
        try:
            for url in ('http://127.0.0.1:9090/-/ready', 'http://127.0.0.1:9093/-/ready',
                        'http://127.0.0.1:3100/ready'):
                get(url)
            if json.loads(get('http://127.0.0.1:3000/api/health'))['database'] != 'ok':
                raise RuntimeError('Grafana database is not ready')
            targets = json.loads(get('http://127.0.0.1:9090/api/v1/targets'))['data']['activeTargets']
            if not targets or any(t['health'] != 'up' or datetime.datetime.fromisoformat(
                    t['lastScrape'].replace('Z', '+00:00')).timestamp() < started for t in targets):
                raise RuntimeError('Scrape targets have not completed a healthy post-deploy scrape')
            loaded = query('prometheus_config_last_reload_successful')
            if not loaded or any(float(v['value'][1]) != 1 for v in loaded):
                raise RuntimeError('Prometheus configuration reload failed')
            query('sum(rate(clumsies_http_request_duration_seconds_count[5m]))')
            result = json.loads(get('http://127.0.0.1:3100/loki/api/v1/query_range?' +
                                   urllib.parse.urlencode({'query': '{service=~"caddy|server"}', 'limit': 1})))
            if result['status'] != 'success':
                raise RuntimeError('Loki query failed')
            # Readiness alone does not catch a failed dashboard/datasource provisioner.
            logs = compose('logs', '--since', str(int(started)), '--no-color', 'grafana')
            if any('logger=provisioning' in line and 'level=error' in line for line in logs.splitlines()):
                raise RuntimeError('Grafana provisioning reported an error')
            return
        except (OSError, ValueError, KeyError, RuntimeError, subprocess.CalledProcessError) as error:
            last = str(error)
            time.sleep(3)
    raise RuntimeError(f'Post-deploy verification timed out: {last}')


def deploy(source, root, commit):
    """Validate first, snapshot managed files, and roll back every failed application."""
    if not re.fullmatch('[0-9a-f]{40}', commit):
        raise ValueError('Expected a full Git commit SHA')
    if not (root / '.env').is_file():
        raise ValueError('Bootstrap the observability stack and its .env before delivery')
    state = root / '.delivery'
    state.mkdir(mode=0o700, exist_ok=True)
    with (state / 'lock').open('w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        # Recompute after acquiring the host lock, including manual deployments.
        changed = [name for name in MANAGED if inventory(source / name) != inventory(root / name)]
        def compose(*args, directory=root, override=None):
            command = ['docker', 'compose', '--project-name', 'clumsies-observability',
                       '--env-file', root / '.env', '--file', directory / 'compose.observability.yml']
            if override:
                command += ['--file', override]
            return run(*command, *args)

        config = json.loads(compose('config', '--format', 'json', directory=source))
        images = {}
        for service in config['services']:
            container = compose('ps', '-q', service)
            if not container or '\n' in container:
                raise RuntimeError(f'Existing running service required: {service}')
            images[service] = {'image': run('docker', 'inspect', '--format', '{{.Image}}', container)}
        current = json.loads(compose('config', '--format', 'json'))
        if set(current['services']) != set(images):
            raise ValueError('Service additions/removals require a separate infrastructure rollout')
        for file in (source / 'grafana/dashboards').glob('*.json'):
            json.loads(file.read_text())
        for service, binary, arguments in (
            ('prometheus', '/bin/promtool', ['check', 'config', '/etc/prometheus/prometheus.yml']),
            ('alertmanager', '/bin/amtool', ['check-config', '/etc/alertmanager/alertmanager.yml']),
            ('alloy', '/bin/alloy', ['validate', '/etc/alloy/config.alloy']),
            ('loki', '/usr/bin/loki', ['-verify-config=true', '-config.file=/etc/loki/loki-config.yml']),
        ):
            run('docker', 'run', '--rm', '--pull=never', '--network=none', '--entrypoint', binary,
                '-v', f'{source / service}:/etc/{service}:ro', images[service]['image'], *arguments)
        if not changed:
            verify(time.time(), compose)
            (state / 'current').write_text(commit + '\n')
            print(f'Configuration unchanged; verified {commit}')
            return
        with tempfile.TemporaryDirectory(prefix='staging-', dir=state) as temporary:
            backup = Path(temporary)
            for name in MANAGED:
                sync(root / name, backup / name)
            (backup / 'installed').mkdir()
            for name, path in HOST_FILES.items():
                shutil.copy2(path, backup / 'installed' / name)
            if (state / 'current').exists():
                shutil.copy2(state / 'current', backup / 'commit')
            (backup / 'images.json').write_text(json.dumps({'services': images}))
            previous = state / 'previous'
            if previous.exists():
                shutil.rmtree(previous)
            shutil.copytree(backup, previous)
        override = previous / 'images.json'

        def apply(restore=False):
            if 'host' in changed and not restore:
                for name, path in HOST_FILES.items():
                    shutil.copy2(root / 'host' / name, path)
                HOST_FILES['clumsies-observability-metrics'].chmod(0o755)
                run('systemctl', 'daemon-reload')
                run('systemctl', 'restart', 'clumsies-observability-metrics.timer')
                run('systemctl', 'start', 'clumsies-observability-metrics.service')
            if 'compose.observability.yml' in changed:
                compose('up', '-d', '--no-build', '--pull', 'never', '--wait', '--wait-timeout', '120', override=override)
            for service in ('prometheus', 'alertmanager', 'grafana', 'blackbox', 'loki', 'alloy'):
                if service in changed:
                    name = 'blackbox-exporter' if service == 'blackbox' else service
                    if service in ('prometheus', 'alertmanager'):
                        compose('kill', '--signal', 'SIGHUP', name)
                    else:
                        compose('restart', name)

        started = time.time()
        try:
            for name in changed:
                sync(source / name, root / name)
            apply()
            verify(started, compose)
        except BaseException:
            print('Deployment failed; restoring previous configuration', file=sys.stderr)
            try:
                for name in changed:
                    sync(previous / name, root / name)
                rollback_started = time.time()
                if 'host' in changed:
                    for name, path in HOST_FILES.items():
                        shutil.copy2(previous / 'installed' / name, path)
                    run('systemctl', 'daemon-reload')
                    run('systemctl', 'restart', 'clumsies-observability-metrics.timer')
                    run('systemctl', 'start', 'clumsies-observability-metrics.service')
                apply(restore=True)
                verify(rollback_started, compose)
            except BaseException:
                print(f'ROLLBACK FAILED: restore files retained at {previous}', file=sys.stderr)
                raise
            print('Previous configuration restored and verified', file=sys.stderr)
            raise
        (state / 'current').write_text(commit + '\n')
        print(f'Deployed and verified observability configuration {commit}')


if __name__ == '__main__':
    if len(sys.argv) != 2:
        sys.exit('Usage: delivery.py FULL_COMMIT_SHA')
    def interrupted(signum, _frame):
        raise RuntimeError(f'Delivery interrupted by signal {signum}')
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGHUP, interrupted)
    deploy(Path(__file__).resolve().parent, Path('/opt/clumsies/observability'), sys.argv[1])
