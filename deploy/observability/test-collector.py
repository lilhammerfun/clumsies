#!/usr/bin/env python3
"""Exercise the installed collector against a disposable, fully migrated PostgreSQL."""

import json
from pathlib import Path
import subprocess
import time

root = Path(__file__).resolve().parent
repo = root.parents[1]
production = json.loads(subprocess.check_output([
    "docker", "compose", "--env-file", str(repo / ".env.example"),
    "--file", str(repo / "compose.production.yml"), "config", "--format", "json",
], text=True))
container = subprocess.check_output([
    "docker", "run", "--detach", "--rm", "--network", "none",
    "--env", "POSTGRES_HOST_AUTH_METHOD=trust", "--env", "POSTGRES_USER=metrics_test",
    "--env", "POSTGRES_DB=metrics_test", "--env", "PGHOST=127.0.0.1",
    "--tmpfs", "/var/lib/postgresql/data", "--tmpfs", "/var/lib/clumsies-observability",
    "--volume", f"{repo / 'crates/server/migrations'}:/migrations:ro",
    "--volume", f"{root / 'host/clumsies-observability-metrics'}:/collector:ro",
    production["services"]["postgres"]["image"],
], text=True).strip()


def sql(statement):
    return subprocess.check_output([
        "docker", "exec", "--interactive", container,
        "psql", "-X", "-v", "ON_ERROR_STOP=1", "-U", "metrics_test", "-d", "metrics_test", "-At",
    ], input=statement, text=True)


def collect():
    subprocess.run(["docker", "exec", container, "sh", "/collector"], check=True, timeout=45)
    output = subprocess.check_output([
        "docker", "exec", container, "cat", "/var/lib/clumsies-observability/textfile/clumsies.prom",
    ], text=True)
    assert " counter\n" not in output
    assert "clumsies_commits_total" not in output
    samples = dict(line.rsplit(" ", 1) for line in output.splitlines() if not line.startswith("#"))
    assert float(samples["clumsies_database_collection_timestamp_seconds"]) > 0
    assert float(samples["clumsies_observability_collection_timestamp_seconds"]) > 0
    return {name: float(value) for name, value in samples.items()}


