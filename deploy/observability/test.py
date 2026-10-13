#!/usr/bin/env python3
"""Validate the shipped configuration using disposable containers and no production credentials."""

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--dashboards-only", action="store_true", help="Evaluate actual dashboard queries and Grafana provisioning without collector/request fixtures")
args = parser.parse_args()

root = Path(__file__).resolve().parent
config = json.loads(subprocess.check_output([
    "docker", "compose", "--env-file", str(root / ".env.example"),
    "--file", str(root / "compose.observability.yml"), "config", "--format", "json",
], text=True))

for service, port in (("prometheus", 9090), ("alertmanager", 9093)):
    assert f"--web.external-url=http://localhost:{port}" in config["services"][service]["command"]
    assert any(int(p["published"]) == p["target"] == port for p in config["services"][service]["ports"])
for service in config["services"].values():
    assert all(p["host_ip"] == "127.0.0.1" for p in service.get("ports", []))

if not args.dashboards_only:
    for service, binary, arguments in (
        ("prometheus", "promtool", ["check", "config", "/etc/prometheus/prometheus.yml"]),
        ("prometheus", "promtool", ["test", "rules", "/etc/prometheus/rules.test.yml"]),
        ("alertmanager", "amtool", ["check-config", "/etc/alertmanager/alertmanager.yml"]),
    ):
        subprocess.run([
            "docker", "run", "--rm", "--network", "none",
            "--entrypoint", f"/bin/{binary}",
            "--volume", f"{root / service}:/etc/{service}:ro",
            "--workdir", f"/etc/{service}", config["services"][service]["image"], *arguments,
        ], check=True)

# Evaluate the actual dashboard expressions, including decreases and the
# seconds-to-hours conversion; both languages must keep identical targets.
dashboards = [json.loads(p.read_text()) for p in sorted((root / "grafana/dashboards").glob("*.json"))]
assert len(dashboards) == 8
def all_panels(dashboard):
    for panel in dashboard["panels"]:
        yield panel
        yield from panel.get("panels", [])


for name in ("overview", "api", "web", "infrastructure"):
    pair = [json.loads((root / f"grafana/dashboards/clumsies-{name}{suffix}.json").read_text())
            for suffix in ("", ".zh")]
    expressions = [
        [[t["expr"] for t in panel.get("targets", [])] for panel in all_panels(dashboard)]
        for dashboard in pair
    ]
    assert expressions[0] == expressions[1]
    panels = list(all_panels(pair[0]))
    assert len({p["id"] for p in panels}) == len(panels)

api_dashboard = json.loads((root / "grafana/dashboards/clumsies-api.json").read_text())
series = {
    "clumsies_commits_created_last_hour": "3x359 2",
    "clumsies_drafts_created_last_hour": "10x360",
    "clumsies_reviews_created_last_hour": "1x360",
    'clumsies_drafts{status="open"}': "0+1x360",
    'clumsies_drafts{status="submitted"}': "1x360",
    'clumsies_drafts{status="merged"}': "0x360",
    'clumsies_drafts{status="discarded"}': "0x360",
    "clumsies_drafts_with_reconciliation_conflicts": "2x360",
    "clumsies_inbox_unread_records": "2x360",
    "clumsies_oldest_open_draft_age_seconds": "6220800x360",
}
expected = [2, 10, 1, None, 60, 2, 2, 72]
expressions = [t["expr"] for p in all_panels(api_dashboard) if p["id"] in range(10, 18)
               for t in p["targets"]]
assert len(expressions) == len(expected)
series["clumsies_database_collection_success"] = "1x360"
series["clumsies_database_collection_timestamp_seconds"] = "0+60x360"
series["clumsies_observability_collection_timestamp_seconds"] = "0+60x360"
series["clumsies_backup_last_success_timestamp_seconds"] = "18000x360"
series["clumsies_restore_drill_last_success_timestamp_seconds"] = "18000x360"
series["clumsies_release_last_success_timestamp_seconds"] = "18000x360"
series["clumsies_db_longest_query_seconds"] = "12x360"
series['clumsies_unit_last_exit_state{unit="clumsies-backup.service",state="success"}'] = "1x360"
series['clumsies_unit_last_exit_timestamp_seconds{unit="clumsies-backup.service"}'] = "18000x360"
checks = [{"expr": f"count(({expr}) == {value})", "eval_time": "6h",
           "exp_samples": [{"labels": "{}", "value": 1}]}
          for expr, value in zip(expressions, expected) if value is not None]
