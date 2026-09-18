# macOS first-ready 与 daemon 同步：把“可用”从“全部完成”中解耦

| 文档属性 | 取值 |
|---|---|
| 文档角色 | 深度实践案例 |
| 核心问题 | first-ready 契约、异步发布、同步并发与会话一致性 |
| 关联专题 | [端到端延迟优化专题](./index.md) |
| 证据台账 | [验证证据台账](./evidence-ledger.md) |
| 证据截止 | 2026-08-26 |
## 摘要

> 下述 first-ready、同步字节数、测试计数与构建结论均为截至 2026-08-26 的证据快照，不是长期 SLO 保证。文中 generation、有界并发、cursor 原子推进、缓存 writer 与启动分支等实现路径已于 2026-08-29 对当前源码交叉核对。

这次优化最重要的变化，不是把某一个香港接口从几百毫秒压到几十毫秒，而是重新定义了 macOS App 的启动契约：

> first-ready 只等待形成一个身份正确、Authority 一致、可以安全交互的最小工作区；Draft、Bundle、Review、同步状态以及非关键兼容性检查在 ready 之后独立加载。

旧版 App 的 first-ready 曾观测到 `17.765 s`。改造后，正常间隔启动 5 次全部落在 `0.873–1.013 s`，按这组小样本的最慢值粗算，首个可用状态缩短约 94.3%。

这个结果不能简单归因于“网络变快了”。实际收益来自两类工作：

- App 端缩短关键路径，把完整 hydration 和同步移出 first-ready。
- daemon 端减少重复请求和写放大，同时用有界并发、事务游标和会话代数保证提速不破坏一致性。

本文只讨论 macOS first-ready 与 daemon 同步。Server 查询重构、跨境压缩和 Caddy 侧证据应放在同专题的其他文档中。

---

## 1. first-ready 到底表示什么

应用启动通常混着三个不同的完成时刻：

1. 进程已经启动。
2. 界面已经可见。
3. 用户已经可以在正确的数据上下文里安全操作。

Clumsies 的 first-ready 指第三个时刻。它不是“Window 出现”，也不是“后台所有任务结束”。

对于当前工作区，first-ready 至少需要：

- daemon 可连接；
- 当前登录用户和 Organization 已确认；
- 当前 Project 已选定；
- Organization Ref、活动 Project Ref 和 Org selection 属于一个稳定 generation；
- Organization 与活动 Project 的 Memory 元数据足以建立导航树；
- 任何允许用户写入的动作都不会落到旧账号、旧 Project 或旧 Authority 上。

first-ready 不要求：

- 所有 Memory 正文已经下载；
- 所有 Draft 的 Base Commit 和详情已经 hydration；
- Bundle 列表及每个 Bundle 的成员详情已经完成；
- Review 列表已经完成；
- daemon sync/MCP 诊断状态已经刷新；
- 已归档 Zig CLI 集成仓库已经检查；
- 搜索索引或其他可重建派生状态已经完全追平。

这一区分很关键。把一个任务移出 first-ready，不等于忽略它；而是承认它不是“进入工作区”的前置条件，并给它独立的加载、失败和重试状态。

---

## 2. 旧关键路径为什么会被放大

旧启动链路大致如下：

```text
App 启动
  └─ 拉起并探测 daemon
      └─ 对齐 managed Agent integrations
          └─ 检查 archived Zig integrations
              └─ 读取配置和凭据
                  └─ 触发或等待 daemon 全量同步
                      └─ GET /me
                          └─ Organization / Project Commit State
                              └─ Org selection
                                  └─ Memory 元数据
                                      └─ Draft 列表
                                          └─ Draft baseline 正文
                                              └─ 每个 Draft 详情
                                                  └─ Bundle 列表与每项详情
                                                      └─ Review 列表
                                                          └─ sync/MCP 状态
                                                              └─ 再次验证 Authority
                                                                  └─ phase = ready
```

这条链路有四个系统性问题。

### 2.1 “完整”被错误地当成“可用”

Draft、Bundle 和 Review 都有自己的使用入口，但它们被放进了全局启动快照。任何一个集合较大、单项详情较慢或兼容性目录不可访问，都会推迟整个 App 的 ready。

尤其是 Draft hydration，不只是拉一页摘要。它还可能需要：

- 找到 Draft 指向的 Authority resource；
- 下载未加载的 baseline 正文；
- 获取每个 Draft 的完整投影；
- 获取旧 Base Commit；
- 将详情映射成可供 UI 使用的本地模型。

