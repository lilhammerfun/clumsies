# 端到端延迟模型与诊断方法

| 文档属性 | 取值 |
|---|---|
| 文档角色 | 稳定方法与基础知识 |
| 适用范围 | App、IPC、HTTP、代理、Server、数据库与本地持久化 |
| 关联专题 | [端到端延迟优化专题](./index.md) |
| 案例证据截止 | 2026-08-26 |

> “请求香港服务器很慢”只是现象，不是诊断。本文建立一套可复用的方法；Clumsies 只作为例子出现，完整实现和数字由各案例文档与[验证证据台账](./evidence-ledger.md)维护。案例数字的证据截止为 2026-08-26，不应当作实时生产指标。

## 先定义到底什么叫“快”

同一个功能至少有三种完成时间：

- **First-ready**：用户已经可以完成核心操作。
- **单次交互延迟**：一次点击、查询或刷新从触发到结果可见的时间。
- **完整同步时间**：所有后台数据、缓存和派生状态都已经处理完毕。

三者不能互相替代。App 可以在 Project metadata 可用后进入 ready，同时继续加载 Review 详情和历史记录；这会缩短 first-ready，但不代表完整同步的总工作已经消失。

first-ready 必须先有契约：

> 当哪些数据和能力可用时，界面可以被认为已经就绪？

“窗口出现”通常太弱，“所有数据全部同步完”往往过重。合理契约应对应用户下一步真正能做的核心操作，并明确身份、Authority 和一致性要求。

## 端到端延迟是一条关键路径

Clumsies 的主要链路可以抽象成：

~~~text
macOS App
  → XPC
  → Rust daemon
  → HTTP/TLS
  → 香港 Caddy
  → Axum Server
  → PostgreSQL
  → 响应返回
  → daemon 解码与本地缓存
  → XPC 返回
  → UI 发布
~~~

若所有步骤串行，端到端时间可以近似表示为：

~~~text
T_e2e =
    T_app
  + T_xpc_queue
  + T_daemon
  + T_connection
  + T_request_transfer
  + T_server_queue
  + T_server_compute
  + T_response_transfer
  + T_decode
  + T_local_commit
  + T_publish
~~~

现实中有些步骤被缓存，有些并行，有些只在冷启动出现。更准确的表达是：

~~~text
T_e2e = 关键路径上所有串行 span 的持续时间之和
~~~

不能把日志里的所有 duration 简单相加。两个独立任务各耗时一秒，并发时用户不一定等待两秒；决定完成时间的是任务依赖图中最慢的必要分支。

对 first-ready：

~~~text
T_ready =
    T_ready 前的固定工作
  + longest_path(必要任务 DAG)
  + T_publish
~~~

因此优化 first-ready 的顺序通常是：

1. 从必要任务图中移除不应阻塞 ready 的节点。
2. 缩短最长依赖路径。
3. 并行真正独立的节点。
4. 防止后台任务争抢关键路径所需的连接、线程、数据库和锁。

把工作移到 ready 之后不等于删除工作。它必须拥有独立的 loading、failed、retry 和最终 settled 观测。

## Latency、throughput 与 concurrency

**Latency** 是单个操作完成多久，**throughput** 是单位时间完成多少操作，**concurrency** 是同一时刻有多少操作正在进行。

提高并发可能增加吞吐，却不一定降低单次延迟。接近资源上限后，更多请求会在连接池、执行器、SQLite writer 或 PostgreSQL 前排队，p95 反而上升。

Little’s Law 可以帮助识别队列积累：

~~~text
系统内平均并发量 ≈ 吞吐量 × 平均延迟
~~~

它不是调参公式。如果吞吐没有增加，并发和等待时间却同时上升，系统很可能只是在积累队列。

有界并发的上限应保护：

- HTTP 连接池；
- daemon 执行器；
- Server worker；
- PostgreSQL 连接池；
- 本地 SQLite 写锁；
- 客户端内存和 UI 发布队列。

并发之前先去重。把重复意图无界地并发出去，只会更快地冲击下游。

## RTT、连接建立与复用

跨境 RTT 较高，会放大每次必须串行发生的协议往返。一个冷连接可能依次支付 DNS、TCP、TLS、HTTP request 和首字节等待；具体往返次数取决于缓存、TLS 版本、协议和会话恢复，但规律不变：

> 串行往返越多，高 RTT 的影响越明显。

连接复用让后续请求不再重复支付完整的 TCP/TLS 成本。HTTP/2 可以让多个请求共享连接，但不能消除 Server 排队、带宽限制、丢包或底层 TCP 的影响。

诊断连接问题时，应分开记录：

- 首次请求与后续请求；
- 新建连接与复用连接；
- 正常间隔与零间隔突发；
- 长期复用的 HTTP client 与每次新建 client；
- 代理路径与直连路径。

只测一次冷请求不能描述稳定态；只测暖连接也会掩盖启动成本。

