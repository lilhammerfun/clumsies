#!/usr/bin/env python3
"""Validate the shipped configuration using disposable containers and no production credentials."""

import json
from pathlib import Path
import subprocess
import sys
import tempfile


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
assert len(dashboards) == 6
def all_panels(dashboard):
    for panel in dashboard["panels"]:
        yield panel
        yield from panel.get("panels", [])


for name in ("overview", "api", "web"):
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
expressions = [t["expr"] for p in all_panels(api_dashboard) for t in p.get("targets", [])
               if any(name.split("{")[0] in t["expr"] for name in series)]
assert len(expressions) == len(expected)
checks = [{"expr": f"count(({expr}) == {value})", "eval_time": "6h",
           "exp_samples": [{"labels": "{}", "value": 1}]}
          for expr, value in zip(expressions, expected) if value is not None]
checks.append({"expr": f'count({expressions[3]})', "eval_time": "6h",
               "exp_samples": [{"labels": "{}", "value": 4}]})
with tempfile.TemporaryDirectory() as directory:
    test_file = Path(directory) / "dashboards.test.json"
    test_file.write_text(json.dumps({"evaluation_interval": "1m", "tests": [{
        "interval": "1m", "input_series": [{"series": name, "values": values} for name, values in series.items()],
        "promql_expr_test": checks,
    }]}))
    test_file.chmod(0o644)  # promtool runs as nobody, including on Linux CI.
    subprocess.run([
        "docker", "run", "--rm", "--network", "none", "--entrypoint", "/bin/promtool",
        "--volume", f"{test_file}:/tests/dashboards.test.json:ro", config["services"]["prometheus"]["image"],
        "test", "rules", "/tests/dashboards.test.json",
    ], check=True)

subprocess.run([sys.executable, str(root / "test-collector.py")], check=True)

subprocess.run([sys.executable, str(root / "test-requests.py")], check=True)

print("Observability configuration, collector, tunnel links, and alert behavior passed.")
