#!/usr/bin/env python3
"""Validate the shipped configuration without production credentials or services."""

import json
from pathlib import Path
import subprocess


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

print("Observability configuration, tunnel links, and alert behavior passed.")
