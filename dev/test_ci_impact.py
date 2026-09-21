"""Exercise component boundaries, real Git ranges, and the required CI gate."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from ci_impact import CHECKS, COMPONENTS, classify, changed_paths, delivery_is_current, event_plan, gate_errors


class ImpactTests(unittest.TestCase):
    def selected(self, *paths):
        return {name for name, enabled in classify(paths).items() if enabled}

    def test_docs_do_not_build_or_deploy_the_app_or_server(self):
        for path in ("README.md", "README.zh-CN.md", "assets/screenshots/dashboard.png", "apps/macos/README.md", "crates/server/README.md"):
            with self.subTest(path=path):
                self.assertEqual(self.selected(path), {"docs"})
        for path in ("docs/zh/index.md", "docs/.vitepress/config.mts", "site/index.html", "site/assets/site.css", "bun.lock", "package.json"):
            with self.subTest(path=path):
                self.assertEqual(self.selected(path), {"docs", "site_delivery"})

    def test_source_and_test_boundaries_preserve_cross_component_coverage(self):
        cases = {
            "crates/server/src/routes.rs": {"server", "daemon", "server_image", "server_delivery"},
            "crates/server/migrations/new.sql": {"server", "daemon", "server_image", "server_delivery"},
            "crates/server/openapi/api.yaml": {"server", "daemon", "macos", "server_image", "server_delivery"},
            "crates/server/tests/health.rs": {"server"},
            "crates/daemon/src/ipc.rs": {"daemon", "runtime", "macos", "package", "scripts"},
            "crates/daemon/tests/server_integration.rs": {"daemon", "runtime"},
            "apps/macos/Sources/App/AppDelegate.swift": {"macos", "package", "scripts"},
            "apps/macos/Tests/App/Test.swift": {"macos"},
            "apps/macos/Scripts/test.sh": {"macos", "scripts"},
        }
        for path, expected in cases.items():
            with self.subTest(path=path):
                self.assertEqual(self.selected(path), expected)

    def test_markdown_and_assets_embedded_in_products_are_not_documentation_only(self):
        for path in ("packages/clumsies/skills/project-memory/SKILL.md", "packages/clumsies/.mcp.json.tpl", "packages/clumsies/.codex-plugin/plugin.json"):
            with self.subTest(path=path):
                self.assertTrue({"daemon", "runtime", "package"} <= self.selected(path))
        for path in ("apps/macos/Resources/CLUMSIES.md", "apps/macos/Resources/MemoryStarter/knowledge/README.md", "apps/macos/Resources/Assets.xcassets/icon.png"):
            with self.subTest(path=path):
                self.assertTrue({"macos", "package"} <= self.selected(path))

    def test_shared_build_configuration_reaches_all_consumers(self):
        for path in ("Cargo.lock", "Cargo.toml", "crates/server/Cargo.toml", "crates/daemon/Cargo.toml", "rust-toolchain.toml", ".cargo/config.toml"):
            with self.subTest(path=path):
                self.assertTrue({"server", "daemon", "runtime", "macos", "package", "server_image", "server_delivery"} <= self.selected(path))
        for path in ("crates/server/Dockerfile", ".dockerignore"):
            self.assertTrue({"server_image", "server_delivery"} <= self.selected(path))

    def test_delivery_scripts_and_workflows_select_their_validation(self):
        for path in ("deploy/site.sh", "deploy/Caddyfile", ".github/workflows/site-delivery.yml"):
            self.assertEqual(self.selected(path), {"docs", "scripts", "site_delivery"})
        self.assertEqual(self.selected("deploy/server/server-release.sh"), {"scripts"})
        self.assertTrue({"scripts", "server", "server_image", "server_delivery"} <= self.selected(".github/workflows/server-delivery.yml"))
        self.assertTrue({"scripts", "runtime", "macos", "package"} <= self.selected(".github/workflows/release.yml"))
        for path in ("compose.production.yml", ".env.example"):
            self.assertTrue({"site_delivery", "server_delivery", "server_image", "scripts"} <= self.selected(path))

    def test_unknown_paths_and_ci_control_plane_fail_closed(self):
        for path in ("new-component/source.ext", ".gitignore", ".github/workflows/ci.yml", "dev/ci_impact.py", "dev/test_ci_impact.py"):
            self.assertEqual(self.selected(path), set(COMPONENTS))
        self.assertEqual(self.selected(), set())
        self.assertEqual(self.selected("README.md", "crates/server/src/main.rs"), {"docs", "server", "daemon", "server_image", "server_delivery"})

    def test_manual_and_missing_ranges_run_full_validation(self):
        for event_name, event in (
            ("workflow_dispatch", {}), ("push", {"before": "0" * 40, "after": "a" * 40}),
            ("push", {}), ("pull_request", {}), ("unknown", {}),
        ):
            plan, reason = event_plan(event_name, event)
            self.assertTrue(all(plan.values()), reason)

    def test_gate_distinguishes_planned_skips_from_failure_or_missing_results(self):
        plan = classify(["README.md"])
        needs = {name: {"result": "success" if plan[name] else "skipped"} for name in CHECKS}
        needs["checks"] = {"result": "success"}
        needs["changes"] = {"result": "success", "outputs": {"plan": json.dumps(plan)}}
        self.assertEqual(gate_errors(needs), [])
        for name in ("changes", "checks", *CHECKS):
            for result in ("failure", "cancelled", "timed_out", None):
                with self.subTest(name=name, result=result):
                    changed = json.loads(json.dumps(needs))
                    changed[name]["result"] = result
                    self.assertTrue(gate_errors(changed))
        for name in ("changes", "checks", "docs"):
            changed = json.loads(json.dumps(needs))
            changed[name]["result"] = "skipped"
            self.assertTrue(gate_errors(changed))
        for invalid in ("{}", "null", "not-json", json.dumps({name: "false" for name in COMPONENTS})):
            needs["changes"]["outputs"]["plan"] = invalid
            self.assertTrue(gate_errors(needs))


class GitRangeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="clumsies-ci-test-")
        self.previous = Path.cwd()
        os.chdir(self.directory.name)
        self.addCleanup(self.directory.cleanup)
        self.addCleanup(os.chdir, self.previous)
        self.run_git("init", "-q", "-b", "main")
        self.run_git("config", "user.name", "CI Test")
        self.run_git("config", "user.email", "ci-test@example.invalid")
        self.base = self.commit("README.md", "base")

    def run_git(self, *args):
        return subprocess.check_output(["git", *args], stderr=subprocess.PIPE).decode().strip()

    def commit(self, path, content):
        file = Path(path)
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(content)
        self.run_git("add", "-A")
        self.run_git("commit", "-qm", "test change")
        return self.run_git("rev-parse", "HEAD")

    def test_push_range_keeps_earlier_code_when_last_commit_is_docs(self):
        self.commit("crates/server/src/main.rs", "server")
        head = self.commit("README.md", "documentation")
        plan, _ = event_plan("push", {"before": self.base, "after": head})
        self.assertTrue(plan["server"])
        self.assertTrue(plan["server_delivery"])
        self.assertTrue(plan["docs"])

    def test_pull_request_uses_merge_base_without_including_unrelated_main_changes(self):
        self.run_git("checkout", "-qb", "topic")
        head = self.commit("README.md", "topic docs")
        self.run_git("checkout", "-q", "main")
        base = self.commit("crates/server/src/main.rs", "unrelated main change")
        plan, _ = event_plan("pull_request", {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}})
        self.assertEqual({key for key, value in plan.items() if value}, {"docs"})

    def test_renames_deletions_and_unusual_filenames_do_not_hide_source_changes(self):
        base = self.commit("crates/server/src/old.rs", "source")
        Path("docs").mkdir()
        self.run_git("mv", "crates/server/src/old.rs", "docs/renamed.md")
        head = self.commit("docs/spaces and\nnewlines.md", "docs")
        paths = changed_paths(base, head)
        self.assertIn("crates/server/src/old.rs", paths)
        self.assertIn("docs/renamed.md", paths)
        self.assertIn("docs/spaces and\nnewlines.md", paths)
        self.assertTrue(classify(paths)["server"])
        self.run_git("rm", "docs/renamed.md")
        deleted = self.commit("README.md", "delete a page")
        self.assertIn("docs/renamed.md", changed_paths(head, deleted))

    def test_intervening_docs_do_not_drop_a_validated_server_delivery(self):
        server = self.commit("crates/server/src/main.rs", "server v1")
        docs = self.commit("docs/index.md", "docs")
        self.assertTrue(delivery_is_current("server_delivery", server, docs))
        newer = self.commit("crates/server/src/main.rs", "server v2")
        self.assertFalse(delivery_is_current("server_delivery", server, newer))
        self.assertTrue(delivery_is_current("site_delivery", docs, newer))
        self.assertTrue(delivery_is_current("server_delivery", newer, newer))

    def test_force_pushed_away_commits_are_not_deployed(self):
        self.run_git("checkout", "-qb", "old")
        old = self.commit("docs/old.md", "old")
        self.run_git("checkout", "-q", "main")
        latest = self.commit("docs/new.md", "new")
        with self.assertRaises(subprocess.CalledProcessError):
            delivery_is_current("server_delivery", old, latest)


if __name__ == "__main__":
    unittest.main()
