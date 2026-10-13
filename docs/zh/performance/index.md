# 性能与可观测性

开发和运维人员可从这里选择指标设计、现场排障、跨层计时模型和性能验证证据。

## 可观测性与排障

- [可观测性设计方案](/zh/performance/observability-plan)：定义采集架构、四信号口径、数据状态、看板与告警，以及旧面板迁移和验收。
- [可观测性运维排障指南](/zh/performance/observability-runbook)：按异常时间、吞吐量与延迟，定位接口、同步、索引、检索或证据链故障。

## 计时模型与验证证据

[HTTP 本地负载测试报告](/zh/performance/observability-load-report)记录项目元数据接口的六组固定负载、完整操作与客户端排队耗时、埋点一致性，以及可复现的原始证据。

1. [延迟模型与诊断](/zh/performance/latency-model)
2. [服务端热路径](/zh/performance/server-hot-path)
3. [macOS 首次就绪](/zh/performance/macos-first-ready)
4. [跨区 gzip 实验](/zh/performance/gzip-experiment)
5. [验证证据台账](/zh/performance/evidence-ledger)
6. [客户端加载与请求审计](/zh/performance/client-loading-audit)
   —— 9 月 17 日的加载状态、请求调度与剩余工作。

7. [客户端错误反馈审计](/zh/performance/client-error-feedback-audit)
   —— 失败呈现、恢复与回归证据。

[签名附录](/zh/performance/signing-boundary)把本地身份失败与
延迟回归区分开来。测量结果是绑定 revision、环境、请求形状与完成定义的快照，
不是永久 SLO。