# Retrieval Run 与评测

本文定义 Rust daemon 与原生 macOS Diagnostics
使用的本地检索历史与评测契约。它是 `docs/architecture.md`
中排序流水线的补充，不改变面向 Agent 的 MCP `activate` 请求或响应。

## 边界

每个有效的 `activate_memory` 请求都会在 daemon 的中心本地
SQLite 数据库中创建一条 Retrieval Run。
一条 Run 记录解释和评测该次 activation 所需的精确排序输入与输出：

- Project、query、activation-state 指纹、
  Effective Memory hash、Index Revision、parser、
  chunker、model 与 ranking-profile 版本。
- 融合召回集合中每个候选的 Exact/BM25、dense-vector、RRF、
  reranker 与最终 rank 和 score。
- 每个未入选候选的稳定 exclusion reason，
  以及每个入选候选的 `add`/`replace`/`reuse` delta
  action。
- 阶段延迟、返回的 fragment/token 计数，
  以及失败 Run 的有界 error stage、code 与 summary。
- 该 Run 使用的完整 Effective Memory resource
  manifest 与 content-addressed resource blob。

MCP response 与持久 trace 由同一份内存排序候选装配而成。
检索历史是本机诊断与评测状态，不是 Server 遥测：daemon 不上传 query、
candidate、judgment 或 metric，
Server 也不暴露 Retrieval Run 端点。

## 存储

中心 `local.db` 拥有：

| 表 | 用途 |
| --- | --- |
| `retrieval_runs` | Run 身份、版本、状态、延迟、结果计数和失败 |
| `retrieval_run_candidates` | 每个 ranked unit 一行，含全部阶段值与 disposition |
| `retrieval_run_resources` | 一条 Run 使用的有序 Effective Memory corpus |
| `retrieval_corpus_blobs` | content-addressed corpus blob 的元数据 |
| `evaluation_corpora` | 不可变、去重的 corpus 身份 |
| `evaluation_corpus_resources` | 某个 corpus 的冻结 resource manifest |
| `evaluation_cases` | 从一条 Run 固定下来的版本化 query 与 corpus 对 |
| `evaluation_judgments` | 人工 0–3 相关度与遗漏证据判断 |

resource 正文保存在 daemon Application Support 下的
`evaluation-corpora/blobs/<prefix>/<sha256>`。
目录与文件使用 owner-only 权限。Project Local Storage
仍只保存可重建的 Commit generation 与检索索引；
改变或清理该位置不会删除 Retrieval Run 或 Evaluation Case。

未固定的历史按 Project 保留，超过 500 条 Run 后清理。
被 Evaluation Case 引用的 Run 会被固定。
Clear History 只删除未固定 Run，
并回收不再被任一 Run 或 Evaluation Corpus 引用的 content
blob。

## 状态与恢复

一条 Run 的状态为 `running`、`succeeded` 或 `failed`。

1. daemon 在确认 Project 与 query 都存在后插入 `running`。
2. 排序流水线填入一份完成 trace。
3. daemon 原子地插入 candidate/resource，并把 Run 标记为终态。
4. 重启时，任何残留的 `running` Run 会变成 `failed`，
   错误码为 `retrieval_interrupted`。

写入诊断历史失败会被记录，但不改变面向 Agent 的检索结果。只要中心数据库仍可写，
检索或模型失败仍会成为终态 failed Run。

## Evaluation Case

把一条成功的 Run 加入 Evaluation Set 会冻结：

```text
query
+ project_id
+ Effective Memory resource manifest and content blobs
+ source Run candidate trace
= immutable Evaluation Corpus and versioned judgments
```

judgment 标识一个已检索 unit 或一个遗漏的 corpus resource，
并使用 0–3 相关度等级。整体替换 judgment 集合必须携带
`expected_judgment_version`；
版本过期的编辑者会收到 `evaluation_judgment_conflict`。
导出会生成自包含的版本化 JSON fixture，包含冻结的 resource body、
source trace、judgment 和当前 benchmark report。

## Benchmark 变体

报告用四种方式评估同一份 source trace：

| Variant | 排序 |
| --- | --- |
| `b1_bm25` | 有 exact identity rank 时用它，否则用 BM25 rank |
| `b2_dense_vector` | dense-vector rank |
| `b3_hybrid_rrf` | RRF rank |
| `b4_reranked` | reranker rank |

每个 variant 报告 Recall@20、nDCG@10、MRR、
Resource Diversity、Scope Violation、Stale Result，
以及 warm p50/p95 阶段延迟。Scope Violation 表示某个被排名的
resource 不在冻结 corpus 中。Stale Result 表示 resource
存在，但候选记录的完整 resource hash 与冻结 corpus hash 不同。
candidate-unit hash 与 full-resource hash 分开存储。

这些计算让评测流水线可执行，但并不宣称生产检索质量。
具有代表性的 query set 与 relevance label 必须来自真实的
Organization 与 Project 使用，并经人工审阅后才能成为质量门。

## 本地 API

原生客户端通过 typed XPC 使用：

- `list_retrieval_runs`
- `get_retrieval_run`
- `create_evaluation_case`
- `replace_evaluation_judgments`
- `clear_retrieval_runs`
- `export_evaluation_set`

daemon 的可执行契约由 Rust 请求与响应类型、
IPC 分派表和 Rust/macOS 契约测试共同定义。
Desktop Diagnostics 分为 Runtime 与 Retrieval 两个页面。
Retrieval 展示近期 Run、阶段 rank 与 score、
最终 disposition、relevance 控件、遗漏证据、benchmark 指标、
export 和 clear-unpinned-history 操作。

## 验证

自动化覆盖包括：

- schema 17 到 18 的迁移与中断 Run 的恢复；
- 通过真实 daemon 入口记录成功与失败的 activation；
- 返回片段与选中 trace candidate 的身份一致；
- 冻结 corpus 的创建、相关度与遗漏证据的替换，以及 CAS；
- B1–B4 导出、pinned Run 保留、历史清理和重启复用；
- Scope Violation 与 Stale Result 的独立语义；
- daemon OpenAPI 生成与原生 Swift 解码。

在收集到具有代表性、经人工审阅的 Evaluation Case，
并审阅导出的 B1–B4 baseline 之前，生产质量门仍然开放。
