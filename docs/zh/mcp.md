# MCP：Memory 工具

MCP 是 Coding Agent 使用当前 Project Memory 的接口。
工具调用中不需要填写 Server 地址、access token 或
`project_id`：受管集成把当前工作目录解析为 Project，
daemon 负责本地数据和带认证的同步。

通常的调用顺序是 **activate → 按需 load → 用户要求维护 Memory 时才 store**。
`activate` 帮你发现相关上下文，
`load` 提供完整且准确的资源，`store` 提出变更。它与 Desktop XPC、
Server HTTP 的区别见[领域接口](/zh/reference/domain-api)。

Clumsies 只向 Coding Agent 暴露一个工具：

| 工具 | 职责 |
|---|---|
| `memory` | 读取当前绑定 Project 的 Effective Memory，或持久化由该 Project 携带的提案 Draft（`store`） |

| operation | 何时使用 | 结果 |
| --- | --- | --- |
| `activate` | 开始实质任务，还不知道哪些资源相关 | 排序后的片段，带资源 ID 和路径 |
| `load` | 需要完整资源，或者即将修改它 | 完整资源、稳定 ID 和完整资源哈希 |
| `store` | 用户明确要求创建、更新、重命名、删除或丢弃受管 Memory | 持久化的本地 Draft 操作及同步状态 |

App 内置的 Rust `clumsiesd mcp serve` 进程是协议代理。
Effective Memory 构建、索引、检索、
精确加载和 Draft 持久化都由常驻 `clumsiesd` 负责，通过本地 XPC 访问。
代理只暴露强类型的 `memory` 工具，不能把任意 JSON 转发给 daemon 方法。

代理在解析当前目录的 Project binding 之前，
会校验常驻 daemon 的 Agent runtime 协议修订和构建标识；
常驻进程在每次 Agent 作用域的分派中都会重新校验该标识。常驻进程或代理缺失、
过期时会显式失败，不会混用不同发布版本。

没有 setup 调用。已移除的 `retrieve` 工具、
host-session binding、`META_PROMPT.md` bootstrap
和 MCP attestation 路径都没有兼容分派。
运行时指引通过 `InitializeResult.instructions` 和工具描述提供。

下文示例展示工具的**参数**或返回的领域对象，省略外层 JSON-RPC 包装。
MCP 成功时在 `structuredContent` 和 `content`
的序列化文本中返回领域对象，并带 `isError: false`；
工具失败使用 `isError: true`。协议报文本身不合法时，
也可能直接在 JSON-RPC 层失败。

## 通用输入规则

- `op` 必须且只能包含 `activate`、`load` 或 `store` 之一。
- 可选字段应直接省略，不要发送 `null`；未知字段会被拒绝。
- 字段名区分大小写。`knownHashes` 使用 camelCase，
  `expected_hash` 使用 snake_case。
- Project binding 属于受管集成，
  调用方不能用 `project_id` 参数改写它。

## `memory`

`memory` 用一个 `op` tagged enum 在单个工具下统一所有 Memory
操作：`activate`、`load` 和 `store`。

### 记忆维护规范（`CLUMSIES.md`）

每个 Project 可以约定一份受管 Memory 指南，
默认位于 Memory 路径 `CLUMSIES.md`。
它是 Effective Memory 内的确切路径，不是让工具打开操作系统中的任意文件。
这份文档定义：

1. **分类与组织**：标准目录结构（例如 `architecture/*`、
   `decisions/*`、`guides/*`）。
2. **更新规则与变更策略**：Agent 何时应提出 Draft、
   写什么 description、哪些内容不应持久化。
3. **弃用策略**：冲突或过时的 Memory 应如何被取代。

维护 Memory 前，Agent 通过 `memory` 的
`op: { load: { ids: ["CLUMSIES.md"] } }` 读取完整规范；
MCP 提示的配置路径不同时，使用那个确切路径。同一任务中仍有效、仍在上下文里的规范可以复用；
普通只读任务无需全文加载。用户在规范中维护的约定决定适用的维护约定；App 内置模板只是起点，
不增加写入授权。

若规范返回 `memory_resource_not_found`，报告缺失的路径。
这只说明当前项目视图中没有该文档，不代表整个组织没有。可以继续检索，
并依据用户指令和现有约定完成要求明确、已获授权的修改；只有依赖缺失规范的决定才需要澄清。
不要自动创建规范、替换为本地文件，或从缺失的自定义路径回退到默认路径。

用户希望设置规范时，可在 App 的空项目 Memory 页面选择
**Set Up Guidelines** 或 **Use Team Guidelines**。
后者选择组织已有资源，需要项目管理员权限。采用规范是可选操作；
内置模板只有在用户采用后才成为 Memory。概念、
预览和研究出处见[记忆维护规范](/zh/guides/memory-guidelines)。

### `activate`

Clumsies 与宿主原生记忆并存。Agent 遵循适用的宿主记忆政策，
也在实质项目任务中查询 Clumsies，即使已经查询过宿主记忆。在一个记忆空间完成读写，
不代表另一个也已完成；维护 Clumsies 时遵循绑定项目的 Memory
Guidelines。用户明确要求跳过 Clumsies 或仅使用其他来源、保存位置时，
尊重该选择。

