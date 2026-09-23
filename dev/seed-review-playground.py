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
        self.manifest_path = self.root / "hotel-review-playground.json"
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

    def login(self, account="owner"):
        verifier = secrets.token_urlsafe(48)
        state = secrets.token_urlsafe(24)
        callback = "http://127.0.0.1/callback"
        arguments = {"redirect_uri": callback, "state": state,
            "code_challenge": base64.urlsafe_b64encode(
                hashlib.sha256(verifier.encode()).digest()).decode().rstrip("="),
            "code_challenge_method": "S256"}
        if account not in ("owner", "project-admin", "member-a", "member-b"):
            raise ValueError("Unknown local test account")
        if self.call("GET", "/api/v1/setup")["state"] == "setup_required":
            if account != "owner":
                raise RuntimeError("Initialize this Dev Instance with owner before logging in members.")
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
                self.me = me
                return
            if parsed.scheme != "http" or parsed.netloc not in allowed:
                raise RuntimeError("OIDC tried to leave this local Dev Instance.")
            try:
                request = url
                if parsed.netloc == urllib.parse.urlsplit(self.issuer).netloc and parsed.path == "/clumsies/authorize":
                    # v4 fake OIDC supports form login even with interactiveLogin disabled.
                    # Each session still completes the server's PKCE/state/token exchange.
                    claims = {"sub": "local-" + account, "email": account + "@clumsies.local",
                              "email_verified": True, "name": "Local " + account.title()}
                    request = urllib.request.Request(url, data=urllib.parse.urlencode({
                        "username": claims["sub"], "claims": json.dumps(claims)}).encode(),
                        headers={"Content-Type": "application/x-www-form-urlencoded"})
                self.http.open(request, timeout=30).close()
                raise RuntimeError("Expected the local fake OIDC provider to redirect.")
            except urllib.error.HTTPError as response:
                if response.code not in (301, 302, 303, 307, 308):
                    raise RuntimeError(f"Local OIDC failed: HTTP {response.code}") from None
                url = urllib.parse.urljoin(url, response.headers["Location"])
        raise RuntimeError("Too many local OIDC redirects.")

    def ensure_test_accounts(self):
        """Invite test identities and grant roles using the ordinary owner APIs."""
        members = {item["email"]: item["user_id"]
                   for item in self.call("GET", "/api/v1/admin/members")["items"]}
        project_members = {item["user"]["user_id"]: item["role"] for item in
            self.call("GET", f"/api/v1/admin/projects/{self.project}/members")["items"]}
        for account in ("project-admin", "member-a", "member-b"):
            email = account + "@clumsies.local"
            user_id = members.get(email)
            if user_id is None:
                member = self.call("POST", "/api/v1/admin/members", {"email": email, "role": "member"})
                user_id = member["user_id"]
            role = "admin" if account == "project-admin" else "member"
            if user_id not in project_members:
                self.call("POST", f"/api/v1/admin/projects/{self.project}/members", {"user_id": user_id, "role": role})
            elif project_members[user_id] != role:
                self.call("PATCH", f"/api/v1/admin/projects/{self.project}/members/{user_id}", {"role": role})

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

    def scenario(self, case, title, description, drafts, check):
        detail = self.review(title, description, drafts)
        self.manifest["scenarios"].append({
            "case": case, "review_id": detail["review"]["review_id"], "title": title, "check": check})
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
        project = self.call("POST", "/api/v1/projects", {
            "name": "青禾酒店", "description": "住客服务指南与门店运营手册。"})
        self.project = project["project_id"]
        prefix = "青禾酒店"
        path = lambda name: f"{prefix}/{name}.md"
        guide = (
            "# 入住指南\n\n"
            "## 办理入住\n\n入住时间为下午 15:00，请携带有效身份证件。\n\n"
            "## 早餐\n\n早餐供应时间为 07:00—09:00，餐厅位于一楼。\n\n"
            "## 客房服务\n\n每天 10:00—16:00 提供客房清洁。\n需要额外毛巾可拨打前台分机 800。\n\n"
            "## 停车\n\n住店客人停车收费为每天 30 元。\n离店前请到前台登记车牌。\n")
        checkout = "# 退房须知\n\n退房时间为中午 12:00。\n\n行李可在前台免费寄存至当天 20:00。\n"
        booking = "# 预订与退房\n\n退房时间为中午 12:00。\n\n预订时请填写入住人姓名。\n到店办理入住时需出示身份证。\n前台全天提供行李寄存。\n\n免费取消截止时间为入住前一天 18:00。\n"
        transport = "# 到店交通\n\n酒店位于青禾路 18 号，从地铁青禾站 A 口步行约 8 分钟。\n"
        pool = "# 泳池服务\n\n泳池每天开放至 20:00，住店客人凭房卡免费使用。\n"
        shuttle = "# 机场班车\n\n酒店每天 09:00 提供一班机场接送，请提前一天向前台预约。\n"
        baseline = {
            "住客服务/入住指南": guide, "住客服务/退房须知": checkout,
            "住客服务/行李寄存": "# 行李寄存\n\n寄存行李请在前台领取号码牌。\n",
            "河畔店/入住指南": guide, "西湖店/入住指南": guide,
            "预订/预订与退房": booking, "交通/交通指引": transport,
            "康体/泳池服务": pool, "交通/机场班车": shuttle,
            "暑期服务/早餐": guide, "暑期服务/泳池": pool, "暑期服务/退房": checkout,
            "团队接待/泳池": pool, "团队接待/入住指南": guide,
            "前台/发票办理": "# 发票办理\n\n电子发票在离店后 3 个工作日内发送到预留邮箱。\n",
            "前台/联系前台": "# 联系前台\n\n客房电话拨打 800 可联系前台，全天提供服务。\n",
            "前台/旧无线网络说明": "# 客房无线网络\n\n请连接 QINGHE-OLD，密码为房卡背面标注的八位数字。\n",
            "花园店/入住指南": guide, "周末服务/入住指南": guide,
            "前台/夜间服务": "# 夜间服务\n\n夜间有紧急事项请拨打前台分机 800。\n",
        }
        self.head = self.current_head()
        self.manifest.update(project_id=self.project, prefix=prefix)
        self.save()
        base_drafts = [self.draft(path(name), text, "create") for name, text in baseline.items()]
        self.publish(self.review("发布住客服务手册", "汇总入住、餐饮、交通及前台服务信息，方便各门店统一答复客人。", base_drafts))
        memories = self.call("GET", "/api/v1/org/memories")["items"]
        self.resources = {m["path"]: m["memory_id"] for m in memories}
        def edit(name, text):
            return self.draft(path(name), text)
        pending = []
        mixed = self.scenario("01", "延长早餐及退房服务",
            "不少家庭客人希望早晨安排更从容，建议早餐延长至十点、退房延长至下午两点，并补充行李寄存提醒。", [
                edit("住客服务/退房须知", checkout.replace("中午 12:00", "下午 14:00")),
                edit("住客服务/入住指南", guide.replace("07:00—09:00", "07:00—10:00")),
                edit("住客服务/行李寄存", "# 行李寄存\n\n寄存行李请在前台领取号码牌，贵重物品请随身携带。\n")],
            "同组包含退房时间冲突、早餐与停车的独立修改、已更新的寄存文件。")
        pending.append(mixed)
        pending.append(self.scenario("02", "两家门店早餐延长至十点",
            "河畔店和西湖店近期家庭客人增多，建议将早餐结束时间从九点延长至十点。",
            [edit(name + "/入住指南", guide.replace("07:00—09:00", "07:00—10:00")) for name in ("河畔店", "西湖店")],
            "Remote 将停车费从 30 元调整为 50 元，草稿仅改早餐。自动结果应同时保留两项。"))
        pending.append(self.scenario("03", "放宽退房和免费取消时间",
            "为方便晚到和返程较晚的客人，建议退房延长至下午两点，免费取消延长至入住前一天晚八点。",
            [edit("预订/预订与退房", booking.replace("中午 12:00", "下午 14:00").replace("前一天 18:00", "前一天 20:00"))],
            "两个独立冲突块，分别选择后均应保留；普通段落不应改变。"))
        pending.append(self.scenario("04", "将交通指引更名为到店交通",
            "客人更常询问如何到店，建议使用更直接的文件名称。",
            [self.draft(path("交通/交通指引"), action="rename", destination=path("交通/到店交通"))],
            "远端已改名为交通与停车；应明确选择路径，正文保留。"))
        pending.append(self.scenario("05", "延长泳池开放至晚九点",
            "夏季晚餐后使用泳池的客人增多，建议延长一小时开放时间。",
            [edit("康体/泳池服务", pool.replace("20:00", "21:00"))],
            "远端因泳池检修已删除服务说明，草稿仍有修改。"))
        pending.append(self.scenario("06", "停止机场班车服务",
            "班车乘坐人数持续下降，建议停止运营，由前台协助客人预约出租车。",
            [self.draft(path("交通/机场班车"), action="delete")],
            "草稿删除文件，远端增加晚间班次；检查删除与保留的选择。"))
        summer = self.scenario("07", "调整暑期早餐和退房安排",
            "暑期家庭客人集中，建议延长早餐和退房时间。泳池安排尚待人员排班确认，另行讨论。", [
                edit("暑期服务/早餐", guide.replace("07:00—09:00", "07:00—10:00")),
                edit("暑期服务/泳池", pool.replace("20:00", "21:00")),
                edit("暑期服务/退房", checkout.replace("中午 12:00", "下午 14:00"))],
            "移除中间的泳池成员后，早餐和退房仍可继续审批。")
        team = self.scenario("08", "延长团队客人早餐时间",
            "团队返程集合较晚，建议延长早餐时间。泳池活动安排暂缓。", [
                edit("团队接待/泳池", pool.replace("20:00", "21:00")),
                edit("团队接待/入住指南", guide.replace("07:00—09:00", "07:00—10:00"))],
            "移除首个泳池成员后，剩余入住指南仍可审批。")
        for detail, index in [(summer, 1), (team, 0)]:
            removed = detail["drafts"][index]["draft"]
            self.call("DELETE", f'/api/v1/drafts/{removed["draft_id"]}', headers={"If-Match": f'"{removed["version"]}"'})
            pending.append(detail)
        updated = self.scenario("10", "花园店早餐延长至十点",
            "花园店周边展会期间晚起客人较多，建议早餐延长一小时。",
            [edit("花园店/入住指南", guide.replace("07:00—09:00", "07:00—10:00"))],
            "已保存整组更新，直接查看最新差异并审批。")
        rejected = self.scenario("11", "周末早餐延长至十点半",
            "周末以休闲住客为主，建议延长早餐供应时间，减少客人错过早餐的情况。",
            [edit("周末服务/入住指南", guide.replace("07:00—09:00", "07:00—10:30"))],
            "已拒绝的 Review 更新后应能重新提交。")
        self.call("POST", f'/api/v1/reviews/{rejected["review"]["review_id"]}/decisions', {
            "expected_review_version": rejected["review"]["version"], "decision": "rejected",
            "body": "请先与餐厅确认周末排班及增加的食材成本，再提交此安排。"})
        pending.append(rejected)
        pending.append(self.scenario("12", "增加宠物入住说明",
            "近期携带宠物的咨询增多，建议允许二十公斤以内的宠物入住，并收取一百元清洁押金。",
            [self.draft(path("住客服务/宠物入住"), "# 宠物入住\n\n允许体重不超过 20 公斤的宠物入住。\n清洁押金为 100 元，离店检查后退还。\n", "create")],
            "双方在同一路径各自新增了不同的宠物政策。"))
        remote = [
            edit("住客服务/退房须知", checkout.replace("中午 12:00", "下午 13:00")),
            *[edit(name + "/入住指南", guide.replace("每天 30 元", "每天 50 元"))
              for name in ("住客服务", "河畔店", "西湖店", "花园店", "周末服务")],
            edit("预订/预订与退房", booking.replace("中午 12:00", "下午 13:00").replace("前一天 18:00", "前一天 16:00")),
            self.draft(path("交通/交通指引"), action="rename", destination=path("交通/交通与停车")),
            self.draft(path("康体/泳池服务"), action="delete"),
            edit("交通/机场班车", shuttle.replace("一班机场接送", "及 18:00 两班机场接送")),
            self.draft(path("住客服务/宠物入住"), "# 宠物入住\n\n允许体重不超过 10 公斤的宠物入住。\n清洁押金为 200 元，离店检查后退还。\n", "create"),
        ]
        self.publish(self.review("更新停车收费及住客服务政策",
            "停车场调整住店收费至每日五十元；退房延长一小时。同步更新交通、宠物政策，并撤下检修期间的泳池说明。", remote))
        plan = self.plan(mixed)
        current_id = mixed["drafts"][2]["draft"]["draft_id"]
        candidate = next(c for c in plan["candidates"] if c["draft_id"] == current_id)
        self.call("POST", f"/api/v1/drafts/{current_id}/rebases", {
            "candidate_id": candidate["candidate_id"], "expected_draft_version": candidate["draft_version"]}, self.etag())
        self.update(updated)
        self.scenario("09", "更新发票办理及前台联系方式",
            "财务已将电子发票处理时间缩短至一个工作日，同时整理前台联系入口并移除停用的无线网络说明。", [
                edit("前台/发票办理", baseline["前台/发票办理"].replace("3 个工作日", "1 个工作日")),
                self.draft(path("前台/联系前台"), action="rename", destination=path("前台/前台联系方式")),
                self.draft(path("前台/旧无线网络说明"), action="delete")],
            "基于最新 Remote，无需更新；覆盖正文、重命名及删除。")
        for item in pending:
            self.plan(item)
        self.manifest["scenarios"].sort(key=lambda s: s["case"])
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
        if len(scenarios) != 12 or {s["case"] for s in scenarios} != set(expected):
            raise RuntimeError("The Review playground is incomplete.")
        for item in scenarios:
            detail = self.call("GET", f'/api/v1/reviews/{item["review_id"]}')
            review = detail["review"]
            actual = (review["status"], review["coordination"]["freshness"],
                      review["coordination"]["reconciliation"], len(detail["drafts"]))
            if actual != expected[item["case"]]:
                raise RuntimeError(f'Fixture state changed: {item["title"]}: {actual}')
            if review["description"] == item["check"]:
                raise RuntimeError("QA instructions must not be stored as the Review description.")
            if item["case"] == "02":
                plan = self.plan(detail)
                if len(plan["candidates"]) != 2:
                    raise RuntimeError("Both hotel guides must have an automatic update candidate.")
                for candidate in plan["candidates"]:
                    original = candidate["base_state"]["content"]["content"]
                    draft = candidate["draft_state"]["content"]["content"]
                    remote = candidate["current_state"]["content"]["content"]
                    merged = candidate["proposed_state"]["content"]["content"]
                    if (draft != original.replace("07:00—09:00", "07:00—10:00")
                            or remote != original.replace("每天 30 元", "每天 50 元")
                            or merged != draft.replace("每天 30 元", "每天 50 元")):
                        raise RuntimeError("Breakfast and parking changes were not both preserved.")
        print("All 12 initial Review states verified through the Server API.")

    def advance_remote(self):
        self.manifest = json.loads(self.manifest_path.read_text())
        self.project = self.manifest["project_id"]
        self.head = self.current_head()
        extension = 801 + self.manifest.get("night_desk_updates", 0)
        item = self.draft(self.manifest["prefix"] + "/前台/夜间服务.md",
                          f"# 夜间服务\n\n夜间有紧急事项请拨打前台分机 {extension}。\n")
        self.publish(self.review("调整夜间前台联系电话",
            "夜间值班人员调整至独立服务台，请住客使用夜班专用分机。", [item]))
        self.manifest["night_desk_updates"] = extension - 800
        self.save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--advance-remote", action="store_true",
                        help="Advance only this playground's remote version to test stale submissions.")
    modes.add_argument("--prepare-app", action="store_true",
                        help="Initialize, sign in and seed before the local Dev App opens.")
    modes.add_argument("--login-only", action="store_true",
                        help="Initialize and sign in the local Dev App without seeding Reviews.")
    modes.add_argument("--verify", action="store_true",
                       help="Check all initial fixture states before interactive testing changes them.")
    parser.add_argument("--account", choices=("owner", "project-admin", "member-a", "member-b"),
                        default="owner", help="Local fake-OIDC identity for --login-only.")
    args = parser.parse_args()
    if args.account != "owner" and not args.login_only:
        parser.error("Member identities require --login-only")
    os.umask(0o077)
    lab = Playground()
    if lab.manifest_path.exists() and not (args.advance_remote or args.prepare_app or args.verify or args.login_only):
        raise SystemExit("This instance already has a playground. Its data was left unchanged.")
    if args.account != "owner":
        owner = Playground()
        owner.login()
        owner.ensure_test_accounts()
    lab.login(args.account)
    if args.login_only:
        lab.prepare_app()
        return
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
