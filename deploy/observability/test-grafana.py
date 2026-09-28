#!/usr/bin/env python3
"""Boot the shipped Grafana provisioning tree and reproduce missing-directory failures."""

import base64
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid

root = Path(__file__).resolve().parent
config = json.loads(subprocess.check_output([
    'docker', 'compose', '--env-file', str(root / '.env.example'),
    '-f', str(root / 'compose.observability.yml'), 'config', '--format', 'json',
], text=True))
expected = {json.loads(p.read_text())['uid']: json.loads(p.read_text())['title']
            for p in (root / 'grafana/dashboards').glob('*.json')}


def check(provisioning, missing_directories):
    name = 'clumsies-grafana-test-' + uuid.uuid4().hex[:10]
    password = uuid.uuid4().hex  # Only for this disposable local container.
    authorization = 'Basic ' + base64.b64encode(('admin:' + password).encode()).decode()
    subprocess.run([
        'docker', 'create', '--name', name, '-p', '127.0.0.1::3000',
        '-e', 'GF_SECURITY_ADMIN_PASSWORD=' + password,
        '-e', 'GF_ANALYTICS_REPORTING_ENABLED=false',
        '-e', 'GF_ANALYTICS_CHECK_FOR_UPDATES=false',
        '-e', 'GF_PLUGINS_PREINSTALL_DISABLED=true',
        '-v', f'{provisioning}:/etc/grafana/provisioning:ro',
        '-v', f'{root / "grafana/dashboards"}:/var/lib/grafana/dashboards:ro',
        config['services']['grafana']['image'],
    ], check=True, stdout=subprocess.DEVNULL)
    try:
        subprocess.run(['docker', 'start', name], check=True, stdout=subprocess.DEVNULL)
        address = 'http://' + subprocess.check_output(['docker', 'port', name, '3000'], text=True).strip()
        deadline = time.monotonic() + 90
        last = None
        while time.monotonic() < deadline:
            try:
                def get(path):
                    request = urllib.request.Request(address + path, headers={'Authorization': authorization})
                    with urllib.request.urlopen(request, timeout=3) as response:
                        return json.load(response)
                assert get('/api/health')['database'] == 'ok'
                for uid, title in expected.items():
                    assert get('/api/dashboards/uid/' + uid)['dashboard']['title'] == title
                for uid in ('prometheus', 'loki'):
                    assert get('/api/datasources/uid/' + uid)['uid'] == uid
                break
            except (OSError, ValueError, AssertionError) as error:
                last = error
                time.sleep(1)
        else:
            raise RuntimeError(f'Grafana provisioning did not finish: {last}')
        logs = subprocess.check_output(['docker', 'logs', name], stderr=subprocess.STDOUT, text=True)
        errors = [line for line in logs.splitlines() if 'logger=provisioning' in line and 'level=error' in line]
        if missing_directories:
            assert any('/provisioning/plugins' in line and 'no such file or directory' in line for line in errors), errors
            assert any('/provisioning/alerting' in line and 'no such file or directory' in line for line in errors), errors
        else:
            assert not errors, '\n'.join(errors)
    finally:
        subprocess.run(['docker', 'rm', '-f', '-v', name], check=True, stdout=subprocess.DEVNULL)


with tempfile.TemporaryDirectory() as temporary:
    incomplete = Path(temporary) / 'provisioning'
    shutil.copytree(root / 'grafana/provisioning', incomplete)
    for directory in ('plugins', 'alerting'):
        shutil.rmtree(incomplete / directory)
    check(incomplete, missing_directories=True)
check(root / 'grafana/provisioning', missing_directories=False)
print('Missing-directory regression reproduced; all shipped dashboards and datasources load without provisioning errors.')
