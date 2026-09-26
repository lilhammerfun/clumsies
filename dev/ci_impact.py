#!/usr/bin/env python3
"""Select CI checks and deliveries from the repository's actual dependency boundaries."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys


CHECKS = ("docs", "scripts", "server", "daemon", "macos", "runtime", "package", "server_image")
DELIVERIES = ("server_delivery", "site_delivery")
COMPONENTS = CHECKS + DELIVERIES
NATIVE = {"macos", "runtime", "package"}
SERVER = {"server", "daemon", "server_image", "server_delivery"}
RUST = SERVER | NATIVE


def classify(paths):
    """Unknown paths run everything; Markdown embedded in a product is still code."""
    selected = set()
    for path in paths:
        if path in {"README.md", "README.zh-CN.md", "apps/macos/README.md", "crates/server/README.md", "dev/check-doc-assets.py"} or path.startswith("assets/screenshots/"):
            selected.add("docs")
        elif path.startswith(("docs/", "site/")) or path in {"package.json", "bun.lock", "dev/check-docs-search.mjs"}:
            selected.update({"docs", "site_delivery"})
        elif path.startswith((".github/ISSUE_TEMPLATE/", "archive/")) or path in {
            "LICENSE", ".github/CODEOWNERS", ".github/copilot-instructions.md",
            ".github/pull_request_template.md", ".github/workflows/pr-template.yml",
            ".github/workflows/pr-template-event.yml", "dev/validate-pr-template.py",
        }:
            pass  # The always-run checks and PR template workflows cover repository metadata.
        elif path in {".github/workflows/ci.yml", "dev/ci_impact.py", "dev/test_ci_impact.py"} or path.startswith(".github/actions/"):
            selected.update(COMPONENTS)
        elif path == ".github/workflows/server-delivery.yml":
            selected.update(SERVER | {"scripts"})
        elif path == ".github/workflows/site-delivery.yml":
            selected.update({"docs", "scripts", "site_delivery"})
        elif path == ".github/workflows/release.yml":
            selected.update(NATIVE | {"scripts"})
        elif path in {"Cargo.toml", "Cargo.lock", "rust-toolchain", "rust-toolchain.toml", "crates/server/Cargo.toml", "crates/clumsiesd/Cargo.toml"} or path.startswith(".cargo/"):
            selected.update(RUST)
        elif path in {"crates/server/Dockerfile", ".dockerignore"}:
            selected.update(SERVER | {"scripts"})
        elif path.startswith("crates/server/tests/") or path == "crates/server/clippy.toml":
            selected.add("server")
        elif path.startswith("crates/server/"):
            selected.update(SERVER)
            if path.startswith("crates/server/openapi/"):
                selected.add("macos")
        elif path.startswith("crates/clumsiesd/tests/"):
            selected.update({"daemon", "runtime"})
        elif path.startswith(("crates/clumsiesd/", "packages/clumsies/")):
            selected.update(NATIVE | {"daemon", "scripts"})
        elif path.startswith("apps/macos/Tests/"):
            selected.add("macos")
        elif path == "apps/macos/Scripts/test.sh":
            selected.update({"macos", "scripts"})
        elif path.startswith("apps/macos/Scripts/"):
            selected.update(NATIVE | {"scripts"})
        elif path.startswith("apps/macos/"):
            selected.update({"macos", "package", "scripts"})
        elif path in {"compose.production.yml", ".env.example"}:
            selected.update(SERVER | {"docs", "scripts", "site_delivery"})
        elif path in {"deploy/Caddyfile", "deploy/site.sh"}:
            selected.update({"docs", "scripts", "site_delivery"})
        elif path.startswith("deploy/server/"):
            selected.add("scripts")
        elif path in {"docker-compose.yml", "justfile"} or path.startswith(("dev/dev-", "dev/oidc/")):
            selected.update({"scripts", "server", "daemon", "runtime", "macos", "package"})
        elif path.startswith("dev/"):
            selected.update({"scripts", "server", "daemon", "macos", "runtime"})
        else:
            selected.update(COMPONENTS)
    return {name: name in selected for name in COMPONENTS}


def git(*args):
    return subprocess.check_output(["git", *args], stderr=subprocess.PIPE)


def changed_paths(base, head, *, pull_request=False):
    """Use the entire push or PR range, including both sides of renames and deletions."""
    for sha in (base, head):
        if not re.fullmatch(r"[0-9a-f]{40}", sha) or sha == "0" * 40:
            raise ValueError("A complete commit range is unavailable")
        git("cat-file", "-e", f"{sha}^{{commit}}")
    if pull_request:
        base = git("merge-base", base, head).decode().strip()
    output = git("diff", "--name-only", "--no-renames", "-z", base, head, "--")
    return [os.fsdecode(path) for path in output.split(b"\0") if path]


def event_plan(event_name, event):
    if event_name == "workflow_dispatch":
        return {name: True for name in COMPONENTS}, "Manual full validation"
    try:
        if event_name == "pull_request":
            pr = event["pull_request"]
            paths = changed_paths(pr["base"]["sha"], pr["head"]["sha"], pull_request=True)
        elif event_name == "push":
            paths = changed_paths(event["before"], event["after"])
        else:
            raise ValueError(f"Unsupported event: {event_name}")
    except (KeyError, ValueError, subprocess.CalledProcessError) as error:
        # A missing base must increase coverage, never silently classify the change as docs-only.
        return {name: True for name in COMPONENTS}, f"Full validation: {error}"
    return classify(paths), f"Classified {len(paths)} changed paths"


def gate_errors(needs):
    """Required checks must succeed; only checks absent from the plan may be skipped."""
    if needs.get("changes", {}).get("result") != "success":
        return ["Change detection did not succeed"]
    try:
        plan = json.loads(needs["changes"]["outputs"]["plan"])
        if set(plan) != set(COMPONENTS) or any(type(value) is not bool for value in plan.values()):
            raise ValueError("Invalid component plan")
    except (KeyError, ValueError, TypeError) as error:
        return [f"Missing or invalid component plan: {error}"]
    errors = []
    for name in ("checks", *CHECKS):
        result = needs.get(name, {}).get("result")
        required = name == "checks" or plan[name]
        if result != "success" and (required or result != "skipped"):
            errors.append(f"{name}: {result or 'missing'} (required={required})")
    return errors


def delivery_is_current(component, target, latest):
    """Allow intervening unrelated commits, but never automatically deploy superseded code."""
    if component not in DELIVERIES:
        raise ValueError(f"Not a delivery component: {component}")
    # A force-pushed-away commit cannot be promoted, even if its tree happens to match.
    git("merge-base", "--is-ancestor", target, latest)
    return not classify(changed_paths(target, latest))[component]


def output(name, value):
    line = f"{name}={value}"
    print(line)
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as handle:
            handle.write(line + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("plan", "gate", "current"))
    parser.add_argument("--component", choices=DELIVERIES)
    args = parser.parse_args()
    if args.command == "plan":
        event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text())
        plan, reason = event_plan(os.environ["GITHUB_EVENT_NAME"], event)
        print(reason)
        output("plan", json.dumps(plan, separators=(",", ":")))
        for name, selected in plan.items():
            output(name, str(selected).lower())
        if os.environ.get("GITHUB_STEP_SUMMARY"):
            with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as handle:
                handle.write(f"## CI impact\n\n{reason}\n\n| Component | Selected |\n|---|---|\n")
                for name, selected in plan.items():
                    handle.write(f"| {name} | {'yes' if selected else 'no'} |\n")
    elif args.command == "gate":
        errors = gate_errors(json.loads(os.environ["CI_NEEDS"]))
        if errors:
            sys.exit("\n".join(errors))
        print("All required checks passed; unrelated checks were explicitly skipped.")
    else:
        if not args.component:
            parser.error("current requires --component")
        target = os.environ["GITHUB_SHA"]
        git("fetch", "origin", "main")
        latest = git("rev-parse", "origin/main").decode().strip()
        try:
            current = delivery_is_current(args.component, target, latest)
        except subprocess.CalledProcessError:
            # An unavailable history or a non-ancestor target is not permission to deploy.
            current = False
        output("current", str(current).lower())
        if not current:
            print(f"Skip superseded {args.component} at {target}; main is {latest}")


if __name__ == "__main__":
    main()
