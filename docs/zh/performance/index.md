# 端到端延迟优化

本节记录 Server、网络、daemon 与 macOS 边界上的性能模型、
实现改动与带日期的证据。

## 阅读顺序

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