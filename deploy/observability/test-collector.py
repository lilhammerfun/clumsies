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
""", text=True, check=True, timeout=90)
    empty = collect()
    assert empty['clumsies_drafts{status="open"}'] == 0
    assert empty['clumsies_reviews{status="approved"}'] == 0
    assert empty["clumsies_drafts_with_reconciliation_conflicts"] == 0
    assert empty["clumsies_oldest_open_draft_age_seconds"] == 0
    assert empty["clumsies_db_active_backends"] == 0  # Excludes the collector itself.
    assert empty["clumsies_db_longest_query_seconds"] == 0

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