checks.append({"expr": f'count({expressions[3]})', "eval_time": "6h",
               "exp_samples": [{"labels": "{}", "value": 4}]})
infrastructure = json.loads((root / "grafana/dashboards/clumsies-infrastructure.json").read_text())
container_cpu = next(p for p in all_panels(infrastructure) if p["id"] == 12)
assert container_cpu["fieldConfig"]["defaults"]["unit"] == "none"
series['container_cpu_usage_seconds_total{name="clumsies-server"}'] = "0+30x360"
checks.append({"expr": "sum(" + container_cpu["targets"][0]["expr"] + ")", "eval_time": "6h",
               "exp_samples": [{"labels": "{}", "value": 0.5}]})
# A frozen textfile must not leave product row counts looking current.
invalid_snapshots = []
for name, success, timestamp in (("failed", "0x360", "0+60x360"), ("stale", "1x360", "0x360")):
    invalid = dict(series)
    invalid["clumsies_database_collection_success"] = success
    invalid["clumsies_database_collection_timestamp_seconds"] = timestamp
    invalid_snapshots.append({"name": name, "interval": "1m",
        "input_series": [{"series": name, "values": values} for name, values in invalid.items()],
        "promql_expr_test": [{"expr": expr, "eval_time": "6h", "exp_samples": []} for expr in expressions]})
# Clock rollback or future receipts must never make current evidence look healthy.
infra_panels = {p["id"]: p for p in all_panels(infrastructure)}
overview = json.loads((root / "grafana/dashboards/clumsies-overview.json").read_text())
confidence = next(p for p in all_panels(overview) if p["id"] == 21)
db_confidence = next(t["expr"] for t in confidence["targets"] if "clumsies_database_collection_success" in t["expr"])
host_confidence = next(t["expr"] for t in confidence["targets"] if t["refId"] == "D")
api_age = next(p for p in all_panels(api_dashboard) if p["id"] == 102)["targets"][0]["expr"]
for name, changed, expectations in (
    ("future database collection", {"clumsies_database_collection_timestamp_seconds": "25200x360"},
     [(db_confidence, -2), (api_age, -2), (infra_panels[3]["targets"][0]["expr"], -2),
      (infra_panels[9]["targets"][0]["expr"], None)]),
    ("future host collection", {"clumsies_observability_collection_timestamp_seconds": "25200x360"},
     [(db_confidence, -2), (host_confidence, -2), (api_age, -2), (infra_panels[3]["targets"][1]["expr"], -2),
      (infra_panels[16]["targets"][0]["expr"], -2), (infra_panels[17]["targets"][0]["expr"], -2),
      (infra_panels[26]["targets"][1]["expr"], -2), (infra_panels[18]["targets"][0]["expr"], None)]),
    ("future success receipts", {metric: "25200x360" for metric in (
        "clumsies_backup_last_success_timestamp_seconds", "clumsies_restore_drill_last_success_timestamp_seconds",
        "clumsies_release_last_success_timestamp_seconds")},
     [(infra_panels[16]["targets"][0]["expr"], -2), (infra_panels[17]["targets"][0]["expr"], -2),
      (infra_panels[26]["targets"][1]["expr"], -2),
      (infra_panels[16]["targets"][1]["expr"], 25200000),
      (infra_panels[17]["targets"][1]["expr"], 25200000),
      (infra_panels[26]["targets"][0]["expr"], 25200000)]),
    ("future unit exit", {'clumsies_unit_last_exit_timestamp_seconds{unit="clumsies-backup.service"}': "25200x360"},
     [(infra_panels[18]["targets"][0]["expr"], None)]),
):
    changed_series = dict(series, **changed)
    query_checks = [{"expr": expr, "eval_time": "6h", "exp_samples": [] if value is None else
                    [{"labels": "{}", "value": value}]} for expr, value in expectations]
    if "collection" in name:
        query_checks += [{"expr": expr, "eval_time": "6h", "exp_samples": []} for expr in expressions]
    invalid_snapshots.append({"name": name, "interval": "1m",
        "input_series": [{"series": metric, "values": values} for metric, values in changed_series.items()],
        "promql_expr_test": query_checks})