这些工作对进入 Memory 工作区并非全部必要。

### 2.2 串行依赖放大了跨境 RTT

一次香港请求可能不慢，但多个本来可以独立的请求排成链后，总时间接近：

```text
总延迟 ≈ Σ 每一层请求的 RTT、服务时间和本地落盘时间
```

网络抖动还会在串行链上逐项累积。此时看到“请求都成功、Server 单次也不慢”，并不能解释为什么 UI 十几秒后才 ready。

### 2.3 daemon 同步与 App 启动互相争用

早期实现曾在身份加载前等待 `retrySync`。第一轮修正把它改成后台 Task，避免直接 await；最终的 first-ready 改造则明确把 retry sync 放进 post-ready 工作。

即使 App 不直接 await，同步如果同时发起大量请求和 SQLite 写入，也会争用：

- HTTP connection pool；
- XPC 请求队列；
- SQLite writer；
- CPU 与文件系统；
- daemon 内部同步锁。

所以“改成 async”不自动意味着“退出关键路径”。任务的启动时机、资源竞争和结果发布边界同样重要。

### 2.4 正常 daemon 启动还做了一次无关的大文件哈希

旧版 daemon 在普通 resident/XPC 启动时，无条件构造 `LaunchAgentConfig`。该构造函数通过：

```rust
Sha256::digest(std::fs::read(&program_path)?)
```

把整个 daemon 二进制读入内存再计算 SHA-256。当时 App 内的 `clumsiesd` 约 103 MiB。

这个哈希是生成或校验 LaunchAgent plist 所需的管理信息，却不属于正常 daemon 服务启动。因此它既增加冷启动 I/O，也增加瞬时内存压力。

---

## 3. 新结构：两条完成通道

改造后的启动被拆成 essential lane 和 deferred lane。

```text
essential lane
  daemon health
    → managed integration 安全对齐
    → 登录身份与活动 Project
    → Organization authority / 活动 Project projection generation
    → Organization authority + 活动 Project projection 元数据
    → generation 二次验证
    → phase = ready

deferred lane（ready 后并行、彼此独立）
  ├─ Draft inventory + baseline/detail hydration
  ├─ Bundle list + details
  ├─ Review inventory
  ├─ archived integration inspection
  ├─ daemon retry sync
  └─ sync status
```

### 3.1 first-ready 保留什么

`WorkspaceLoader.load` 的核心结果缩小为：

- daemon health；
- `project_config`；
- `/api/v1/me` 返回的用户、Organization、能力和 Project 引用；
- Organization commit-state；
- 活动 Project 的 commit-state 与 Org selection；
- Organization authority 和活动 Project projection 的 Memory metadata；
- 当前数据来自 live 还是 stale cache。

Memory 此时可以只有路径、标题、content hash 和 Ref commit id，正文维持 `contentLoaded = false`，等用户打开或后续任务确实需要时再加载。

这不是“先显示一个可能错误的空壳”。Loader 在返回前仍会再次读取 Organization commit-state，并再次确认活动 Project 的：

- `refCommitId`；
- `selectedOrgResourceIds`；
- `orgSelectionRevision`。

如果前后不一致，当前加载被判定为 `sharedStateChangedDuringLoad`，不会发布一个混合 generation 的工作区。

### 3.2 ready 后每类数据有独立 loader

post-ready 不再是另一个隐藏的“全有或全无”大任务，而是多个独立 Task：

- `draftInventoryLoadTask`
- `bundleLoadTask`
- `reviewLoadTask`
- `legacyAgentAdapterInspectionTask`
- `postReadyRetrySyncTask`
- `postReadySyncTask`

Draft、Bundle 和 Review 分别维护 `loading / loaded / failed` 状态。某一类加载失败时：

- App 仍保持 ready；
- 其他集合继续加载；
- 旧数据不会被清空；
- 对应页面可以显示局部错误并单独重试。

这种拆分既缩短了关键路径，也改善了故障隔离。Archived Zig integration inspection 是兼容性提醒，不应再阻塞 Authority 工作区；managed integration 的安全对齐仍保留在 essential lane。

---

## 4. 异步发布比异步请求更难

把请求放到后台很容易，真正困难的是防止旧任务在未来覆盖新状态。

这次实现同时使用了三种 generation。