每个实质任务开始时调用一次 `memory`，传入
`op: { activate: ... }`：

```json
{
  "op": {
    "activate": {
      "query": "调整 MCP 混合检索接口",
      "state": "optional-opaque-state"
    }
  }
}
```

| 字段 | 必填 | 语义 |
|---|---:|---|
| `query` | 是 | 非空的自然语言任务或检索线索 |
| `state` | 否 | 上一次响应的 `next_state`；仅在上一次片段仍完整保留于模型上下文时传入 |

daemon 在一次调用内完成 BM25 和向量召回、RRF 融合、
Cross-Encoder 重排、资源多样性限制、token 预算和片段增量计算。`kind`、
`group`、`limit`、模型名和排序参数都不是 Agent 可见的输入。

检索参数属于 daemon；调用方描述任务，而不是调优检索引擎。
排序和诊断细节见[检索评测](/zh/retrieval-evaluation)。

响应包含：

```json
{
  "index_revision": "search_...",
  "profile": "agent_activation.v2",
  "next_state": "opaque-state",
  "fragments": [
    {
      "action": "add",
      "unit_key": "mem_123/memory-delta/0/0",
      "content_hash": "sha256:...",
      "resource_id": "mem_123",
      "scope": "org",
      "kind": "memory",
      "path": "architecture/retrieval.md",
      "heading_path": ["MCP", "Memory Delta"],
      "content": "..."
    }
  ],
  "removed": []
}
```

响应还可能包含用于本地诊断的 `run_id`。`add` 与 `replace` 携带正文；
`reuse` 表示调用方上下文里已有同一片段，因此省略正文；`removed` 只撤销已删除、
失去权限或重新解析后消失的单元。仅对当前 query 不相关的单元不会被移除。

上下文压缩、旧工具输出已丢弃或开始新任务时必须省略 `state`。
无效或不支持的 state 返回 `invalid_activation_state`，
不会静默按空状态处理。

daemon 在后台准备固定的本地模型。模型就绪前，
activate 返回 `search_model_preparing`，并给出当前和总字节数，
不会一直占用 MCP 请求。模型准备在后台重试，没有仅词法的降级路径。

### `load`

对已通过 ID 或精确路径确定的完整资源（包括 `CLUMSIES.md` 这类项目规范），
使用 `memory` 的 `op: { load: ... }`：

```json
{
  "op": {
    "load": {
      "ids": ["mem_123", "CLUMSIES.md", "architecture/retrieval.md"],
      "knownHashes": {
        "mem_123": "sha256:..."
      }
    }
  }
}
```

`ids` 必填、非空、无重复，且每项都是字符串。`knownHashes` 可选。
已知哈希与当前完整资源一致时返回 `changed=false` 并省略正文。
任一请求目标不存在时返回 `memory_resource_not_found`，不会静默忽略。

`load` 与 `activate` 读取同一份 Effective Memory，
包括当前本地 Draft overlay。它不做模糊检索、embedding 或重排。

没有匹配 known hash 时，响应类似下面这样；ID 和哈希仅用于示例：

```json
{
  "resources": [
    {
      "resource_id": "mem_123",
      "scope": "org",
      "kind": "memory",
      "path": "architecture/retrieval.md",
      "title": "检索",
      "description": "项目如何检索 Memory",
      "content_hash": "sha256:example",
      "changed": true,
      "content": "# 检索\n\n编辑前先加载完整资源。"
    }
  ]
}
```

修改时使用返回的 `resource_id`，`expected_hash` 使用返回的
`content_hash`。activation 中的片段哈希只标识片段，
不是 `store.update` 要求的完整资源哈希。

### `store`

只有用户明确要求创建、更新、重命名、删除或丢弃受管 Memory 时，
才调用 `memory` 的 `op: { store: ... }`。

MCP 在当前目录绑定的 Project 中创建归该 Project 所有的 Draft。
更新或重命名被选中的 Org 引用会创建一个显式的 Project 适配，
带独立身份和固定来源版本。删除 Org 引用需要在 App 中移除它的选择，
或提出显式的 Org 提案。已有的 Org Draft 保留其原始发布目标。
保存只修改该 Project 的本地 Effective Memory；
发布仍然需要一次 Review。MCP 不提供 Review 决定或合并操作。
项目 PR 和可选的 Org 贡献是相互独立的 Review，权限和结果各自独立。

operation：

| operation | 必填字段 | 可选字段 |
|---|---|---|
| `create` | `path`、`body` | `description` |
| `update` | `id`、`expected_hash`、`replacements` | `description` |
| `rename` | `id`、`new_path` | `description` |
| `delete` | `id` | `description` |
| `discard` | `id` | 无 |

`resource` 是 `store` 上的可选字段，
与 `create` 或 `update` 并列，不放在该 operation 内部。
它唯一允许的值是 `memory`，也是默认值。ID 可能带 `mem_` 前缀，
也可能是历史 `ctx_` / `rul_` / `wfl_` 值；历史 ID 保持稳定、
不透明，永不改写。

