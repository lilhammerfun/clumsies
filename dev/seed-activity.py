#!/usr/bin/env python3
"""Seed source previews and sortable retrieval traces in this worktree's local Dev Instance."""

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
import os
from pathlib import Path
import runpy
import sqlite3

PREFIX = "activity-preview-"


def digest(text):
    return "sha256:" + hashlib.sha256(text.encode()).hexdigest()


def dataset(workspace):
    sections = [
        ("版本检查", "# 自动更新\n\n## 版本检查\n\n检查本机版本与 appcast 中的版本。保留签名验证，确认更新按钮只在存在新版本时出现。\n"),
        ("发布清单与下载地址", "## 发布清单与下载地址\n\n| 通道 | 清单 | 签名 |\n| --- | --- | --- |\n| Preview | preview-appcast.xml | 必须验证 |\n| Release | appcast.xml | 必须验证 |\n\n"),
        ("更新失败时的诊断命令与完整排查记录——保留原始标题、代码及换行", "## 更新失败时的诊断命令\n\n```sh\ncurl -I https://example.invalid/preview-appcast.xml\n```\n\n" + "检查清单地址、版本号、下载地址与签名，记录每一步实际结果。\n" * 65),
    ]
    full = "\n".join(text for _, text in sections)
    full_hash = digest(full)
    candidates = []
    offset = 0
    for index, (heading, content) in enumerate(sections):
        rank = [2, 11, 1][index]
        candidates.append(dict(
            unit_key=f"{PREFIX}memory/{index}", resource_id=PREFIX + "memory", scope="project",
            kind="memory", path="knowledge/macos/更新检查与发布流程.md", heading_path=["自动更新", heading],
            locator=dict(type="markdown_span", start_byte=offset, end_byte=offset + len(content.encode()),
                         heading_path=["自动更新", heading]),
            content_hash=digest(content), resource_content_hash=full_hash, token_count=len(content),
            evidence_excerpt=content[:1200] + ("…" if len(content) > 1200 else ""),
            exact_rank=None, bm25_rank=rank, bm25_score=12.0 / rank,
            vector_rank=[11, 1, 2][index], vector_score=[0.61, 0.94, 0.82][index],
            rrf_rank=[1, 2, 11][index], rrf_score=[0.08, 0.07, 0.02][index],
            reranker_rank=[2, 11, 1][index], reranker_logit=[1.2, -0.4, 2.8][index],
            reranker_relevance=[0.77, 0.40, 0.94][index], final_rank=index + 1,
            selected=True, exclusion_reason="selected", delta_action="add"))
        offset += len(content.encode()) + 1
    excluded = dict(candidates[0], unit_key=PREFIX + "missing-ranks", exact_rank=1,
                    bm25_rank=None, bm25_score=None, vector_rank=None, vector_score=None,
                    rrf_rank=4, rrf_score=0.01, reranker_rank=None, reranker_logit=None,
                    reranker_relevance=None, final_rank=None, selected=False,
                    exclusion_reason="not_reranked", delta_action=None)
    candidates.append(excluded)
    now = datetime.now(timezone.utc).replace(microsecond=0)
    sessions = []
    runs = []

    def session(name, messages):
        timestamp = (now - timedelta(minutes=len(sessions))).isoformat().replace("+00:00", "Z")
        events = [dict(timestamp=timestamp, type="session_meta", payload=dict(
            id=PREFIX + name, timestamp=timestamp, cwd=str(workspace), originator="codex_desktop"))]
        for index, (message, searches) in enumerate(messages):
            events.append(dict(timestamp=timestamp, type="event_msg",
                               payload=dict(type="user_message", message=message)))
            for number, (query, run_name, status, fragments) in enumerate(searches):
                run_id = PREFIX + run_name if run_name else None
                result = dict(structuredContent=dict(run_id=run_id, fragments=fragments), isError=status == "failed")
                if status == "failed":
                    result["content"] = [dict(type="text", text="Preview fixture: retrieval model unavailable.")]
                events.append(dict(timestamp=timestamp, type="event_msg", payload=dict(type="item_completed", item=dict(
                    type="McpToolCall", id=f"{name}-{index}-{number}", server="clumsies", tool="memory",
                    arguments=dict(op=dict(activate=dict(query=query))), status="completed", result=result))))
                if run_name and status != "missing":
                    runs.append(dict(run_id=run_id, query=query, status=status, created_at=timestamp,
                                     reuse=run_name == "reuse"))
        sessions.append((name, events))

    session("normal", [
        ("现在这两个问题完全修复了？完全修复了的话就提交并 PR。\n\n怎么实现的？刚好讲讲更新的原理？", [
            ("验证 macOS 更新按钮和 appcast 清单，检查发布流程与签名验证。", "ordering", "succeeded", []),
            ("继续核对已经提供的更新流程，复用不变的记忆。", "reuse", "succeeded", [])]),
        ("检查有没有关于旧版安装包的额外说明。", [("查找旧版安装包的迁移说明。", "empty", "succeeded", [])]),
        ("收到，先保留现在的实现。", [])])
    session("missing", [("历史记录已清理时，仍能看到原来记录的 source 吗？", [
        ("读取之前返回的更新说明。", "removed", "missing", [dict(
            unit_key=PREFIX + "historical", resource_id=PREFIX + "memory", scope="project",
            path="knowledge/macos/历史更新说明.md", heading_path=["历史更新说明"], action="add",
            content="# 历史更新说明\n\n这是会话保留的原始片段；对应的检索记录已清理。\n" + "保留当时记录，不替换成当前文档。\n" * 80)])])])
    session("failed", [("检索失败时能否看到错误并保留用户消息？", [
        ("查询更新故障排查流程。", "failed", "failed", [])])])
    return full, candidates, sessions, runs


