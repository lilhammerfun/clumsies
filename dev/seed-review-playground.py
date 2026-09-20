#!/usr/bin/env python3
"""Create interactive Review scenarios through the current local Dev Instance's API."""

import argparse
import base64
import hashlib
import http.cookiejar
import json
import os
from pathlib import Path
import secrets
import subprocess
import urllib.error
import urllib.parse
import urllib.request


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Playground:
    def __init__(self):
        worktree = Path(__file__).resolve().parent.parent
        instance = hashlib.sha256(str(worktree).encode()).hexdigest()[:12]
        root = Path(os.environ.get("CLUMSIES_DEV_ROOT",
                    str(Path.home() / "Library/Application Support/ai.clumsies.dev")))
        self.root = root / "instances" / instance
        runtime = json.loads((self.root / "runtime.json").read_text())
        if (runtime["mode"] != "local" or runtime["instance_id"] != instance
                or runtime["worktree_path"] != str(worktree)
                or runtime["identities"]["bundle_id"] != "ai.clumsies.desktop.dev." + instance):
            raise RuntimeError("This command requires this worktree's local Dev Instance.")
        self.instance = instance
        self.runtime = runtime
        self.origin = runtime["server_url"]
        self.issuer = runtime["oidc_issuer"]
        for url in (self.origin, self.issuer):
            parsed = urllib.parse.urlsplit(url)
            if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or not parsed.port:
                raise RuntimeError("Only loopback Dev Server and fake OIDC are allowed.")
        self.http = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), NoRedirect(),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
        self.token = None
        self.head = None
        self.manifest_path = self.root / "review-playground.json"
        self.manifest = {"server_url": self.origin, "scenarios": []}
        self.resources = {}

    def call(self, method, path, body=None, headers=None):
        url = self.origin + path
        request_headers = {"Content-Type": "application/json", **(headers or {})}
        if self.token:
            request_headers["Authorization"] = "Bearer " + self.token
        if method not in ("GET", "HEAD"):
            request_headers["Idempotency-Key"] = secrets.token_hex(16)
        request = urllib.request.Request(url, method=method, headers=request_headers,
            data=None if body is None else json.dumps(body).encode())
        try:
            with self.http.open(request, timeout=60) as response:
                data = response.read()
                return json.loads(data) if data else None
        except urllib.error.HTTPError as error:
            # API error envelopes contain no credential headers or request bodies.
            raise RuntimeError(f"{method} {path}: {error.code}: {error.read().decode()}") from None

    def login(self):
        verifier = secrets.token_urlsafe(48)
        state = secrets.token_urlsafe(24)
        callback = "http://127.0.0.1/callback"
        arguments = {"redirect_uri": callback, "state": state,
            "code_challenge": base64.urlsafe_b64encode(
                hashlib.sha256(verifier.encode()).digest()).decode().rstrip("="),
            "code_challenge_method": "S256"}
        if self.call("GET", "/api/v1/setup")["state"] == "setup_required":
            settings = dict(line.split("=", 1) for line in
                (self.root / "compose.env").read_text().splitlines() if "=" in line)
            session = self.call("POST", "/api/v1/setup/sessions",
                {"setup_code": settings["CLUMSIES_SETUP_CODE"]})
            headers = {"x-csrf-token": session["csrf_token"]}
            self.call("PUT", "/api/v1/setup/configuration", {
                "org_name": "Review UX Lab", "default_project_name": "Review Playground",
                "allowed_email_domains": []}, headers)
            url = self.call("POST", "/api/v1/setup/oidc-authorizations",
                            arguments, headers)["authorization_url"]
        else:
            url = self.origin + "/oauth2/authorization/oidc?" + urllib.parse.urlencode(
                {**arguments, "client_kind": "desktop"})
        allowed = {urllib.parse.urlsplit(u).netloc for u in (self.origin, self.issuer)}
        for _ in range(12):
            parsed = urllib.parse.urlsplit(url)
            if parsed.scheme == "http" and parsed.netloc == "127.0.0.1" and parsed.path == "/callback":
                result = urllib.parse.parse_qs(parsed.query)
                if result.get("state") != [state] or "code" not in result:
                    raise RuntimeError("OIDC callback did not match this login.")
                token = self.call("POST", "/api/v1/auth/token", {
                    "grant_type": "authorization_code", "code": result["code"][0],
                    "redirect_uri": callback, "code_verifier": verifier})
                self.token = token["access_token"]
                self.refresh_token = token["refresh_token"]
                me = self.call("GET", "/api/v1/me")
                self.project = me["default_project_id"]
                return
            if parsed.scheme != "http" or parsed.netloc not in allowed:
                raise RuntimeError("OIDC tried to leave this local Dev Instance.")
            try:
                self.http.open(url, timeout=30).close()
                raise RuntimeError("Expected the local fake OIDC provider to redirect.")
            except urllib.error.HTTPError as response:
                if response.code not in (301, 302, 303, 307, 308):
                    raise RuntimeError(f"Local OIDC failed: HTTP {response.code}") from None
                url = urllib.parse.urljoin(url, response.headers["Location"])
        raise RuntimeError("Too many local OIDC redirects.")

    def prepare_app(self):
        # Use the same daemon bootstrap and credential installation as native login.
        # Tokens stay in memory and an stdin pipe; the daemon owns Keychain storage.
        paths = self.runtime["paths"]
        expected = {
            "app": self.root / "macos-derived/Build/Products/Debug" / f"ClumsiesDev-{self.instance}.app",
            "daemon_root": self.root / "daemon", "cache": self.root / "cache",
            "logs": self.root / "logs/daemon", "launch_agents": self.root / "LaunchAgents",
            "codex_home": self.root / "codex-home",
        }
        if any(Path(paths[key]) != value or not value.resolve().is_relative_to(self.root.resolve())
               for key, value in expected.items()):
            raise RuntimeError("Dev paths must belong to this instance.")
        # A rebuilt ad-hoc binary can prompt for access to the previous item's
        # ACL. Replace only this disposable fake-OIDC session before bootstrap;
        # the current daemon then creates and owns its own Keychain item.
        removed = subprocess.run(["security", "delete-generic-password",
            "-s", "ai.clumsies.dev." + self.instance, "-a", "server-session"],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=15)
        if removed.returncode not in (0, 44):  # 44: errSecItemNotFound
            raise RuntimeError("Could not renew this local Dev Instance's test credentials.")
        environment = {**os.environ, "CLUMSIES_DEV_INSTANCE_ID": self.instance,
            "CLUMSIES_SERVER_URL": self.origin, "CLUMSIES_DAEMON_ROOT": paths["daemon_root"],
            "CLUMSIES_DAEMON_CACHE_DIR": paths["cache"], "CLUMSIES_DAEMON_LOG_DIR": paths["logs"],
            "CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR": paths["launch_agents"],
            "CODEX_HOME": paths["codex_home"]}
        subprocess.run([str(Path(paths["app"]) / "Contents/Resources/clumsiesd"),
                        "--reconcile-launch-agent"], env=environment, check=True,
                       stdout=subprocess.DEVNULL, timeout=45)
        credentials = json.dumps({"server_url": self.origin, "project_id": self.project,
            "access_token": self.token, "refresh_token": self.refresh_token})
        subprocess.run(["swift", str(Path(__file__).with_name("dev-login.swift")), self.instance],
                       input=credentials, text=True, check=True, timeout=90)
        subprocess.run(["defaults", "write", self.runtime["identities"]["bundle_id"],
                        "ClumsiesAgentSetupCompleted", "-bool", "true"], check=True)

    def current_head(self):
        latest = self.call("GET", "/api/v1/org/commit-state")["latest"]
        return latest["commit_id"] if latest else None

    def etag(self):
        return {"If-Match": '"' + (self.head or "ref-none") + '"'}

    def draft(self, path, text=None, action="update", destination=None):
        resource = {"scope": "org", "path": path}
        if path in self.resources:
            resource["id"] = self.resources[path]
        operation = {"action": action, "resource": resource}
        if text is not None:
            operation["content"] = {"content": text}
        if destination:
            operation["new_path"] = destination
        return self.call("POST", "/api/v1/drafts", {
            "daemon_installation_id": "review-playground", "project_id": self.project,
            "base_commit_id": self.head, "title": path, "resource": resource,
            "operations": [operation]})

    def review(self, title, description, drafts):
        return self.call("POST", "/api/v1/reviews", {
            "title": title, "description": description,
            "drafts": [{"draft_id": d["draft"]["draft_id"],
                        "expected_draft_version": d["draft"]["version"]} for d in drafts]}, self.etag())

    def publish(self, detail):
        result = self.call("POST", f'/api/v1/reviews/{detail["review"]["review_id"]}/merges',
            {"expected_review_version": detail["review"]["version"]}, self.etag())
        self.head = result["commit_id"]

    def scenario(self, title, description, drafts):
        detail = self.review(title, description, drafts)
        self.manifest["scenarios"].append({
            "review_id": detail["review"]["review_id"], "title": title, "check": description})
        self.save()
        return detail

    def save(self):
        self.manifest_path.write_text(json.dumps(self.manifest, ensure_ascii=False, indent=2))
        self.manifest_path.chmod(0o600)

    def detail(self, review):
        return self.call("GET", f'/api/v1/reviews/{review["review"]["review_id"]}')

    def plan(self, detail):
        detail = self.detail(detail)
        return self.call("POST", f'/api/v1/reviews/{detail["review"]["review_id"]}/update-plans',
            {"expected_review_version": detail["review"]["version"]})

    def update(self, detail):
        plan = self.plan(detail)
        candidates = {c["draft_id"]: c for c in plan["candidates"]}
        if any(c["status"] != "clean" for c in candidates.values()):
            raise RuntimeError("A fixture expected to merge automatically has a conflict.")
        requests = [{"draft_id": item["draft"]["draft_id"],
                     "expected_draft_version": item["draft"]["version"],
                     **({"candidate_id": candidates[item["draft"]["draft_id"]]["candidate_id"]}
                        if item["draft"]["draft_id"] in candidates else {})}
                    for item in plan["detail"]["drafts"]]
        return self.call("POST", f'/api/v1/reviews/{detail["review"]["review_id"]}/updates', {
            "expected_review_version": plan["detail"]["review"]["version"], "drafts": requests},
            self.etag())

    def seed(self):
        run = secrets.token_hex(3)
        prefix = f"review-playground/{run}"
        path = lambda name: f"{prefix}/{name}.md"
        log = "# 日志策略\n\n日志保留 7 天。\n\n归档后压缩。\n"
        automatic = "# 交付检查\n\n版本：1\n\n说明段落一。\n说明段落二。\n说明段落三。\n\n状态：待检查\n"
        multiple = "# 中文与多段冲突\n\n日志保留 7 天。\n\n中间说明一。\n中间说明二。\n中间说明三。\n\n重试次数：3\n\n尾部说明一。\n尾部说明二。\n尾部说明三。\n\n=======\n这行是原文中的分隔符，应原样保留。\n"
        baseline = {
            "01-mixed/log-policy": log, "01-mixed/auto-merge": automatic,
            "01-mixed/already-current": "# 当前文件\n\n草稿内容。\n",
            "02-auto/guide-a": automatic, "02-auto/guide-b": automatic,
            "03-sections/中文规则": multiple,
            "04-rename/original": "# 重命名冲突\n\n正文保持不变。\n",
            "05-remote-delete/note": "# Remote 删除\n\n原始内容。\n",
            "06-draft-delete/note": "# Draft 删除\n\n原始内容。\n",
            "07-discard-secondary/keep-a": log, "07-discard-secondary/discard": log,
            "07-discard-secondary/keep-b": log,
            "08-discard-primary/discard": log, "08-discard-primary/keep": log,
            "09-ready/edit": log, "09-ready/rename": log, "09-ready/delete": log,
            "10-updated/note": automatic, "11-rejected/note": automatic,
            "_remote-tick": "# Remote revision\n\nInitial version.\n",
        }
        self.head = self.current_head()
        base_drafts = [self.draft(path(name), text, "create") for name, text in baseline.items()]
        self.publish(self.review("Fixture setup · 原始版本", "测试数据基线。", base_drafts))
        memories = self.call("GET", "/api/v1/org/memories")["items"]
        self.resources = {m["path"]: m["memory_id"] for m in memories}
        self.manifest.update(project_id=self.project, prefix=prefix)
        def edit(name, text):
            return self.draft(path(name), text)
        pending = []
        mixed = self.scenario("01 · 混合文件：冲突 / 自动合并 / 已是最新",
            "log-policy：Remote 为 14 天，Draft 为 30 天，在详情直接选择。"
            "auto-merge 显示普通 Diff 和 Auto-rebased 标签。already-current 显示普通 Diff。", [
                edit("01-mixed/log-policy", log.replace("7 天", "30 天")),
                edit("01-mixed/auto-merge", automatic.replace("待检查", "草稿已检查")),
                edit("01-mixed/already-current", "# 当前文件\n\n本次草稿已更新到最新 Remote。\n")])
        pending.append(mixed)
        pending.append(self.scenario("02 · 全部自动合并（两个文件）",
            "两个文件都是独立行修改，详情显示普通 Diff 和 Auto-rebased 标签。通过工具栏保存整组更新后可批准。",
            [edit("02-auto/" + name, automatic.replace("待检查", "草稿已检查")) for name in ("guide-a", "guide-b")]))
        pending.append(self.scenario("03 · 多段正文冲突与中文",
            "分别处理两个冲突：日志保留天数、重试次数。可混合选择 Remote / Draft，切换文件或页签应保留输入。"
            "正文原有的 ======= 分隔符不能被误判为冲突。",
            [edit("03-sections/中文规则", multiple.replace("7 天", "30 天").replace("次数：3", "次数：5"))]))
        pending.append(self.scenario("04 · 同一个文件被重命名到不同路径",
            "Remote 改名为 remote-name.md，Draft 改名为 draft-name.md。确认最终路径，正文不应丢失。",
            [self.draft(path("04-rename/original"), action="rename", destination=path("04-rename/draft-name"))]))
        pending.append(self.scenario("05 · Remote 删除，Draft 修改",
            "Remote 已删除文件，草稿仍修改正文。在详情选择保留 Draft 文件或采用 Remote 删除。",
            [edit("05-remote-delete/note", "# Remote 删除\n\n草稿希望保留的新增说明。\n")]))
        pending.append(self.scenario("06 · Draft 删除，Remote 修改",
            "草稿要删除文件，但 Remote 增加了内容。在详情选择保留 Remote 或确认删除，随后显示对应 Diff。",
            [self.draft(path("06-draft-delete/note"), action="delete")]))
        for number, folder, names, discarded_index in [
            ("07", "discard-secondary", ["keep-a", "discard", "keep-b"], 1),
            ("08", "discard-primary", ["discard", "keep"], 0),
        ]:
            detail = self.scenario(f"{number} · 丢弃{'中间' if number == '07' else '首个'}文件后继续审批",
                "已通过实际 API 丢弃一个成员。文件树只应显示剩余文件；更新后能正常批准，不会被已丢弃内容卡住。",
                [edit(f"{number}-{folder}/{name}", log.replace("7 天", "30 天")) for name in names])
            removed = detail["drafts"][discarded_index]["draft"]
            self.call("DELETE", f'/api/v1/drafts/{removed["draft_id"]}',
                headers={"If-Match": f'"{removed["version"]}"'})
            pending.append(detail)
        updated = self.scenario("10 · 已完成整组更新，等待批准",
            "已完成整组更新。此 Review 不应再显示保存更新按钮，可以直接批准。",
            [edit("10-updated/note", automatic.replace("待检查", "草稿已检查"))])
        rejected = self.scenario("11 · 已拒绝，更新后重新提交",
            "切换列表筛选到 Rejected 或 All。作者应能更新 Remote，然后 Resubmit，再完成审批。",
            [edit("11-rejected/note", automatic.replace("待检查", "草稿已检查"))])
        self.call("POST", f'/api/v1/reviews/{rejected["review"]["review_id"]}/decisions', {
            "expected_review_version": rejected["review"]["version"], "decision": "rejected",
            "body": "请更新到最新 Remote 后重新提交。"})
        pending.append(rejected)
        pending.append(self.scenario("12 · 同一路径各自新增文件",
            "Remote 和 Draft 都创建了 new-guide.md，内容不同。选择最终版本，不应误建第二份同名文件。",
            [self.draft(path("12-add-add/new-guide"), "# 草稿新增\n\n采用 30 天方案。\n", "create")]))
        remote = [
            edit("01-mixed/log-policy", log.replace("7 天", "14 天")),
            edit("01-mixed/auto-merge", automatic.replace("版本：1", "版本：2")),
            *[edit("02-auto/" + name, automatic.replace("版本：1", "版本：2")) for name in ("guide-a", "guide-b")],
            edit("03-sections/中文规则", multiple.replace("7 天", "14 天").replace("次数：3", "次数：8")),
            self.draft(path("04-rename/original"), action="rename", destination=path("04-rename/remote-name")),
            self.draft(path("05-remote-delete/note"), action="delete"),
            edit("06-draft-delete/note", "# Draft 删除\n\nRemote 新增的重要说明。\n"),
            edit("10-updated/note", automatic.replace("版本：1", "版本：2")),
            edit("11-rejected/note", automatic.replace("版本：1", "版本：2")),
            self.draft(path("12-add-add/new-guide"), "# Remote 新增\n\n采用 14 天方案。\n", "create"),
        ]
        self.publish(self.review("Fixture setup · Remote 已生效的修改", "用于制造真实冲突。", remote))
        plan = self.plan(mixed)
        current_id = mixed["drafts"][2]["draft"]["draft_id"]
        candidate = next(c for c in plan["candidates"] if c["draft_id"] == current_id)
        self.call("POST", f"/api/v1/drafts/{current_id}/rebases", {
            "candidate_id": candidate["candidate_id"], "expected_draft_version": candidate["draft_version"]},
            self.etag())
        self.update(updated)
        self.scenario("09 · 无需更新：正文 / 仅重命名 / 仅删除",
            "当前版本的 Review，不应出现更新入口。逐项检查正文 diff、仅重命名、仅删除空状态，批准可直接生效。", [
                edit("09-ready/edit", log.replace("7 天", "30 天")),
                self.draft(path("09-ready/rename"), action="rename", destination=path("09-ready/renamed")),
                self.draft(path("09-ready/delete"), action="delete")])
        for item in pending:
            self.plan(item)
        for item in self.manifest["scenarios"]:
            detail = self.call("GET", f'/api/v1/reviews/{item["review_id"]}')
            item.update(status=detail["review"]["status"],
                        freshness=detail["review"]["coordination"]["freshness"],
                        reconciliation=detail["review"]["coordination"]["reconciliation"],
                        file_count=len(detail["drafts"]))
        self.manifest["scenarios"].sort(key=lambda s: s["title"])
        self.manifest["head"] = self.head
        self.save()
        self.verify()

    def verify(self):
        """Check the real API fixture contract before handing it to a tester."""
        self.manifest = json.loads(self.manifest_path.read_text())
        expected = {
            "01": ("open", "behind", "conflicts", 3),
            "02": ("open", "behind", "clean", 2),
            "03": ("open", "behind", "conflicts", 1),
            "04": ("open", "behind", "conflicts", 1),
            "05": ("open", "behind", "conflicts", 1),
            "06": ("open", "behind", "conflicts", 1),
            "07": ("open", "behind", "clean", 2),
            "08": ("open", "behind", "clean", 1),
            "09": ("open", "current", "unknown", 3),
            "10": ("open", "current", "unknown", 1),
            "11": ("rejected", "behind", "clean", 1),
            "12": ("open", "behind", "conflicts", 1),
        }
        scenarios = self.manifest["scenarios"]
        if len(scenarios) != 12 or {s["title"][:2] for s in scenarios} != set(expected):
            raise RuntimeError("The Review playground is incomplete.")
        for item in scenarios:
            detail = self.call("GET", f'/api/v1/reviews/{item["review_id"]}')
            review = detail["review"]
            actual = (review["status"], review["coordination"]["freshness"],
                      review["coordination"]["reconciliation"], len(detail["drafts"]))
            if actual != expected[item["title"][:2]]:
                raise RuntimeError(f'Fixture state changed: {item["title"]}: {actual}')
        print("All 12 initial Review states verified through the Server API.")

    def advance_remote(self):
        self.manifest = json.loads(self.manifest_path.read_text())
        self.project = self.manifest["project_id"]
        self.head = self.current_head()
        item = self.draft(self.manifest["prefix"] + "/_remote-tick.md",
                          "# Remote revision\n\n" + secrets.token_hex(8) + "\n")
        self.publish(self.review("Fixture · Remote 再次更新",
            "先打开冲突编辑，再执行此命令，检查旧结果提交被拒绝且输入保留。", [item]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--advance-remote", action="store_true",
                        help="Advance only this playground's remote version to test stale submissions.")
    modes.add_argument("--prepare-app", action="store_true",
                        help="Initialize, sign in and seed before the local Dev App opens.")
    modes.add_argument("--verify", action="store_true",
                       help="Check all initial fixture states before interactive testing changes them.")
    args = parser.parse_args()
    os.umask(0o077)
    lab = Playground()
    if lab.manifest_path.exists() and not (args.advance_remote or args.prepare_app or args.verify):
        raise SystemExit("This instance already has a playground. Its data was left unchanged.")
    lab.login()
    if args.prepare_app:
        lab.prepare_app()
    if args.verify:
        lab.verify()
    elif args.advance_remote:
        lab.advance_remote()
    elif lab.manifest_path.exists():
        lab.manifest = json.loads(lab.manifest_path.read_text())
    else:
        lab.seed()
    print(lab.manifest_path)
    for scenario in lab.manifest["scenarios"]:
        print(scenario["title"])


if __name__ == "__main__":
    main()
