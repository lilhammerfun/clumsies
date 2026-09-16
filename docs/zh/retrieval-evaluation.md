# Memory 检索与评测设计

本文定义 resident daemon 当前的本地检索索引、`activate_memory` 排序链、Retrieval Run
历史和 Evaluation Case 评测契约。Memory corpus 的来源与合成规则见
[《统一 Memory 模型》](./unified-memory-model.md)。检索历史是本机诊断数据，不是 Server
遥测；daemon 不上传 query、candidate、evidence 或 benchmark。

## 1. 边界与快照语义

搜索索引位于每个 Project 的 Local Storage，属于可重建缓存。一次检索只查询当前
`ready` 的 `search_head`；当 Effective Memory 变化而新索引仍在后台构建时，旧 ready
revision 继续服务查询，构建成功后再原子切换 head。

因此 `activate_memory` 不承诺每次都命中调用瞬间最新的 Effective Memory。Retrieval Run
会固化**实际查询**的 index revision、indexed effective hash 和 corpus，诊断与评测必须
以这组值为准。如果 Project 从未生成可用 ready revision，调用返回 index/model
preparing 或明确的构建失败，而不是查询半成品索引。

Project Local Storage 只保存可重建的 index revision；中心 `local.db` 保存 Retrieval Run
与 Evaluation Case。移动或清理 Project Local Storage 不等于清除评测历史。
当前中心 SQLite schema 为 42，Project search-index SQLite 是独立的 schema 7；两者的
版本号和迁移职责不能混用。

## 2. 资源、检索单元与索引

一个 `SourceResource` 会产生多个 `RetrievalUnit`：

- 非空 description 单独成为一个 unit，使摘要能独立参与 BM25、向量和 rerank；
- 正文不超过 384 token 时作为一个 root unit；
- 更长 Markdown 优先按 heading section 与 block 边界切分；超过 480 token 的 span 再按
  384 token 窗口切分，并保留 48 token overlap；
- `unit_key` 由 resource、heading identity、同名 heading occurrence 和 part index 构成，
  是跨重建可解释的逻辑身份；
- `unit_rowid` 只是当前 SQLite revision 内部的 join/FTS rowid，不能暴露为稳定身份。

Project index 的主要表为：

| 表 | 用途 |
| --- | --- |
| `search_revisions` / `search_heads` | building/ready revision 与当前可查询 head |
| `search_resources` | 本 revision 的完整 resource 元数据与正文 |
| `search_units` | unit、locator、token、向量和稳定 `unit_key` |
| `search_units_fts` | 与 `search_units.unit_rowid` 对齐的 FTS5 检索面 |
| `search_vector_cache` | 按输入 hash 与 embedding revision 复用向量 |

`search_units_fts` 使用大小写不敏感 trigram tokenizer；`revision_id` 和 `unit_key` 为
UNINDEXED，参与 BM25 的列及权重依次是 path 8、title 6、heading 4、body 1。scope 和 kind
不进入 FTS 条件；可见 corpus 已由 Project 的 Effective Memory 边界确定。

向量输入由 path、heading path 和 unit text 组成。当前 `token_count` 只统计 unit text，
不计额外 path/heading 与模型 passage 前缀；极端长路径或标题可能让真实 embedding 输入
超过按正文切块推导的预算，这是已知限制。

## 3. 查询与组装链

当前 `agent_activation.v2` 按以下顺序执行：

```text
identity exact/prefix + FTS5 BM25（最多 60）
  -> query embedding + 全 corpus 向量相似度（最多 60）
  -> RRF(k = 60，保留 40)
  -> rerank 前 24
  -> relevance / overlap / 每资源 / token / fragment 预算
  -> activation delta
```

identity recall 先匹配 resource ID，再匹配 path/title 的完整值，最后匹配三者前缀。FTS5
查询少于 3 个字符的 term 会跳过；exact/prefix 和向量 recall 仍可工作。非 ASCII query
拆为最多 32 个三字符窗口并以 OR 查询，这改善 CJK substring recall，但不是语言学分词。

