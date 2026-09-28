# MCP 工具契约

Clumsies 只向 Coding Agent 暴露两个 MCP 工具：

| 工具 | 职责 |
|---|---|
| `memory` | 联合检索当前 Project 的 Semantic/Episodic Memory，读取完整 Semantic 资源或 Episode Evidence，或创建由该 Project 携带的 Memory 提案 Draft |
| `kanban` | 读取和维护当前 Project 的原生 Issue，并显式执行工作状态转换 |

`clumsiesd mcp serve` 是短生命周期的 stdio 协议代理。Effective Memory 构建、索引、检索、Draft 持久化、Issue 与 AgentRun 状态都由常驻 `clumsiesd` 管理，代理通过本地 XPC 调用它。代理只接受本文列出的强类型输入，不能把任意 JSON 转发给 daemon。

启动时，代理会校验常驻 daemon 的 Agent runtime 协议修订和构建标识，再根据当前目录解析 Project binding。版本不一致、daemon 不可用或严格 host-plugin 模式下无法解析 binding 时都会显式失败。当前协议没有 setup 调用，也不兼容已移除的 `retrieve` 工具、host-session binding、`META_PROMPT.md` bootstrap 或 MCP attestation。

## 通用输入规则

两个工具都使用相同的 tagged operation 结构：

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
- MCP 进程的 Project binding 决定 `memory` 的 Effective Memory 和 `kanban` 的 Project-local 操作范围；调用方不能在参数中改写 `project_id`。

## `memory`

`memory` 包含 `activate`、`load`、`evidence` 和 `store` 四个 operation。

### Project Memory 指南

Project 可以约定一份 Memory 指南，默认路径为 `CLUMSIES.md`；受管集成也可以提供其他路径。它通常定义：

- 目录与命名方式；
- 何时允许提出 Memory 变更；
- description、替代与弃用规则；
- 不应持久化的内容。

进行实质性 Memory 维护前，先通过 `load` 读取该指南。指南本身仍是普通 Memory，不是安装到 Agent 本地目录的特殊 skill。

### `activate`

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

daemon 把当前 Project 的 Effective Memory（Semantic）与当前 Episode summaries
（Episodic）装配为一个 `MemoryCorpus`，再在一次调用内完成 BM25、向量召回、RRF
融合、Cross-Encoder 重排、资源多样性限制、token 预算和片段增量计算。模型名、候选
数量、排序参数及来源权重由 daemon 管理，不是 Agent 输入；普通查询不因 Episode 较新就
自动覆盖相关性，只有明确时间意图才使用 `activity_at`。

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
      "memory_system": "semantic",
      "authority": "organization",
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

Episodic fragment 使用 `memory_system: "episodic"`、`authority: "project"`，并额外
返回 `episode_id`、源 `run_id`、`activity_at` 和 `evidence_hash`。这些字段用于判断来源
和显式下钻，不会把 Episode 伪装成可编辑 Memory 或 Organization authority。

`add` 与 `replace` 携带正文；`reuse` 表示调用方上下文里已有同一片段，因此省略正文；`removed` 只撤销已删除、失去权限或重新解析后消失的单元，不会因为本次 query 不相关就撤销旧片段。

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

当 `knownHashes` 与当前资源一致时，结果返回 `changed = false` 并省略 `content`。任一请求
目标不存在时返回 `memory_resource_not_found`，不会静默忽略。`load` 只解析 Semantic
Effective Memory（包括当前 Project 的 Draft overlay）；Episodic summary 通过
`activate` 命中后，原始详情使用独立的 `evidence` operation。

### `evidence`

`evidence` 在命中 Episodic summary 后按稳定 Episode ID 分页读取原始证据：

```json
{
  "op": {
    "evidence": {
      "episode_id": "episode_0123456789abcdef0123456789abcdef",
      "cursor": "12:65536"
    }
  }
}
```

| 字段 | 必填 | 语义 |
|---|---:|---|
| `episode_id` | 是 | `episode_` + 32 位小写十六进制稳定 ID |
| `cursor` | 否 | 上一页返回的 opaque cursor，最长 512 bytes |

调用方不能提供 `project_id`、workspace path 或宿主日志路径。MCP proxy 从当前 binding
注入 Project，daemon 绕过通用 Server response cache，Server 每次按当前 membership
重新鉴权并写入不含正文的审计记录。响应以条数和字节双重上限分页；巨型单条记录可以在
后续 cursor 继续，不会静默截断。

Evidence 响应始终包含 `untrusted: true` 和警告。正文是用于核验的历史数据，即使其中
出现“系统指令”或工具请求，也不能覆盖当前 Agent 指令或被自动执行。`activate` 只返回
摘要与 provenance；只有显式调用 `evidence` 才加载正文。

