# 端到端延迟优化：验证证据台账

| 文档属性 | 取值 |
|---|---|
| 文档角色 | 截至证据截止日的验证快照与易变事实汇总 |
| 关联专题 | [端到端延迟优化专题](./index.md) |
| 验证等级 | mixed |
| 证据截止 | 2026-08-26 |

> 本文维护“截至 2026-08-26 可以确认什么、还不能确认什么”。它不是实时看板，也不保证旧数值仍代表当前生产 SLO。机制解释归各案例文档所有；这里不把测试全绿、容器健康或全部 200 当成性能因果证据。

## 证据截止时的验收状态

| 目标 | 截止时证据 | 判定 |
|---|---|---|
| Server Reviews p95 < 500 ms | 优化后生产 Reviews n=50，p95 44.296 ms | 已满足 |
| warm first-ready p95 < 1.5 s | 正常间隔 5/5 为 0.873–1.013 s | 小样本满足；仍需扩大冷/热分布 |
| App→daemon→香港 Reviews p95 < 500 ms | gzip 后两组各 n=40，p95 158.839 ms / 87.118 ms | 两组均满足 |
| 50 Draft events catch-up < 3 s | 较早阶段记录 0.39 s；最终组合版本未做生产等价受控 replay | 未闭环 |
| 无变化同步 < 50 KiB/轮 | 7 Project、8 request 的 exact block 为 2,859 B | 单轮满足；不能外推其他同步 |
| request-id 串联 Caddy/Server | route、status、duration、upstream 可关联，敏感字段过滤 | 已实现 |
| SIGTERM 5 s 内 drain、exit 0、无 137 | 单测、隔离 preflight 与精确 exit-0 gate 已通过 | 带在途请求的生产等价演练未闭环 |
| 功能与交付回归 | 本地 targeted、GitHub CI、Server Delivery 均通过 | 已满足证据截止时的自动门禁 |

本文只记录截至 2026-08-26 的验证证据，不能据此断言后续状态，也不能把证据截止时的主体部署写成“全部完成”。

## 三阶段性能证据

### Server 热路径

相同 route /api/v1/reviews、相同 62,632 B response body、全部 200：

| 阶段 | n | Caddy p50 / p95 / max | upstream p50 / p95 / max |
|---|---:|---:|---:|
| Server 重构前 | 12 | 2,328 / 2,497 / 2,497 ms | 2,328.658 / 2,497.464 / 2,497.464 ms |
| Server 重构后、gzip 前 | 41 | 25 / 37 / 40 ms | 25.331 / 37.076 / 40.014 ms |

这是强生产 before/after，不是随机分流、同时段或可逆 A/B。机制和查询路径见[Server 热路径与 Rust 架构重构](./server-hot-path.md)。

最终生产测量窗口：

| 范围 | n | p50 | p95 | max |
|---|---:|---:|---:|---:|
| structured all-route | 623 | 2.675 ms | 25.528 ms | 62.507 ms |
| Reviews | 50 | 25.528 ms | 44.296 ms | 62.507 ms |

623 个请求全部返回 200。这个窗口描述优化后运行画像，不单独计算“提升倍数”。

### macOS first-ready

| 场景 | n | 结果 | 结论边界 |
|---|---:|---:|---|
| 历史旧 App 基线 | 1 | 17.765 s | 单个历史点，不是旧 p95 |
| 新版正常间隔启动 | 5 | 0.873–1.013 s | 小样本 p95/max 1.013 s |
| 零间隔连续压力 | 10 | 9 次 AX marker 1.07–5.38 s；1 次 20 s 内未出现 marker | Server 请求全 200；UI/进程/AX 尾部未归因 |

可以确认常规启动进入约 1 s 区间，不能确认所有冷启动和突发场景已经闭环。机制见[macOS first-ready 与 daemon 同步](./macos-first-ready.md)。

### gzip 跨境传输

| 测试块 | n | p50 | p95 | max | decoded body |
|---|---:|---:|---:|---:|---:|
| gzip 前 Reviews | 40 | 391.522 ms | 815.801 ms | 1049.878 ms | 62,632 B |
| gzip 后第一组 | 40 | 73.417 ms | 158.839 ms | 414.386 ms | 62,632 B |
| gzip 后独立复测 | 40 | 79.637 ms | 87.118 ms | 90.939 ms | 62,632 B |

wire 62,632 B → 10,194 B，减少 83.7%；Caddy p95 37 ms → 39 ms。因果解释见[跨境传输与 gzip 因果实验](./gzip-experiment.md)。

轻量对照 /api/v1/me：n=20，body 1,185 B，p50/p95/max 为 36.349/39.085/41.717 ms。

### 无变化同步

一个 exact block：

- 7 个 Project；
- 8 个 request，全部 200；
- Caddy response body 合计 2,859 B；
- 该 block upstream 1.75–4.36 ms；
- 近期相邻轮出现过约 19.55–23.37 ms upstream 尾值。

