#!/usr/bin/env python3
"""Add isolated, repeatable Memory UI fixtures through the local daemon/API."""
import json
import hashlib
import secrets
from pathlib import Path
import runpy
import time
from urllib.parse import urlsplit


def main():
    module = runpy.run_path(str(Path(__file__).with_name("dev-instance-linux.py")))
    instance = module["Instance"]()
    runtime = json.loads(Path(instance.runtime).read_text())
    if (runtime["worktree_path"] != instance.worktree
            or runtime["instance_id"] != instance.instance_id
            or urlsplit(runtime["server_url"]).hostname != "127.0.0.1"
            or runtime["daemon_root"] != instance.daemon_root):
        raise SystemExit("Requires this worktree's isolated loopback Linux instance")
    manifest_path = Path(instance.root) / "memory-ui-fixtures.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {}

    def save():
        manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
        manifest_path.chmod(0o600)

    def ipc(method, payload):
        return module["call"](instance.daemon_root, method, payload)

    server = instance.server

    def request(method, path, body=None, headers=None):
        headers = dict(headers or {})
        if method not in ("GET", "HEAD"):
            key = hashlib.sha256(json.dumps([method, path, body], sort_keys=True).encode()).hexdigest()
            headers["Idempotency-Key"] = manifest.setdefault("requests", {}).setdefault(key, secrets.token_hex(16))
            save()
        return server(method, path, body, headers)

    instance.server = request
    if "project_id" not in manifest:
        project = instance.server("POST", "/api/v1/projects", {
            "name": "Memory UI 验收", "description": "可修改、删除的独立 UI 测试资料。"})
        manifest["project_id"] = project["project_id"]
        save()
    project = manifest["project_id"]
    def supplement():
        if manifest.get("directory_fixtures_done"):
            return
        checkout = ipc("project_checkout", {"project_id": project})
        commit = checkout.get("commit_id")
        paths = {"08-空目录": {"content": "", "is_directory": True},
                 "04-草稿状态/待删除.md": {"content": "# 删除状态\n\n这份已发布文件带有删除草稿，文件名应显示红色。\n"}}
        if "directory_baseline" not in manifest:
            manifest["directory_baseline"] = ipc("desktop_create_memory_drafts", {
                "project_id": project, "base_commit_id": commit,
                "operations": [{"create": {"path": path, "content": content}}
                               for path, content in paths.items()]})
            save()
        if "directory_review" not in manifest:
            for _ in range(60):
                drafts = instance.server("GET", f"/api/v1/drafts?project_id={project}")["items"]
                drafts = [d for d in drafts if d.get("resource", {}).get("path") in paths and d["status"] == "open"]
                if len(drafts) == len(paths):
                    break
                time.sleep(1)
            else:
                raise SystemExit("Directory drafts not synchronized; rerun to resume")
            manifest["directory_review"] = instance.server("POST", "/api/v1/reviews", {
                "title": "空目录和删除状态验收资料",
                "drafts": [{"draft_id": d["draft_id"], "expected_draft_version": d["version"]} for d in drafts]
            }, {"if-match": '"' + (commit or "ref-none") + '"'})["review"]
            save()
        if "directory_commit" not in manifest:
            review = manifest["directory_review"]
            result = instance.server("POST", f"/api/v1/reviews/{review['review_id']}/merges", {
                "expected_review_version": review["version"]
            }, {"if-match": '"' + (review.get("ref_etag") or commit or "ref-none") + '"'})
            manifest["directory_commit"] = result["commit_id"]
            save()
        ipc("project_retry_sync", {"project_id": project, "channel": "commits"})
        for _ in range(60):
            checkout = ipc("project_checkout", {"project_id": project})
            target = next((r for r in checkout["resources"] if r["path"] == "04-草稿状态/待删除.md"), None)
            if target:
                break
            time.sleep(1)
        else:
            raise SystemExit("Fixture checkout not ready; rerun to resume")
        if "deletion_draft" not in manifest:
            manifest["deletion_draft"] = ipc("desktop_store_draft_operation", {
                "project_id": project, "base_commit_id": manifest["directory_commit"],
                "scope": "project", "resource": "memory", "source": "desktop",
                "op": {"delete": {"id": target["resource_id"]}}})
            save()
        if "empty_directory_draft" not in manifest:
            manifest["empty_directory_draft"] = ipc("desktop_create_memory_drafts", {
                "project_id": project, "base_commit_id": manifest["directory_commit"],
                "operations": [{"create": {"path": "09-未发布空目录", "content": {"content": "", "is_directory": True}}}]})
            save()
        manifest["directory_fixtures_done"] = True
        save()

    if "edited_draft" in manifest:
        supplement()
        print(f"Memory UI 验收 already prepared: {project}; preserving your edits.\n{manifest_path}")
        return
    documents = {
        "00-从这里开始.md": "# Memory UI 验收\n\n这整个项目都是可丢弃的测试资料。\n\n- 检查文件树背景、选中和悬停\n- 展开深层目录，折叠后重启检查恢复\n- Ctrl 多选、Shift 范围选择、右键菜单\n- 长文件名应截断，悬停可读全名\n- 检查文件名状态颜色，打开编辑和 diff\n- 批量目录用于滚动、重命名和删除测试\n",
        "01-目录层级/欢迎.md": "# 欢迎\n\n中文文件名与普通文档。\n",
        "01-目录层级/一级/二级/三级/深层文档.md": "# 深层文档\n\n检查缩进与折叠。\n",
        "01-目录层级/一级/说明.md": "# 说明\n\n一级目录中的文件。\n",
        "02-长名称/这是一个很长很长的中文文件名用于检查截断和悬停提示以及选中区域是否超出边界.md": "# 长文件名\n\n完整文件名应能通过悬停查看。\n",
        "02-长名称/a-very-long-english-filename-for-truncation-and-tooltip-testing.md": "# English filename\n\nCheck truncation and tooltip.\n",
        "03-同名文件/前端/README.md": "# 前端 README\n",
        "03-同名文件/后端/README.md": "# 后端 README\n",
        "04-草稿状态/已修改.md": "# 发布版本\n\n这是原始内容，用于对照 diff。\n",
        "04-草稿状态/未修改.md": "# 未修改\n\n此文档应没有 draft 标签。\n",
        "05-特殊名称/中文 空格 & symbols (v2).md": "# 特殊名称\n\n路径含空格、括号和 &。\n",
    }
    documents.update({f"06-滚动与批量/文档-{i:02d}.md": f"# 批量文档 {i:02d}\n\n用于范围选择、批量操作和滚动。\n" for i in range(1, 25)})

    def create_batch(entries):
        return ipc("desktop_create_memory_drafts", {
            "project_id": project, "base_commit_id": manifest.get("commit_id"),
            "operations": [{"create": {"path": path, "content": {"content": body}},
                            "update": None, "rename": None, "delete": None, "discard": None}
                           for path, body in entries.items()]})

    if "baseline" not in manifest:
        manifest["baseline"] = create_batch(documents)
        save()
    if "review" not in manifest:
        expected_paths = set(documents)
        for _ in range(60):
            drafts = instance.server("GET", f"/api/v1/drafts?project_id={project}")["items"]
            drafts = [d for d in drafts if d.get("resource", {}).get("path") in expected_paths and d["status"] == "open"]
            if len(drafts) == len(documents):
                break
            time.sleep(1)
        else:
            raise SystemExit("Draft upload did not finish; rerun to resume")
        manifest["review"] = instance.server("POST", "/api/v1/reviews", {
            "title": "初始化 Memory UI 验收资料", "description": "仅包含独立测试项目的合成资料。",
            "drafts": [{"draft_id": d["draft_id"], "expected_draft_version": d["version"]} for d in drafts]
        }, {"if-match": '"ref-none"'})["review"]
        save()
    if "commit_id" not in manifest:
        review = manifest["review"]
        result = instance.server("POST", f"/api/v1/reviews/{review['review_id']}/merges", {
            "expected_review_version": review["version"]},
            {"if-match": '"' + (review.get("ref_etag") or "ref-none") + '"'})
        manifest["commit_id"] = result["commit_id"]
        save()
    ipc("project_retry_sync", {"project_id": project, "channel": "commits"})
    for _ in range(60):
        checkout = ipc("project_checkout", {"project_id": project})
        if len(checkout["resources"]) >= len(documents):
            break
        time.sleep(1)
    else:
        raise SystemExit("Checkout is not ready; rerun to resume")
    if "new_drafts" not in manifest:
        manifest["new_drafts"] = create_batch({
            "04-草稿状态/尚未发布的新文件.md": "# 新文件\n\n这是一份未发布的创建草稿，文件名应显示绿色。\n",
            "07-仅草稿目录/待评审.md": "# 待评审\n\n整个目录仅由未发布的草稿构成。\n"})
        save()
    if "edited_draft" not in manifest:
        resource = next(r for r in checkout["resources"] if r["path"] == "04-草稿状态/已修改.md")
        manifest["edited_draft"] = ipc("desktop_store_draft_operation", {
            "project_id": project, "base_commit_id": manifest["commit_id"], "draft_id": None,
            "scope": "project", "resource": "memory", "source": "desktop",
            "op": {"create": None, "update": {"id": resource["resource_id"],
                "content": {"content": "# 修改后的版本\n\n这是修改后的内容。\n\n- 新增一条测试说明\n"}},
                "rename": None, "delete": None, "discard": None}})
        save()
    supplement()
    print(json.dumps({"project": "Memory UI 验收", "project_id": project,
                      "published": len(checkout["resources"]), "fixture_drafts": 3,
                      "manifest": str(manifest_path)}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
