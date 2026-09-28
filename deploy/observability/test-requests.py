#!/usr/bin/env python3
"""Exercise request isolation, real edge logs and dashboard queries without production data."""

import json
from pathlib import Path
import re
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
import uuid

root = Path(__file__).resolve().parent


def docker(*args):
    return subprocess.check_output(["docker", *args], text=True).strip()


def wait_for(check, description, timeout=60):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            result = check()
            if result:
                return result
        except (OSError, ValueError, AssertionError) as error:
            last = error
        time.sleep(0.2)
    raise AssertionError(f"Timed out waiting for {description}: {last}")


def get(url):
    with urllib.request.urlopen(url, timeout=10) as response:
        return response.read()


# Evaluate shipped PromQL rather than copies of its intended behavior.
api = json.loads((root / 'grafana/dashboards/clumsies-api.json').read_text())
web = json.loads((root / 'grafana/dashboards/clumsies-web.json').read_text())
series = []
for host, slow in [('app.clumsies.ai', False), ('docs.clumsies.ai', True)]:
    for handler in ['subroute', 'file_server']:
        for bound in ['0.1', '1.0', '2.5', '5.0', '+Inf']:
            n = 0 if slow and bound in ['0.1', '1.0', '2.5'] else 10
            series.append({'series': f'caddy_http_request_duration_seconds_bucket{{job="caddy",handler="{handler}",host="{host}",le="{bound}"}}', 'values': f'0+{n}x10'})
        series.append({'series': f'caddy_http_request_duration_seconds_count{{job="caddy",handler="{handler}",host="{host}"}}', 'values': '0+10x10'})
for route, n in [('/api/v1/me', 10), ('/metrics', 1000), ('/api/v1/admin/health', 1000), ('/idle', 0)]:
    for bound in ['0.1', '1.0', '+Inf']:
        series.append({'series': f'clumsies_http_request_duration_seconds_bucket{{job="server",route="{route}",le="{bound}"}}', 'values': f'0+{n}x10'})
    series.append({'series': f'clumsies_http_request_duration_seconds_count{{job="server",route="{route}"}}', 'values': f'0+{n}x10'})
checks = []


def expect(expr, value):
    checks.append({'expr': expr, 'eval_time': '10m', 'exp_samples': [{'labels': '{}', 'value': value}]})


latency = web['panels'][0]['targets'][2]['expr']
expect(latency.replace('$site', 'docs.clumsies.ai'), 4.975)
expect(latency.replace('$site', 'app.clumsies.ai'), 0.099)
expect(web['panels'][1]['targets'][0]['expr'].replace('$site', 'docs.clumsies.ai'), 50)
expect(api['panels'][1]['targets'][0]['expr'].replace('$route', '.*'), 50)
checks.append({'expr': api['panels'][0]['targets'][2]['expr'].replace('$route', '/idle'), 'eval_time': '10m', 'exp_samples': []})
# Evaluate every Prometheus panel expression as well, including folded panels.
# An empty intersection makes the expected result independent of fixture values.
for path in (root / 'grafana/dashboards').glob('*.json'):
    if '.zh.' in path.name:
        continue
    dashboard = json.loads(path.read_text())
    panels = dashboard['panels'] + [child for panel in dashboard['panels'] for child in panel.get('panels', [])]
    for panel in panels:
        if panel.get('datasource', {}).get('type') != 'prometheus':
            continue
        for target in panel['targets']:
            expr = target['expr'].replace('$route', '.*').replace('$site', 'docs.clumsies.ai')
            checks.append({'expr': f'({expr}) and on() (vector(0) > 0)', 'eval_time': '10m', 'exp_samples': []})

with tempfile.TemporaryDirectory() as temp:
    f = Path(temp) / 'queries.json'
    f.write_text(json.dumps({'evaluation_interval': '1m', 'fuzzy_compare': True, 'tests': [{'interval': '1m', 'input_series': series, 'promql_expr_test': checks}]}))
    f.chmod(0o644)
    docker('run', '--rm', '--network', 'none', '-v', f'{f}:/tests.json:ro', '--entrypoint', '/bin/promtool', 'prom/prometheus:latest', 'test', 'rules', '/tests.json')

