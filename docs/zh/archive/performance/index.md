# 端到端延迟优化专题

| 文档属性 | 取值 |
|---|---|
| 文档角色 | 专题索引 |
| 目标读者 | 客户端、daemon、后端、SRE 与质量工程师 |
| 关联工作 | Pull Request #203、#204、#205、#206 |
| 证据截止 | 2026-08-26 |

> 本专题的运行数字、构建身份与验收状态均为截至 2026-08-26 的证据快照，不代表 2026-08-29 或更晚时间的实时生产状态。基础模型、三个核心实践和验证证据分别维护；索引只负责说明它们之间的关系。

## 一次问题，三条不同的关键路径

| 阶段 | 真正的问题 | 代表性结果 | 深入阅读 |
|---|---|---|---|
| Server 热路径 | list/metadata 接口执行 N+1 与完整 payload hydration | 同 route/body 的生产 upstream 从 2.3–2.5 s 降到 25–40 ms | [Server 热路径与 Rust 架构重构](./server-hot-path.md) |
| App 与 daemon | first-ready 等待非必要全量同步，本地缓存与同步工作挤进实时路径 | 历史 17.765 s；新版正常间隔 5/5 为 0.873–1.013 s | [macOS first-ready 与 daemon 同步](./macos-first-ready.md) |
| 跨境传输 | Caddy 支持压缩，但 daemon 没有协商 gzip | wire 62,632 B → 10,194 B；XPC p95 815.801 ms → 158.839 / 87.118 ms | [跨境传输与 gzip 因果实验](./gzip-experiment.md) |

这三阶段不能合并解释为一次笼统的“网络优化”：第一阶段减少 Server 实际工作，第二阶段重画用户可用关键路径，第三阶段减少相同业务结果的跨境传输量。

## 文档地图

- [客户端加载与请求排查](./client-loading-audit.md)：2026-09-17 的加载状态、请求安排、本机证据与待改进项；与下方历史实验的证据日期分别维护。

### 先建立方法

- [端到端延迟模型与诊断方法](./latency-model.md)：first-ready、TTFB、RTT、连接复用、wire bytes、百分位、关键路径、边界观测、假设树和单变量 A/B。

### 再看三个深度案例

- [Server 热路径与 Rust 架构重构](./server-hot-path.md)：N+1、过度 hydration、批量 projection、metadata-only，以及 bounded-context-first 重构和剩余架构债。
- [macOS first-ready 与 daemon 同步](./macos-first-ready.md)：ready 契约、任务 DAG、有界并发、cursor、会话屏障和异步缓存写入。
- [跨境传输与 gzip 因果实验](./gzip-experiment.md)：压缩协商、A/B 控制、wire/upstream/XPC 三层证据和结论边界。

### 最后查证据快照

- [验证证据台账](./evidence-ledger.md)：SLO、版本、生产构建、测试口径、部署证据和未闭环事项。数字是否仍然有效，以这里和各实验原始记录为准。
- [附录：本地 Debug 与分发签名边界](./signing-boundary.md)：解释 ad-hoc、Developer ID、managed install/update 与 archived Zig 只读检查。它是相关工程纠错，不属于性能根因。

## 推荐阅读路径

- **第一次了解**：模型与诊断方法 → Server → first-ready/daemon → gzip → 证据台账。
- **正在排查“请求很卡”**：模型中的边界观测与假设树 → 对应案例 → 证据台账中的复现条件。
- **做代码评审**：直接阅读对应案例；不要只引用索引里的结果数字。
- **做验收或发布**：从证据台账开始，再回到案例核对机制与实验边界。

## 内容所有权

- 索引拥有导航和跨文档关系，不拥有完整实验解释。
- 模型文档拥有稳定方法，不维护本次部署的易变状态。
- 三个案例文档各自拥有该机制的代码路径、实验设计和局部证据。
- 证据台账拥有证据截止时的 SLO 判定、commit、镜像、测试与未闭环清单。
- 签名附录独立维护安全与分发边界，避免再次污染性能归因。

可复现代码、原始日志和生产事实优先于文档叙述。发生冲突时应修正文档，不为旧结论辩护，也不创建带日期或版本号的重复专题。

## 客户端交互审计

- [macOS 错误反馈审计与修复](./client-error-feedback-audit.md)：连接状态、页面错误、恢复策略与回归验证。

## 证据截止时状态

截至 2026-08-26，主线收益已有部署证据。台账当时尚未闭环：最终组合版本的 50-event 生产等价回放、带在途请求的 SIGTERM 演练，以及零延迟连续启动中的 AX marker 尾部。此后的状态不能由这份快照推断，详见[验证证据台账](./evidence-ledger.md)。
