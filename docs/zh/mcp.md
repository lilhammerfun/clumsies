# MCP：Memory 工具

MCP 是 Coding Agent 使用当前 Project Memory 的接口。工具调用中不需要填写 Server 地址、access token 或 `project_id`：受管集成把当前工作目录解析为 Project，daemon 负责本地数据和带认证的同步。

通常的调用顺序是 **activate → 按需 load → 用户要求维护 Memory 时才 store**。`activate` 帮你发现相关上下文，`load` 提供完整且准确的资源，`store` 提出变更。它与 Desktop XPC、Server HTTP 的区别见[领域接口](/zh/reference/domain-api)。

Clumsies 只向 Coding Agent 暴露一个 MCP 工具：

| 工具 | 职责 |
|---|---|
| `memory` | 读取当前绑定 Project 的 Effective Memory，或创建由该 Project 携带的 Memory 提案 Draft |

| operation | 何时使用 | 结果 |
| --- | --- | --- |
| `activate` | 开始实质任务，还不知道哪些资源相关 | 排序后的片段，带资源 ID 和路径 |
| `load` | 需要完整资源，或者即将修改它 | 完整资源、稳定 ID 和完整资源哈希 |
| `store` | 用户明确要求创建、更新、重命名、删除或丢弃受管 Memory | 持久化的本地 Draft 操作及同步状态 |

`clumsiesd mcp serve` 是短生命周期的 stdio 协议代理。Effective Memory 构建、索引、检索、Draft 持久化都由常驻 `clumsiesd` 管理，代理通过本地 XPC 调用它。代理只接受本文列出的强类型输入，不能把任意 JSON 转发给 daemon。

启动时，代理会校验常驻 daemon 的 Agent runtime 协议修订和构建标识，再根据当前目录解析 Project binding。版本不一致、daemon 不可用或严格 host-plugin 模式下无法解析 binding 时都会显式失败。当前协议没有 setup 调用，也不兼容已移除的 `retrieve` 工具、host-session binding、`META_PROMPT.md` bootstrap 或 MCP attestation。

下文示例展示工具的**参数**或返回的领域对象，省略外层 JSON-RPC 包装。成功时，领域对象同时出现在 `structuredContent` 和 `content` 的序列化文本中，`isError` 为 false；工具失败时为 true。协议报文本身不合法时，也可能直接返回 JSON-RPC 错误。

## 通用输入规则

`memory` 使用 tagged operation 结构：

```json
{
  "op": {
    "operation_name": {}
  }
}
```

- `op` 必填，并且必须且只能包含一个 operation。
- 输入不能包含 `null`；可选字段应直接省略。
- operation 和字段名区分大小写，未知字段会被拒绝。
- `knownHashes` 使用 camelCase，`expected_hash` 使用 snake_case，不能自行统一改名。
- MCP 进程的 Project binding 决定 `memory` 的 Effective Memory；调用方不能在参数中改写 `project_id`。

## `memory`

`memory` 包含 `activate`、`load` 和 `store` 三个 operation。

### 记忆维护规范（Memory Guidelines）

Project 可以约定一份受管 Memory 指南，默认路径为 `CLUMSIES.md`；受管集成也可以提供其他路径。这里的路径位于 Effective Memory 内，不是让工具打开操作系统中的任意文件。它通常定义：

- 目录与命名方式；
- 何时允许提出 Memory 变更；
- description、替代与弃用规则；
- 不应持久化的内容。

维护 Memory 前，通过 `load` 按 MCP 提示的确切路径读取完整规范。同一任务中仍有效、仍在上下文里的规范可以复用；普通只读任务无需全文加载。用户在规范中维护的约定适用于相应内容，App 内置模板只提供初始正文。规范本身仍是普通 Memory，不增加写入授权，也不安装为宿主 Skill。

若规范返回 `memory_resource_not_found`，说明缺失路径。这只表示当前项目视图中没有该文档，不代表整个组织没有。可以继续检索，并依据用户指令和现有约定完成要求明确、已获授权的修改；只有依赖缺失规范的决定才需要澄清。不要自动创建规范、替换为本地文件，或从缺失的自定义路径回退到默认路径。