`delete` 从 Local Effective Memory 中移除目标项。
如果该项是尚未发布的 Create Draft，
daemon 会把操作归一化为 `discard`；
只有删除权威资源才会留下可提交 Review 的开放式 Delete Draft。

Create 示例：

```json
{
  "op": {
    "store": {
      "create": {
        "path": "release/RELEASE.md",
        "description": "项目发布步骤和发布前验证要求",
        "body": "# 发布\n\n发布前先完成验证。"
      }
    }
  }
}
```

更新资源前先调用 `load`，并把它返回的完整资源 `content_hash` 作为
`expected_hash`。一次 update 包含一个或多个精确文本替换：

```json
{
  "op": {
    "store": {
      "update": {
        "id": "mem_123",
        "expected_hash": "sha256:...",
        "replacements": [
          {
            "old_text": "发布前先完成验证。",
            "new_text": "发布前先完成构建、测试和文档验证。"
          }
        ]
      }
    }
  }
}
```

每个 `old_text` 必须在当前 Effective Memory 资源中恰好出现一次。
同一次 update 内的替换不能重叠，并针对同一份原始内容原子应用。哈希过期、匹配缺失、
匹配不唯一或区间重叠都会拒绝整个 update，不创建任何 Draft 操作。
`new_text` 可以为空，用于删除文本。

路径属于受管 Memory 命名空间，不是任意的本地文件系统文件。
create 的 `body` 是完整的资源正文；
update 从不接受 Agent 提供的完整正文——daemon
会把已验证的替换具体化为完整的 Draft 结果。Memory 正文是 Markdown；
一份资源读起来是规则、流程还是上下文，由内容和路径表达，而不是由 wire type 表达。
`description` 是可选的语义摘要和检索字段。
它的发布目前存在 Server 持久化缺口，参见
[数据模型的实现说明](/zh/data-model)。
Metaprompt 和 `mpf` 不是合法的 wire 值。

成功的结果包含本地操作 ID、Draft ID、队列状态和同步状态。
它表示操作已持久化在本机并排队等待自动同步，不表示 Review 已合并或某个权威 Ref
已前进。普通 Project 成员可以提出并提交变更，
Project owner/administrator 决定并合并 Project
Review。Org Review 需要 Org owner/administrator 权限。

例如，一个已排队的本地写入可以返回：

```json
{
  "local_operation_id": "lop_example",
  "draft_id": "drf_example",
  "queued": true,
  "sync_status": "queued"
}
```

`sync_status` 可能为 `queued`、`syncing`、`retrying`、
`synced` 或 `failed`。即使是 `synced`，
也只表示 Draft 到达 Server，不表示已获批或发布。

## 错误与恢复

| 错误或状态 | 应如何处理 |
| --- | --- |
| `search_model_preparing` | 等待后台模型准备；响应会报告进度 |
| `invalid_activation_state` | 丢弃失效 state，用新上下文重新 activate；不要声称缺失的片段仍然可用 |
| `memory_resource_not_found` | 检查精确 ID/路径和当前 Project；load 不会静默跳过缺失目标 |
| `memory_content_changed` | 重新加载完整资源，重新判断要做的修改 |
| `text_replacement_not_found` / `text_replacement_ambiguous` / `text_replacement_overlap` | 根据新加载的正文修正精确替换范围；update 会被原子拒绝 |
| `agent_runtime_mismatch` | 让受管代理与常驻 daemon 的构建保持一致；可能需要重启仍使用旧集成的 Agent host |

update 失败不意味着可以改用完整正文覆盖。
activate 失败也不意味着没有相关 Memory。应把它们视为失败的操作，
再按明确的错误恢复。

## Daemon 操作

| XPC 方法 | 使用方 |
|---|---|
| `activate_memory` | MCP `activate` |
| `load_memory` | MCP `load` |
| `store_draft_operation` | MCP `store`、Desktop 及其他客户端 |
| `search_index_status` | Desktop 诊断与测试 |
| `rebuild_search_index` | 恢复、测试与开发工具 |

每次有效的 `activate_memory` 调用还会用与响应相同的候选轨迹写入一条本地
Retrieval Run。这不会为 MCP `activate` schema 增加字段。
检索历史、Evaluation Case 和 B1–B4 导出是 daemon/Desktop
的诊断 API，见 `docs/retrieval-evaluation.md`；
它们不是额外的 MCP 工具，也永远不会发送给 Server。

默认检索 profile 没有静默的仅 BM25、旧子串搜索或回退到不兼容索引的路径。
兼容的替换索引正在构建时，上一个就绪的 generation 仍可查询；
调度器在新 generation 构建完成后原子发布它。模型、向量、
generation 和状态失败都保持为显式的协议错误。

## 实现入口

输入校验和对外工具 schema 位于
[mcp_contract.rs](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/agent_runtime/mcp_contract.rs)。
[mcp.rs](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/agent_runtime/mcp.rs)
负责 stdio/JSON-RPC 和结果包装；
[检索响应类型](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/search/mod.rs)
和
[daemon 操作类型](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/types.rs)
定义返回数据。
