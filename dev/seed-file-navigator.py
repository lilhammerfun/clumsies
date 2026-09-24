#!/usr/bin/env python3
"""Prepare file navigation exercises in this worktree's isolated local Dev Instance."""
import json
from pathlib import Path
import runpy


def main():
    playground = runpy.run_path(str(Path(__file__).with_name("seed-review-playground.py")))
    lab = playground["Playground"]()  # Validates instance ownership and loopback URLs.
    manifest = lab.root / "file-navigator-playground.json"
    if manifest.exists():
        print(manifest)
        return
    lab.login()
    lab.project = lab.call("POST", "/api/v1/projects", {
        "name": "文件导航器测试", "description": "独立的文件导航器体验项目。"
    })["project_id"]
    documents = {
        "入门/欢迎.md": "# 欢迎\n\n这里记录团队的常用资料。\n",
        "入门/团队介绍.md": "# 团队介绍\n\n团队负责产品设计、研发和客户服务。\n",
        "手册/发布/检查清单.md": "# 发布检查清单\n\n发布前确认版本号和回滚方案。\n",
        "手册/发布/回滚方案.md": "# 回滚方案\n\n恢复上一版服务并确认健康检查通过。\n",
        "手册/检查清单.md": "# 文档检查清单\n\n确认文档内容与当前流程一致。\n",
        "目录操作/命中文件.md": "# 会议安排\n\n每周四下午讨论产品进展。\n",
        "目录操作/其他文件.md": "# 联系方式\n\n会议变更由项目负责人通知。\n",
        "保留目录/资料.md": "# 参考资料\n\n这里保存日常工作中的参考资料。\n",
    }
    lab.head = lab.current_head()
    drafts = [lab.draft(path, body, "create") for path, body in documents.items()]
    lab.publish(lab.review("初始化团队资料", "整理团队介绍、发布流程和会议资料。", drafts))
    resources = lab.call("GET", "/api/v1/org/memories")["items"]
    ids = [item["memory_id"] for item in resources if item["path"] in documents]
    comparison = lab.call("POST", "/api/v1/projects", {"name": "文件导航器对照项目"})["project_id"]
    for project in [lab.project, comparison]:
        current = lab.call("GET", f"/api/v1/projects/{project}/org-selections")
        lab.call("PUT", f"/api/v1/projects/{project}/org-selections",
                 {"resource_ids": ids}, {"If-Match": str(current["revision"])})
    manifest.write_text(json.dumps({"project_id": lab.project, "comparison_project_id": comparison,
        "paths": list(documents)}, ensure_ascii=False, indent=2) + "\n")
    manifest.chmod(0o600)
    print(manifest)


if __name__ == "__main__":
    main()