try:
    # No host paths, ports, credentials or systemd services are accessed. The
    # Docker shim only forwards the collector's psql command inside this container.
    subprocess.run(["docker", "exec", "--interactive", container, "sh", "-eu"], input="""
for attempt in $(seq 1 60); do
  if pg_isready -h 127.0.0.1 -U metrics_test -d metrics_test >/dev/null; then break; fi
  sleep 1
done
for migration in /migrations/*.sql; do
  psql -X --single-transaction -v ON_ERROR_STOP=1 -q -U metrics_test -d metrics_test -f "$migration" >/dev/null
done
mkdir -p /var/lib/clumsies-observability/textfile
cat > /usr/local/bin/docker <<'SH'
#!/bin/sh
set -eu
while [ "$1" != postgres ]; do shift; done
shift
exec "$@"
SH
chmod +x /usr/local/bin/docker
mkdir -p /test-systemd
cat > /usr/local/bin/systemctl <<'SH'
#!/bin/sh
set -eu
property="$3"
unit="$5"
file="/test-systemd/${unit}.${property}"
if [ -f "$file" ]; then
  cat "$file"
else
  case "$property" in
    LoadState) echo loaded ;;
    ActiveState) echo inactive ;;
    Result) echo success ;;
  esac
fi
SH
chmod +x /usr/local/bin/systemctl
""", text=True, check=True, timeout=90)
    empty = collect()
    assert empty['clumsies_drafts{status="open"}'] == 0
    assert empty['clumsies_reviews{status="approved"}'] == 0
    assert empty["clumsies_drafts_with_reconciliation_conflicts"] == 0
    assert empty["clumsies_oldest_open_draft_age_seconds"] == 0
    assert empty["clumsies_db_active_backends"] == 0  # Excludes the collector itself.
    assert empty["clumsies_db_longest_query_seconds"] == 0

    # A loaded-but-never-run unit is not a failed backup. Last success is
    # persisted through failure, reboot-like systemd reset and missing unit data.
    for unit in ('clumsies-backup.service', 'clumsies-restore-drill.service'):
        assert empty[f'clumsies_unit_last_exit_state{{unit="{unit}",state="never"}}'] == 1
        assert f'clumsies_unit_last_exit_ok{{unit="{unit}"}}' not in empty
    assert 'clumsies_restore_drill_last_success_timestamp_seconds' not in empty
    def unit_state(unit, **properties):
        for name, value in properties.items():
            subprocess.run(['docker', 'exec', '--interactive', container, 'sh', '-c',
                            'cat > "$1"', '_', f'/test-systemd/{unit}.{name}'],
                           input=value, text=True, check=True)
    for unit in ('clumsies-backup.service', 'clumsies-restore-drill.service'):
        unit_state(unit, ExecMainExitTimestamp='2026-10-01 00:00:00', Result='success')
    succeeded = collect()
    success_metrics = ('clumsies_backup_last_success_timestamp_seconds',
                       'clumsies_restore_drill_last_success_timestamp_seconds')
    for metric in success_metrics:
        assert succeeded[metric] > 0
    for unit in ('clumsies-backup.service', 'clumsies-restore-drill.service'):
        unit_state(unit, ExecMainExitTimestamp='2026-10-02 00:00:00', Result='exit-code')
    failed_units = collect()
    for metric in success_metrics:
        assert failed_units[metric] == succeeded[metric]
    for unit in ('clumsies-backup.service', 'clumsies-restore-drill.service'):
        assert failed_units[f'clumsies_unit_last_exit_state{{unit="{unit}",state="failed"}}'] == 1
        assert failed_units[f'clumsies_unit_last_exit_ok{{unit="{unit}"}}'] == 0
        unit_state(unit, ExecMainExitTimestamp='', Result='success')
    reset_units = collect()
    for metric in success_metrics:
        assert reset_units[metric] == succeeded[metric]
    unit_state('clumsies-restore-drill.service', LoadState='not-found')
    missing_unit = collect()
    assert missing_unit['clumsies_unit_last_exit_state{unit="clumsies-restore-drill.service",state="missing"}'] == 1
    assert 'clumsies_unit_last_exit_ok{unit="clumsies-restore-drill.service"}' not in missing_unit
    unit_state('clumsies-restore-drill.service', LoadState='loaded', ActiveState='activating')
    assert collect()['clumsies_unit_last_exit_state{unit="clumsies-restore-drill.service",state="running"}'] == 1
    for unit in ('clumsies-backup.service', 'clumsies-restore-drill.service'):
        unit_state(unit, ActiveState='inactive', ExecMainExitTimestamp='2026-10-03 00:00:00', Result='success')
    recovered_units = collect()
    for metric in success_metrics:
        assert recovered_units[metric] > succeeded[metric]

    unit_state('clumsies-backup.service', ExecMainExitTimestamp='', Result='resources')
    startup_failure = collect()
    assert startup_failure['clumsies_unit_last_exit_state{unit="clumsies-backup.service",state="failed"}'] == 1
    assert startup_failure['clumsies_unit_last_exit_ok{unit="clumsies-backup.service"}'] == 0
    assert startup_failure['clumsies_backup_last_success_timestamp_seconds'] == recovered_units['clumsies_backup_last_success_timestamp_seconds']
    # A successful drill followed by a failure before the next scrape still has
    # an authoritative receipt; it does not depend on observing transient success.
    receipt = int(recovered_units['clumsies_restore_drill_last_success_timestamp_seconds']) + 86400
    subprocess.run(['docker', 'exec', '--interactive', container, 'sh', '-c',
                    'mkdir -p /opt/clumsies/backups; cat > /opt/clumsies/backups/.restore-drill-last-success'],
                   input=str(receipt), text=True, check=True)
    unit_state('clumsies-restore-drill.service', Result='exit-code')
    assert collect()['clumsies_restore_drill_last_success_timestamp_seconds'] == receipt
    subprocess.run(['docker', 'exec', '--interactive', container, 'sh', '-c',
                    'cat > /opt/clumsies/backups/.restore-drill-last-success'],
                   input='invalid timestamp', text=True, check=True)
    assert collect()['clumsies_restore_drill_last_success_timestamp_seconds'] == receipt

    # Future wall-clock history is retained verbatim, so rules/UI can classify
    # it as invalid instead of erasing the receipt or reporting a fresh success.
    future_receipt = int(time.time()) + 86400
    subprocess.run(['docker', 'exec', '--interactive', container, 'sh', '-c',
                    'cat > /opt/clumsies/backups/.restore-drill-last-success'],
                   input=str(future_receipt), text=True, check=True)
    future_history = collect()
    assert future_history['clumsies_restore_drill_last_success_timestamp_seconds'] == future_receipt
    current_exit = time.strftime('%Y-%m-%d %H:%M:%S', time.gmtime())
    unit_state('clumsies-restore-drill.service', Result='success', ExecMainExitTimestamp=current_exit)
    # A valid unit exit must not conceal the still-future authoritative receipt.
    assert collect()['clumsies_restore_drill_last_success_timestamp_seconds'] == future_receipt
    current_receipt = int(time.time())
    subprocess.run(['docker', 'exec', '--interactive', container, 'sh', '-c',
                    'cat > /opt/clumsies/backups/.restore-drill-last-success'],
                   input=str(current_receipt), text=True, check=True)
    assert collect()['clumsies_restore_drill_last_success_timestamp_seconds'] == current_receipt
    raw_receipt = subprocess.check_output(['docker', 'exec', container, 'cat',
                                          '/opt/clumsies/backups/.restore-drill-last-success'], text=True)
    assert raw_receipt == str(current_receipt)
    cache = subprocess.check_output(['docker', 'exec', container, 'cat',
                                     '/var/lib/clumsies-observability/textfile/clumsies.prom'], text=True)
    cache = '\n'.join(f'clumsies_backup_last_success_timestamp_seconds {future_receipt}'
                      if line.startswith('clumsies_backup_last_success_timestamp_seconds ') else line
                      for line in cache.splitlines()) + '\n'
    subprocess.run(['docker', 'exec', '--interactive', container, 'sh', '-c',
                    'cat > /var/lib/clumsies-observability/textfile/clumsies.prom'],
                   input=cache, text=True, check=True)
    # No new successful candidate means the invalid cache remains visible.
    assert collect()['clumsies_backup_last_success_timestamp_seconds'] == future_receipt
    unit_state('clumsies-backup.service', Result='success', ExecMainExitTimestamp=current_exit)
    assert collect()['clumsies_backup_last_success_timestamp_seconds'] <= int(time.time())

    idle = subprocess.Popen([
        "docker", "exec", "--interactive", "--env", "PGAPPNAME=collector-idle-test", container,
        "psql", "-X", "-U", "metrics_test", "-d", "metrics_test",
    ], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, text=True)
    try:
        idle.stdin.write("BEGIN; SELECT 1;\n")
        idle.stdin.flush()
        for _ in range(50):
            if sql("SELECT count(*) FROM pg_stat_activity WHERE application_name = 'collector-idle-test' AND state = 'idle in transaction';").strip() == "1":
                break
            time.sleep(0.1)
        else:
            raise AssertionError("Idle transaction did not start")
        idle_snapshot = collect()
        assert idle_snapshot["clumsies_db_active_backends"] == 0
        assert idle_snapshot["clumsies_db_longest_query_seconds"] == 0
    finally:
        idle.stdin.close()
        idle.wait(timeout=5)

    sql("""
INSERT INTO orgs (org_id, name) VALUES ('o', 'Test');
INSERT INTO users (user_id, email, role, status) VALUES ('u', 'test@example.invalid', 'owner', 'active');
INSERT INTO projects (project_id, org_id, name) VALUES ('p', 'o', 'Test');
INSERT INTO trees (tree_id) VALUES ('t');
INSERT INTO commits (commit_id, scope, org_id, project_id, tree_id, version, created_at) VALUES
 ('c0', 'project', 'o', 'p', 't', 1, now() - interval '2 hours'),
 ('c1', 'project', 'o', 'p', 't', 2, now() - interval '10 minutes'),
 ('o1', 'org', 'o', NULL, 't', 1, now() - interval '10 minutes'),
 ('deletable', 'project', 'o', 'p', 't', 3, now() - interval '10 minutes');
INSERT INTO refs (ref_id, ref_name, scope, org_id, project_id, commit_id) VALUES
 ('pr', 'refs/heads/main', 'project', 'o', 'p', 'c1'),
 ('or', 'refs/heads/main', 'org', 'o', NULL, 'o1');
INSERT INTO drafts (draft_id, project_id, author_user_id, title, resource_scope, resource_kind,
                    status, daemon_installation_id, base_commit_id, created_at)
SELECT id, 'p', 'u', id, scope, 'memory', status, 'test', base,
       now() - CASE WHEN id = 'current-project' THEN interval '72 days' ELSE interval '20 minutes' END
FROM (VALUES
 ('current-project', 'project', 'open', NULL), ('current-org', 'org', 'submitted', NULL),
 ('old-version', 'project', 'open', NULL), ('old-ref', 'project', 'open', NULL),
 ('invalidated', 'project', 'open', NULL), ('merged', 'project', 'merged', NULL),
 ('discarded', 'project', 'discarded', NULL), ('clean', 'project', 'open', NULL),
 ('old-base', 'project', 'open', 'c0'), ('up-to-date', 'project', 'open', 'c1'),
 ('unevaluated', 'project', 'open', NULL)
) d(id, scope, status, base);
INSERT INTO draft_reconciliation_candidates
 (candidate_id, draft_id, draft_version, base_commit_id, current_commit_id, status,
  base_state, current_state, draft_state, conflicts, invalidated_at)
SELECT draft_id, draft_id, CASE WHEN draft_id = 'old-version' THEN 0 ELSE version END,
 CASE WHEN draft_id = 'old-base' THEN NULL ELSE base_commit_id END,
 CASE WHEN resource_scope = 'org' THEN 'o1' WHEN draft_id = 'old-ref' THEN 'c0' ELSE 'c1' END,
 CASE WHEN draft_id = 'clean' THEN 'clean' ELSE 'conflicts' END,
 '{}', '{}', '{}', '[]', CASE WHEN draft_id = 'invalidated' THEN now() ELSE NULL END
FROM drafts WHERE draft_id <> 'unevaluated';
INSERT INTO reviews (review_id, draft_id, project_id, author_user_id, title, status, created_at) VALUES
 ('r1', 'current-project', 'p', 'u', 'Old', 'open', now() - interval '2 hours'),
 ('r2', 'merged', 'p', 'u', 'Recent', 'merged', now() - interval '20 minutes');
INSERT INTO inbox_notifications
 (user_id, notification_id, org_id, project_id, kind, target_id, event_key, read_version, archived_version)
VALUES ('u', 'unread', 'o', 'p', 'shared_update', 'p', 'e', 0, 0),
       ('u', 'read', 'o', 'p', 'shared_update', 'p', 'e', 1, 0),
       ('u', 'archived', 'o', 'p', 'shared_update', 'p', 'e', 0, 1);
""")
    populated = collect()
    assert populated["clumsies_database_collection_success"] == 1
    assert populated["clumsies_commits_created_last_hour"] == 3
    assert populated["clumsies_drafts_created_last_hour"] == 10
    assert populated["clumsies_reviews_created_last_hour"] == 1
    assert populated['clumsies_drafts{status="open"}'] == 8
    assert populated['clumsies_drafts{status="submitted"}'] == 1
    assert populated['clumsies_reviews{status="approved"}'] == 0
    assert populated["clumsies_drafts_with_reconciliation_conflicts"] == 2
    assert populated["clumsies_inbox_unread_records"] == 2
    assert 72 * 86400 <= populated["clumsies_oldest_open_draft_age_seconds"] < 72 * 86400 + 60

    # Future-created rows remain stored, but cannot count as the last 60 minutes.
    sql("""
INSERT INTO commits (commit_id, scope, org_id, project_id, tree_id, version, created_at)
VALUES ('future-commit', 'project', 'o', 'p', 't', 4, now() + interval '1 day');
INSERT INTO drafts (draft_id, project_id, author_user_id, title, resource_scope, resource_kind,
                    status, daemon_installation_id, created_at)
VALUES ('future-draft', 'p', 'u', 'Future', 'project', 'memory', 'discarded', 'test', now() + interval '1 day');
INSERT INTO reviews (review_id, draft_id, project_id, author_user_id, title, status, created_at)
VALUES ('future-review', 'future-draft', 'p', 'u', 'Future', 'merged', now() + interval '1 day');
""")
    future_rows = collect()
    for kind in ('commits', 'drafts', 'reviews'):
        metric = f'clumsies_{kind}_created_last_hour'
        assert future_rows[metric] == populated[metric]
        assert sql(f"SELECT count(*) FROM {kind} WHERE created_at > now();").strip() == '1'

    # Deletion is a decreasing gauge, not a counter reset or a creation spike.
    sql("DELETE FROM commits WHERE commit_id = 'deletable'; UPDATE drafts SET status = 'discarded';")
    closed = collect()
    assert closed["clumsies_commits_created_last_hour"] == 2
    assert closed['clumsies_drafts{status="open"}'] == 0
    assert closed['clumsies_drafts{status="submitted"}'] == 0
    assert closed["clumsies_drafts_with_reconciliation_conflicts"] == 0
    assert closed["clumsies_oldest_open_draft_age_seconds"] == 0

    # A real SQL error must replace previous samples with an explicit failure.
    sql("ALTER TABLE drafts RENAME TO unavailable_drafts;")
    failed = collect()
    assert failed["clumsies_database_collection_success"] == 0
    assert "clumsies_commits_created_last_hour" not in failed
    assert 'clumsies_drafts{status="open"}' not in failed
    assert 'clumsies_unit_last_exit_ok{unit="clumsies-backup.service"}' in failed
    sql("ALTER TABLE unavailable_drafts RENAME TO drafts;")
    assert collect()["clumsies_database_collection_success"] == 1
    print("Collector SQL, state transitions, deletion, failure and recovery passed.")