| Generation | 作用 |
|---|---|
| `workspaceReloadGeneration: UUID` | 标识一次 App 工作区加载意图 |
| Authority generation | 由 Org/Project Ref commit、Org selection revision 表示共享数据版本 |
| daemon `session_revision: u64` | 标识当前 Server 身份/会话世代 |

它们解决的是三个不同问题，不能互相替代。

### 4.1 Workspace generation 阻止旧 Task 回写

每次 reload 都创建新的 `workspaceReloadGeneration`。所有 post-ready Task 捕获当次 generation，并在发布前同时检查：

- generation 仍是当前值；
- `phase == .ready`；
- Task 未被取消。

reload、sign out 或 Authority 切换会取消旧 post-ready Task，并推进 generation。

仅依赖 `Task.cancel()` 不够。一个请求可能已经完成，或者底层调用不立即响应取消；发布前的 generation 比较才是最终写屏障。

### 4.2 Authority 边界阻止 stale cache 切换身份

缓存可用于降级读取，但不能成为账号切换的 Authority 证据。

当前实现规则是：

- 冷启动且没有已加载工作区时，可以使用 stale snapshot 进入降级状态；
- 已有工作区且仍是同一用户、同一 Organization 时，stale 响应不能覆盖更新的内存 generation；
- 已有工作区但身份或 Organization 发生变化时，stale 数据不能完成切换；旧 Authority 数据会被清除，并要求取得 fresh account data。

这里比较的不只是 Project ID，而是用户和 Organization 身份。否则旧账号的缓存可能在登录切换窗口里被误当成新工作区。

### 4.3 Deferred 数据采用逐记录三方合并

后台加载期间，用户可能已经修改、删除或新建了记录。直接执行：

```swift
self.reviews = loadedReviews
```

会让较早发起的请求覆盖较新的本地状态。

实现因此保留三份集合：

- `baseline`：启动后台加载时看到的状态；
- `current`：请求结束时 App 当前状态；
- `loaded`：后台请求得到的新状态。

逐记录合并遵循：

- `current == baseline`：说明本地没有变化，可以采用 `loaded`；
- `current != baseline`：说明后台请求期间本地发生了变化，保留 `current`；
- 远端已经删除、而本地也未修改的记录，从结果中删除；
- 后台期间本地新增或改变的记录不被旧响应抹掉。

这本质上是一个轻量三方合并，而不是数组替换。它让独立 loader 可以并发运行，却不牺牲用户操作的因果顺序。

---

## 5. daemon 同步：先去重，再做有界并发

App 缩短 first-ready 后，daemon 仍必须高效地完成真实同步。这里没有采用无界 `spawn`，而是针对两类请求设置不同并发上限。

### 5.1 Project Commit State：4 路并发

旧 `sync_refs` 对每个 Project 依次执行：

1. 获取 Project commit-state；
2. 从 Project 响应确定 Organization；
3. 获取同一个 Organization commit-state；
4. 安装 Org Ref；
5. 安装 Project Ref。

如果账号下有 7 个 Project，同一个 Organization state 会被请求和安装 7 次。

新流程是：

1. 收集并校验所有 Project ID；
2. 确保活动 Draft 所需的 Base Commit 已缓存；
3. 使用 `MAX_CONCURRENT_PROJECT_STATE_REQUESTS = 4` 获取各 Project commit-state；
4. 从结果确认期望的 Organization ID；
5. 只获取一次 Organization commit-state；
6. 验证所有 Project 都属于同一 Organization；
7. 安装一次 Org Ref，再安装各 Project Ref；
8. 聚合错误并返回确定性的第一个错误。

一次无变化的 7 Project 实测同步轮次正好产生 8 个请求：7 个 Project state 加 1 个 Organization state。Caddy 记录的总响应体只有 `2,859 B`，upstream 时间为 `1.75–4.36 ms`。

这组证据证明了请求拓扑已经从近似 `2N` 收敛为 `N+1`。它不等同于完整 UI 同步耗时，也不能单独证明跨境端到端 p95。

### 5.2 Draft Projection：8 路并发

一个 Draft event page 可能包含：

- 同一 Draft 的多个 version 事件；
- 多个 Draft 指向同一个 Base Commit；
- 已关闭或已合并 Draft 的历史事件。

新同步先按 `draft_id` 聚合事件，只为每个 Draft 发出一次 projection 请求，并记录这一页要求的最高 minimum version。

随后使用：

```text
MAX_CONCURRENT_DRAFT_PROJECTION_REQUESTS = 8
```

