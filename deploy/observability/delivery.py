#!/usr/bin/env python3
"""Deploy configuration to an existing stack, preserving images, credentials and volumes."""

import datetime
import fcntl
import json
from pathlib import Path
import re
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.parse
import urllib.request


# Runtime notification credentials and routing, like .env, belong to the installation.
MANAGED = ('compose.observability.yml', 'prometheus', 'grafana',
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


def expected_targets(root):
    """Read configured discovery, including the pre-catalog bootstrap rollback."""
    targets = set()
    catalogs = [root / 'prometheus' / name for name in ('targets.json', 'probes.json')]
    if any(path.exists() for path in catalogs):
        for path in catalogs:
            for group in json.loads(path.read_text()):
                targets.update((group['labels']['job'], instance) for instance in group['targets'])
    else:
        # shortcut: only the former static job format, remove after all stacks use catalogs.
        text = (root / 'prometheus/prometheus.yml').read_text().partition('scrape_configs:')[2]
        jobs = re.findall(r'^  - job_name: ([a-zA-Z0-9_-]+)\n(.*?)(?=^  - job_name:|\Z)', text, re.M | re.S)
        for job, block in jobs:
            if 'static_configs:' not in block or 'file_sd_configs:' in block:
                raise ValueError(f'Legacy discovery format is unsupported: {job}')
            instances = []
            for flow in re.findall(r'^\s+- targets: (\[[^\n]+\])\s*$', block, re.M):
                instances.extend(json.loads(flow))
            instances.extend(re.findall(r'^\s+- (https?://\S+)\s*$', block, re.M))
            if not instances:
                raise ValueError(f'Legacy target list is empty or unsupported: {job}')
            targets.update((job, instance) for instance in instances)
    if not targets:
        raise ValueError('Expected target discovery is empty')
    return targets


def verify_dashboards(root, compose):
    """Inspect provisioned content read-only; initial admin env is not a login."""
    expected = {dashboard['uid']: dashboard for path in (root / 'grafana/dashboards').glob('*.json')
                for dashboard in [json.loads(path.read_text())]}
    if not expected:
        raise RuntimeError('No expected dashboards')
    container = compose('ps', '-q', 'grafana')
    mounts = json.loads(run('docker', 'inspect', '--format', '{{json .Mounts}}', container))
    mounted = {mount['Destination']: Path(mount['Source']) for mount in mounts}
    dashboard_path = mounted['/var/lib/grafana/dashboards']
    for path in (root / 'grafana/dashboards').glob('*.json'):
        if (dashboard_path / path.name).read_bytes() != path.read_bytes():
            raise RuntimeError(f'Grafana dashboard mount is stale: {path.name}')
    database = mounted['/var/lib/grafana'] / 'grafana.db'
    with sqlite3.connect(database.as_uri() + '?mode=ro', uri=True, timeout=5) as connection:
        tables = {row[0] for row in connection.execute("SELECT name FROM sqlite_master WHERE type='table'")}
        stored = {}
        folder_ready = False
        resource_folders = {}
        folder_names = set()
        if 'resource' in tables:
            for name, value, folder in connection.execute(
                    'SELECT name,value,folder FROM resource WHERE resource=? AND "group"=? AND namespace=?',
                    ('dashboards', 'dashboard.grafana.app', 'default')):
                stored[name] = json.loads(value)['spec']
                resource_folders[name] = folder
            folder_names = {name for name, value in connection.execute(
                'SELECT name,value FROM resource WHERE resource=? AND "group"=? AND namespace=?',
                ('folders', 'folder.grafana.app', 'default')) if json.loads(value)['spec'].get('title') == 'Clumsies'}
            folder_ready = bool(folder_names)
        if 'dashboard' in tables:
            for uid, data, is_folder in connection.execute('SELECT uid,data,is_folder FROM dashboard WHERE org_id=1'):
                dashboard = json.loads(data)
                if is_folder:
                    folder_ready = folder_ready or dashboard.get('title') == 'Clumsies'
                else:
                    stored.setdefault(uid, dashboard)
        if not folder_ready:
            raise RuntimeError('Grafana Clumsies folder is not provisioned')
        for uid, dashboard in expected.items():
            actual = stored.get(uid)
            if uid in resource_folders and resource_folders[uid] not in folder_names:
                raise RuntimeError(f'Grafana dashboard is outside the Clumsies folder: {uid}')
            if actual is None or any(actual.get(key) != dashboard.get(key) for key in ('title', 'panels', 'templating', 'links')):
                raise RuntimeError(f'Grafana dashboard content has not been provisioned: {uid}')


def verify(started, compose, root):
    """Allow scrape/provisioning convergence, but never accept empty target discovery."""
    expected = expected_targets(root)
    collector = (root / 'host/clumsies-observability-metrics').read_text()
    collection_timestamp = ('clumsies_observability_collection_timestamp_seconds'
                            if 'clumsies_observability_collection_timestamp_seconds' in collector
                            else 'clumsies_database_collection_timestamp_seconds')
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
            observed = {(target['labels']['job'], target['labels']['instance']): target for target in targets}
            missing = expected - observed.keys()
            if missing:
                raise RuntimeError(f'Expected scrape targets are missing: {sorted(missing)}')
            current = time.time()
            if any(observed[key]['health'] != 'up' or not started <= datetime.datetime.fromisoformat(
                    observed[key]['lastScrape'].replace('Z', '+00:00')).timestamp() <= current for key in expected):
                raise RuntimeError('Expected scrape targets have not completed a healthy post-deploy scrape with valid time')
            probes = {entry['metric']['instance']: float(entry['value'][1]) for entry in query('probe_success{job="blackbox-public"}')}
            if any(probes.get(instance) != 1 for job, instance in expected if job == 'blackbox-public'):
                raise RuntimeError('Public probes have not passed after deployment')
            nodes = {key for key in expected if key[0] == 'node'}
            samples = {}
            for metric in {'clumsies_database_collection_success', collection_timestamp,
                           'clumsies_database_collection_timestamp_seconds'}:
                samples[metric] = {(entry['metric']['job'], entry['metric']['instance']): float(entry['value'][1])
                                   for entry in query(metric)}
            current = time.time()
            if not nodes or any(samples['clumsies_database_collection_success'].get(key) != 1
                    or any(not int(started) <= samples[metric].get(key, float('nan')) <= current
                           for metric in {collection_timestamp, 'clumsies_database_collection_timestamp_seconds'})
                    for key in nodes):
                raise RuntimeError('Operational/database collection has not succeeded after deployment with valid time')
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
            verify_dashboards(root, compose)
            return
        except (OSError, ValueError, KeyError, RuntimeError, sqlite3.Error, subprocess.CalledProcessError) as error:
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
            ('blackbox', '/bin/blackbox_exporter', ['--config.check', '--config.file=/etc/blackbox/config.yml']),
            ('alloy', '/bin/alloy', ['validate', '/etc/alloy/config.alloy']),
            ('loki', '/usr/bin/loki', ['-verify-config=true', '-config.file=/etc/loki/loki-config.yml']),
        ):
            run('docker', 'run', '--rm', '--pull=never', '--network=none', '--entrypoint', binary,
                '-v', f'{(root if service == "alertmanager" else source) / service}:/etc/{service}:ro', images['blackbox-exporter' if service == 'blackbox' else service]['image'], *arguments)
        if not changed:
            started = time.time()
            run('systemctl', 'start', 'clumsies-observability-metrics.service')
            verify(started, compose, root)
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
            if 'compose.observability.yml' in changed:
                compose('up', '-d', '--no-build', '--pull', 'never', '--wait', '--wait-timeout', '120', override=override)
            for service in ('prometheus', 'grafana', 'blackbox', 'loki', 'alloy'):
                if service in changed:
                    name = 'blackbox-exporter' if service == 'blackbox' else service
                    if service == 'prometheus':
                        compose('kill', '--signal', 'SIGHUP', name)
                    else:
                        compose('restart', name)
            run('systemctl', 'start', 'clumsies-observability-metrics.service')

        started = time.time()
        try:
            for name in changed:
                sync(source / name, root / name)
            apply()
            verify(started, compose, root)
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
                verify(rollback_started, compose, root)
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
