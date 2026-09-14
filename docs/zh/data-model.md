# 核心数据模型

Clumsies 保存的是团队知识及其变更历史。理解数据模型时，先区分三件事：**已经发布的内容、准备发布的修改、某个 Project 当前实际读取的内容**。它们互相关联，但不是同一份状态。

本页通过一份《部署回滚检查单》说明核心对象、字段和存储关系。第一次阅读，依次看完“先认识六个对象”“从一份清单走完生命周期”和“版本与并发”即可；需要定位代码时再查后面的表映射。系统部署与调用关系见[整体架构](/zh/architecture)，请求格式见[接口参考](/zh/reference/)。

## 先认识六个对象

| 对象 | 回答的问题 | 例子 |
|---|---|---|
| Organization | 这份共享知识由哪个组织发布？ | Acme 团队 |
| Project | 哪个项目使用哪些知识，修改从哪里提出？ | Payments 项目 |
| Memory | 具体保存哪份知识？ | `operations/deployment-rollback.md` |
| Draft | 准备怎么修改这份知识？ | 为清单增加回滚步骤 |
| Review | 哪些修改一起交给人确认并发布？ | 包含清单和告警说明两个 Draft |
| Commit / Ref | 某次发布的完整内容是什么，当前使用哪一版？ | 不可变快照 C2、指向 C2 的当前指针 |

Organization 是唯一的 Memory 发布源。Project 选择 Organization 中的 Memory，并持有选择结果的版本快照。选择不复制 Memory 的身份；两个 Project 可以选中同一个 `memory_id`。

```text
Organization
  ├─ 已发布 Memory ────────────────┐
  └─ Organization Ref → Commit    │
                                  ↓ 按 Memory ID 选择
Project → Org Selection → Project Ref → Project Commit
  └─ Draft → Review → merge ──────→ Organization 的新版本

本机当前可读内容 = 已安装的 Project Commit + 当前 Project 的 Draft 修改
```

最后一行叫 **Effective Memory**：daemon 在本机合成的有效内容视图。“有效”表示当前项目会读到它，并不表示其中所有内容都已经批准发布。

## 从一份清单走完生命周期

以下 ID 和版本均为教学示例。`C1`、`P1` 是便于阅读的快照代号，实际 Commit ID 是内容寻址生成的字符串。

1. **发布基线。** Organization 已有 Memory `mem_example_rollback`，路径为 `operations/deployment-rollback.md`，正文是部署回滚检查单。Organization Ref 指向包含它的 Commit `C1`。
2. **Project 选择。** Payments 把 `mem_example_rollback` 加入 Org Selection。Server 生成 Project Commit `P1`，其中 Tree entry 的 `id` 仍是 `mem_example_rollback`，`source` 为 `selected_org`。Project Ref 指向 `P1`。
3. **本机安装。** daemon 下载 `P1` 并保存快照。Agent 通过 `memory.activate` 找到相关片段，通过 `memory.load` 读取正文。
4. **提出修改。** Agent 使用 `memory.store` 加入“发布前确认回滚方案”。daemon 先保存 Draft 和操作，再同步到 Server。这个 Draft 的 `project_id` 是 Payments，发布目标 `resource.scope` 是 `org`，`base_commit_id` 是 Organization 的 `C1`。Project 快照 `P1` 与 Draft 基线 `C1` 是两个不同的 Commit。
5. **提前在当前项目使用。** Draft 处于 `open` 或 `submitted` 时，修改会叠加到 Payments 的本地 Effective Memory。别的 Project 不会因此读到这份未发布提案。
6. **提交 Review。** 作者把这个 Draft，或一组有顺序的 Draft，提交到 Review。Server 校验作者、Project、Draft version、上游版本和修改目标。
7. **发布。** 有权限的 Organization owner/admin 合并 Review。Server 在一个事务中应用整组修改、生成 Organization Commit `C2`、推进 Organization Ref，并刷新受影响 Project 的投影。若有任何发布校验失败，这组内容不会部分发布。
8. **同步新版本。** daemon 安装新的 Project Commit。已合并 Draft 不再作为未发布修改叠加，Agent 读到新发布的正文。

**如果步骤 4 是新建 Memory：** 合并前使用 Draft 的临时身份；合并时 Server 分配新的 `mem_…` 资源 ID，并自动加入发起 Project 的选择集合。不要把新建 Draft 的 ID 当成最终 Memory ID。