用户希望设置规范时，可在 App 的空项目 Memory 页面选择 **Set Up Guidelines** 或 **Use Team Guidelines**。后者选择组织已有资源，需要项目管理员权限。采用规范是可选操作；内置模板只有在用户采用后才成为 Memory。概念、预览和研究出处见[记忆维护规范](/zh/guides/memory-guidelines)。

### `activate`

Clumsies 与宿主原生记忆并存。Agent 遵循适用的宿主记忆政策，也在实质项目任务中查询 Clumsies，即使已经查询过宿主记忆。在一个记忆空间完成读写，不代表另一个也已完成；维护 Clumsies 记忆时遵循绑定项目的记忆维护规范。用户明确要求跳过 Clumsies 或仅使用其他来源、保存位置时，尊重该选择。

每个实质任务开始时调用一次 `activate`，让 daemon 从当前 Effective Memory 中返回最相关的片段：

```json
{
  "op": {
    "activate": {
      "query": "校正 MCP 工具契约和并发修订语义"
    }
  }
}
```

| 字段 | 必填 | 语义 |
|---|---:|---|
| `query` | 是 | 非空的自然语言任务或检索线索 |
| `state` | 否 | 上一次响应的 `next_state`；仅在上一次片段仍完整保留于模型上下文时传入 |

daemon 在一次调用内完成 BM25、向量召回、RRF 融合、Cross-Encoder 重排、资源多样性限制、token 预算和片段增量计算。模型名、候选数量和排序参数由 daemon 管理，不是 Agent 输入。调用方描述任务即可；排序和诊断细节见[检索评测](/zh/retrieval-evaluation)。

响应的主要结构为：

```json
{
  "index_revision": "search_...",
  "profile": "agent_activation.v2",
  "next_state": "opaque-state",
  "fragments": [
    {
      "action": "add",
      "unit_key": "mem_123/mcp/0/0",
      "content_hash": "sha256:...",
      "resource_id": "mem_123",
      "scope": "org",
      "kind": "memory",
      "path": "architecture/mcp.md",
      "heading_path": ["MCP", "并发控制"],
      "content": "..."
    }
  ],
  "removed": []
}
```

响应还可能包含用于本地诊断的 `run_id`。`add` 与 `replace` 携带正文；`reuse` 表示调用方上下文里已有同一片段，因此省略正文；`removed` 只撤销已删除、失去权限或重新解析后消失的单元，不会因为本次 query 不相关就撤销旧片段。

上下文压缩、旧工具输出已丢弃或开始新任务时必须省略 `state`。无效状态返回 `invalid_activation_state`，不会静默按空状态处理。固定检索模型尚未就绪时返回 `search_model_preparing` 和下载进度，不会退化为另一套检索算法。

### `load`

`load` 按稳定资源 ID 或精确路径加载完整资源，不做模糊检索和重排：

```json
{
  "op": {
    "load": {
      "ids": ["mem_123", "CLUMSIES.md"],
      "knownHashes": {
        "mem_123": "sha256:..."
      }
    }
  }
}
```

| 字段 | 必填 | 语义 |
|---|---:|---|
| `ids` | 是 | 非空、无重复的 ID 或精确路径数组；每项都必须是非空字符串 |
| `knownHashes` | 否 | 以请求 ID/路径为 key 的已知完整资源哈希 |

当 `knownHashes` 与当前资源一致时，结果返回 `changed = false` 并省略 `content`。任一请求目标不存在时返回 `memory_resource_not_found`，不会静默忽略。`load` 与 `activate` 读取同一份 Effective Memory，包括当前 Project 的 Draft overlay。

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

修改时使用返回的 `resource_id` 和 `content_hash`。activation 中的片段哈希对应片段，不是 `store.update` 要求的完整资源哈希。

### `store`

只有用户明确要求维护 Memory 时才调用 `store`。

每次 `store` 只能提供一个变更 operation：

| operation | 必填字段 | 可选字段 |
|---|---|---|
| `create` | `path`、`body` | `description` |
| `update` | `id`、`expected_hash`、`replacements` | `description` |
| `rename` | `id`、`new_path` | `description` |
| `delete` | `id` | `description` |
| `discard` | `id` | 无 |