## TTFB、完整响应与本地后处理

客户端总耗时可进一步拆成：

~~~text
T_total =
    T_before_headers
  + T_body_transfer
  + T_decode
  + T_after_response
~~~

TTFB 通常表示从请求开始到响应头或首字节。它可能包含客户端排队、DNS/TCP/TLS、请求发送、Server 排队与计算、首字节网络往返。

TTFB 不包含完整 body 下载、透明解压、JSON 解码、SQLite 写入和 UI 发布。因此 upstream duration 很低，只能说明 Server 观测到的那一段较快；它不能自动推出用户也应该同样快。

当 Server 只有几十毫秒、XPC 却接近一秒，应继续检查：

- daemon 中是否排队；
- 是否反复建连；
- 响应体是否过大；
- body 完成后是否同步落本地缓存；
- UI 是否还等待不属于 ready 契约的任务。

## Wire bytes 与解码后大小

客户端拿到的 body 常常已经被自动解压，body 长度不能证明网络上传输了多少字节。

判断压缩是否生效，需要同时观察：

- 请求是否声明可接受的 Content-Encoding；
- 响应协商结果；
- Caddy 或 Server 记录的实际 wire bytes；
- 解码后的业务结果；
- upstream duration；
- 客户端完整 wall time。

gzip 的有效单变量 A/B 应满足：

1. 开关压缩时请求相同数据。
2. 解码后的状态码、语义和 body 一致。
3. wire bytes 明显变化。
4. Server 计算没有同步发生相同幅度变化。
5. 客户端完整耗时按预期改变。
6. 独立复测能重现方向。

如果只看到客户端变快，仍无法排除连接变暖、缓存命中或 Server 负载下降。

压缩也不是所有接口都值得使用。小响应减少不了多少字节，却仍有压缩与解压成本；较大的 JSON、重复字段和文本结构通常收益更明显。

带宽时延积可以帮助理解长距离链路：

~~~text
BDP = bandwidth × RTT
~~~

压缩不会降低 RTT 本身，但会减少需要通过拥塞窗口和链路传输的数据总量。

## 百分位和样本边界

性能报告至少应同时给出：

- 样本数；
- p50；
- p95；
- max；
- 测试环境；
- 冷/热进程与连接；
- 请求间隔和并发度；
- build、commit 或镜像 digest。

p50 描述典型体验，p95 描述尾部，max 暴露极端异常。平均值会稀释长尾，单个 max 又容易受工具、冷缓存和调度噪声影响。

小样本的 p95 很粗糙。5 次启动中的 p95 几乎就是 max；20 个样本的 p95 接近第二慢样本。此时应保留原始分布和 max，不给一个百分位数字过高的统计权威。

以下结果不能混成一组：

- 冷启动与 warm start；
- 正常间隔与零延迟压力；
- 首次连接与复用连接；
- 小响应与大响应；
- 无变化同步与大量变更；
- 不同版本或不同数据快照。

零延迟连续启动回答资源竞争和退化边界，不能替代正常用户节奏。

## 在每个边界建立可关联观测

不必先建设完整分布式追踪。统一 request-id、结构化日志和单调时钟 span 已能解决大部分归因。

| 边界 | 需要记录 | 能回答的问题 |
|---|---|---|
| App 触发 | 用户事件、启动模式、版本、冷暖状态 | 测量从哪里开始 |
| App → XPC | 方法、开始、返回、等待 | 是否卡在 IPC 或 daemon 可用性 |
| daemon 接收 | 到达、排队、发起 HTTP | daemon 内是否存在队列 |
| HTTP client | attempt、连接、headers/body 完成 | 是连接、TTFB 还是 body |
| Caddy | request-id、status、duration、wire bytes | 公网入口看到了什么 |
| Axum | route template、handler/service span | Server 代码消耗多少 |
| PostgreSQL | 查询数、批次、rows、bytes、锁 | 是否 N+1 或过量 hydration |
| daemon 响应后 | decode、hash、SQLite、cache publish | 网络结束后是否仍阻塞 |
| UI ready | ready 契约满足与 publish | 用户实际等了多久 |

同一进程内使用单调时钟。不同机器的绝对时钟可能有偏差，不应直接相减；跨进程用 request-id 关联，再分别分析本地 duration。

日志应记录数据规模、状态码、重试、缓存命中和构建身份，但不能记录 token、凭据、完整正文或敏感 header。

## 从症状建立可证伪的假设树

好的假设必须附带：“如果它是真的，我们应该看到什么？”