进行有界并发。每个结果都必须验证：

- 返回的 `draft_id` 与请求一致；
- `project_id` 一致；
- projection version 不低于该页事件要求的最高 version。

8 路不是一个理论最优值，而是当前在减少 RTT 串行等待与限制 Server、连接池、内存压力之间的工程上限。没有生产等价压力数据时，不应把它描述为最终最优并发度。

### 5.3 Base Commit 先去重再缓存

旧路径可能随事件或 Draft 重复调用 `ensure_commit_cached`。

新路径从有效 projection 中提取仍处于 `Open` 或 `Submitted` 状态的 `base_commit_id`，放入 `BTreeSet` 后逐个确保缓存。这样：

- 同一页多个 Draft 共用 Base Commit 时只处理一次；
- 同一 Draft 多个事件不会重复处理；
- terminal Draft 不再制造无意义的 Base Commit 工作；
- 顺序确定，错误可复现。

有界并发并不是替代去重。正确顺序应当是：

```text
归并重复意图 → 验证 → 有界并发获取 → 去重共享依赖 → 原子提交
```

---

## 6. Cursor 必须与投影原子推进

并发获取 Draft projection 后，最危险的优化错误是先更新 cursor，再慢慢落本地状态。一旦进程在两者之间退出，下一轮会从新 cursor 开始，永远跳过尚未落地的事件。

当前一页事件的提交顺序是：

1. 获取 event page；
2. 验证非空页、`has_more` 与 `next_cursor`；
3. 拒绝不前进的 cursor；
4. 聚合同一 Draft 的事件；
5. 并发获取并验证 projection；
6. 去重并缓存活动 Draft 的 Base Commit；
7. 开启 SQLite `BEGIN IMMEDIATE`；
8. 写入所有 Draft projection；
9. 幂等写入 remote event；
10. 更新 `META_DRAFT_EVENTS_CURSOR`；
11. 在同一事务中为受影响 Project 入队搜索索引；
12. commit；
13. commit 后通知 search worker。

因此，projection、event 记录、搜索任务和 cursor 对 SQLite 来说是一个原子状态转换。

如果网络请求、projection 校验、Base Commit 缓存或事务任一步失败，cursor 都不会越过该页。下一轮可能重复请求，但事件插入和 projection 更新具备幂等或版本检查，重复优于丢失。

需要注意：这是 Draft event page 内的原子性，不代表整个多 Project sync 是一个全局事务。各 Ref generation 仍按各自安装边界推进。

---

## 7. GET 响应缓存：从同步落盘改为 bounded/coalesced writer

macOS `ServerClient` 实际通过 XPC 让 daemon 发出 Server 请求。旧实现收到一个成功 GET 后，会在把结果返回 App 前同步执行 SQLite upsert：

```text
HTTP body 完成
  → 序列化 headers
  → SQLite insert/update
  → prune
  → transaction commit
  → XPC 返回 App
```

这让可重建缓存写入进入了每个读请求的延迟路径。并发 GET 越多，对 SQLite writer 的争用越明显。

新实现先将响应交给调用方，再由一个后台 writer 落盘。这个 writer 有明确边界：

- 仅缓存 GET 的成功 `2xx` 或 `404` 响应；
- 单个待写 body 最大 `1 MiB`；
- 最多保留 16 个不同 key 的 pending write；
- 同一 `(server_url, path)` 的多个响应只保留 revision 最新的一份；
- 只有一个后台 SQLite writer；
- 持久缓存最多 512 项、逻辑体积最多 64 MiB；
- 写失败只记录 warning，不让已经成功的网络请求变成业务失败。

“异步”不是无限排队。超过 body 上限的响应不会复制进 pending queue；pending key 达到上限时会丢弃一个待写 key，避免流量高峰把内存缓存队列变成第二个数据库。

Revision 还防止乱序覆盖：

```text
请求 A revision=10，先发出、后完成
请求 B revision=11，后发出、先完成
```

如果 B 已经成为该 key 的 latest success，A 即使随后完成也不能把旧 body 覆盖回 SQLite。

这类缓存是可重建派生状态，因此可以接受 best-effort durability；Authority 数据、Draft 操作队列和凭据不能套用同样策略。

---

## 8. 401 refresh 与会话切换屏障

请求并发增加后，401 不再只是“刷新一次 token”这么简单。以下竞态都可能发生：