# Run the shipped Caddy routes and Alloy pipeline with isolated dependencies.
# Only replace TLS/listener and backend fixture details; never use the user's data.
name = 'clumsies-request-test-' + uuid.uuid4().hex[:10]
containers = []


def start(service, image, *args, mounts=(), ports=()):
    container = name + '-' + service
    command = ['create', '--name', container, '--network', name, '--network-alias', service,
               '--label', 'clumsies.observability.test=' + name,
               '--label', 'com.docker.compose.service=' + service]
    for path, target in mounts:
        command += ['-v', f'{path}:{target}:ro']
    for port in ports:
        command += ['-p', f'127.0.0.1::{port}']
    docker(*command, image, *args)
    containers.append(container)
    docker('start', container)
    return container


def address(container, port):
    return 'http://' + docker('port', container, str(port)).splitlines()[0]


try:
    docker('network', 'create', name)
    with tempfile.TemporaryDirectory() as temp:
        temp = Path(temp)
        config = (root.parent / 'Caddyfile').read_text()
        for host in ['app.clumsies.ai', 'docs.clumsies.ai', 'clumsies.ai', 'www.clumsies.ai']:
            config = config.replace('\n' + host + ' {', '\nhttp://' + host + ' {')
        (temp / 'Caddyfile').write_text(config)
        (temp / 'index.html').write_text('test website')
        # Larger than the socket send buffer, so delaying the reader produces a slow website transfer.
        (temp / 'large.bin').write_bytes(b'x' * (16 * 1024 * 1024))
        (temp / 'server.py').write_text('''import json,time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
class Handler(BaseHTTPRequestHandler):
 def do_GET(self):
  request_id=self.headers.get("X-Request-ID")
  elapsed=1200 if self.path.startswith("/slow") else 2
  if elapsed==1200: time.sleep(1.2)
  print(json.dumps({"fields":{"message":"http request completed","duration_ms":elapsed,"status":200},"span":{"route":"/slow" if elapsed==1200 else "/api/v1/me","request_id":request_id}}),flush=True)
  self.send_response(200);self.send_header("X-Request-ID",request_id or "missing");self.end_headers();self.wfile.write(b"ok")
 def log_message(self,*args): pass
ThreadingHTTPServer(("0.0.0.0",8080),Handler).serve_forever()
''')
        alloy = (root / 'alloy/config.alloy').read_text().replace('discovery.docker "containers" {', 'discovery.docker "containers" {\n  filter {\n    name = "label"\n    values = ["clumsies.observability.test=' + name + '"]\n  }')
        (temp / 'config.alloy').write_text(alloy)
        loki = start('loki', 'grafana/loki:latest', '-config.file=/etc/loki/loki-config.yml', mounts=[(root / 'loki/loki-config.yml', '/etc/loki/loki-config.yml')], ports=[3100])
        loki_url = address(loki, 3100)
        wait_for(lambda: get(loki_url + '/ready'), 'Loki readiness')
        start('server', 'python:3-alpine', 'python', '-u', '/server.py', mounts=[(temp / 'server.py', '/server.py')])
        caddy = start('caddy', 'caddy:2-alpine', 'caddy', 'run', '--config', '/etc/caddy/Caddyfile', mounts=[(temp / 'Caddyfile', '/etc/caddy/Caddyfile'), (temp, '/srv/docs'), (temp, '/srv/www')], ports=[80, 2019])
        edge_url = address(caddy, 80)
        metrics_url = address(caddy, 2019) + '/metrics'
        wait_for(lambda: get(metrics_url), 'Caddy readiness')
        collector = start('alloy', 'grafana/alloy:latest', 'run', '--server.http.listen-addr=0.0.0.0:12345', '/etc/alloy/config.alloy', mounts=[(temp / 'config.alloy', '/etc/alloy/config.alloy'), (Path('/var/run/docker.sock'), '/var/run/docker.sock')], ports=[12345])
        wait_for(lambda: get(address(collector, 12345) + '/-/ready'), 'Alloy readiness')
        ids = {}
        for host, path in [('app.clumsies.ai', '/api/v1/me'), ('app.clumsies.ai', '/slow'), ('docs.clumsies.ai', '/large.bin'), ('clumsies.ai', '/friendly-page'), ('www.clumsies.ai', '/')]:
            req = urllib.request.Request(edge_url + path + '?token=do-not-log', headers={'Host': host, 'Authorization': 'Bearer do-not-log', 'Cookie': 'secret=do-not-log'})
            # Do not follow the www redirect to the public internet.
            class NoRedirect(urllib.request.HTTPRedirectHandler):
                def redirect_request(self, *args):
                    return None
            opener = urllib.request.build_opener(NoRedirect)
            try:
                response = opener.open(req, timeout=20)
            except urllib.error.HTTPError as error:
                assert error.code == 301
                response = error
            with response:
                ids[path + host] = response.headers['X-Request-ID']
                if path == '/large.bin':
                    time.sleep(1.3)  # Deliberately hold the reader to simulate a slow transfer.
                response.read()
        def access_logs_ready():
            output = docker('logs', caddy)
            return output if output.count('"request_id"') >= 5 else None

        raw = wait_for(access_logs_ready, 'all access logs')
        assert 'do-not-log' not in raw
        logs = [json.loads(line) for line in raw.splitlines() if '"request_id"' in line]
        assert {x['site'] for x in logs} == {'app.clumsies.ai', 'docs.clumsies.ai', 'clumsies.ai', 'www.clumsies.ai'}
        assert all('?' not in x['path'] and 'headers' not in x['request'] for x in logs)
        assert next(x for x in logs if x['site'] == 'clumsies.ai')['path'] == '/friendly-page'
        assert next(x for x in logs if x['path'] == '/large.bin')['duration'] > 1000
        assert next(x for x in logs if x['path'] == '/api/v1/me')['duration'] < 1000
        metrics = get(metrics_url).decode()
        assert 'host="docs.clumsies.ai"' in metrics and 'host="app.clumsies.ai"' in metrics
        for host, expected in [('app.clumsies.ai', 2), ('docs.clumsies.ai', 1), ('clumsies.ai', 1), ('www.clumsies.ai', 1)]:
            counts = [float(line.rsplit(' ', 1)[1]) for line in metrics.splitlines()
                      if line.startswith('caddy_http_request_duration_seconds_count{')
                      and f'host="{host}"' in line and 'handler="subroute"' in line]
            assert sum(counts) == expected, (host, counts)

        def query(expr):
            url = loki_url + '/loki/api/v1/query_range?' + urllib.parse.urlencode({'query': expr, 'limit': 1000})
            return json.loads(get(url))['data']['result']

        slow_server = api['panels'][4]['targets'][0]['expr'].replace('${route:regex}', '.*')
        slow_site = web['panels'][4]['targets'][0]['expr'].replace('$site', 'docs.clumsies.ai')
        wait_for(lambda: query(slow_server), 'backend slow request in Loki')
        wait_for(lambda: query(slow_site), 'website slow request in Loki')
        derived = (root / 'grafana/provisioning/datasources/loki.yml').read_text()
        pattern = re.search(r"matcherRegex: '(.+)'", derived).group(1)
        assert all(re.search(pattern, json.dumps(line)).group(1) == line['request_id'] for line in logs)
        # A derived request link must find both edge and backend, without joining unrelated requests.
        request_id = ids['/slowapp.clumsies.ai']
        def correlated():
            result = query('{service=~"caddy|server"} |= ' + json.dumps(request_id))
            return result if {x['stream']['service'] for x in result} == {'caddy', 'server'} else None

        wait_for(correlated, 'request correlation across both services')
        for dashboard in (api, web):
            for panel in dashboard['panels']:
                if panel['type'] == 'logs':
                    expr = panel['targets'][0]['expr'].replace('${route:regex}', '.*').replace('${request_id:doublequote}', json.dumps(request_id)).replace('$site', 'docs.clumsies.ai')
                    query(expr)  # Execute every shipped LogQL query against real Loki.
        collector_metrics = get(address(collector, 12345) + '/metrics').decode()
        assert 'loki_write_sent_entries_total' in collector_metrics
        print('Request isolation, slow site transfer, log redaction, ingestion, correlation and LogQL passed.')
finally:
    for container in reversed(containers):
        subprocess.run(['docker', 'rm', '-f', container], check=True, stdout=subprocess.DEVNULL)
    subprocess.run(['docker', 'network', 'rm', name], check=True, stdout=subprocess.DEVNULL)
