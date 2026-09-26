# Activity：本地 Agent 记忆活动

Activity 展示**本地 Agent 会话中的记忆使用情况**。
它把不同宿主的日志投影成同一套小层级：

> Agent 活动 → 用户请求 → Memory 检索 →
> 交付给 Agent 的 Memory 片段

它不是通用聊天记录，也不是完整 transcript 归档器。Assistant 回复、
与 Clumsies 无关的工具调用，以及 ChatGPT
`conversations.json` 导出都不在范围内。

## 界面呈现

界面按最新会话优先排列，使用三栏布局：

| 区域 | 内容 |
|---|---|
| 全局侧栏 | 现有全局侧栏，Activity 处于选中状态。 |
| 会话列表 | 已绑定工作区中的 Agent 活动；每行显示 **DSH** 或 **Codex** host badge、标题和时间。 |
| 详情 | 用户请求、Agent 写出的确切 Memory 查询、检索耗时，以及以原始 source 三行预览呈现的选中 Memory 片段。 |

host badge 是会话身份的一部分：session id 只要求在各自 host 内唯一。

有 run 身份的每次 Memory 检索提供 **Retrieval Process**
入口。点击后，工作区展示该 run 的摘要与完整候选表，并临时收起会话列表，同时保留全局侧栏；
通过原生导航栈的工具栏返回按钮恢复原会话和检索位置。详情顶部只显示 Agent query，
不重复会话标题和用户 prompt；导航容器按详情区可用宽度布局，缩窄窗口时不把内容挤出窗口。
这次下钻属于一次 activation；一个用户请求可以包含多次检索。
详情复用 **Report Inaccurate** 和
**Review Evidence**，可以为该 run 建立评估案例并核对证据。
共享的 Diagnostics 视图也支持跨会话的 Retrieval Runs 列表。
点击 Final、BM25、Vector、RRF 或 Rerank 列标题，
可按该阶段的数值排名升序或降序排列；缺失排名的项始终在末尾。

## 一次 Memory 检索的含义

用户请求和 Memory 查询是两份不同的数据：

- **用户请求**是 Agent harness 记录的人类消息。
- **Memory 查询**是 Agent 原样提交给 `memory.activate`
  的确切文本。daemon 只做首尾空白处理，不执行 LLM 查询改写、意图提取、
  对话扩展或隐藏 filter 推断。

原始查询依次参与精确 id/path/title 匹配、BM25 全文检索、语义向量检索、
reciprocal-rank fusion 与 cross-encoder rerank。
最终装配会去除重叠片段，并应用相关性、单资源数量、总片段数和 token budget 限制。
Activity 只展示最终提供给 Agent 的片段。
候选分数和被排除的候选放在按需打开的 Retrieval Process 详情里，
让会话阅读仍围绕请求与选中的片段展开。

“结果”列的信息按钮解释所有交付动作和排除原因，悬停单个结果可查看该行的说明。
`Not Reranked` 表示没有记录到重排结果：可能未进入重排范围，
也可能是该 run 在重排完成前已中止，不代表相关性低。

交付状态是上下文增量，不是 Memory 编辑：

- `add`：上次 activation state 中没有该片段，
  本次将它发送给 Agent；
- `replace`：Agent 已持有同一片段的旧版本，本次用新版本替换；
- `reuse`：片段未变化且已在 Agent 上下文中，本次响应可以省略正文。

UI 把这些值翻译成面向用户的交付标签，不会把 `add` 表述成创建或修改已存储的
Memory。

## 会话数据源

### DSH

DSH 会话位于
`~/.dsh/sessions/<encoded-workspace>/<session>/session.jsonl.zstd`。
该文件是 append-only 的多 frame zstd 归档：
DSH 把每条 JSONL 记录写成一个独立 frame。读取器必须消费所有完整 frame；
只解第一个 frame 只会得到 `session` header，使每个任务计数都为零。

投影只消费：

- `session` 与 `session/title`：身份、工作区、时间和标题；
- `user/message` 且 `source.kind == "user"`：人类请求；
- `tool/call`：当前 `mcp__clumsies__memory` 的
  activation 形状
  `{"op":{"activate":{"query":"...","state":"..."}}}`，
  以及旧 `mcp__clumsies__activate` / 顶层 `query` 形状；
- `tool/result`：用 `callId` 与调用配对，
  读取结构化 activation 结果或错误。

### Codex Desktop/App

Codex rollout 从 `~/.codex/sessions` 与
`~/.codex/archived_sessions` 发现。
`session_meta.payload.cwd` 把 rollout 绑定到工作区；
活动副本与归档副本按 session id 去重。

投影消费结构化的人类 `response_item` 记录、
旧 `user_message` 事件和已完成的 MCP 调用。
当前 activation 以原子方式记录为：

```text
event_msg.payload.type = "item_completed"
payload.item = {
  type: "McpToolCall",
  id: "...",
  server: "clumsies",
  tool: "memory",
  arguments: { op: { activate: { query: "...", state?: "..." } } },
  result?: { structuredContent: { run_id?, fragments, ... }, isError? },
  error?: { message: "..." }
}
```

旧 `mcp_tool_call_end` 事件，
以及更早的专用工具名 `memory/activate` 和 `activate`（包括
JSON 编码参数），作为兼容输入接受。其他 MCP Server 和 `memory`
的其他 operation 被忽略。

