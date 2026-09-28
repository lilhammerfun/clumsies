# Server 热路径与 Rust 架构重构

> 本文保留 2026-08-29 性能优化时的结构与技术债记录。后续资源目录重构已移除全局 `ServerRepository`、API 重导出和混合 `shared.rs`；当前结构及测试入口见 [Server 源码说明](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/README.md)。下文的旧文件树和当时的债务分析应结合这一变更阅读。


| 文档属性 | 取值 |
|---|---|
| 文档角色 | 深度实践案例 |
| 核心问题 | N+1、过度 hydration、读模型与 Rust 边界 |
| 关联专题 | [端到端延迟优化专题](./index.md) |
| 证据台账 | [验证证据台账](./evidence-ledger.md) |
| 证据截止 | 2026-08-26 |

> 对应实现：[PR #203 — Fix systemic startup and sync latency](https://github.com/lilhammerfun/clumsies/pull/203)
> 本文只讨论 Server：热路径的数据形状、PostgreSQL 访问方式、Rust 代码边界、验证证据和遗留债。App first-ready、daemon 同步与传输压缩分别属于其他专题。性能数字与测试计数的证据截止为 2026-08-26；热路径、模块边界和遗留债已于 2026-08-29 对当前源码交叉核对。

## 1. 结论先行：慢的不是“查不到”，而是“查了根本不需要的数据”

这次 Server 优化最重要的发现，不是某个索引缺失，也不是香港链路不稳定，而是**列表接口与实现所做的工作不匹配**。

`GET /api/v1/reviews` 返回的是 Review 列表元数据：Review 自身字段、关联 Draft ID，以及用于界面判断 current/behind/conflicts 的 coordination 摘要。旧实现却把它当成详情页来加载：先取 Review，再逐个取关联 Draft，继续加载 Draft operations、ref 状态、base/current resource state，部分路径还会走到完整 tree/blob。大量中间结果只用于计算几个摘要字段，最终不会进入列表响应。

同类问题也出现在 commit 接口：commit list 和 commit-state 只需要 commit metadata，旧实现却为每个 commit 完整物化 tree entries 和 blob body。

生产证据很直接：在相同的 `/api/v1/reviews` route、相同的 62,632 B 响应体、全部返回 200 的条件下，Server 重构前 upstream p95 为 2,497.464 ms，重构后、启用 gzip 之前为 37.076 ms。这里改变的是 Server 内部读取的数据形状，不是响应契约，也不是网络压缩。

因此，本次优化的核心不是“让同一批 SQL 跑快一点”，而是：

1. 列表只读取列表需要的 projection；
2. 用集合查询替代应用层 N+1；
3. metadata 与 payload 使用不同读取路径；
4. 用真实 PostgreSQL 和锁行为测试，证明列表不再依赖 blob/tree payload；
5. 再把代码按 bounded context 和职责拆开，让这种边界能够被看见、测试和继续收紧。

## 2. 旧 Reviews 路径：一次列表请求如何变成乘法级数据库工作

### 2.1 第一层浪费：取到列表行后，只留下 ID

旧 `list_reviews` 首先查询最多 200 条 Review。查询已经选出了 title、description、status、version、author 等列表所需字段，但实现随后只从这些 row 中提取 `review_id`，丢弃其余字段，再逐个调用 `load_review`：

```text
SELECT review list rows LIMIT 200
  -> 提取 review_id，丢弃已取回的列表字段
  -> for review_id in review_ids
       -> load_review(review_id)
```

这意味着第一条列表 SQL 不是完整的 projection，只是下一轮 N+1 的 ID 生成器。

### 2.2 第二层浪费：每个 Review 再逐个加载 Draft detail

旧 `load_review` 对每个 Review 执行以下流程：

```text
load_review(review_id)
  -> 再查询一次 Review + author/decision user
  -> 查询 review_drafts，得到 draft_ids
  -> for draft_id in draft_ids
       -> load_draft_detail(draft_id)
            -> 查询 Draft metadata
            -> 查询全部 Draft operations
            -> 读取 current org/project ref
            -> 比较 base/current resource state
            -> 查询 reconciliation candidate
  -> 聚合多个 DraftCoordination
```

这里不是简单的 `1 + N`。设列表中有 `R` 个 Review，每个 Review 平均关联 `D` 个 Draft，则 Review 重查和 draft-id 查询随 `R` 增长，Draft detail 及其 coordination 查询随 `R × D` 增长；当 Draft behind 时，resource-state 比较还会继续访问 commit tree。具体查询数会因 current/behind、org/project scope 等分支变化，但增长方向已经由代码结构决定。

更关键的是，列表最后只保留每个 Draft 的 coordination 摘要。Draft operations、资源正文和大部分 detail 数据都没有进入 `ReviewListResponse`。

### 2.3 commit list/state 的 full hydration

旧 commit list 先取最多 50 个 `commit_id`，随后对每个 ID 调用 `get_commit_payload`。一个 payload load 不仅查询 commit metadata，还会执行：

```sql
SELECT e.item_id, e.resource_kind, e.scope, e.project_id, e.path, e.blob_id,
       e.source, e.description, b.content
FROM tree_entries e
JOIN blobs b ON b.blob_id = e.blob_id
WHERE e.tree_id = $1
ORDER BY e.resource_kind, e.path NULLS LAST, e.item_id
```

也就是说，一个“列出 50 条 commit metadata”的接口，最坏会重复进行 50 次完整 tree/blob 物化。commit-state 也有同样问题：为了返回最新 commit 的 ID、版本、parent 和时间，它加载了完整 payload。

完整 payload 本身不是错误。`GET /api/v1/commits/{commit_id}` 的职责就是下载 commit 内容，那里必须保留 tree 与 blob。错误在于**metadata 接口复用了 payload loader**。

### 2.4 62 KB 响应背后的 113 MB 内部放大

旧生产路径曾观测到约 113 MB 的内部数据读取和 23,207 条 `tree_entries` row，而客户端最终收到的 Reviews JSON 只有 62,632 B。两者不是同一个口径，不能直接当成精确压缩比；但它清楚揭示了三个数量级的工作放大：Server 为计算列表摘要，物化了不会出现在响应中的历史树和正文。

这也解释了为什么“香港服务器慢”会造成误判：客户端看到请求耗时约 2.5 秒，很容易先怀疑代理、TLS 或跨境 RTT；Caddy 的 upstream timing 却显示，这 2.5 秒几乎全部消耗在上游 Server 内部。

## 3. 新路径：先定义列表需要什么，再让 SQL 只返回这些东西

### 3.1 Reviews：列表 row 直接构造 Review

新 `list_reviews` 不再把首条 SQL 降级成 ID 查询。Review、author 和 decision user 的列表字段由这条查询一次取回，row 本身直接用于构造 `Review`。

随后实现收集当前页的全部 `review_id`，一次调用 `load_review_list_projections`：

```rust
let review_ids = rows
    .iter()
    .map(|row| row.try_get::<String, _>("review_id"))
    .collect::<Result<Vec<_>, _>>()?;

let mut projections = load_review_list_projections(tx, &review_ids).await?;
```

projection 查询使用集合条件：

```sql
WHERE rd.review_id = ANY($1)
ORDER BY rd.review_id, rd.ordinal
```

它只选择列表真正需要的字段：`review_id`、`draft_id`、base/current commit ID、candidate 状态，以及通过 tree-entry identity/path/blob-id 比较得到的 `has_upstream_resource_changes`。它不加载 Draft operations，也不 join `blobs.content`。

SQL 返回后，Rust 用 `BTreeMap` 按 `review_id` 聚合 draft IDs 和 coordination，再构造最终列表。数据库仍需对每个 Draft 判断 coordination，但它发生在一个集合语句中，不再由应用层发起逐 Review、逐 Draft 的往返。

新列表主路径因此收敛为近似固定次数的数据库交互：

```text
1 次：读取当前页 Review metadata
1 次：为这一页全部 review_id 批量读取 coordination projection
```

SQL 内部的 LATERAL 子查询仍有成本，但它不再携带无关正文，也把执行计划、join 顺序和缓存利用交还给 PostgreSQL，而不是用 Rust `for + await` 固化 N+1。

### 3.2 详情仍然是详情

优化没有把所有读取都改成“浅对象”。`load_review_with_drafts` 和 `load_review_detail` 仍会加载 Draft details、operations 与 comments，因为详情、审批和 merge 用例确实需要这些数据。

这条边界非常重要：

- list projection 是一个独立读模型；
- detail hydration 保留完整业务语义；
- 不能为了让列表变快而偷偷删掉详情接口需要的数据；
- 也不能因为详情已经有 loader，就强迫列表复用它。

### 3.3 Commit：metadata 与 payload 明确分路

新实现增加 `load_commit_metadata`，只读取：

```text
commit_id, scope, org_id, project_id, tree_id,
parent_commit_id, version, created_at
```

commit list 现在用一条 SQL 直接返回最多 50 个 metadata row；commit-state 通过 `load_commit_metadata` 构造 `latest`。只有显式 payload endpoint 才调用 `load_commit_payload` 并 join `tree_entries + blobs`。

这不是缓存技巧，而是接口语义在持久化层的直接表达：

```text
metadata endpoint -> metadata query
payload endpoint  -> metadata + tree/blob query
```

## 4. 为什么先改数据形状，而不是只补索引

索引解决的是“如何更快定位 row”，不能解决“本来就不该读取这些 row 和 body”。在旧路径上先补索引，最多让错误的数据访问方式稍快一些。

| 旧路径问题 | 只补索引为什么不够 | 本次修复 |
| --- | --- | --- |
| 列表逐 Review、逐 Draft 发 SQL | 单条查询可能更快，但数据库往返次数仍随 `R × D` 增长 | `ANY($1)` 批量 projection |
| commit metadata 接口完整读取 tree/blob | 索引不能消除 23,207 行结果和约 113 MB 正文物化 | metadata-only query |
| Draft operations/body 被加载后丢弃 | 索引无法减少反序列化、内存分配和 Rust 对象构造 | SELECT 只投影列表字段 |
| 列表依赖 blob/tree 表 | 即使命中索引，仍建立了不必要的表、锁和故障依赖 | list path 不访问 blob payload |
| 首条 Review 查询结果被丢弃后重查 | 索引只会加速两次重复工作 | 直接用首条 row 构造响应 |

正确顺序是：

1. 先确认接口契约需要哪些字段；
2. 删除不需要的 hydration 和应用层 N+1；
3. 用生产数据验证剩余 SQL；
4. 再对仍然昂贵的 filter、join、order 使用 `EXPLAIN (ANALYZE, BUFFERS)` 和索引。

“先改形状”不等于“索引不重要”。它只是避免用索引掩盖接口与查询语义不一致的问题。

## 5. 生产证据：同 route、同 body、gzip 之前，upstream 从秒级降到毫秒级

### 5.1 强生产 before/after

以下两组都来自生产 Caddy 结构化日志，route 均为 `/api/v1/reviews`，响应 body 均为 62,632 B，全部返回 200：

| 阶段 | 样本 | Caddy p50 / p95 / max | upstream p50 / p95 / max |
| --- | ---: | --- | --- |
| Server 重构前 | 12 | 2,328 / 2,497 / 2,497 ms | 2,328.658 / 2,497.464 / 2,497.464 ms |
| Server 重构后、gzip 前 | 41 | 25 / 37 / 40 ms | 25.331 / 37.076 / 40.014 ms |

p95 从约 2.50 秒降至 37 毫秒，约为原来的 1.5%，即约 67 倍改善。Caddy 总耗时与 upstream 耗时在两个窗口都几乎一致，说明主要变化发生在 Server，而不是 Caddy 本身。

这是一组**强生产 before/after**，不是随机分流、双版本并行或可逆 A/B。顺序部署无法形式化排除所有时间变量；但它具有很强的因果证据：

- route 相同；
- 响应体字节数相同；
- 全部成功，排除错误快速返回；
- Caddy 与 upstream 同时从约 2.5 秒降到几十毫秒；
- 新样本采于 gzip 之前，排除响应压缩贡献；
- 代码层已经移除了可解释该数量级差异的 N+1/full hydration；
- 锁行为回归测试进一步证明新列表不再依赖 blob payload。

因此可以把“Server 读取路径重构导致主要改善”视为高置信度结论，但不应把这组数据错误命名为随机 A/B 实验。

### 5.2 发布后的持续窗口

最终生产窗口包含 623 条结构化请求，全部返回 200：

| 范围 | 样本 | p50 | p95 | max |
| --- | ---: | ---: | ---: | ---: |
| 所有 route | 623 | 2.675 ms | 25.528 ms | 62.507 ms |
| Reviews | 50 | 25.528 ms | 44.296 ms | 62.507 ms |

这个窗口的意义不是制造一个更漂亮的最小值，而是确认部署后的 Reviews 延迟稳定停留在几十毫秒量级，没有回到秒级，也没有通过错误响应换取速度。

## 6. Rust 重构：让代码边界匹配业务和读取边界

数据形状变化解释了性能改善；目录重构本身不是“快 67 倍”的原因。但旧 Server 把 HTTP、业务编排、SQL、模型和跨领域协作集中在几个巨型文件里，定位一次列表究竟加载了什么非常困难。#203 同步进行了代码组织重构，使热路径能够被独立阅读和测试。

其中 `repository.rs` 从接近八千行的 God module 收缩为只保留共享 `PgPool` wrapper 和 `ServerError` 的薄文件；截至 2026-08-29 当前文件为 97 行。顶层 `api.rs` 从一千余行模型集合收敛为各 context API 的兼容重导出和 `PageInfo`。

### 6.1 bounded-context-first 的多领域单体

重构后的主要结构是：

```text
src/
├── main.rs
├── lib.rs
├── bootstrap.rs
├── app.rs
├── http.rs
├── auth/
│   ├── api.rs
│   ├── error.rs
│   ├── http.rs
│   ├── model.rs
│   ├── postgres.rs
│   └── service.rs
├── installation/
│   ├── api.rs
│   ├── error.rs
│   ├── http.rs
│   ├── model.rs
│   ├── postgres.rs
│   └── service.rs
├── organization/
│   ├── api.rs
│   ├── http.rs
│   ├── postgres.rs
│   └── service.rs
├── memory/
│   ├── api.rs
│   ├── http.rs
│   ├── postgres.rs
│   └── service.rs
├── changes/
│   ├── api.rs
│   ├── http.rs
│   ├── postgres.rs
│   └── service.rs
├── health.rs
├── middleware.rs
├── telemetry.rs
├── config.rs
├── db.rs
├── repository.rs
└── shared.rs
```

这符合团队《Rust 后端代码组织规范》的多领域单体分叉方式：`auth`、`installation`、`organization`、`memory`、`changes` 是当前层的 bounded context；每个 context 内再按稳定技术职责分解。

它没有强迫每个目录机械拥有完全对称的 `model.rs` 或 `error.rs`。只有 auth、installation 已形成独立模型和错误边界；其他 context 仍复用其 `api.rs` 类型或顶层错误。规范要求的是职责清楚，不是目录阵列整齐。

### 6.2 默认服务路径：`main -> run -> build_app`

进程入口保持薄，但不再“只有”服务启动：无参数时调用 `server::run()`，另有 `migrate-project-authority` 历史迁移命令分支。默认服务路径仍是：

```text
main（无参数） → server::run → build_app
```

迁移命令不进入 HTTP 服务启动路径。

`bootstrap::run` 负责进程级工作：

- 初始化 telemetry；
- 从环境加载 `ServerConfig`；
- 建立 `PgPool` 并执行 migrations；
- 构造 `InstallationService` 与 `AuthService`；
- 调用 `build_app`；
- 绑定 listener；
- 处理 Ctrl-C/SIGTERM 和四秒 drain deadline。

`build_app(pool, auth, installation)` 不读取环境、不连接数据库、不执行 migration、不绑定端口。它只把已构造的依赖交给 HTTP Router，并挂载全局 telemetry。生产 bootstrap 和测试因此可以复用同一应用构造路径。

这对应内部规范 R-05（薄入口）和 R-06（应用可独立构造、无需绑定真实端口即可测试）。

### 6.3 `http / service / model / postgres` 分别负责什么

| 职责 | 本项目中的边界 |
| --- | --- |
| `http` | 处理 Axum 的 `State`、`Extension<AuthPrincipal>`、`Path`、`Query`、`Json`，调用用例并映射为 JSON/HTTP error；不写 SQL。 |
| `service` | 表达 create/submit/rebase/merge 等用例，负责权限前置、步骤编排和事务 begin/commit；不依赖 Axum request/response。简单读取可以直接委托 postgres，不制造空业务层。 |
| `model` | 承载确有独立业务语义的概念、不变量和领域错误。不是每个 context 必须有同名文件；部分公开契约暂时位于 context `api.rs`。 |
| `postgres` | 保存 SQL、Row mapping、锁与数据库错误语义；接受 `PgPool` 或显式 `Transaction`。Reviews 的 `ANY($1)` projection、metadata/payload 分路都位于这里。 |

这种结构没有为每张表创建 repository trait。当前只有 PostgreSQL 一个真实实现，SQL 正确性通过真实 PostgreSQL 测试验证；提前制造 `ReviewRepository`、`DraftRepository`、`CommitRepository` trait 和 mock，只会把真实查询问题藏在假实现后面。这符合内部规范 R-08：具体实现优先，抽象服从已经出现的变化。

### 6.4 架构与性能之间的真实关系

需要避免两个极端结论：

- “拆文件让程序更快”是错的。Rust module 重新命名不会把 2.5 秒自动变成 37 毫秒；真正的性能因果是 projection、batch query 和 metadata/payload 分路。
- “架构重构与性能无关”也不准确。旧 God module 让列表、详情和 payload 共享 loader，才使过度 hydration 长期不明显；拆出 context 和 postgres leaf 后，读取契约可以独立命名、测试和审查。

架构重构的价值是降低再次写出错误热路径的概率，而不是代替性能证据。

## 7. 测试：不要用脆弱的毫秒阈值证明“没有读 payload”

### 7.1 真实 PostgreSQL，而不是 SQLite 或 repository mock

Server 集成测试通过 `testcontainers` 为测试启动真实 PostgreSQL，连接后执行生产 migrations。`TestPostgres` 持有 container 和 pool，资源随测试对象生命周期释放；全局 semaphore 将并发 PostgreSQL container 数限制为 4，避免本机和 CI 被无界启动压垮。

这类测试验证的是 PostgreSQL 的真实 SQL、约束、事务和锁语义。SQLite 或 mock repository 无法证明 `ANY($1)`、LATERAL、`FOR UPDATE`、PostgreSQL array binding 和实际隔离行为正确。

### 7.2 应用级端到端：Router + middleware + handler + PostgreSQL

黑盒测试通过 `tower::ServiceExt::oneshot` 调用完整 Axum Router，不绑定固定端口。认证流程使用协议级 Fake OIDC provider 隔离不可控外部身份服务，但 token、middleware、权限检查、handler、service、PostgreSQL 和响应反序列化都走生产路径。

例如授权边界测试从公开 HTTP 入口验证：成员可以读取共享 project，无法读取私有 project/draft，批量写入也不能绕过所有权。它不是直接调用一个纯 service 后就宣称 API 已验证。

### 7.3 热路径使用“依赖阻断”测试，而不是只断言 `elapsed < N ms`

纯时间断言受 CI 负载影响，既容易误报，也不能说明为什么变慢。#203 增加了两类更有解释力的回归：

1. 在事务中对 `blobs` 表持有 `ACCESS EXCLUSIVE` lock，同时调用 `list_drafts` 和 `list_reviews`。两个列表必须在三秒 timeout 内完成。若未来又复用 payload loader，请求会等待 blob lock，测试稳定失败。
2. 删除某个 commit 的 `tree_entries` 后，`get_project_commit_state` 和 `list_project_commits` 仍应返回 metadata；显式 `get_commit_payload` 则必须失败。这个测试同时证明浅读取与完整 payload 的语义边界没有混淆。

另一个测试锁住 ref mutation row，再读取 Draft，验证普通读取不会等待 ref 写锁。这里测试的是依赖图和锁边界，比“我的电脑上快了 20 ms”更适合作为长期回归门。

### 7.4 当前测试边界

`bootstrap` 已使用 paused Tokio time 验证 drain 在四秒内完成或超时退出；这能覆盖 shutdown 状态机，但不等价于真实进程接收 SIGTERM 时仍有在途请求的生产等价测试。后者应作为独立进程级验收保留，而不是把所有业务测试都升级为慢速真实 socket/进程测试。

## 8. 尚未完成的架构债

#203 把边界从“几乎不可见”推进到“可定位”，但没有把多领域单体变成编译器完全隔离的 contexts。下面三项必须如实保留。

### 8.1 `ServerRepository` 仍是跨 context 的共享门面

`repository.rs` 虽然只剩 `PgPool` wrapper 和 `ServerError`，但 `changes/service.rs`、`memory/service.rs`、`organization/service.rs` 仍分别通过 `impl ServerRepository` 把方法加到同一个类型上。

好处是迁移成本低，现有调用方和大量真实 PostgreSQL 测试不必一次性重写；它也避免了为每张表制造 trait。代价是：

- context 所有权没有体现在类型上；
- 拿到 `ServerRepository` 的代码理论上能调用所有公开用例；
- 方法面会继续增长，容易再次形成逻辑上的 God service；
- 事务和跨 context 调用的真实归属不够直观。

系统性修复不应是再套一层 `dyn Repository`。更稳妥的渐进规则是：停止向共享类型增加新的跨领域方法；新用例优先进入 context-owned concrete service；旧调用方按触达范围逐步迁移，最后再删除兼容门面。只有出现真实多实现或 crate 依赖隔离需求时才引入 trait。

### 8.2 `shared.rs` 已经混合多种变化原因

当前 `shared.rs` 同时包含：

- portable resource path 校验和 materialization collision 检查；
- ID/token 生成；
- secret/content/object hash；
- pagination；
- scope 字符串解析；
- path 到 name 的转换。

这些函数本身不一定有问题，但把它们放在同一个 `shared` 命名下，会形成内部规范明确警告的“共享垃圾桶”：新代码只要不知道放哪，就继续往这里加。

不需要立即把 161 行拆成十个目录。应在下次真实修改时按所有权迁移：resource path/materialization 归 memory 能力，身份/secret 归 auth 或具名 identity 能力，pagination 保留为明确的小型协议能力。只有两个以上 context 真正共享且语义稳定的函数，才留在顶层具名 module。

### 8.3 `changes -> memory` 仍直接依赖跨 context 持久化能力

`changes/postgres.rs` 目前直接从 `memory` 导入一组低层能力，包括 ref load/lock/advance、commit create/validate、resource operation apply、org selection projection 与 impact refresh。Review merge 的确需要原子地产生 memory commit 并推进 ref，因此业务耦合是真实的；问题在于依赖落在了另一个 context 的持久化细节，而不是窄的应用能力。

风险包括：

- memory 的表和 ref 实现变化会直接扩散到 changes；
- changes 逐渐承担 memory 的不变量和提交顺序；
- module visibility 很难表达“允许 merge 这个用例，禁止任意操纵 ref”；
- 跨 context 事务边界只能靠调用者知识维持。

未来应把这组调用收敛为一个或少数具名、transaction-aware 的 memory 应用能力，例如“在给定事务中应用已批准 operations 并推进目标 ref”。它必须保留显式 `&mut Transaction`，因为 Review merge 与 memory commit 需要原子性；不要用通用 event bus、复制 SQL 或 repository trait 把事务边界藏起来。

这项提取应在 merge 语义稳定、或下一次跨 context 变化再次触达这些调用时完成。当前先记录债，比为了目录纯度仓促增加抽象更诚实。

## 9. 以后审查 Server 读取路径时看什么

遇到新的列表或同步性能问题，先按下面顺序审查：

1. **写出响应契约。** 这个 route 真正返回 metadata、summary，还是 payload？
2. **画出 loader 调用树。** 搜索循环中的 `.await`，尤其是 `for id -> load_detail`。
3. **检查 SELECT 列。** 是否读取 body、operations、tree、blob 后没有进入响应？
4. **区分 list/detail/download。** 不要为了复用一个函数，让三个读模型共享 full hydration。
5. **在 PostgreSQL 侧集合化。** 优先 join、`ANY($1)` 或明确 batch，而不是 Rust N+1。
6. **先删工作，再看索引。** 数据形状正确后才分析剩余执行计划。
7. **用边界锁做回归。** 如果列表不应访问 blob，就锁 blob 并证明列表仍完成。
8. **生产比较保持可比。** 同 route、状态码、body、样本窗口，同时观察 proxy total 与 upstream。
9. **不要混淆两种收益。** 数据形状解释延迟；模块边界解释可维护性和复发概率。
10. **记录有意保留的债。** `ServerRepository`、`shared.rs` 和 changes→memory 不能因为目录已拆开就被宣告解决。

## 10. 本次实践真正可复用的原则

本次 Server 优化可以压缩成一句话：**列表接口应该拥有自己的读模型，Rust 模块应该让这个读模型的边界可见。**

113 MB 内部读取与 62 KB 外部响应的反差说明，最危险的性能问题常常不是数据库“慢”，而是应用要求数据库做了错误的工作。`ANY($1)`、metadata-only 和真实 PostgreSQL 锁测试分别从执行、契约和回归三个层面修复了这个问题。

Rust 架构重构则提供了长期约束：薄 `main`、集中 bootstrap、可独立构造的 app、bounded-context-first、HTTP/用例/模型/PostgreSQL 职责分离、具体实现优先。它已经让热路径可读，但 `ServerRepository`、`shared.rs` 与 changes→memory 仍提醒我们：目录只是边界的开始，visibility、类型所有权和窄应用接口才是边界最终能否被编译器与测试守住的关键。