结论仅限“一个代表性无变化稳态轮次的数据量达标”。它不能代表首次全量、冷缓存、大量变更或长期 p95。

## 证据强度

| 等级 | 本次对应证据 | 可以怎么表述 |
|---|---|---|
| 强因果 | gzip：业务 body 不变、wire -83.7%、upstream 不变、两组 XPC 复现 | 主要收益可归因于 response transfer 减少 |
| 强运行 before/after | Server：同 route/body，upstream 从秒级到几十毫秒 | Server hot path 是第一阶段主瓶颈；不能称随机 A/B |
| 实现机制 + 小样本体验 | first-ready：关键路径重画，n=1 历史点与 n=5 新样本 | 常规启动显著改善；不能声称完整尾部分布 |
| 单轮运行证据 | no-change sync 2,859 B | 该轮满足，不外推长期和其他场景 |
| 正确性/交付证据 | tests、CI、health、restarts=0 | 证明门禁通过，不证明性能因果 |

## 证据截止时的自动验证口径

不同环境的测试计数必须分开。

### 本地 macOS / targeted

- daemon library：267 passed、2 ignored；
- daemon main：4 passed；
- real XPC：1 passed；
- lifecycle：65 passed；
- keychain：1 passed；
- server integration：8 passed；
- fmt、clippy、macOS native、API contract、diff check 通过。

### GitHub main workspace CI

- daemon library：265 passed、2 ignored；
- daemon main：4 passed；
- agent runtime XPC E2E 因平台/feature gate 为 0；
- lifecycle：60 passed；
- server integration：8 passed。

Router oneshot + 真实 PostgreSQL容器 + migrations 属于应用级端到端测试；直接调用 ServerRepository 的 read_path_performance 属于组件/集成语义回归；daemon 随机 TCP listener 的 gzip 测试属于网络边界组件测试；香港 Caddy + 真实 XPC 才是生产链路验证。不要把它们统称成 E2E。

独立 correctness review：GO，0 blocker。

## 证据截止时的构建与生产交付

| 项目 | 结果 |
|---|---|
| PR #204 main commit | 8b33ec481014e3ca07be39df5be7cbf7b88d742d |
| PR #205 main commit | 408de49048021a3fdf53622157f519a88b50392d |
| PR #206 source / squash main | 53fabaf0bcfbaee6aa597768b2a340ea7830bb2b / 133fa5a7c784a314dcc85f4da0c613432e08557f |
| #205 main CI / delivery | 32868219409 / 32869884040，success |
| #206 main CI / delivery | 32919506973 / 32920318605，success |
| 最终生产镜像 | ghcr.io/lilhammerfun/clumsies-server@sha256:98454460ba43353a21791bf9fffd708c314a7559bfd1d9394cd4ad63810e03f0 |
| 容器 | running、healthy、restarts=0、FailingStreak=0 |
| release candidate | 36 tables / 22 migrations |
| 发布保护 | preflight backup、cutover 前 backup、公网 health 200 |

这些事实证明证据采集时运行的是预期不可变构建，并通过当时的发布门禁；不证明该镜像或容器状态在证据截止日之后仍然有效，也不替代上面的性能实验。

## 签名旁路事实

证据采集时本地已安装的 Debug runtime：

~~~text
Signature=adhoc
TeamIdentifier=not set
Identifier=ai.clumsies.daemon
designated requirement: identifier "ai.clumsies.daemon"
codesign --verify --strict: passed
~~~

没有创建或导入 Developer ID。签名告警来自 archived Zig 只读检查在错误边界验证未使用 runtime，已由 PR #205 修复；它不是本次性能根因。完整边界见[附录：本地 Debug 与分发签名边界](./signing-boundary.md)。

## 未闭环清单

1. 在最终组合版本、生产等价环境重放至少 50 个 Draft events，确认 catch-up < 3 s。
2. 对带在途请求的候选 Server 执行 SIGTERM，确认 5 s 内 drain、exit 0、无 137。
3. 扩大 first-ready 冷启动与 warm start 样本，区分 App phase、AX marker 和 daemon ready。
4. 归因零间隔 10 次启动中的 1 次 AX marker timeout。
5. 分别测量首次全量、冷缓存、大量变更和 post-ready collections settled。
6. 为 cache writer 增加 queue depth、coalesced、evicted 与 write failure 指标。
7. 若要正式排除系统代理，固定有效路由后做代理开/关隔离实验。

较早记录的“50 events 0.39 s”只能作为历史/集成证据，不能代替第 1 项的最终组合版本复测。

## 追溯链接

- [PR #203](https://github.com/lilhammerfun/clumsies/pull/203)
- [PR #204](https://github.com/lilhammerfun/clumsies/pull/204)
- [PR #205](https://github.com/lilhammerfun/clumsies/pull/205)
- [PR #206](https://github.com/lilhammerfun/clumsies/pull/206)

新证据到达时原位更新本台账。案例文档只在机制、实现或实验解释变化时更新，不为每次复测复制一套专题。