## 投影与 Retrieval Run 身份

每条真实人类消息开始一个任务；之后的 Clumsies activation 归入该任务，
直到下一条人类消息。DSH 用 `callId` 配对调用和结果；
Codex 的 `McpToolCall` 完成事件已同时包含两端。

新日志中，`structuredContent.run_id` 是关联
`retrieval_runs` 的权威身份；来自该确切 run 的选中候选提供稳定的片段身份、
heading、结果顺序和预览文本。查询文本只用于展示，不是身份键。
同一次关联读取还提供可选的 `total_us`：只显示已结束 run 记录的检索耗时，
运行中或无法关联时保持缺失。该值不是模型回复耗时，也不能通过简单相加各阶段计时得出。
片段数量包含 `reuse`，不表示本次新发送的内容数量。

检索历史只在每个候选行中保存有界预览，但也保留该 run 使用的完整 resource 正文。
在被截断或为空的片段预览上展开 **Show source** 时，
使用 `run_id + unit_key` 和候选的冻结 locator 读取完整正文，
并按原始 source 展示，不渲染 Markdown，
也不读取可能已经变化的当前 Memory。description-only 单元没有正文
byte range，因此只能如实回退到保存的预览。快照缺失或检索历史已清理时，
同样保留工具结果中记录的预览；历史记录不可用不等于没有发生检索。

旧日志可能没有 `run_id`。此时仍展示工具结果中自带的片段或错误；
daemon 只有在 `(project_id, query)` 恰好匹配唯一
retrieval run 时才补关联，不会从多个同 query 结果中擅自选择“最新一条”。
匹配不唯一时，`run_id` 与 run 状态保持缺失。

## 加载与分页

Activity 在首个请求前恢复该服务器、组织和账号上次选择的具体项目。不可用时，
依次回退到当前 Memory 项目和第一个可访问项目。
**All Projects** 是本次会话中的主动选择，不是启动默认值。
没有项目时不发起初始 Activity 请求。

`list_recalls` 只返回摘要，每页 20 条（最多 100 条）。
discovery 在阻塞线程池读取有界日志头/预览，不读取完整会话或检索记录。
它的不透明游标在不重新扫描目录的情况下延续一个不可变顺序。显式刷新才会发现新快照。

`get_recall_session` 使用摘要的不透明 `session_token`，
返回一页任务。只有选中的会话会被完整解析；后续页复用它的原始快照，只为当前页补齐检索详情。
原先 500 个任务/100 次 activation 的截断已移除。
完整 Memory 片段只在用户展开被截断或为空的预览时读取。

daemon 在内存中保留八个列表快照和四个选中会话正文，
不额外持久化 transcript 归档。句柄被淘汰或 daemon 重启后需要
**Refresh Activity**。绑定变化会使旧快照的访问失效。
单个损坏的日志头会被跳过；选中文件读取失败时在详情区域显示错误与重试。

首次加载在等待区域显示一个原生进度指示器。刷新保留内容和选中项；翻页进度停留在列表/任务末尾。
旧响应不能覆盖另一个项目的列表或另一个会话的详情。
该设计参考 Apple 的
[Loading](https://developer.apple.com/design/human-interface-guidelines/loading)
和
[Progress indicators](https://developer.apple.com/design/human-interface-guidelines/progress-indicators)；
共享视图不再把转圈与装饰性骨架行叠加在一起。

## 范围

- 数据源是已绑定工作区的 DSH 与 Codex Desktop/App rollout。
- 该模型是记忆活动投影，不是完整 transcript 的可复用归一化，
  也不是 ChatGPT 数据导出解析器。
- 浏览会话不会修改 Memory、Issue 或 session 文件。
  报告不准确的 run 和核对证据会更新检索评估记录，并为该评估保留 source run。

## 实现索引

| 关注点 | 路径 |
|---|---|
| 共享投影与 DSH reader | `crates/clumsiesd/src/recall.rs` |
| 摘要快照与任务分页 | `crates/clumsiesd/src/recall/paging.rs` |
| Codex rollout reader | `crates/clumsiesd/src/recall/codex.rs` |
| XPC dispatch | `crates/clumsiesd/src/state.rs`（`list_recalls`、`get_recall_session`、`get_recall_fragment`） |
| XPC client 与模型 | `apps/macos/Sources/Services/Daemon/DaemonXPCClient.swift`、`apps/macos/Sources/Libraries/Models/DaemonModels.swift` |
| 侧栏 section | `apps/macos/Sources/Libraries/Models/MemoryModels.swift`（`WorkspaceSection.sessions`） |
| Activity UI、host badge 与片段详情 | `apps/macos/Sources/Features/Activity/ActivityView.swift`、`ActivityModel.swift` |
| Workspace 接线 | `apps/macos/Sources/Features/Workspace/WorkspaceView.swift` |

## 本地界面预览

运行 `just dev-macos` 启动当前 worktree 的独立 Dev
Instance，再运行 `python3 dev/seed-activity.py`。
重新打开 Dev App，在 Activity 中选择
**Activity Preview**。数据覆盖原始标题、表格、代码、长 source、
复用片段、历史缺失、空/失败检索和可排序的数值排名。
`python3 dev/seed-activity.py --check`
可在不启动实例的情况下校验数据。
