#!/usr/bin/env python3
"""Own an isolated PostgreSQL/HTTP fixture and schedule bounded local arrivals."""

import argparse
import concurrent.futures
from collections import Counter
import datetime
import hashlib
import http.client
import json
import math
import os
from pathlib import Path
import platform
import secrets
import signal
import subprocess
import sys
import threading
import time
import urllib.parse
import uuid


ROOT = Path(__file__).resolve().parents[1]


def percentiles(values):
    """Nearest-rank quantiles, with no observations represented by null."""
    values = sorted(values)
    return {f'p{int(q * 100)}': values[max(0, math.ceil(q * len(values)) - 1)] if values else None
            for q in (0.50, 0.95, 0.99)}


def workload(profile, index, projects):
    if profile == 'read-write' and index % 4 == 3:
        return 'project_update', projects[(index // 4) % len(projects)]
    operation = ('project_list', 'project_detail', 'project_members')[index % 3]
    return operation, projects[index % len(projects)]


class Client:
    """One reusable HTTP connection per worker, with bounded writer preconditions."""

    def __init__(self, fixture, settings):
        address = urllib.parse.urlsplit(fixture['address'])
        if address.scheme != 'http' or address.hostname != '127.0.0.1' or not address.port or address.path or address.query:
            raise ValueError('The driver only accepts its HTTP fixture on IPv4 loopback')
        self.fixture, self.settings = fixture, settings
        self.port = address.port
        self.local = threading.local()
        self.connections = []
        self.connection_lock = threading.Lock()
        self.write_locks = {project: threading.Lock() for project in fixture['projects']}

    def request(self, method, path, deadline, sample, body=None, revision=None):
        remaining = deadline - time.perf_counter()
        if remaining <= 0:
            raise TimeoutError('planned-arrival deadline expired')
        connection = getattr(self.local, 'connection', None)
        if connection is None:
            connection = http.client.HTTPConnection('127.0.0.1', self.port, timeout=remaining)
            self.local.connection = connection
            with self.connection_lock:
                self.connections.append(connection)
        connection.timeout = remaining
        if connection.sock is not None:
            connection.sock.settimeout(remaining)
        headers = {'Authorization': 'Bearer ' + self.fixture['access_token'],
                   'Content-Type': 'application/json', 'User-Agent': 'clumsies-local-load-fixture/1'}
        if revision is not None:
            headers['If-Match'] = str(revision)
        sample['http_attempts'] += 1
        connection.request(method, path, json.dumps(body).encode() if body is not None else None, headers)
        response = connection.getresponse()
        sample['http_statuses'].append(response.status)
        data = response.read(4 * 1024 * 1024 + 1)
        if len(data) > 4 * 1024 * 1024:
            raise ValueError('fixture response exceeded 4 MiB')
        if response.status != 200:
            raise ValueError(f'HTTP {response.status}')
        value = json.loads(data)
        if time.perf_counter() > deadline:
            raise TimeoutError('response completed beyond planned-arrival deadline')
        return value

    def execute(self, index, planned, submitted, started):
        operation, project = workload(self.settings['profile'], index, self.fixture['projects'])
        deadline = planned + self.settings['deadline']
        sample = {'index': index, 'operation': operation, 'planned_ms': (planned - started) * 1000,
                  'submitted_ms': (submitted - started) * 1000, 'actual_ms': None,
                  'http_attempts': 0, 'http_statuses': [], 'outcome': 'queue_deadline'}
        lock = self.write_locks[project] if operation == 'project_update' else None
        acquired = False
        try:
            remaining = deadline - time.perf_counter()
            if remaining <= 0:
                return self.finish(sample, planned, started)
            if lock is not None:
                acquired = lock.acquire(timeout=remaining)
                if not acquired:
                    return self.finish(sample, planned, started)
            if time.perf_counter() >= deadline:
                return self.finish(sample, planned, started)
            sample['actual_ms'] = (time.perf_counter() - started) * 1000
            path = f'/api/v1/projects/{project}'
            if operation == 'project_list':
                value = self.request('GET', '/api/v1/projects?limit=50', deadline, sample)
                if not isinstance(value.get('items'), list) or len(value['items']) != 50:
                    raise ValueError('project list fixture was incomplete')
            elif operation == 'project_members':
                value = self.request('GET', path + '/members', deadline, sample)
                if not isinstance(value.get('items'), list) or len(value['items']) != 1:
                    raise ValueError('project members fixture was incomplete')
            else:
                value = self.request('GET', path, deadline, sample)
                if value.get('project_id') != project or not isinstance(value.get('revision'), int):
                    raise ValueError('project detail did not match the fixture')
                if operation == 'project_update':
                    revision = value['revision']
                    # Alternate fixed-size values each complete writer round; no new rows.
                    description = ('A' if index // (4 * len(self.fixture['projects'])) % 2 == 0 else 'B') * 256
                    value = self.request('PATCH', path, deadline, sample,
                                         {'description': description}, revision)
                    if value.get('project_id') != project or value.get('revision') != revision + 1:
                        raise ValueError('project write did not advance its revision')
            sample['outcome'] = 'success'
        except TimeoutError:
            if sample['http_attempts'] == 0:
                sample['actual_ms'] = None
            sample['outcome'] = 'deadline' if sample['http_attempts'] else 'queue_deadline'
            self.drop_connection()
        except (OSError, http.client.HTTPException) as error:
            sample['outcome'] = 'transport_error'
            sample['error'] = type(error).__name__
            self.drop_connection()
        except (ValueError, KeyError, TypeError) as error:
            sample['outcome'] = 'http_error' if any(code != 200 for code in sample['http_statuses']) else 'invalid_response'
            sample['error'] = str(error)
            self.drop_connection()
        finally:
            if acquired:
                lock.release()
        return self.finish(sample, planned, started)

    @staticmethod
    def finish(sample, planned, started):
        completed = time.perf_counter()
        sample['completed_ms'] = (completed - started) * 1000
        sample['end_to_end_ms'] = (completed - planned) * 1000
        sample['client_queue_ms'] = ((sample['actual_ms'] if sample['actual_ms'] is not None else sample['completed_ms'])
                                     - sample['planned_ms'])
        sample['transport_ms'] = sample['completed_ms'] - sample['actual_ms'] if sample['actual_ms'] is not None else None
        return sample

    def drop_connection(self):
        connection = getattr(self.local, 'connection', None)
        if connection is not None:
            connection.close()
            self.local.connection = None

    def close(self):
        for connection in self.connections:
            connection.close()


def arrivals(settings, execute):
    """Schedule every planned arrival before waiting for any completed operation."""
    count = math.ceil(settings['rate'] * settings['duration'])
    started = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=settings['workers']) as executor:
        futures = []
        for index in range(count):
            planned = started + index / settings['rate']
            time.sleep(max(0, planned - time.perf_counter()))
            futures.append(executor.submit(execute, index, planned, time.perf_counter(), started))
        samples = [future.result() for future in futures]
    if len(samples) != count or {sample['index'] for sample in samples} != set(range(count)):
        raise RuntimeError('Planned arrivals were lost')
    return samples


def summarize(samples, duration):
    counts = Counter(sample['outcome'] for sample in samples)
    elapsed = max(sample['completed_ms'] for sample in samples) / 1000
    return {'planned': len(samples), 'actual': sum(sample['actual_ms'] is not None for sample in samples),
            'completed': len(samples), 'success': counts['success'], 'outcomes': dict(counts),
            'http_attempts': sum(sample['http_attempts'] for sample in samples),
            'http_responses': sum(len(sample['http_statuses']) for sample in samples),
            'offered_operations_per_second': len(samples) / duration,
            'success_operations_per_second_including_drain': counts['success'] / elapsed,
            'actual_in_scheduling_window': sum(sample['actual_ms'] is not None and sample['actual_ms'] < duration * 1000 for sample in samples),
            'drain_seconds': max(0, elapsed - duration),
            'successful_latency_ms': percentiles([sample['end_to_end_ms'] for sample in samples if sample['outcome'] == 'success']),
            'all_planned_latency_ms': percentiles([sample['end_to_end_ms'] for sample in samples]),
            'client_queue_ms': percentiles([sample['client_queue_ms'] for sample in samples]),
            'transport_ms': percentiles([sample['transport_ms'] for sample in samples if sample['transport_ms'] is not None])}


def validate(settings):
    if settings['profile'] not in ('read', 'read-write'):
        raise ValueError('Unknown load profile')
    if any(not math.isfinite(settings[key]) or settings[key] <= 0 for key in ('rate', 'duration', 'deadline')):
        raise ValueError('Rate, duration and deadline must be positive finite values')
    if not 1 <= settings['workers'] <= 64 or math.ceil(settings['rate'] * settings['duration']) > 100_000:
        raise ValueError('Use 1–64 workers and at most 100,000 planned operations')


def driver():
    fixture = json.load(sys.stdin)
    settings = fixture['settings']
    validate(settings)
    client = Client(fixture, settings)
    def metrics():
        connection = http.client.HTTPConnection('127.0.0.1', client.port, timeout=5)
        try:
            connection.request('GET', '/metrics')
            response = connection.getresponse()
            data = response.read(4 * 1024 * 1024 + 1)
            if response.status != 200 or len(data) > 4 * 1024 * 1024:
                raise RuntimeError('Fixture metrics snapshot failed')
            return data.decode()
        finally:
            connection.close()
    try:
        before = metrics()
        samples = arrivals(settings, client.execute)
        after = metrics()
    finally:
        client.close()
    summary = summarize(samples, settings['duration'])
    report = {'run_id': fixture['run_id'], 'status': 'measured', 'fixture_cleanup': 'pending',
              'scope': 'Same-host generator and release-built Server application, PostgreSQL on its local Docker engine; no Caddy, TLS, daemon or production traffic.',
              'settings': settings, 'fixture': {key: fixture[key] for key in
                  ('server_version', 'postgres_version', 'project_count', 'description_bytes', 'database_max_connections', 'server_workers')},
              'host': {'platform': platform.platform(), 'logical_cpus': os.cpu_count(),
                       'python': platform.python_version()},
              'summary': summary,
              'operations': {operation: summarize([sample for sample in samples if sample['operation'] == operation], settings['duration'])
                             for operation in sorted({sample['operation'] for sample in samples})},
              'server_metrics_before': before, 'server_metrics_after': after,
              'samples': samples}
    with Path(settings['output']).open('x') as output:
        json.dump(report, output, indent=2)
        output.write('\n')
    print(json.dumps(summary))
    return 0 if summary['success'] == summary['planned'] else 2


def command(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()


def file_sha256(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as source:
        while data := source.read(64 * 1024):
            digest.update(data)
    return digest.hexdigest()


def source_fingerprint(root=ROOT):
    """Hash the explicitly recorded build/request sources, including dirty files."""
    paths = {root / 'Cargo.toml', root / 'Cargo.lock', root / 'dev/load-http.py',
             root / 'crates/server/tests/observability_load.rs'}
    paths.update(root.glob('crates/**/Cargo.toml'))
    for directory in ('crates/server/src', 'crates/server/migrations', 'crates/server/tests/common'):
        paths.update(path for path in (root / directory).rglob('*') if path.is_file())
    for name in ('rust-toolchain.toml', 'rust-toolchain', '.cargo/config.toml'):
        if (root / name).is_file():
            paths.add(root / name)
    files = {}
    digest = hashlib.sha256()
    for path in sorted(paths, key=lambda path: path.relative_to(root).as_posix()):
        if path.is_symlink():
            raise ValueError(f'Source fingerprint requires regular files: {path}')
        name = path.relative_to(root).as_posix()
        files[name] = file_sha256(path)
        digest.update(name.encode('utf-8') + b'\0' + bytes.fromhex(files[name]) + b'\n')
    return {'source_sha256': digest.hexdigest(), 'source_files_sha256': files,
            'source_fingerprint_algorithm': 'SHA256(sorted UTF-8 relative path + NUL + binary file SHA256 + LF)'}


def run(settings):
    validate(settings)
    output = Path(settings['output']).resolve()
    if output.exists():
        raise ValueError(f'Output already exists: {output}')
    output.parent.mkdir(parents=True, exist_ok=True)
    settings['output'] = str(output)
    context_endpoint = json.loads(command('docker', 'context', 'inspect', '--format', '{{json .Endpoints.docker.Host}}'))
    endpoint = context_endpoint if os.environ.get('DOCKER_CONTEXT') else os.environ.get('DOCKER_HOST') or context_endpoint
    if not endpoint.startswith('unix://'):
        raise ValueError('Load fixtures require a local Docker Unix socket')
    docker_environment = dict(os.environ, DOCKER_HOST=endpoint)
    docker_environment.pop('DOCKER_CONTEXT', None)
    settings['docker_engine'] = json.loads(command('docker', 'info', '--format',
        '{"cpus":{{.NCPU}},"memory_bytes":{{.MemTotal}},"server_version":{{json .ServerVersion}}}', env=docker_environment))
    settings['source_head'] = command('git', '-C', str(ROOT), 'rev-parse', 'HEAD')
    settings['source_dirty'] = bool(command('git', '-C', str(ROOT), 'status', '--porcelain'))
    settings['started_at'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
    identity = source_fingerprint()
    settings.update(identity)
    # Build before starting the disposable database; compilation is outside measurement.
    build = command('cargo', 'test', '--release', '--locked', '-p', 'server', '--test',
                    'observability_load', '--no-run', '--message-format=json', cwd=ROOT)
    artifacts = [json.loads(line) for line in build.splitlines() if line.startswith('{')]
    executable = next(message['executable'] for message in artifacts if message.get('reason') == 'compiler-artifact'
                      and message['target']['name'] == 'observability_load' and message.get('executable'))
    settings['executable_sha256'] = file_sha256(executable)
    if source_fingerprint() != identity:
        raise RuntimeError('Fingerprinted source changed during compilation; rerun from a quiet checkout')
    run_id = uuid.uuid4().hex
    container = process = None
    error = None
    cleanup_errors = []
    try:
        container = command('docker', 'run', '--detach', '--rm', '--name', f'clumsies-load-{run_id}',
                            '--label', f'clumsies.load.fixture={run_id}', '--publish', '127.0.0.1::5432',
                            '--env', 'POSTGRES_USER=load_fixture', '--env', 'POSTGRES_DB=load_fixture',
                            '--env', 'POSTGRES_PASSWORD=' + (password := secrets.token_hex(24)), 'postgres:16-alpine', env=docker_environment)
        settings['postgres_image_id'] = command('docker', 'inspect', '--format', '{{.Image}}', container, env=docker_environment)
        address = command('docker', 'port', container, '5432/tcp', env=docker_environment)
        if not address.startswith('127.0.0.1:') or '\n' in address:
            raise RuntimeError('PostgreSQL fixture is not exclusively bound to loopback')
        for _ in range(120):
            ready = subprocess.run(['docker', 'exec', container, 'sh', '-c',
                                    'test "$(cat /proc/1/comm)" = postgres && pg_isready -U load_fixture -d load_fixture'],
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=docker_environment)
            if ready.returncode == 0:
                break
            time.sleep(0.5)
        else:
            raise RuntimeError('PostgreSQL fixture failed to become ready')
        environment = dict(docker_environment,
                           CLUMSIES_LOAD_DATABASE_URL=f'postgres://load_fixture:{password}@{address}/load_fixture',
                           CLUMSIES_LOAD_SETTINGS=json.dumps(dict(settings, run_id=run_id, container=container,
                                                                 python=sys.executable)))
        process = subprocess.Popen([executable, '--ignored', '--exact', 'serve_load_fixture', '--nocapture'],
                                   cwd=ROOT, env=environment, start_new_session=True)
        if process.wait() != 0:
            raise RuntimeError('Load fixture failed; inspect the report and test output')
    except BaseException as failure:
        error = failure
    finally:
        if process is not None:
            try:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGINT)
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait(timeout=5)
                        raise RuntimeError('Fixture needed forced termination')
                # The Rust fixture normally reaps its driver. Also terminate an
                # orphan driver if the fixture exited unexpectedly.
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                    deadline = time.monotonic() + 3
                    while time.monotonic() < deadline:
                        try:
                            os.killpg(process.pid, 0)
                        except ProcessLookupError:
                            break
                        time.sleep(0.1)
                    else:
                        os.killpg(process.pid, signal.SIGKILL)
                        raise RuntimeError('Orphan driver did not exit cleanly')
                except ProcessLookupError:
                    pass
            except BaseException as failure:
                cleanup_errors.append(str(failure))
        if container is not None:
            try:
                command('docker', 'rm', '--force', '--volumes', container, env=docker_environment)
            except BaseException as failure:
                cleanup_errors.append(str(failure))
        if output.exists():
            report = json.loads(output.read_text())
            if report.get('run_id') != run_id:
                raise RuntimeError('Output ownership changed during the run')
            report['fixture_cleanup'] = 'failed' if cleanup_errors else 'complete'
            report['status'] = 'failed' if error or cleanup_errors else 'passed'
            report['cleanup_errors'] = cleanup_errors
            output.write_text(json.dumps(report, indent=2) + '\n')
    if cleanup_errors:
        raise RuntimeError('Fixture cleanup failed: ' + '; '.join(cleanup_errors)) from error
    if error:
        raise error
    print(f'Fixture cleaned up; report: {output}')


def self_check():
    from unittest.mock import patch
    assert percentiles(range(1, 101)) == {'p50': 50, 'p95': 95, 'p99': 99}
    assert percentiles([]) == {'p50': None, 'p95': None, 'p99': None}
    settings = {'profile': 'read-write', 'rate': 1000, 'duration': .004, 'workers': 1, 'deadline': 1}
    assert [workload('read-write', index, ['a', 'b'])[0] for index in range(4)].count('project_update') == 1
    fixture = {'address': 'http://127.0.0.1:9', 'projects': ['a'], 'access_token': 'fixture'}
    client = Client(fixture, settings)
    old = time.perf_counter() - 2
    expired = client.execute(0, old, old, old)
    assert expired['outcome'] == 'queue_deadline' and expired['actual_ms'] is None and expired['http_attempts'] == 0
    try:
        Client(dict(fixture, address='http://example.com:80'), settings)
    except ValueError:
        pass
    else:
        raise AssertionError('External target was accepted')

    fault = None
    class Future:
        def __init__(self, executor, arguments):
            self.executor, self.arguments = executor, arguments
        def result(self):
            if fault == 'worker':
                raise RuntimeError('injected worker failure')
            assert self.executor.submitted == 4, 'Scheduler waited for completion before scheduling every arrival'
            return {'index': self.arguments[0], 'planned': self.arguments[1]}
    class Executor:
        def __init__(self, **_): self.submitted = 0
        def __enter__(self): return self
        def __exit__(self, *_):
            if fault == 'cleanup':
                raise RuntimeError('injected cleanup failure')
            return False
        def submit(self, _execute, *arguments):
            self.submitted += 1
            return Future(self, arguments)
    with patch.object(concurrent.futures, 'ThreadPoolExecutor', Executor):
        samples = arrivals(settings, None)
    assert len(samples) == 4 and abs(samples[-1]['planned'] - samples[0]['planned'] - .003) < 1e-6
    for fault in ('worker', 'cleanup'):
        with patch.object(concurrent.futures, 'ThreadPoolExecutor', Executor):
            try:
                arrivals(settings, None)
            except RuntimeError as error:
                assert 'injected' in str(error)
            else:
                raise AssertionError(f'{fault} failure was swallowed')

    # An otherwise successful fixture with failed container removal must return
    # failure and mark its report; no Docker, Cargo or network access is made.
    import tempfile
    from types import SimpleNamespace
    class Process:
        pid = 999999
        def __init__(self, _arguments, **kwargs):
            self.settings = json.loads(kwargs['env']['CLUMSIES_LOAD_SETTINGS'])
        def wait(self, **_):
            Path(self.settings['output']).write_text(json.dumps({'run_id': self.settings['run_id'], 'status': 'measured', 'settings': self.settings}))
            return 0
        def poll(self): return 0
    def fake_command(*arguments, **_):
        if arguments[:3] == ('docker', 'context', 'inspect'):
            return json.dumps('unix:///fixture-only')
        if arguments[:2] == ('docker', 'info'): return json.dumps({'cpus': 4, 'memory_bytes': 4 * 1024 ** 3, 'server_version': 'fixture'})
        if arguments[:2] == ('docker', 'inspect'): return 'sha256:fixture'
        if arguments[0] == 'git': return ''
        if arguments[0] == 'cargo':
            return json.dumps({'reason': 'compiler-artifact', 'target': {'name': 'observability_load'}, 'executable': str(fixture_executable)})
        if arguments[:2] == ('docker', 'run'): return 'f' * 64
        if arguments[:2] == ('docker', 'port'): return '127.0.0.1:54321'
        if arguments[:2] == ('docker', 'rm'): raise RuntimeError('injected container cleanup failure')
        raise AssertionError(arguments)
    with tempfile.TemporaryDirectory() as temporary, \
         patch('__main__.command', side_effect=fake_command), \
         patch.object(subprocess, 'Popen', Process), \
         patch.object(subprocess, 'run', return_value=SimpleNamespace(returncode=0)), \
         patch.object(os, 'killpg', side_effect=ProcessLookupError):
        fixture_executable = Path(temporary) / 'fixture-executable'
        fixture_executable.write_bytes(b'fixture executable')
        output = Path(temporary) / 'failure.json'
        try:
            run(dict(settings, output=str(output)))
        except RuntimeError as error:
            assert 'cleanup failed' in str(error)
        else:
            raise AssertionError('Failed container cleanup passed')
        report = json.loads(output.read_text())
        assert report['status'] == 'failed' and report['fixture_cleanup'] == 'failed'
        assert report['settings']['executable_sha256'] == hashlib.sha256(b'fixture executable').hexdigest()
        assert report['settings']['source_sha256'] == source_fingerprint()['source_sha256']
        assert 'dev/load-http.py' in report['settings']['source_files_sha256']
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        for name in ('Cargo.toml', 'Cargo.lock', 'dev/load-http.py', 'crates/server/Cargo.toml',
                     'crates/server/tests/observability_load.rs', 'crates/server/src/main.rs',
                     'crates/server/migrations/1.sql', 'crates/server/tests/common/mod.rs'):
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        first = source_fingerprint(root)
        assert first == source_fingerprint(root)
        changed = root / 'crates/server/src/main.rs'
        changed.write_text('dirty source')
        second = source_fingerprint(root)
        assert first['source_sha256'] != second['source_sha256']
        assert first['source_files_sha256'][changed.relative_to(root).as_posix()] != second['source_files_sha256'][changed.relative_to(root).as_posix()]
    print('Load driver self-check passed: quantiles, fixed mix, deadlines, loopback, open-loop scheduling and failure/cleanup propagation and source/executable identity.')


if __name__ == '__main__':
    if sys.argv[1:] == ['--driver']:
        sys.exit(driver())
    if sys.argv[1:] == ['--self-check']:
        self_check()
        sys.exit(0)
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', choices=('read', 'read-write'), required=True)
    parser.add_argument('--rate', type=float, default=10, help='Planned business operations per second')
    parser.add_argument('--duration', type=float, default=20, help='Arrival scheduling duration in seconds')
    parser.add_argument('--deadline', type=float, default=5, help='Budget from planned arrival, including client queue')
    parser.add_argument('--workers', type=int, default=16)
    parser.add_argument('--output', required=True)
    def interrupted(signum, _frame):
        raise KeyboardInterrupt(f'Interrupted by signal {signum}')
    signal.signal(signal.SIGTERM, interrupted)
    run(vars(parser.parse_args()))
