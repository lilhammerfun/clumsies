# 概览

clumsies 是面向编码 Agent 的外部记忆与协作基础设施。它把可持久、可检索的 Memory 放在
单次模型会话之外，并把 Agent 写入变成可审查的 Draft，而不是不可见的本地修改。

## 核心对象

| 对象 | 当前语义 |
| --- | --- |
| Semantic Memory | Organization 权威中的 Markdown 内容对象及当前 Project Draft overlay，表达当前有效知识 |
| Episodic Memory | Server 从当前 Project 的 AgentRun Evidence 生成的版本化生产经历摘要；不走 Review、不跨 Project |
| ProjectEpisode / Evidence | 以 Project + AgentRun 为边界的长期经历和不可变原始依据；稳定 `episode_id` 支持显式下钻 |
| Organization authority | 唯一可发布 Memory 命名空间，拥有 Ref 与不可变 Commit 历史 |
| Project | 仓库绑定、成员授权、Organization Memory 选择、投影 Ref 与合并前 Draft overlay 的 carrier |
| Bundle | 某成员保存的共享 Memory ID 集合，不复制内容或建立新权威 |
| Draft / Review | 提案、协调与授权发布边界；Review 可有序包含一个或多个 Draft |
| Issue | Server 共享的原生 Kanban 对象，与 Memory 完全独立 |
| AgentRun | daemon 本机的 Agent 生命周期遥测，可绑定 Issue，但不直接决定 Issue 状态 |

新 Memory 使用 `mem_` 前缀；历史 `ctx_`、`rul_`、`wfl_` ID 保持稳定且不因统一模型
改写。规则、流程、项目背景等用途由正文和路径表达，不再由封闭的 Context、Rule、
Workflow 类型决定。

`description` 是目标模型中的语义摘要和独立检索字段，但当前写入/merge 链仍可能让它
为空或未被持久化；这属于已知实现缺口，详见[统一 Memory 数据模型](/unified-memory-model)。

## 产品与运行面

| Surface | 职责 |
| --- | --- |
| macOS Desktop | 浏览 Organization/Project/Effective Memory，编辑 Draft，Review/merge，配置/预览 Episode 摘要，查看 Issue、Activity 与诊断 |
| 常驻 `clumsiesd` | Draft 与队列、Commit/Episode sync、Project storage、Semantic/Episodic 联合检索、Issue 本地副本、AgentRun 和 XPC |
| Agent runtime | App 内 `clumsiesd mcp serve` 与 `_agent issue-run-event` 短进程代理 |
| Server | PostgreSQL 上的共享身份、Organization Memory、Project 投影、Review/Commit、Project Episode/Evidence/摘要、Issue/claim 权威 |
| MCP | 两个 Agent 工具：`memory`（`activate` / `load` / `evidence` / `store`）与 `kanban` |
| Web Admin | Organization、成员、Project、token、审计与健康管理 |

Organization 是唯一 Memory 发布权威。Project 视图由所选 Organization Memory 的
投影和 Project 携带的 Draft overlay 组成，不是第二个 scope。遗留
`/projects/{project_id}/memories` 接口只读取历史 Project-authority 数据，不是当前投影或
Effective Memory API。

Agent 检索使用 `MemoryCorpus = Semantic Effective Memory + 当前 Episodic summaries`。
二者共用一个 Project index，但结果显式保留 `semantic|episodic` 与
`organization|project` provenance；联合检索不把 Project Episode 变成 Organization
发布内容。

## Memory 生命周期

```text
任务线索
  -> memory.activate 返回相关片段
  -> 必要时 memory.load 读取完整资源
  -> Agent 在当前上下文中使用
  -> 用户明确要求时 memory.store 创建/修改本地 Draft
  -> daemon 自动同步 Draft
  -> 有序 Draft 提交 Review
  -> 授权决定与原子 merge
  -> Organization Ref 前移并刷新受影响 Project 投影
```

`memory.store` 从不直接修改权威。成功只表示操作已在 daemon 本地持久化并进入同步队列；
只有授权 Review merge 才创建新的 Organization Commit。

Episode 的独立路径是：结束 AgentRun → durable daemon outbox → Server Evidence → 绑定
Evidence hash 的版本化摘要 → `memory.activate`。摘要不足时才用 `memory.evidence` 分页
读取不可信原始记录；Server ACK 后不再依赖宿主 Session log 或 Project Local Storage。

## 版本模型

系统对等于 Git 的概念使用 Git 术语：

- Blob：内容寻址的不可变正文；
- Tree：某个版本的资源集合；
- Commit：带 parent 的不可变权威版本；
- Ref：可前移的头指针；Organization Ref 是发布权威，Project Ref 标识选择投影。

HTTP `ETag` / `If-Match` 和对象 revision 保护各自的并发边界，不能替代 Commit 历史，也
不能跨对象混用。Draft 保留明确 `base_commit_id`；上游 Ref 前进时 Draft 不会被隐式
改写。

## 当前实现边界

当前主链已经覆盖 Organization authority、Project Org Selection/Ref、Project-carried
Draft overlay、有序多 Draft Review、Commit 同步、Project Episodic Memory、MemoryCorpus
联合检索、
Retrieval Run/Evaluation Case、原生 Issue/claim、macOS XPC、OIDC/Keychain，以及
Codex、Claude Code、opencode、dsh、Antigravity 接入。

仍需如实保留的主要缺口：

- Public OpenAPI `TreeEntry` 仍声明旧 kind 且缺少 `description`，与 Rust/数据库不一致；
- `description` 非空与 merge 持久化尚未端到端保证；
- macOS 仍有 `MemoryKind` 驱动的创建、校验、预览和分组；
- Kanban 本地 SQLite 与 Server 之间没有持久 outbox，部分 mutation 存在本地先提交窗口；
- 自动三方冲突解决、代表性生产检索评测集和完整生产安装生命周期尚未闭环。

系统边界与决策见[系统架构](/architecture)，各专题当前权威见
[工程文档](/engineering-documents)。