| 假设 | 预期观测 | 最小实验 |
|---|---|---|
| 每次重新建连 | 首次显著慢，复用后下降 | 比较同 client 的首个与后续请求 |
| 代理或路由异常 | 客户端网络段变化，upstream 基本稳定 | 固定版本和数据做代理/直连对照 |
| response transfer 过慢 | body 阶段与 wire bytes 强相关 | 对同一响应做压缩 A/B |
| Server 查询过重 | upstream 与数据库 span 同时偏高 | 记录查询数并做 batch 对照 |
| daemon 内排队 | XPC 到达与 HTTP 发起有明显空档 | 增加 daemon 边界时间戳 |
| 本地缓存阻塞返回 | body 已完成，XPC 仍延迟 | 测量缓存提交并验证异步化 |
| ready 等待全量同步 | 必要 DAG 包含非必要节点 | 标记节点并单独测 ready |
| 突发并发退化 | 正常间隔稳定，零间隔尾部升高 | 分开两种负载模型 |

“网络慢”“Rust 慢”“数据库慢”都不可证伪。更有效的排查方式是先找到最大的未知区间，再增加一个观测边界，像二分一样缩小范围。

## 单变量 A/B 的因果解释力

A/B 的目标不是得到两个不同数字，而是解释哪个机制导致变化。

实验前固定：

- client 与 Server 版本；
- endpoint、身份、参数和数据集；
- 并发与请求间隔；
- 网络位置和代理状态；
- 冷暖连接；
- 数据库与缓存条件。

同时记录两类指标：

- **结果指标**：first-ready、完整 wall time、p50、p95、错误率。
- **机制指标**：wire bytes、连接时间、query count、upstream duration、cache commit。

只有结果指标变好、机制指标没有支持时，只能说相关，不能说因果。跨境链路天然抖动，可交替执行 A/B，必要时做 A/B/A，并进行独立第二组复测。

常见有效对照：

- 压缩开/关，解码后业务结果相同；
- 新连接/复用连接，请求内容相同；
- N+1/batch，返回语义和顺序相同；
- 将非必要任务移出 first-ready，ready 契约和最终同步结果相同。

任何性能 A/B 都先通过正确性门槛。更快返回错误数据不属于优化。

## 缩短关键路径的通用顺序

### 先不做无用工作

- list 只返回列表所需 metadata；
- 不在热点路径加载完整 Tree/Blob；
- 删除重复序列化、哈希和转换；
- 确认 limit 或字段选择真的被 Server 执行。

### 再移动非必要工作

将非前置任务移到 ready 后，并继续观测 settled duration、失败率和重试。

### 批量化跨边界操作

避免 N+1，合并可共同获取的 metadata。批量也有上限：响应过大仍会增加内存、decode 和尾部。

### 有界并发独立节点

并行只适用于没有依赖关系的节点。上限由实际连接池、数据库和内存验证，不从项目数量直接推导。

### 复用会话与连接

长期复用 HTTP client、TLS session 和连接池，通常比在每个 Project 或同步轮次重新创建更重要。

### 减少 wire bytes

压缩、字段裁剪、分页和增量同步都可降低传输量，但必须有 wire-level 证据。

### 调整响应后的本地工作

cache、index、hash 和派生状态可能发生在 HTTP 结束后。移出实时路径前先回答：进程此时退出，这份数据能否从 Authority 安全重建？若不能，就不能套用 best-effort 策略。

## 可重复执行的性能 Playbook

1. 把症状写成有起点、结束契约、场景和百分位的 SLO。
2. 画出 App、IPC、HTTP、代理、Server、数据库、本地持久化和 UI publish。
3. 给最大未知区间添加 request-id 与 span。
4. 分冷/暖、正常/突发、小/大响应建立基线。
5. 先做“预期关键路径收益高、根因置信度高”的工作。
6. 一次只改变一个主要机制，并保持可回滚。
7. 同时验证结果、机制、业务语义和错误率。
8. 做独立复测和压力验证，分别报告。
9. 在生产等价环境核对 commit、digest、日志、健康和回滚。
10. 把成立条件、证据和未验证边界一起写进工程记忆。

## 常见误区

- 看到香港 Server 就先归因代理或跨境网络。
- 用 Server p95 代表用户端到端等待。
- 只看平均值，或者只保留最好的一轮。
- 用自动解压后的 body 长度代表 wire bytes。
- 混合冷启动、warm connection 和零延迟压力。
- 把工作移到后台后不再观测完整同步。
- 用无限并发掩盖串行等待。
- 客户端传了分页参数，就假定 Server 一定执行。
- 同时修改 SQL、缓存、并发和压缩，再把收益全归给一项。
- 性能数字变好后忽略错误、顺序、一致性、取消和优雅退出。

## 本专题中的三个对应案例

- [Server 热路径与 Rust 架构重构](./server-hot-path.md)回答“每个请求做了多少不必要工作”。
- [macOS first-ready 与 daemon 同步](./macos-first-ready.md)回答“哪些工作必须阻塞用户可用”。
- [跨境传输与 gzip 因果实验](./gzip-experiment.md)回答“相同业务结果需要传输多少 wire bytes”。

三者不能用一个数字概括，也不能互相替代证据。证据截止时的 SLO 判定、构建身份和未闭环事项见[验证证据台账](./evidence-ledger.md)。