最终最多返回 12 个 fragment、每个资源最多 2 个，总正文预算 2400 token。每个候选保存
exact、BM25、vector、RRF、reranker 和 final rank/score，以及稳定的 exclusion reason。
低于 rerank relevance、重叠、单资源上限、token 上限、fragment 上限和未进入 rerank 都
是可区分结果，不能统称为“搜索没找到”。

响应 fragment 携带 `unit_key`、`content_hash`、resource ID、scope、kind、path、heading
path 和 delta action。客户端把上一轮 activation state 原样传回时，daemon 按稳定 unit
身份与内容 hash 生成 `add`、`replace`、`reuse`，并列出应该从上下文移除的 unit；`reuse`
省略正文。state 只描述仍留在模型上下文中的上一轮片段，不是永久缓存句柄。

整个 activation 有 60 秒 deadline。lexical、embedding/vector、fusion、rerank 与 assembly
当前是顺序链，不应把它描述为并行检索。

## 4. Retrieval Run

对通过 Project/query 基础校验的 activation，daemon 会 best-effort 创建一条 Run。历史
写入失败会记录本机日志但不改变检索结果，此时 MCP 响应的 `run_id` 可以为空。成功创建
的 Run 状态为 `running`，随后以一个终态完成：

- `succeeded`：保存实际 revision/hash/corpus、全部 fused candidates、各阶段 rank/score、
  exclusion reason、delta action、阶段耗时与返回预算；
- `failed`：保存能够取得的版本和阶段信息，以及有界的 error stage/code/summary；
- daemon 重启时残留的 `running` 会改为 `failed`，错误码为 `retrieval_interrupted`。

MCP response 与持久 trace 来自同一份内存候选结果。若最终历史落盘失败，调用仍返回已经
完成的检索；因此历史是可解释性与评测能力，不是 agent-facing success 的事务组成部分。

中心数据库和内容存储包括：

| 表或目录 | 用途 |
| --- | --- |
| `retrieval_runs` | query、实际 revision/version、状态、延迟、结果计数和错误 |
| `retrieval_run_candidates` | 全部候选的阶段排名、分数、结果和 delta |
| `retrieval_run_resources` | 本 Run 实际查询的有序 corpus manifest |
| `retrieval_corpus_blobs` | content-addressed blob 元数据 |
| `evaluation-corpora/blobs/<prefix>/<sha256>` | owner-only 的完整 resource body |

每个 Project 最多保留 500 条未 pin Run。被 Evaluation Case 引用的 Run 会保留；Clear
History 只删除未 pin Run，并回收不再被 Run 或 Evaluation Corpus 引用的 blob。

## 5. Evaluation Case 与二元 evidence

`create_evaluation_case(run_id)` 从一条具有完整 corpus 的 Run 创建或返回唯一 Case，冻结
query、Project、实际 Effective Memory manifest、resource body 和 source candidate trace。
新 Case 为 `draft`，version 从 1 开始。

当前评测模型是**已确认的相关 evidence 集合**，不是 0–3 relevance judgment：

- `evaluation_evidence` 保存 `(case_id, resource_id, unit_key?)`；
- 有 `unit_key` 表示 source Run 中的具体候选 unit；
- `unit_key` 为空表示整个 frozen resource 可作为相关 evidence；
- excerpt 由 daemon 从候选或 frozen resource preview 生成，不接受客户端伪造；
- 同一 Case 中 evidence identity 不得重复。

`resolve_evaluation_case` 必须携带 `expected_version`，并且只接受二选一：

1. 非空 `evidence[]` 且 `none_matched = false`：整体替换 evidence，状态变为 `ready`；
2. 空 `evidence[]` 且 `none_matched = true`：状态变为 `needs_evidence`。

过期 version 返回 `evaluation_case_conflict`。`none_matched` 表示当前 frozen corpus/trace 中
尚未确认 evidence，不等同于负相关评分；`needs_evidence` 不参与报告。只有 `ready` Case
会生成单 Case report，并可进入集合 export。