- 旧请求收到 401 时，用户已经退出登录；
- 旧账号正在刷新 token 时，App 已经切换 Server；
- 两个请求同时收到 401；
- 旧账号的 GET 已进入缓存写队列，新账号开始发布；
- refresh 失败后清 token，却误清了刚登录的新会话。

daemon 为此引入 `session_revision` 和两层锁。

### 8.1 请求携带会话快照

每个认证请求开始时捕获：

- `session_revision`；
- `server_url`；
- access token；
- refresh token。

收到 401 后先进入单例 `token_refresh` 临界区，再比较当前状态是否仍属于同一 revision、Server 和 access token。

如果另一个请求已经完成了同一会话的 token rotation，可以复用新 token；如果登录、退出或 Server 切换已经推进了 session revision，则旧请求被判定为 `Superseded`，不得改写当前会话。

### 8.2 refresh 发布前再次比较

网络 refresh 完成不代表结果仍然有效。持久化新 token 前还要获得 `project_config_mutation` 锁，并再次比较：

- session revision；
- Server URL；
- 被 401 拒绝的旧 access token。

只有条件仍成立，才先更新 Keychain，再发布新的内存 token。Token rotation 属于同一登录会话，不推进 session revision；账号登录、退出或 Server 身份切换才创建新 session generation。

refresh 返回认证失败时，也只能在 revision、Server 和 token 仍匹配时清理凭据。

### 8.3 会话切换同时隔离缓存

新会话发布前会：

1. 锁住响应缓存 writer；
2. 提高 `minimum_revision`，使旧 pending write 失效；
3. 清空 pending 状态；
4. 清理 SQLite response cache；
5. 成功后才发布新的 project config 和 session revision。

如果缓存清理失败，新身份不会半发布。这样旧账号已经完成但尚未落盘的 GET，不能在新账号会话中重新出现。

这部分代码的直接目标是正确性，但它是并发优化的前提：没有 session barrier，就不能安全地增加请求并发或把缓存写入异步化。

---

## 9. 移除普通 daemon 启动时的 103 MiB 哈希

`LaunchAgentConfig::from_daemon_config` 仍然需要 daemon SHA-256，因为 plist 通过 `CLUMSIES_DAEMON_BINARY_SHA256` 判断安装定义是否与当前二进制一致。

修复没有删除这个安全检查，而是把构造时机收窄到真正使用 LaunchAgent 管理命令时：

```text
有 CLI 管理参数
  → 构造 LaunchAgentConfig
  → 读取并哈希 daemon
  → install/status/reconcile/print plist

普通 resident daemon，无参数
  → 不构造 LaunchAgentConfig
  → 不读取、不哈希约 103 MiB 二进制
  → 直接初始化 XPC 与同步服务
```

这是典型的关键路径优化：不是尝试把 SHA-256 写得更快，而是证明普通服务启动根本不需要做这件事。

---

## 10. 性能证据与边界

### 10.1 first-ready 样本

| 场景 | 样本 | 结果 | 可以支持的结论 |
|---|---:|---:|---|
| 旧 App first-ready 基线 | 1 | `17.765 s` | 旧关键路径存在秒级系统性等待 |
| 新版正常间隔启动 | 5 | `0.873–1.013 s` | 常规启动已进入约 1 秒区间 |
| 新版零间隔连续压力启动 | 10 | 9 次 AX marker 为 `1.07–5.38 s`，1 次在 20 秒超时 | 极端连续启动仍有 App/进程/AX 层异常尾部 |

新版正常样本只有 5 个，`p95/max = 1.013 s` 在这里几乎等于最大值，不足以代表稳定的生产尾延迟。旧基线也只有一个样本，94.3% 只能作为方向明确的工程对比，不能视为统计置信区间。

### 10.2 压力样本不能归咎于 Server

零间隔压力中，所有观测到的 Server 请求都返回 200，但仍有一次 AX ready marker 在 20 秒内未出现。

这说明：

- marker timeout 不能自动解释为香港网络或接口失败；
- Server 200 也不能证明 UI 已 ready；
- 连续启动可能引入 AppKit、进程退出、LaunchServices、XPC bootstrap、daemon 复用或 Accessibility 采样本身的竞争。

在没有更细的本地时间线前，不能把这一个 timeout 填成“20 秒延迟样本”，也不能据此计算有效 p95。

### 10.3 first-ready 不等于后台 settled

证据截止时的 first-ready 测量不包含：

- Draft inventory 全部完成；
- Bundle 和 Review 全部完成；
- daemon retry sync 完成；
- sync/MCP 状态刷新完成；
- 搜索索引完全追平。

