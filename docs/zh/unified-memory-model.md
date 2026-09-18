# 统一 Memory 设计

本页解释数据模型背后的约束与取舍，适合已经读过[核心数据模型](/zh/data-model)、准备实现或评审功能的人。这里描述当前代码行为；历史迁移见 [Project 权威切换](/zh/project-authority-migration)。

## 为什么只有一种 Memory

部署回滚检查单、编码约束、架构说明，都保存为具有稳定 ID 的 Markdown Memory。当前协议不会因为文件放在 `rules/` 或 `workflow/` 下就赋予它新的类型、权限或执行能力。

这样做把两种变化分开：团队可以调整知识的组织方式，不必同时改 Server、daemon、MCP 和每个客户端的枚举。系统仍负责身份、版本、权限和发布流程。检索负责判断内容与当前任务是否相关，不能代替批准，也不能把 Draft 提升成已发布内容。

当前 wire contract 的 Memory 内容只有正文及可选摘要；没有 Category/Tag、`content_type`、`content_format`、`agent_instruction` 或 `invocable_skill` 字段。历史 `ctx_`、`rul_`、`wfl_` ID 保留，不能从 ID 前缀推断当前类型。

## 一个发布源，多个 Project 投影

这里的“发布源”指决定共享内容当前正式版本的地方。Organization Ref 指向当前 Organization Commit；一次 Review merge 更新这个版本。

Project Org Selection 只保存 Memory ID。Server 用它从 Organization 当前内容中生成 Project Commit，并更新 Project Ref。这种把已有数据按用途生成视图的做法称为“投影”。Project Ref 因而是同步单元，不是第二个发布入口。

为什么保留 Project Commit，而不是每次 Agent 请求都去筛选整个组织？因为 daemon 可以下载一个明确版本的 Project 快照，在本机安装、索引和读取，并追踪它来自哪个版本。选择变化和相关上游资源变化都会刷新投影。

两个边界不能混淆：

- 删除 Project 选择只改变这个 Project 的基线，不删除 Organization Memory。
- Draft 从 Project 提出，发布目标仍为 Organization。新建 Memory 合并后自动加入发起 Project 的选择；已有 Memory 的变更必须针对该 Project 已选择的资源。

## 本地视图与发布视图

```text
已安装的 Project 投影
  + 该 Project 的 open/submitted Draft 操作
  = Effective Memory
  → 匹配该内容与模型版本的 Index Revision
  → memory.activate / memory.load
```

`memory.store` 成功表示 daemon 已持久化提案并安排同步，不表示 Server 已收到、Review 已批准或 Organization Ref 已前移。同步成功也不等于发布成功。

overlay 保留资源的 Commit 或 Draft 来源。搜索索引只是这份视图的派生数据；索引落后时必须显式处理就绪状态，不能把错误版本的检索结果当成当前内容。

本机可能尚未下载最新 Project Commit。因此“当前本地有效”与“Server 当前已发布”仍可能有时间差。排查读取问题时，应同时查看安装的 Ref、Draft 状态和索引版本。

## 三种独立的 Draft 状态

| 维度 | 值 | 判断什么 |
|---|---|---|
| 生命周期 `status` | `open`、`submitted`、`merged`、`discarded` | 提案进行到了哪一步 |
| 新鲜度 `freshness` | `current`、`behind` | Base Commit 是否等于当前上游 Commit |
| 协调结果 `reconciliation` | `unknown`、`clean`、`conflicts` | 有没有可用比较结果，能否无冲突组合 |

例如一个 Draft 可以同时为 `submitted + behind + clean`：已经提交，上游有新发布，但修改可以协调。把 behind 当成“失败”或把 conflicts 当成终态，都会遗漏后续可恢复流程。

Server 根据 Base、Current、Draft Result 生成候选，候选绑定 Draft version 与当前 Ref。生成候选不修改 Draft。rebase 才会保存旧 Draft revision、推进 Base、以新基线重新表达操作；它也不发布。

提交一个或多个 Draft 时，每个 behind Draft 可以携带自己的确认候选。Server 在 Review 创建或重新提交的事务内应用它们。缺少候选时会返回需要协调的信息；候选已经过期时必须重新读取和比较。

## Review 的事务与批准语义