`store` 本身还可带 `resource`，与 `create` / `update` 并列，不放在它们内部。唯一允许值是 `memory`，省略时默认使用该值。description 是可选的语义说明和检索字段；其发布时的 Server 持久化仍有已知缺口，参见[数据模型的实现说明](/zh/data-model)。资源 ID 可能带 `mem_` 或历史 `ctx_` / `rul_` / `wfl_` 前缀，应原样保存，不改写旧 ID。

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

Update 不接受完整的新正文。先 `load` 资源，再把返回的完整资源 `content_hash` 作为 `expected_hash`，提交一个或多个精确替换：

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

每个 `old_text` 必须在当前完整资源中恰好出现一次；同一请求内的替换不能重叠，并基于同一份原文原子应用。哈希过期、匹配缺失、匹配不唯一或区间重叠都会拒绝整个 update，不创建部分 Draft operation。`new_text` 可以为空字符串，用于删除文本。

路径属于受管 Memory 命名空间，不是任意本地文件路径。create 的 `body` 是完整 Markdown 正文；资源表达的是规则、流程还是背景知识，由内容和路径说明，wire type 统一为 `memory`。Metaprompt 和 `mpf` 不是合法类型。

`delete` 的目标如果只是尚未发布的 Create Draft，daemon 会把它归一化为 `discard`；只有删除已发布资源时才会保留待 Review 的 Delete Draft。`discard` 取消 Draft，不发布删除。

成功结果包含本地 operation ID、Draft ID、队列状态和同步状态。它只表示变更已在本机持久化并排队同步：

- Draft 由当前绑定 Project 携带；
- 新 Draft 默认归 Project 所有；修改 Org 引用会创建独立项目适配并记录来源版本；
- merge 前只影响该 Project 的 Effective Memory；
- 项目 PR 由项目 owner/admin 处理；修改 Org 原文需要另一个由 Org owner/admin 处理的 PR；
- `store` 不能审批 Review、merge 或前移任何已发布 Ref。

例如，已排队的本地写入可以返回：

```json
{
  "local_operation_id": "lop_example",
  "draft_id": "drf_example",
  "queued": true,
  "sync_status": "queued"
}
```

`sync_status` 可能为 `queued`、`syncing`、`retrying`、`synced` 或 `failed`。即使是 `synced`，也只表示 Draft 到达 Server，不表示已获批或发布。

## 错误与恢复

| 错误或状态 | 应如何处理 |
| --- | --- |
| `search_model_preparing` | 等待后台模型准备；响应提供进度 |
| `invalid_activation_state` | 丢弃失效 state，用当前上下文重新 activate；不要假定缺失片段仍在上下文中 |
| `memory_resource_not_found` | 检查精确 ID/路径和当前 Project；load 不会静默跳过缺失目标 |
| `memory_content_changed` | 重新加载完整资源，重新判断要修改的内容 |
| `text_replacement_not_found` / `text_replacement_ambiguous` / `text_replacement_overlap` | 根据新加载的正文修正精确替换范围；整个 update 已被原子拒绝 |
| `agent_runtime_mismatch` | 让受管代理与常驻 daemon 使用同一构建；可能需要重启仍使用旧集成的 Agent host |

update 失败不意味着可以改用完整正文覆盖。activate 失败也不意味着没有相关 Memory。应把它视为操作失败，再按明确错误恢复。

## 私有 daemon 边界

MCP operation 会映射到 daemon 的 `activate_memory`、`load_memory` 与 `store_draft_operation`。Desktop 还使用 Review、merge 和检索诊断等私有 XPC 方法；它们不是额外 MCP 工具。

每次有效 `activate` 会基于同一候选轨迹写入一条本地 Retrieval Run，但不会改变 MCP 响应 schema。Retrieval Run、Evaluation Case 和评测导出属于 daemon/Desktop 诊断能力，参见[检索运行与评测](/zh/retrieval-evaluation)，不会发送给 Server。

## 实现入口

输入校验和对外工具 schema 位于 [mcp_contract.rs](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/agent_runtime/mcp_contract.rs)。[mcp.rs](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/agent_runtime/mcp.rs)负责 stdio/JSON-RPC 和结果包装；[检索响应类型](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/mod.rs)和 [daemon 操作类型](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/types.rs)定义返回数据。