**如果只是重命名：** `operations/deployment-rollback.md` 可以改成 `runbooks/deployment-rollback.md`，`mem_example_rollback` 保持不变。已有选择关系仍通过 ID 指向它。

## Memory：身份、路径与正文

领域中叫 Memory，数据库表名仍为 `resources`。HTTP 详情是 `{ memory, content, etag }`：元数据在 `memory` 内，正文 `content` 在外层。不要把数据库列名直接当成 JSON 字段。

| 字段 | 位置 / 类型 | 含义与空值规则 |
|---|---|---|
| `memory_id` / `resource_id` | HTTP / 数据库，string | 同一稳定资源身份。新资源使用 `mem_` 前缀；历史 ID 保留。 |
| `org_id` | 数据库，string | 所属 Organization；组织 HTTP 路由通过登录上下文确定它。 |
| `scope` | 两者，string | 当前发布 Memory 为 `org`。`project` 只保留历史读取和清理兼容。 |
| `project_id` | 两者，string 或 null | Organization 资源为 null；这不表示没有 Project 选中它。选择关系在独立表中。 |
| `path` | 两者，string | Organization 内的相对路径；活跃资源之间唯一，可重命名。不是 ID，也不保证等于本机文件位置。 |
| `name` | 两者，string | Server 从路径最后一段生成，例如 `deployment-rollback.md`。 |
| `description` | 两者，string | 语义摘要。数据库非 null，但允许空字符串；当前 merge 尚未可靠保留 Draft 的摘要，见[实现边界](/zh/unified-memory-model#当前实现边界)。 |
| `content` / `body` | HTTP 外层 / 数据库，string | Markdown 正文。当前没有独立 `content_format` 字段。 |
| `content_hash` | 两者，string | 正文的 `sha256:…` 哈希，用来检查内容是否变化；重命名而正文不变时可以不变。 |
| `revision` | 数据库，整数 | 此资源的修订号。HTTP `MemoryMeta` 不直接返回它，详情 `etag` 形如 `"rev-3"`。 |
| `status` | 两者，string | `active`、`deprecated`、`archived`。当前 delete 操作会归档资源；旧快照仍保留历史正文。 |
| `created_at` / `updated_at` | 数据库时间；HTTP 元数据返回 `updated_at` | 创建和最近修改时间；不能代替并发版本。 |

三个容易混淆的名称：`name` 来自路径；daemon 的展示 `title` 来自 Markdown 第一个标题，没有标题时从文件名派生；Draft `title` 是这次修改的说明。修改正文标题不会自动改变路径或资源身份。

## Selection 与 Bundle：都存 ID，职责不同

| | Project Org Selection | Bundle |
|---|---|---|
| 所有者 | 一个 Project | 一个用户 |
| 内容 | Organization Memory ID 集合 | Organization Memory ID 集合 |
| 用途 | 决定 Project 的发布内容基线 | 收藏和复用一组共享知识 |
| 是否影响 Project Ref | 更新选择会重新生成投影 | 不会 |
| 是否复制 Memory 正文 | 不会 | 不会 |

Selection 替换请求使用 `resource_ids: [...]`，读取结果则包含 `memories` 元数据和整个集合的 `revision`。空数组表示空选择，不能理解成“所有 Memory”。Bundle 与 Selection 独立：收藏一份 Memory 不等于把它启用到 Project。

## Draft：基线加有序修改

Draft 保存“基于哪个已发布版本、对哪个资源、做了什么”。它同时关联 Project 和 Organization：前者承载提案及本地视图，后者是发布目标。

| 字段 | 含义 |
|---|---|
| `draft_id` | Draft 身份。daemon 还可能记录对应的远端 Draft ID，用于同步本地提案。 |
| `project_id` | 承载提案的 Project，必填。 |
| `resource.scope` | 当前可发布提案必须为 `org`。 |
| `resource.id` / `resource.path` | 资源定位。已有 Memory 优先使用稳定 ID；新建时尚无正式 ID，需要路径。字段在 DTO 中可为 null，但动作仍有具体校验要求。 |
| `base_commit_id` | 修改的 Organization 快照基线；没有初始快照时可为 null。 |
| `operations` | 按明确顺序应用的 create / update / rename / delete。Server 以 `draft_operations.ordinal` 保存顺序。 |
| `version` | Draft 并发版本；写入携带预期版本，防止覆盖别人刚改过的提案。 |
| `status` | `open`、`submitted`、`merged`、`discarded`。 |
| `coordination` | 计算出的上游关系：是否落后、当前 Commit、有无资源变化和协调候选。不是另一个生命周期状态。 |

操作中，create/update 的 `content` 是 `{ content: "Markdown…", description?: "摘要" }`；rename 使用 `new_path`；delete 表示删除目标。Server 保存的 update 是正文结果，daemon 可以接受局部文本替换并转换成同步操作。一个 Draft 中先 create 再 update/rename，发布时会物化成最终的新资源。

`behind` 只表示 Base Commit 与当前 Organization Ref 不同。例如别人发布了另一份无关文档，当前清单的 Draft 也可能变成 behind，但不一定有内容冲突。Server 比较 **Base（旧基线）、Current（当前发布状态）、Draft Result（提案结果）**，形成 reconciliation candidate；确认后的 rebase 更新 Draft 基线，不发布内容。

## Review：一组修改的发布边界

一个 Review 至少包含一个 Draft。`review_drafts` 关联表保存 Draft 及其顺序，同一个 Draft 不能同时属于不同 Review。API 中 `draft_ids` 和详情 `drafts[]` 表达完整集合；保留的单数 `draft_id`、`draft`、`operations` 对应首个 Draft，不能据此漏掉其余文件。

创建请求中，每个 Draft 都带 `expected_draft_version`。落后的 Draft 还可带 `candidate_id`，有冲突时带完整 `resolved_state`；Server 可在创建或重新提交 Review 的事务内逐个应用确认候选。

Review 状态为 `open`、`approved`、`rejected`、`merged`。`version` 约束针对该 Review 的操作。`approved_result_hash` 绑定被批准的完整结果；如果结果改变，旧批准不能继续授权新内容。rebase 后结果不变时可以保留批准。有权限的发布者也可以直接合并 open Review，Server 同时记录发布决定和决定人。

评论关联 `review_version`，行评论的 `anchor_path` 与 `anchor_line` 必须同时存在；没有行定位时两者都为 null。这样评论能说明它针对哪一版、哪一处修改。

## Blob、Tree、Commit、Ref：为什么需要四层

它们借用了版本控制的思路，保存的是 Clumsies Memory 快照，**不是仓库的 Git commit**。

| 对象 | 保存什么 | 为什么单独存在 |
|---|---|---|
| Blob | 一段不可变正文；`blob_id`、`content` | 相同正文可被多个版本引用，正文改变才需要新 Blob。 |
| Tree | 一组 entry；每项连接 Memory ID、路径、来源、摘要和 Blob | 记录“这个版本有哪些资源，各自位于哪里”。同一正文换路径，Blob 可复用，Tree 会变化。 |
| Commit | `tree_id`、可空的 `parent_commit_id`、scope、version、时间 | 给完整快照加上历史关系和所属 Organization / Project。根 Commit 的 parent 为 null。 |
| Ref | 命名的当前指针，`commit_id` 可为 null | 回答“现在用哪一版”；尚未建立快照时可以没有目标。 |

当前 Server 对 Blob、Tree、Commit 使用带对象类型前缀参与计算的 SHA-256 内容寻址；这是 ID 生成规则，不是让客户端自行拼接 ID 的要求。Memory 的正文 `content_hash` 与 Blob ID 的算法输入不同，不能互换。

Project Tree 中，所选 Memory 使用 `type: memory`、`source: selected_org`；还有一项 `type: project_org_selection` 保存选择快照。这是系统配置 entry，不是供 Agent 阅读的 Memory。

`GET /api/v1/commits/{commit_id}` 返回 `commit`、`tree`、`blobs` 和可空的 `project_org_selection`，是**整个快照**，不是某一份文件的详情。多个文件共享同一 Commit 时，消费者应复用同次加载的快照，避免按文件反复下载。

## 版本与并发：不能混用的值

并发控制的基本做法是：客户端说明“我修改时看到的是哪个版本”，Server 只有在这个条件仍成立时才接受。它通常称为 compare-and-swap（CAS）。冲突时应重新读取并协调，而不是盲目重试旧请求。

| 值 | 保护什么 | 不能替代什么 |
|---|---|---|
| Memory `revision` / 详情 `etag` | 某个已发布资源的修订身份 | Draft version、Organization 当前 Ref |
| Selection `revision` | Project 的整个选择集合 | Memory revision |
| Draft `version` | 提案内容和生命周期的当前状态 | Base Commit |
| Review `version` | Review 的当前协调与决定状态 | 其中每个 Draft 的 version |
| Organization Ref 的 `commit_id` | 发布或协调所依据的上游快照 | Project Ref 的 `commit_id` |
| `content_hash` | 正文字节是否相同 | 路径、审批、权限或发布时间 |
| Effective Memory hash / Index Revision | 本机有效内容与检索索引是否匹配 | Server 发布版本或用户批准 |

精确的请求头、错误码和响应示例见[接口参考](/zh/reference/)。

## 数据实际存在哪里

### Server：PostgreSQL

这些表由 Server 事务维护。外部集成应使用接口，不应直接改表。

| 领域对象 | 实际表 | 关键关系 |
|---|---|---|
| 组织、用户、Project | `orgs`、`users`、`projects`、`project_members` | 一个组织多个 Project；成员关系键为 `(project_id, user_id)`。 |
| Memory 当前状态 | `resources` | `resource_id` 主键；active org 资源的 `(org_id, path)` 唯一。 |
| Project 选择 | `project_org_selection_states`、`project_org_resource_selections` | 前者保存集合 revision；后者以 `(project_id, resource_id)` 关联。 |
| Bundle | `personal_bundles`、`personal_bundle_items` | 用户所有；item 以资源 ID 关联，并有排序位置。 |
| Draft 与同步事件 | `drafts`、`draft_operations`、`draft_events` | operation 归属于 Draft；事件序号供增量同步。 |
| 上游协调历史 | `draft_reconciliation_candidates`、`draft_revisions`、`draft_rebases` | 候选绑定 Draft version 与 Base/Current；rebase 保留此前 revision。 |
| Review | `reviews`、`review_drafts`、`review_comments`、`review_merges` | 有序 Draft 集合、版本化评论、最终 merge 记录。 |
| 发布快照 | `blobs`、`trees`、`tree_entries`、`commits`、`refs` | Ref → Commit → Tree → entry → Blob。 |

### 本机 daemon：SQLite 与文件

本机数据要进一步区分“尚未同步的用户修改”与“能从已知输入重建的缓存”。Draft 不能按普通缓存随意清理。

| 状态 | 主要存储 | 性质 |
|---|---|---|
| 目录 → Project 绑定 | 中心 SQLite `project_bindings` | 本安装配置；键为 Server 地址与规范目录。 |
| 本地 Draft 与待同步操作 | `local_drafts`、`local_draft_operations` | 本地持久化提案；操作记录也承载同步状态，可能尚未到达 Server。 |
| 同步与 HTTP 读取缓存 | `remote_draft_events`、`sync_retries`、`server_response_cache` | 事件游标、重试与读取副本。HTTP 缓存不是发布源。 |
| 下载的不可变快照 | `cached_blobs`、`cached_trees`、`cached_commits`、`cached_refs`；安装 generation 文件 | Server 快照的本地副本。 |
| Effective Memory | 从已安装快照和 Draft 操作合成 | 派生视图，不存在一张远端 Effective Memory 权威表。 |
| 检索索引 | Project SQLite 的 `search_revisions`、`search_resources`、`search_units`、`search_units_fts`、`search_heads` 等 | 可重建；记录来源 Commit/Draft，必须匹配 Effective Memory 与模型版本。 |

Project Local Storage 只允许移动受管理的 generation 和检索数据。中心 Draft、同步队列、凭据等不会跟着移动，详见[本地运行时](/zh/runtime)。AgentRun 与检索评测各有自己的数据边界，见[术语表](/zh/glossary)，不参与 Memory 的发布快照。

## 继续阅读与实现依据

- [Organization Memory](/zh/artifact)：发布源、共享内容与 Bundle。
- [Project](/zh/workspace)：选择、目录绑定与本地视图。
- [统一 Memory 设计](/zh/unified-memory-model)：不变量、事务与当前实现边界。
- [Server 数据结构](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/api.rs)与[变更数据结构](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/api.rs)：实际 JSON 字段、可空值和枚举。
- [数据库迁移](https://github.com/lilhammerfun/clumsies/tree/main/crates/server/migrations)：需要依次阅读，初始 schema 包含后来移除的字段。
- [快照与资源持久化](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/postgres.rs)、[Review 事务](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/postgres.rs)、[Draft overlay](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/overlay.rs)、[索引 schema](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/index.rs)：各层行为的实现依据。