Review 保存有序、去重的 Draft 集合，至少一项。所有 Draft 必须属于同一 Project、同一发布 scope，并由提交者拥有。Draft 内操作顺序和 Review 内 Draft 顺序都属于数据语义，不能依赖 UUID 排序或异步请求返回顺序。

发布事务主要做五件事：

1. 锁定协调所需数据、Review 和 Draft，检查 Review version、Draft 状态及当前 Organization Ref。
2. 确保各 Draft 的 Base 已与当前 Ref 对齐；落后的提案需要先协调。
3. 对已批准 Review 验证完整结果哈希，防止批准旧内容后发布新内容。
4. 按顺序物化并应用修改，创建一个 Organization Commit，推进一次 Organization Ref。
5. 刷新受影响 Project 投影，记录 merge，并把 Review 和 Draft 标记为 merged。

失败时，发布修改整体回滚。为了让客户端继续协调，某些失败路径会单独保留生成的 reconciliation candidate；这不代表发布了部分 Memory。

owner/admin 可以先 approve 再 merge，也可以直接 merge open Review。直接合并同样经过授权和完整校验，并记录决定人。批准绑定的是结果：rebase 保持完整结果不变时，可以保留批准；结果变化时旧批准失效。时间戳、标题或“之前批准过”都不能代替结果校验。

## 快照读取的成本边界

Commit payload 包含完整 Tree 和 Blob 正文，当前 commit-state 返回 `incremental_supported: false`。一个 Review 的多个文件可能引用同一个 Base/Current Commit；它们共享快照，不各自拥有一份独立的远端文件版本。

实现客户端时应先按唯一 Commit ID 组织一次加载，再把内容映射到各文件。文件数、唯一快照数、响应大小和页面就绪时间是不同指标。一个 HTTP 请求成功，只能说明该请求完成，不能证明整个 Review 页面已经就绪。

<span id="当前实现缺口"></span>

## 当前实现边界

以下是核对当前代码后仍存在的限制，不应把设计意图写成已完成能力：

| 边界 | 当前行为 | 对使用者的影响 |
|---|---|---|
| TreeEntry 的 OpenAPI 声明落后 | Rust/数据库为 `memory`、`project_org_selection`，运行时可返回 `description`；公共 OpenAPI 仍列旧三分类且遗漏摘要 | 构建集成时核对真实 DTO，不能仅根据旧生成类型推断字段 |
| Memory 摘要写入不完整 | Draft 可携带 description，`resources.description` 非 null；merge create/update SQL 尚未写入该字段 | 已发布摘要可能为空或保留旧值，不能保证端到端摘要检索 |
| macOS 旧分类仍有残留 | `MemoryKind` 仍参与部分 UI、路径和展示逻辑 | UI 标签不是 Server 领域类型或 Agent 执行能力 |
| 历史 Project Memory 读取路由 | `/projects/{project_id}/memories` 查询历史 project-scoped resources | 它不返回 Selection + Draft 的 Effective Memory，不能据此构建当前 Project 内容视图 |

这些边界应随对应代码修复一起更新。历史兼容读取并不授权创建新的 Project 发布源，也不恢复已退役的 Rule/Workflow/Context 写入协议。

## 评审实现时检查什么

- Memory ID 是否在重命名、投影和迁移中保持稳定；路径冲突是否在同一命名空间内检查。
- 是否保留 Organization 发布、Project 选择、本地 Draft 三层来源；是否把本地 store 成功误报成已发布。
- 是否使用操作本身要求的版本和 Ref，而不是混用 resource revision、Draft version、Review version。
- 多 Draft 发布是否全成全败，是否检查每个 Draft，而不是只检查首项。
- 派生索引是否与当前 Effective Memory、解析器和模型版本一致。

实现依据：[Memory DTO](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/api.rs)、[Review/Draft DTO](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/api.rs)、[发布事务与 reconciliation](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/postgres.rs)、[快照生成与资源写入](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/postgres.rs)、[commit-state](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/service.rs)、[索引实现](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/index.rs)、[公共 OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml)、[macOS MemoryKind](https://github.com/lilhammerfun/clumsies/blob/main/apps/macos/Sources/Libraries/Models/MemoryModels.swift)。