def insert(db, table, values):
    columns = list(values)
    db.execute(f"INSERT OR REPLACE INTO {table} ({','.join(columns)}) VALUES ({','.join('?' for _ in columns)})",
               [values[column] for column in columns])


def seed():
    # Reuse the existing loopback-only ownership checks and in-memory OIDC login.
    playground = runpy.run_path(str(Path(__file__).with_name("seed-review-playground.py")))["Playground"]()
    playground.login()
    root = playground.root
    manifest = root / "activity-preview.json"
    if manifest.exists():
        project = json.loads(manifest.read_text())["project_id"]
    else:
        project = playground.call("POST", "/api/v1/projects", {
            "name": "Activity Preview", "description": "原始片段预览与检索列排序的本地测试数据。"})["project_id"]
    playground.project = project
    playground.prepare_app()
    daemon = root / "daemon"
    codex = root / "codex-home"
    if Path(playground.runtime["paths"]["daemon_root"]) != daemon or Path(playground.runtime["paths"]["codex_home"]) != codex:
        raise RuntimeError("Fixture paths must belong to this Dev Instance.")
    workspace = root / "fixtures/activity-workspace"
    workspace.mkdir(parents=True, exist_ok=True)
    full, candidates, sessions, runs = dataset(workspace)
    full_hash = digest(full)
    blob = daemon / "evaluation-corpora/blobs" / full_hash[7:9] / full_hash[7:]
    blob.parent.mkdir(parents=True, exist_ok=True)
    blob.write_text(full)
    with sqlite3.connect(f"file:{daemon / 'local.db'}?mode=rw", uri=True, timeout=15) as db:
        insert(db, "project_bindings", dict(server_url=playground.origin, workspace_root=str(workspace),
                                           project_id=project, revision=1))
        insert(db, "retrieval_corpus_blobs", dict(content_hash=full_hash, byte_length=len(full.encode())))
        for run in runs:
            rows = candidates if run["run_id"] in (PREFIX + "ordering", PREFIX + "reuse") else []
            insert(db, "retrieval_runs", dict(
                run_id=run["run_id"], project_id=project, query=run["query"], activation_state_fingerprint="fixture",
                status=run["status"], effective_hash="fixture", index_revision="fixture",
                resource_count=int(bool(rows)), unit_count=len(rows), total_us=142000,
                returned_fragment_count=sum(row["selected"] for row in rows),
                returned_token_count=sum(row["token_count"] for row in rows if row["selected"]),
                error_summary="Preview fixture: retrieval model unavailable." if run["status"] == "failed" else None,
                created_at=run["created_at"], completed_at=run["created_at"]))
            db.execute("DELETE FROM retrieval_run_candidates WHERE run_id = ?", (run["run_id"],))
            if rows:
                insert(db, "retrieval_run_resources", dict(run_id=run["run_id"], resource_order=0,
                    resource_id=PREFIX + "memory", scope="project", kind="memory", path=rows[0]["path"],
                    title="自动更新", content_hash=full_hash, content_preview=full[:1200]))
            for order, candidate in enumerate(rows):
                row = dict(candidate, run_id=run["run_id"], candidate_order=order)
                row["heading_path_json"] = json.dumps(row.pop("heading_path"), ensure_ascii=False)
                row["locator_json"] = json.dumps(row.pop("locator"), ensure_ascii=False)
                if run["reuse"] and row["selected"]:
                    row["delta_action"] = "reuse"
                insert(db, "retrieval_run_candidates", row)
    folder = codex / "sessions/2026/09/21"
    folder.mkdir(parents=True, exist_ok=True)
    for name, records in sessions:
        (folder / f"rollout-{PREFIX}{name}.jsonl").write_text(
            "\n".join(json.dumps(record, ensure_ascii=False) for record in records) + "\n")
    manifest.write_text(json.dumps(dict(project_id=project, workspace=str(workspace),
        sessions=[PREFIX + name for name, _ in sessions], checks=[
            "User message / Memory search: matching labels, no Request number or Agent query.",
            "Source: literal # headings, table pipes and code fences; three-line preview; expand/collapse.",
            "Retrieval: click Final / BM25 / Vector / RRF / Rerank twice; numeric ranks, missing last.",
            "Same document, different chunks; reused content; no results; failed search; missing history."
        ]), ensure_ascii=False, indent=2))
    print(f"Activity Preview ready: {manifest}")


def check():
    full, candidates, sessions, runs = dataset(Path("/fixture"))
    for candidate in candidates:
        locator = candidate["locator"]
        source = full.encode()[locator["start_byte"]:locator["end_byte"]].decode()
        assert digest(source) == candidate["content_hash"]
        assert candidate["resource_content_hash"] == digest(full)
    assert {c["reranker_rank"] for c in candidates} == {1, 2, 11, None}
    assert max(len(c["evidence_excerpt"]) for c in candidates) == 1201
    assert len(sessions) == 3 and len(runs) == 4
    assert {r["status"] for r in runs} == {"succeeded", "failed"}
    print("Activity fixture checks passed.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    os.umask(0o077)
    check() if args.check else seed()