# systemd startup failure (for example Result=resources) can be known without an exit receipt.
for legacy in (False, True):
    startup = {metric: values for metric, values in series.items()
               if not metric.startswith("clumsies_unit_last_exit")}
    startup['clumsies_unit_last_exit_ok{unit="clumsies-backup.service"}'] = "0x360"
    if not legacy:
        startup['clumsies_unit_last_exit_state{unit="clumsies-backup.service",state="failed"}'] = "1x360"
    invalid_snapshots.append({"name": "known startup failure without exit timestamp" + (" legacy" if legacy else ""),
        "interval": "1m", "input_series": [{"series": metric, "values": values} for metric, values in startup.items()],
        "promql_expr_test": [
            {"expr": infra_panels[18]["targets"][0]["expr"], "eval_time": "6h", "exp_samples": [] if legacy else
             [{"labels": '{__name__="clumsies_unit_last_exit_state",unit="clumsies-backup.service",state="failed"}', "value": 1}]},
            {"expr": infra_panels[18]["targets"][1]["expr"], "eval_time": "6h", "exp_samples": [] if not legacy else
             [{"labels": '{__name__="clumsies_unit_last_exit_ok",unit="clumsies-backup.service"}', "value": 0}]},
        ]})
# Boundary equality is trusted, stale host collection suppresses current operational results.
for expr, value in ((db_confidence, 1), (host_confidence, 1), (api_age, 0),
                    (infra_panels[16]["targets"][0]["expr"], 1),
                    (infra_panels[26]["targets"][1]["expr"], 1)):
    checks.append({"expr": expr, "eval_time": "6h", "exp_samples": [{"labels": '{__name__="clumsies_database_collection_success"}' if expr == db_confidence else "{}", "value": value}]})
invalid_creation = dict(series)
invalid_creation["clumsies_oldest_open_draft_age_seconds"] = "-3600x360"
invalid_snapshots.append({"name": "future Draft creation", "interval": "1m",
    "input_series": [{"series": metric, "values": values} for metric, values in invalid_creation.items()],
    "promql_expr_test": [{"expr": expressions[-1], "eval_time": "6h", "exp_samples": [{"labels": "{}", "value": -2}]}]})
stale_host = dict(series)
stale_host["clumsies_observability_collection_timestamp_seconds"] = "0x360"
invalid_snapshots.append({"name": "stale host collection", "interval": "1m",
    "input_series": [{"series": metric, "values": values} for metric, values in stale_host.items()],
    "promql_expr_test": [{"expr": expr, "eval_time": "6h", "exp_samples": []}
        for expr in expressions + [infra_panels[16]["targets"][0]["expr"],
                                   infra_panels[17]["targets"][0]["expr"],
                                   infra_panels[18]["targets"][0]["expr"]]]})
for dashboard in dashboards:
    assert {link["title"] for link in dashboard["links"]}
    assert any("infrastructure" in link["url"] for link in dashboard["links"])
    assert all(link.get("keepTime") for link in dashboard["links"])
with tempfile.TemporaryDirectory() as directory:
    test_file = Path(directory) / "dashboards.test.json"
    test_file.write_text(json.dumps({"evaluation_interval": "1m", "tests": [{
        "interval": "1m", "input_series": [{"series": name, "values": values} for name, values in series.items()],
        "promql_expr_test": checks,
    }] + invalid_snapshots}))
    test_file.chmod(0o644)  # promtool runs as nobody, including on Linux CI.
    subprocess.run([
        "docker", "run", "--rm", "--network", "none", "--entrypoint", "/bin/promtool",
        "--volume", f"{test_file}:/tests/dashboards.test.json:ro", config["services"]["prometheus"]["image"],
        "test", "rules", "/tests/dashboards.test.json",
    ], check=True)

if not args.dashboards_only:
    subprocess.run([sys.executable, str(root / "test-collector.py")], check=True)
    subprocess.run([sys.executable, str(root / "test-requests.py")], check=True)

subprocess.run([sys.executable, str(root / "test-grafana.py")], check=True)

print("Dashboard PromQL and eight-dashboard Grafana provisioning passed." if args.dashboards_only else
      "Observability configuration, collector, tunnel links, and alert behavior passed.")
