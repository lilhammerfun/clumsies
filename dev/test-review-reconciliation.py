#!/usr/bin/env python3
"""Test concurrent submissions through dev HTTP and leave native UI fixtures for inspection."""
import argparse
import importlib.util
import json
from pathlib import Path
import secrets

spec = importlib.util.spec_from_file_location("ownership", Path(__file__).with_name("test-memory-ownership.py"))
api = importlib.util.module_from_spec(spec)
spec.loader.exec_module(api)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prepare-app", action="store_true")
    args = parser.parse_args()
    owner = api.module.Playground()
    owner.login()
    owner.ensure_test_accounts()
    author, publisher = api.module.Playground(), api.module.Playground()
    author.login("member-a")
    publisher.login("project-admin")
    assert len({client.me["user"]["user_id"] for client in (owner, author, publisher)}) == 3
    project = owner.call("POST", "/api/v1/projects", {"name": "冲突处理验收 " + secrets.token_hex(3)})["project_id"]
    for client, role in ((author, "member"), (publisher, "admin")):
        owner.call("POST", f"/api/v1/admin/projects/{project}/members", {
            "user_id": client.me["user"]["user_id"], "role": role})
    for client in (owner, author, publisher):
        client.project = project
    base = "# 部署规范\n\n超时时间：30 秒\n\n## 检查\n\n发布前运行测试。\n"
    shared = base.replace("30 秒", "60 秒")
    paths = {"identical": "内容一致.md", "clean": "自动合并.md", "conflict": "超时时间.md", "path": "文件路径.md", "preview": "交互预览.md"}
    for path in paths.values():
        api.merge(owner, api.review(owner, api.proposal(owner, path, base)))
    resources = {r["path"]: r["memory_id"] for r in owner.call("GET", f"/api/v1/projects/{project}/memories")["items"]}
    texts = {"identical": shared, "clean": base.replace("发布前运行测试。", "发布前运行测试并检查日志。"),
             "conflict": base.replace("30 秒", "45 秒"), "preview": base.replace("30 秒", "45 秒")}
    samples = {}
    native = {}
    for case, text in texts.items():
        samples[case] = api.proposal(author, paths[case], text, resources[paths[case]])
        native[case] = api.proposal(owner, paths[case], text, resources[paths[case]])["draft"]["draft_id"]
    path_ref = {"scope": "project", "id": resources[paths["path"]], "path": paths["path"]}
    samples["path"] = author.call("POST", "/api/v1/drafts", {
        "daemon_installation_id": "conflict-acceptance", "project_id": project,
        "base_commit_id": api.head(author), "title": "重命名文件", "resource": path_ref,
        "operations": [{"action": "rename", "resource": path_ref, "new_path": "同名文件.md"}]})
    for case in texts:
        api.merge(publisher, api.review(publisher, api.proposal(publisher, paths[case], shared, resources[paths[case]])))
    api.merge(publisher, api.review(publisher, api.proposal(publisher, "同名文件.md", base)))
    current = api.head(owner)
    candidates = {}
    for case, detail in samples.items():
        draft = detail["draft"]
        candidates[case] = author.call("POST", f'/api/v1/drafts/{draft["draft_id"]}/reconciliation-candidates',
                                      {"expected_draft_version": draft["version"]})
    identical = candidates["identical"]
    assert identical["status"] == "clean" and not identical["conflicts"]
    assert identical["current_state"] == identical["draft_state"]
    empty = author.call("POST", f'/api/v1/drafts/{identical["draft_id"]}/auto-rebases',
                        {"expected_draft_version": identical["draft_version"]})
    assert empty["draft"]["status"] == "open" and not empty["operations"]
    assert empty["draft"]["base_commit_id"] == current
    clean = candidates["clean"]
    assert clean["status"] == "clean" and not clean["conflicts"]
    assert "60 秒" in clean["proposed_state"]["content"]["content"]
    assert "检查日志" in clean["proposed_state"]["content"]["content"]
    collision = candidates["path"]
    assert collision["status"] == "conflicts"
    assert collision["current_state"]["content"] == collision["draft_state"]["content"]
    assert [c["kind"] for c in collision["conflicts"]] == ["path_occupied"]
    conflict = candidates["conflict"]
    assert conflict["status"] == "conflicts" and [c["field"] for c in conflict["conflicts"]] == ["content"]
    unchanged = author.call("POST", f'/api/v1/drafts/{conflict["draft_id"]}/auto-rebases',
                            {"expected_draft_version": conflict["draft_version"]})
    assert unchanged["draft"]["base_commit_id"] == samples["conflict"]["draft"]["base_commit_id"]
    assert unchanged["operations"] == samples["conflict"]["operations"]
    submitted = author.call("POST", "/api/v1/reviews", {"title": "保留 45 秒并补充发布检查", "drafts": [
        {"draft_id": c["draft_id"], "expected_draft_version": c["draft_version"], "candidate_id": c["candidate_id"],
         "resolved_state": c["draft_state"] if c["status"] == "conflicts" else None}
        for c in (clean, conflict)]}, api.etag(current))
    assert submitted["review"]["status"] == "open" and len(submitted["drafts"]) == 2
    assert api.head(owner) == current, "creating a Review must not publish"
    manifest = {"instance_id": owner.instance, "server_url": owner.origin, "project_id": project,
                "drafts": native, "paths": paths}
    manifest_path = owner.root / "review-reconciliation-test.json"
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
    if args.prepare_app:
        owner.prepare_app()
    print("PASS: identical results need no choices; independent edits merge; equal text retains real path collisions")
    print("PASS: two-author conflicts preserve edits; resolved submission creates one Review without publishing")
    print("Native inspection fixtures:", manifest_path)


if __name__ == "__main__":
    main()