### `store`

只有用户明确要求维护 Memory 时才调用 `store`。Issue 必须由 `kanban` 管理，不能伪装成 Memory 文档。

每次 `store` 只能提供一个变更 operation：

| operation | 必填字段 | 可选字段 |
|---|---|---|
| `create` | `path`、`body` | `description` |
| `update` | `id`、`expected_hash`、`replacements` | `description` |
| `rename` | `id`、`new_path` | `description` |
| `delete` | `id` | `description` |
| `discard` | `id` | 无 |

`store` 本身还可带 `resource`，当前唯一允许值是 `memory`，省略时默认使用该值。description 在当前 MCP 契约中是可选字段；其 Server merge 持久化仍有已知缺口，参见[统一 Memory 数据模型](/unified-memory-model#当前实现缺口)。

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

`delete` 的目标如果只是尚未发布的 Create Draft，daemon 会把它归一化为 `discard`；只有删除已发布资源时才会保留待 Review 的 Delete Draft。`discard` 取消 Draft，不发布删除。

成功结果包含本地 operation ID、Draft ID、队列状态和同步状态。它只表示变更已在本机持久化并排队同步：

- Draft 由当前绑定 Project 携带；
- Draft 的权威目标是 Organization；
- merge 前只影响该 Project 的 Effective Memory；
- `store` 不能审批 Review、merge 或前移 Organization Ref。

## `kanban`

`kanban` 管理 Clumsies 原生 Issue，不等同于 GitHub Issues 等外部工单。Issue 记录语义状态；AgentRun 记录一次 root 或 subagent 执行。生命周期 hook 只观察 AgentRun，不会根据 Start、Stop 或失败事件自动推进 Issue。

### revision 的属主

`expected_revision` 不是一个全局通用版本号，必须按 operation 选择来源：

| operation | `expected_revision` 属主 | 正确来源 |
|---|---|---|
| `update` | Issue | `list` / `get` 返回的 Issue `revision`，或上一次内容 mutation 返回的 `revision` |
| `begin_work` | AgentRun | lifecycle hook 注入的当前 run revision |
| `request_closure` | AgentRun | `begin_work` 或后续 run-changing operation 返回的 `run.revision` |
| `unclaim` | Issue | `list` / `get` 返回的 Issue `revision`，或工作流 mutation 返回的 `state_revision` |

不得把 Issue revision 传给 `begin_work`/`request_closure`，也不得把 AgentRun revision 传给 `update`/`unclaim`。

### operation 一览

| operation | 必填字段 | 可选字段 | 语义 |
|---|---|---|---|
| `list` | 无 | 无 | 返回绑定 Project 的 Issue board 和近期未关联 AgentRun |
| `get` | `issue_id`、`issue_key` 二选一 | 无 | 按全局 ID 或 Project-local key 读取完整 Issue |
| `create` | `title`、`description` | `acceptance_criteria`、`verification_level`、`verification_steps`、`external_references`、`dependencies`、`blocking_facts` | 创建 Todo Issue |
| `update` | `issue_key`、Issue `expected_revision`，以及至少一个语义字段 | 其余可更新语义字段 | 更新内容，不改变 board state |
| `begin_work` | `issue_key`、Hook 发放的 `run_id`、AgentRun `expected_revision` | 无 | 将当前 run 绑定到 Issue 并进入 In Progress |
| `pause_issue` | `run_id`、`issue_key` | 无 | 持有该 Issue 的 run 将它暂停 |
| `resume_issue` | `run_id`、`issue_key` | `takeover` | 恢复 Paused Issue；其他 run 接管时必须显式传 `takeover: true` |
| `request_closure` | `run_id`、AgentRun `expected_revision` | `issue_key`、`summary` | root run 请求关闭；目标以 run 当前绑定的 Issue 为准，`issue_key` 可省略 |
| `unclaim` | `issue_key`、Issue `expected_revision`、`run_id` | 无 | 当前绑定 run 释放 In Progress/Paused Issue并将其放回 Todo |
| `export` | `issue_key` | 无 | 导出确定性的 Markdown 快照，不创建 Memory Draft |

`unclaim.run_id` 在 MCP 契约中必填。daemon 的 Desktop 人工释放接口允许省略它，但该能力不属于 Agent-facing MCP。

`request_closure.issue_key` 在 MCP 中可选。daemon 根据 `run_id` 的现有绑定确定目标 Issue；即使传入该字段，也不能用它改变目标。`request_closure` 仅允许 root AgentRun，subagent 不能请求关闭。

### 标识与字段约束

- `issue_id` 为 `issue_` 加 32 个小写十六进制字符，全局唯一。按 `issue_id` 调用 `get` 时，结果会返回其真实 `project_id`。
- `issue_key` 为 Project-local 的 `ISSUE-NNN`，必须恰好三位数字，`ISSUE-000` 无效。
- `run_id` 必须来自受信任的 host lifecycle hook。Agent 不能自行编造，也不能从 `list` 的“最近 run”推断当前身份。
- `verification_level` 为 `agent_self`、`human_required` 或 `mixed`；创建时省略则默认为 `agent_self`。
- `request_closure.summary` 最多 1,000 个 UTF-8 bytes。
- `external_references` 最多 16 项，每项包含 `kind = issue | pull_request` 和不含嵌入凭据的绝对 HTTP(S) `url`。
- `dependencies` 最多 16 个同 Project `ISSUE-NNN`。自依赖、重复、缺失目标和依赖环都会被拒绝；所有依赖 Done 前，Issue 为 blocked。
- `blocking_facts` 最多 16 项，包含稳定 `fact_id`、`kind = host_capability | external`、说明、可选 value 和 `satisfied`。未满足事实会使 Issue 为 blocked。

对 `external_references`、`dependencies`、`blocking_facts` 和其他数组字段，Create 省略表示空数组；Update 省略表示保留当前值，显式传 `[]` 表示清空。

### 工作流示例

先读取 board，避免重复创建或重复认领：

```json
{
  "op": {
    "list": {}
  }
}
```

开始工作时，使用 hook 注入的 `run_id` 与 AgentRun revision：

```json
{
  "op": {
    "begin_work": {
      "issue_key": "ISSUE-123",
      "run_id": "arun_123",
      "expected_revision": 1
    }
  }
}
```

完成验收判断后，root Agent 使用 `begin_work` 响应中的最新 `run.revision` 请求关闭。假设该值为 `2`，这里省略可选的 `issue_key`：

```json
{
  "op": {
    "request_closure": {
      "run_id": "arun_123",
      "expected_revision": 2,
      "summary": "验收标准已满足，构建与文档检查通过。"
    }
  }
}
```

`request_closure` 只把 Issue 移到 In Review。Agent 无权批准；用户在 Desktop 的 Approve gate 通过后，Issue 才成为 Done。需要人工验证的 `human_required` 或 `mixed` Issue 如果没有 `verification_steps`，请求关闭会被拒绝。

另一种分支是：Issue 仍处于 In Progress 或 Paused 时，当前 run 决定放弃认领。此时 `unclaim` 必须使用 Issue revision；工作流 mutation 在 `state_revision` 中返回它。假设 `begin_work` 返回的 `state_revision` 为 `7`：

```json
{
  "op": {
    "unclaim": {
      "issue_key": "ISSUE-123",
      "run_id": "arun_123",
      "expected_revision": 7
    }
  }
}
```

成功后，Issue 回到 Todo，run 与 Issue 的绑定解除。

### AgentRun 与 Issue 的边界

- hook 在 root 或 subagent start 成功后注入当前 `run_id` 与 revision；这些值只属于该次执行。
- 同一个 session 最多持有一个 In Progress Issue。开始另一个 Issue 前，先 pause、request closure 或 unclaim 当前 Issue。
- `begin_work` 重试同一绑定是幂等的；把已有 run 改绑到另一个 Issue 会冲突。
- Stop、StopFailure、SubagentStop 与 SessionEnd 是运行遥测，不自动把 Issue 移到 In Review、Done 或 Todo。
- 私有 hook bridge 本身不保存原始 hook JSON、prompt、transcript、tool payload 或 assistant
  message；独立的 Episode ingestor 只会在已绑定 root AgentRun 结束后，从宿主权威日志按
  Run 边界建立受治理 Evidence。
- board 中的 blocked、blocking reasons 和失联 run 状态是 daemon 投影；它们帮助 Agent 判断是否可开始工作，但不代替显式语义转换。

## 私有 daemon 边界

MCP operation 会映射到 daemon 的 `activate_memory`、`load_memory`、
`get_episode_evidence`、`store_draft_operation` 以及 Issue 查询/转换方法。Desktop 还使用
审批、人工释放、归档、删除、Episode policy/预览/重建、检索诊断等私有 XPC/Server
能力；它们不是额外 MCP 工具。

每次有效 `activate` 会基于同一候选轨迹写入一条本地 Retrieval Run，但不会改变 MCP 响应 schema。Retrieval Run、Evaluation Case 和评测导出属于 daemon/Desktop 诊断能力，参见[检索运行与评测](/retrieval-evaluation)，不会发送给 Server。