finally:
    subprocess.run(["docker", "rm", "--force", container], check=True, stdout=subprocess.DEVNULL)


# Blackbox shares only a loopback namespace with a disposable fixture server;
# neither fixture has external networking or access to host credentials/data.
fixture = subprocess.check_output([
    'docker', 'run', '--detach', '--rm', '--network', 'none', 'python:3-alpine',
    'python', '-u', '-c', r'''from http.server import BaseHTTPRequestHandler, HTTPServer
class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        cases = {
            '/ok': (200, '{"status":"ok","database":{"status":"ok"}}'),
            '/reordered': (200, '{"version":"test","database":{"status":"ok"},"status":"ok"}'),
            '/down': (200, '{"status":"down","database":{"status":"ok"}}'),
            '/degraded': (200, '{"status":"degraded"}'),
            '/missing': (200, '{"database":{"status":"ok"}}'),
            '/malformed': (200, '{"status":"ok"'),
            '/error': (503, '{"status":"ok"}'),
            '/static': (200, '<html>Documentation</html>'),
        }
        status, body = cases[self.path]
        self.send_response(status)
        self.end_headers()
        self.wfile.write(body.encode())
    def log_message(self, *_): pass
HTTPServer(('127.0.0.1', 8088), Handler).serve_forever()
''',
], text=True).strip()
blackbox = None
try:
    blackbox = subprocess.check_output([
        'docker', 'run', '--detach', '--rm', '--network', f'container:{fixture}',
        '--volume', f'{root / "blackbox"}:/etc/blackbox_exporter:ro',
        'prom/blackbox-exporter:v0.28.0', '--config.file=/etc/blackbox_exporter/config.yml',
    ], text=True).strip()
    def probe(path, module='https_ready'):
        expression = f"import urllib.request; print(urllib.request.urlopen('http://127.0.0.1:9115/probe?module={module}&target=http://127.0.0.1:8088{path}').read().decode())"
        output = subprocess.check_output(['docker', 'exec', fixture, 'python', '-c', expression], text=True)
        return float(next(line.split()[1] for line in output.splitlines() if line.startswith('probe_success ')))
    for _ in range(50):
        try:
            assert probe('/ok') == 1
            break
        except subprocess.CalledProcessError:
            time.sleep(0.1)
    else:
        raise AssertionError('Blackbox fixture did not become ready')
    assert probe('/reordered') == 1
    for path in ('/down', '/degraded', '/missing', '/malformed', '/error', '/static'):
        assert probe(path) == 0, path
    assert probe('/static', 'https_follow') == 1
    print('Blackbox JSON readiness, HTTP errors and static-site module passed.')
finally:
    if blackbox:
        subprocess.run(['docker', 'rm', '--force', blackbox], check=True, stdout=subprocess.DEVNULL)
    subprocess.run(['docker', 'rm', '--force', fixture], check=True, stdout=subprocess.DEVNULL)