当前表名是 `evaluation_evidence`。历史 migration 曾短暂使用
`evaluation_judgments`、0–3 relevance 和 missed 标志，但它们不是现行 schema 或 API，
文档、SQL 和客户端不得继续使用这些名称。

## 6. Benchmark 与导出

同一 source candidate trace 以二元 evidence 评估四个排序视图：

| Variant | 排序依据 |
| --- | --- |
| `b1_bm25` | exact identity rank 优先，否则 BM25 rank |
| `b2_dense_vector` | vector rank |
| `b3_hybrid_rrf` | RRF rank |
| `b4_reranked` | reranker rank |

每个 variant 报告 Recall@20、binary nDCG@10、截断到 top 10 的 MRR，以及基于 top 20
的 Resource Diversity、Scope Violation 和 Stale Result。evidence 权重全部为 1；
resource-level evidence 可由该 resource 的任一候选匹配。Scope Violation 表示候选
resource 不在 frozen corpus，Stale Result 表示 resource 存在但候选记录的完整 resource
hash 与 frozen hash 不同，两者独立计算。

报告字段还包含 `warm_p50_us` / `warm_p95_us`，但当前实现只是按 variant 汇总 source Run
已记录的阶段延迟，并未重新执行隔离的 warm benchmark。它们适合比较这组历史 trace，
不应宣传为受控压测结果。

`export_evaluation_set` 只选择 `ready` Case，返回两个并列字段：

- `fixture_json`：自包含的版本化 JSON，含 frozen bodies、Case、evidence、source Run 和
  candidate trace；
- `report`：本次选择集合的 benchmark report。

report **不嵌在** `fixture_json` 中。当前 macOS Save Panel 只把 `fixture_json` 写入用户
选择的文件，虽然 XPC response 同时解码 report；离线消费者若需要 report，必须单独保存
或重新计算，不能假定导出 JSON 已包含它。

## 7. 本地 API 与 Diagnostics

原生客户端通过 typed XPC 使用：

- `list_retrieval_runs`
- `get_retrieval_run`
- `create_evaluation_case`
- `resolve_evaluation_case`
- `clear_retrieval_runs`
- `export_evaluation_set`

daemon 的可执行线协议由 Rust 请求/响应类型、IPC 分派表与 Rust/macOS 契约测试共同定义。
macOS Diagnostics 的 Retrieval 页面展示近期 Run、阶段 rank/score、最终 disposition、
evidence suggestions、Case 状态、benchmark、export 和 clear-unpinned-history；它不经
Server，也不改变 MCP `activate_memory` 的请求格式。

## 8. 已知限制与验证

- 索引重建期间查询旧 ready head，因此“Memory 已更新”与“本次 Run 已检索新内容”必须
  通过 Run 的 indexed hash/revision 区分。
- trigram FTS 对不足 3 字符的查询不工作，CJK 最多展开 32 个窗口；exact/prefix 和 vector
  是当前补充，不代表短查询质量已有保证。
- 向量相似度当前在内存中扫描当前 revision 的全部 unit，corpus 增长后的性能边界需要用
  warm p50/p95 与实际数据验证。
- embedding token 预算未计 path/heading/prefix；极端元数据可能超出模型输入边界。
- Evaluation Case 只支持二元 evidence，不能表达分级相关性、负例或独立 annotator。
- macOS 文件导出不包含 response 中的 report。

自动化至少覆盖 ready-head 切换、旧 head 持续可查询、chunk/unit identity、FTS5 与向量
召回、RRF/rerank/budget、返回片段与 trace 一致、失败 Run 与重启恢复、evidence CAS、
四个 variant、pinned Run 保留、history 清理、Scope Violation 与 Stale Result 独立语义，
以及 OpenAPI/Swift 解码。把评测结果升级为质量门之前，仍需收集真实 Project query 并由
人确认 evidence；实现自洽不等于检索质量已经达标。
