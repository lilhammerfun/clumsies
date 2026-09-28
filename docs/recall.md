# Activity：本地 Agent 记忆活动

Activity 是 macOS App 中只读的本地诊断视图。它把 DSH 与 Codex Desktop/App 的会话日志投影成同一套小模型：

> Agent 活动 → 用户请求 → Memory 检索 → 实际交付给 Agent 的 Memory 片段

它不是通用聊天记录，也不是完整 transcript 归档器。Assistant 回复、与 Clumsies 无关的工具调用、其他 MCP Server 的调用，以及 ChatGPT `conversations.json` 导出都不在范围内。

Activity 与 Project Episodic Memory 是两条不同产品路径：Activity 按需在本机投影会话，
用于回答“当时检索了什么”；Episodic Memory 则只在已绑定 Project 的 root AgentRun 结束
或恢复结束后截取该 Run 的 Evidence，经 durable outbox 上传 Server，生成可联合检索的
项目生产经历。后者不上传 Activity 页面、不创建 whole-session 对象，也不会进入
Organization Semantic authority。命中摘要后的原文下钻见 [`memory.evidence`](/mcp#evidence)。

## 当前契约

Activity 只回答三个问题：

1. 某个已绑定工作区中的用户请求是什么；
2. Agent 当时传给 `memory.activate` 的原始查询是什么；
3. 那次检索最终向 Agent 交付了哪些 Memory 片段。

界面按最新会话优先排列，使用三栏布局：

| 区域 | 内容 |
|---|---|
| 全局侧栏 | 选中 Activity 工作区。 |
| 会话列表 | 已绑定工作区中的 Agent 活动；每行显示 DSH 或 Codex host、标题、请求数和时间。 |
| 详情 | 用户请求、原始 Memory 查询、检索状态与已交付片段；存在历史快照时可打开完整片段。 |

会话身份是 `(host, session_id)`，因为 session id 只要求在各自 host 内唯一。项目筛选会包含该 Project 绑定的全部工作区。

## 用户请求与 Memory 查询不是同一份数据

- **用户请求**来自 Agent harness 记录的人类消息。
- **Memory 查询**是 Agent 原样提交给 `memory.activate` 的 `query`。daemon 只做首尾空白处理，不执行 LLM 改写、意图提取、对话扩展或隐藏 filter 推断。

查询依次参与精确 id/path/title 匹配、BM25 全文检索、语义向量检索、reciprocal-rank fusion 与 cross-encoder rerank。最终装配会去除重叠片段，并应用相关性、单资源数量、总片段数和 token budget 限制。

Activity 只展示最终交付给 Agent 的片段。候选分数、排序中间量和被排除的候选属于检索诊断数据，不作为 Activity 的用户内容显示。

### `add`、`replace` 与 `reuse`

这些值描述上下文增量，不是 Memory 写操作：

| 值 | 含义 |
|---|---|
| `add` | 上次 activation state 中没有该片段，本次将它发送给 Agent。 |
| `replace` | Agent 已持有同一片段的旧版本，本次用新版本替换。 |
| `reuse` | 片段未变化且已在 Agent 上下文中，本次响应可以省略正文。 |

UI 使用面向用户的交付标签，不会把 `add` 表述成创建或修改已存储的 Memory。

## 本地数据源

Activity 由本机 daemon 直接读取本地日志和本地 retrieval history。当前 App 只传 Project filter（或不传 filter），这两条 UI 路径都从 daemon 的 workspace binding 表取根目录，因此正常界面只列出已绑定工作区。

底层 `ListRecallsRequest.workspace_root` 是一个需要如实记录的例外：当前 daemon 会规范化调用方显式传入的路径，却不会先证明它存在于 binding 表。能够调用本地 XPC 的客户端若直接传这个字段，可能读取对应目录的 DSH/Codex 投影，但因 Project id 为空无法关联 retrieval history。当前 App 不使用该参数；在 daemon 补上 binding 拒绝之前，不能把“任意调用路径都强制限制为已绑定 workspace”写成已完成的权限保证。

### DSH

DSH 会话位于：

```text
~/.dsh/sessions/<encoded-workspace>/<session>/session.jsonl.zstd
```

`session.jsonl.zstd` 是 append-only 的多 frame zstd 文件：DSH 每追加一条 JSONL 记录就写入一个独立 frame。读取器必须消费所有完整 frame；只解第一个 frame 通常只会得到 `session` header，使任务数看起来为零。若读取时正好遇到未写完的末尾 frame，Activity 保留此前已完整解出的前缀，等待下次刷新。

投影只消费：

- `session` 与 `session/title`：session id、工作区、时间和标题；
- `user/message` 且 `source.kind == "user"`：人类请求；
- `tool/call`：当前统一工具 `mcp__clumsies__memory` 的 `{"op":{"activate":{"query":"...","state":"..."}}}`，以及旧 `mcp__clumsies__activate` 的顶层 `query` 兼容格式；
- `tool/result`：用 `callId` 与调用配对，读取结构化 activation 结果或错误。

统一 `memory` 工具中的 `load`、`store` 等其他 operation 不会被误判为 activation。

### Codex Desktop/App

Codex rollout 从以下目录递归发现：

```text
~/.codex/sessions
~/.codex/archived_sessions
```

开发实例可以通过 daemon 配置使用独立的 Codex home。`session_meta.payload.cwd` 必须能匹配已绑定工作区；subagent rollout 被排除。活动目录和归档目录出现同一 session 时，活动副本优先；标题从本地 `session_index.jsonl` 读取。

当前结构化 activation 记录为：

```text
event_msg.payload.type = "mcp_tool_call_end"
payload.call_id
payload.invocation = {
  server: "clumsies",
  tool: "memory",
  arguments: { op: { activate: { query: "...", state?: "..." } } }
}
payload.result = {
  Ok: { structuredContent: { run_id?, fragments, ... }, isError? }
} | {
  Err: "..."
}
```

用户请求优先读取结构化 `response_item` 中标记为 `user.text` 的内容，并兼容旧 `event_msg.payload.type = "user_message"`。旧工具名 `memory/activate`、`activate` 及 JSON 字符串参数继续作为只读兼容输入；其他 MCP Server 和 `memory` 的其他 operation 被忽略。

## 请求、activation 与 Retrieval Run 的关联

每条真实人类消息开始一个请求；之后的 Clumsies activation 归入该请求，直到下一条人类消息。DSH 用 `callId` 配对调用和结果；Codex 的 `mcp_tool_call_end` 已同时包含两端。

新日志中，`structuredContent.run_id` 是关联本地 `retrieval_runs` 的权威身份。daemon 还会校验该 run 属于工作区绑定的 Project；查询文本只用于展示，不是身份键。

选中候选的 `unit_key`、heading、最终顺序和预览来自该 run。打开片段时使用 `run_id + unit_key` 和冻结 locator，从该次 run 保留的 corpus body 读取历史正文，而不是读取可能已经变化的当前 Memory。description-only 检索单元没有正文 byte range，因此只能如实返回当时保存的预览。

旧日志可能没有 `run_id`。此时仍展示工具结果中自带的片段或错误；daemon 只有在 `(project_id, query)` 恰好匹配唯一 retrieval run 时才补关联，不会从多个同 query 结果中擅自选择“最新一条”。匹配不唯一时，run 状态与历史正文保持缺失。

## 隐私与权限边界

Activity 不会为了生成视图把本地会话日志上传到 Server，也不会调用模型总结日志。读取和
投影发生在本机 daemon；App 通过本地 XPC 请求结果。独立 Episode ingestor 对结束
AgentRun 的显式摄取不改变这一诊断边界；它产生的是 Project-governed Server Evidence，
不是 Activity 的远端副本。

但“本地”不等于“不敏感”：Activity 会显示日志中记录的完整用户请求、Agent 写出的原始 Memory 查询，以及被选中的 Memory 片段。能够访问本机账号、这些日志文件或已解锁 App 的人，可能看到这些内容。当前实现不提供额外的字段脱敏、按消息授权或 Activity 专属加密层；敏感信息不应写入 prompt、查询或 Memory 正文。

当前 UI 路径与历史正文读取具有以下边界：

- App 的无 filter / Project filter 只从 daemon binding 表发现 workspace；
- Project filter 只包含绑定到该 Project 的 workspace；
- `run_id` 与完整历史片段读取必须再次通过 Project 边界校验。

底层 `workspace_root` filter 尚未验证 binding，是当前隐私边界缺口，不应由文档掩盖。它不改变“日志只在本机读取”的事实，但会扩大本地 XPC 调用者可选择的目录范围。

Activity 页面及其 XPC 方法不会修改 Memory、Issue、session 文件或 retrieval history，也不会导入 ChatGPT 数据导出。源日志与 retrieval history 的保留和删除仍由各自的本地存储生命周期负责，不由 Activity 视图管理。

## 资源上限与失败行为

- 默认最多返回 50 个 session，请求上限为 200，按新到旧排序；
- 每个 session 最多投影 500 个用户请求；
- 每个请求最多投影 100 次 activation；
- 单个损坏、正在追加或无法读取的 session 会被跳过并记录 warning，不会清空其他来源；
- Codex discovery 整体失败时会记录 warning，并保留已经读取的 DSH 结果；DSH session 文件单项失败也只跳过该文件。DSH 工作区目录本身无法枚举时，当前请求会失败。

这些上限用于控制本地诊断视图的读取成本；Activity 不是无限历史导出接口。

## 实现索引

| 关注点 | 路径 |
|---|---|
| 共享投影、绑定过滤与 DSH reader | `crates/daemon/src/recall.rs` |
| Codex rollout discovery 与 parser | `crates/daemon/src/recall/codex.rs` |
| 历史片段与 Project 边界 | `crates/daemon/src/retrieval_history.rs` |
| XPC dispatch | `crates/daemon/src/state.rs`（`list_recalls`、`get_recall_fragment`） |
| XPC client 与模型 | `apps/macos/Sources/Infrastructure/DaemonXPCClient.swift`、`DaemonModels.swift` |
| Activity UI 与 host badge | `apps/macos/Sources/Features/RecallView.swift`、`RecallModel.swift` |
| Workspace 接线 | `apps/macos/Sources/Features/WorkspaceView.swift` |