因此“约 1 秒 ready”不能写成“约 1 秒完成全量同步”。后续应分别定义：

- `first_ready_duration`
- `post_ready_collections_settled_duration`
- `sync_round_duration`
- `startup_success_rate`
- 各 deferred loader 的失败率

### 10.4 回归证据

最终代码经过 daemon 与 XPC 全量回归，包括：

- daemon library：267 passed，2 ignored；
- daemon main：4 passed；
- real XPC：1 passed；
- lifecycle：65 passed；
- Keychain：1 passed；
- Server integration：8 passed；
- clippy、fmt 和 diff check 通过。

这些测试证明了已覆盖路径的行为没有回归，但测试通过本身不是生产延迟证据。尤其是 8/4 并发上限、50-event replay 和退出时在途请求仍需要专门的生产等价实验。

---

## 11. 这次实践可复用的设计原则

### 11.1 先删关键路径，再优化关键路径

优化顺序应是：

1. 这个工作真的是 ready 前置条件吗？
2. 能否只加载 metadata？
3. 能否按需加载正文？
4. 能否移到 ready 后？
5. 确实必须等待时，再优化查询、并发和序列化。

把无关工作从关键路径删除，通常比把它加速 30% 更可靠。

### 11.2 并发之前先做意图归并

Draft event 先按 Draft 聚合，再发 8 路请求；Base Commit 先做集合去重，再缓存；Organization state 从每 Project 一次收敛为整轮一次。

无界并发会把重复工作更快地打到下游，并不是真正优化。

### 11.3 每个后台发布都需要 generation

任何满足以下条件的 UI 请求，都应携带 generation 或等价的 observed state：

- 请求期间用户可以切换账号或 Project；
- 请求结果会替换集合；
- 请求可能使用 stale cache；
- 请求完成顺序不确定；
- 用户可能在等待期间修改同一记录。

Cancellation 是资源优化，generation check 才是正确性边界。

### 11.4 缓存写入可以异步，身份切换不能异步含糊

响应缓存是可重建数据，可以 bounded、coalesced、best-effort；会话、Authority Ref、Draft operation cursor 必须有明确的原子提交和失败语义。

判断某项 I/O 能否移出响应路径前，应先回答：

> 如果进程现在退出，这份数据丢失后能否从 Authority 安全重建？

### 11.5 性能数字必须绑定完成定义

“启动 1 秒”必须说明是：

- 进程启动；
- Window 可见；
- AX marker；
- phase ready；
- 首个 Memory 正文可见；
- 还是全量后台 settled。

没有完成定义的数字无法比较，也容易把 UI、daemon、网络和 Server 的问题混成一件事。

---

## 12. 尚未闭环

本次改造已经解决常规 first-ready 的主要系统性延迟，但以下事项仍需单独收口：

- 扩大冷启动和 warm start 样本，至少分场景采集稳定的 p50/p95；
- 定位零间隔 10 次启动中的一次 AX marker timeout；
- 为 post-ready Draft、Bundle、Review 分别采集 settled duration 和失败率；
- 在生产等价环境完成 50 Draft event 的受控 replay；
- 验证 daemon 收到 SIGTERM 时，在途网络请求、Draft page 事务和缓存 writer 的行为；
- 增加 cache writer queue depth、coalesced、evicted 和 write failure 指标；
- 根据真实压力数据重新评估 Draft 8 路、Project 4 路并发是否需要调整。

在这些实验完成前，可以确认的是“常规 first-ready 已显著缩短，核心一致性边界有测试覆盖”；不能确认的是“所有压力场景的尾延迟已经闭环”。

---

## 实现索引

- [PR #203：Server 规范化与热路径](https://github.com/lilhammerfun/clumsies/pull/203)
- [PR #204：first-ready、同步与 daemon](https://github.com/lilhammerfun/clumsies/pull/204)
- macOS 入口：`apps/macos/Sources/Features/Workspace/WorkspaceCoordinator.swift`
- daemon Ref 同步：`crates/daemon/src/commit_sync.rs`
- Draft event 同步：`crates/daemon/src/draft.rs`
- daemon 会话与响应缓存：`crates/daemon/src/state.rs`
- 401 token refresh：`crates/daemon/src/server_client.rs`
- LaunchAgent binary hash：`crates/daemon/src/config.rs`
- daemon 启动分支：`crates/daemon/src/main.rs`
