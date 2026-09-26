# 跨境传输与 gzip：一次可归因的性能实验

| 文档属性 | 取值 |
|---|---|
| 文档角色 | 深度实践与强因果实验 |
| 核心问题 | 相同业务结果为什么在香港链路上仍有数百毫秒尾延迟 |
| 关联实现 | [PR #206](https://github.com/lilhammerfun/clumsies/pull/206) |
| 关联专题 | [端到端延迟优化专题](./index.md) |
| 证据台账 | [验证证据台账](./evidence-ledger.md) |
| 证据截止 | 2026-08-26 |

> 这是截至 2026-08-26 本专题中因果证据最完整的一次优化：业务响应、解码后 body 与 Server timing 基本不变，wire bytes 单独下降，客户端端到端延迟随之在两组样本中显著下降。当前源码仍启用了 daemon gzip 协商与回归测试，但下述运行数字不是实时生产指标。

## 问题已经不在 Server 计算

gzip 实验发生在 Server 热路径重构之后。此时生产 Reviews 的 Caddy/upstream 已经从 2.3–2.5 s 降到 25–40 ms，但真实 App→XPC→daemon→香港 Reviews 仍有明显尾部：

| 控制块 | n | p50 | p95 | max | 解码后 body |
|---|---:|---:|---:|---:|---:|
| gzip 前 Reviews | 40 | 391.522 ms | 815.801 ms | 1049.878 ms | 62,632 B |

同一环境下的轻量对照是 /api/v1/me：

| 控制块 | n | p50 | p95 | max | body |
|---|---:|---:|---:|---:|---:|
| /api/v1/me | 20 | 36.349 ms | 39.085 ms | 41.717 ms | 1,185 B |

这两组数据不能单独证明压缩是根因，但能切开责任边界：

- 固定的认证、HTTP 和香港入口成本没有达到数百毫秒。
- Reviews 的 Server upstream 已是几十毫秒，不能解释 XPC p95 815.801 ms。
- 大响应比小响应多出的尾部，提示继续检查 response transfer、wire bytes、解压和本地后处理。

这时再继续重写 SQL，已经不是证据驱动的下一步。

## Caddy 已支持压缩，为什么客户端仍传原文

Caddy 原本已有：

~~~text
encode zstd gzip
~~~

这只表示 Server 可以在客户端声明接受相应编码时压缩响应。HTTP 压缩是协商结果，不是服务端单方面配置：

~~~text
client: Accept-Encoding: gzip
server: Content-Encoding: gzip
wire: compressed bytes
client library: transparent decompression
application: original decoded body
~~~

daemon 的 reqwest 使用 default-features = false，却没有显式启用 gzip feature。因此问题不是“香港 Caddy 没开 gzip”，而是 daemon 没有完成压缩协商。

PR #206 的功能变化很窄：让 daemon 的 HTTP client 声明 gzip 能力，并由 reqwest 透明解压。上层 XPC、JSON decode 和 SQLite response cache 仍看到与改动前相同的业务 body。

这个区别很重要。若文档写成“给 Server 新增 gzip”，会让后续排查者错误地检查 Caddy 配置，而漏掉客户端依赖 feature 和实际请求 header。

## 实验设计

本次 A/B 固定了：

- endpoint 与身份；
- 数据集和返回语义；
- 解码后 body 为 62,632 B；
- 已完成 Server 热路径重构的生产版本；
- 同一 macOS/XPC/daemon 调用方式；
- 每组 40 次请求；
- Caddy duration、upstream duration 与 wire bytes 观测口径。

改动变量是 daemon 是否协商并透明解压 gzip。

判断标准不是“客户端数字变小”，而是以下机制链能否同时成立：

1. 状态码和业务结果保持一致。
2. 解码后 body 保持 62,632 B。
3. wire bytes 明显下降。
4. Caddy/upstream 没有出现对应幅度的计算下降。
5. XPC 完整耗时显著下降。
6. 独立第二组能复现改善方向。

## XPC 结果

| 测试块 | n | p50 | p95 | max | 解码后 body |
|---|---:|---:|---:|---:|---:|
| gzip 前 | 40 | 391.522 ms | 815.801 ms | 1049.878 ms | 62,632 B |
| gzip 后第一组 | 40 | 73.417 ms | 158.839 ms | 414.386 ms | 62,632 B |
| gzip 后独立复测 | 40 | 79.637 ms | 87.118 ms | 90.939 ms | 62,632 B |

第一组已经让 p95 进入 500 ms 目标内；第二组更紧，但两组都必须保留，不能只挑最好的一组报告。

独立复测的意义不是把 87.118 ms 当成永久承诺，而是降低“第一组恰好遇到暖连接或网络低谷”的可能性。跨境网络仍有抖动，因此真实 SLO 需要更长时间窗口。

## Wire 与 upstream 对照

| 指标 | gzip 前 | gzip 后 |
|---|---:|---:|
| 解码后 body | 62,632 B | 62,632 B |
| Caddy wire bytes | 62,632 B | 10,194 B |
| wire reduction | — | 83.7% |
| Caddy p95 | 37 ms | 39 ms |

gzip 后 exact 40 block：

| 层级 | p50 | p95 | max |
|---|---:|---:|---:|
| Caddy duration | 26 ms | 39 ms | 43 ms |
| upstream duration | 25.232 ms | 38.466 ms | 42.862 ms |

Server 计算没有同步变快，Caddy p95 甚至从 37 ms 轻微变为 39 ms；改变的是网络实际传输量。wire 从 62,632 B 降到 10,194 B，减少 83.7%，而 XPC p95 从 815.801 ms 降到 158.839 ms，第二组为 87.118 ms。

因此可以形成一条闭合证据链：

~~~text
启用 daemon gzip 协商
  → 业务 body 不变
  → wire bytes -83.7%
  → upstream 基本不变
  → 两组 XPC 端到端延迟显著下降
~~~

这比“代码里加了 gzip feature，所以应该更快”强得多。

## 为什么这比普通 before/after 更接近因果

Server 重构的生产对照很强，但它是顺序部署的 before/after，无法完全排除时段和负载变化。

gzip 证据更强的原因是：

- 功能改动窄；
- response 语义和解码后大小相同；
- wire bytes 是直接机制指标；
- upstream timing 排除了 Server 同步变快；
- 有两组独立 XPC 样本；
- gzip 前后都位于 Server 已经优化的阶段。

它仍不是随机化、同时段的实验。网络路径、连接状态和本机调度可能变化；但协议级机制指标使“主要收益来自 response transfer 减少”具有很高置信度。

## 自动化回归守住什么

daemon 的 gzip 网络边界测试使用本地随机端口 TCP listener，而不是 mock 一个 reqwest 返回值。测试需要证明：

- 请求实际携带 Accept-Encoding: gzip；
- Server 返回 gzip body 后，reqwest 能透明解压；
- 上层读取到原始业务内容；
- 现有状态码、header 处理和 response cache 语义不被破坏。

这是网络边界组件测试，不是香港生产 E2E。它守住“代码确实协商并解压”这一机制；真实 XPC + Caddy 的生产采样才证明跨境收益。

完整测试计数、CI run 与最终镜像由《验证证据台账》维护，避免每篇案例各自复制一份易过期的发布状态。

## 不能从这次实验外推什么

### gzip 没有优化 DNS、TCP、TLS 或 TTFB

轻量 /me 仍约 36–42 ms，说明固定链路成本仍在。gzip 只减少响应体传输量，不会降低 RTT，也不会修复代理 reset。

### 小响应不一定值得压缩

1 KiB 左右响应能减少的绝对字节有限，CPU、header 和实现复杂度可能超过收益。是否压缩应看实际数据分布，而不是统一假设。

### 87.118 ms 不是永久生产 SLO

它是 40 次独立复测的 p95。长期窗口、不同网络、冷连接和高负载仍需持续观测。

### 压缩不能替代字段裁剪和增量同步

62 KiB JSON 被压到约 10 KiB 是成功，但如果业务本来只需要 5 KiB metadata，更优先的修复仍是改变响应数据形状。Server 案例中的 metadata-only 与本篇 gzip 属于两种不同手段：

- 前者减少应用和数据库做的工作；
- 后者减少相同结果的网络传输。

### “Caddy 配了 gzip”不能作为验收

必须验证 client negotiation、Content-Encoding、wire bytes、透明解压和业务语义。任何一环缺失，都不能声称端到端压缩已经生效。

## 可复用的压缩实验清单

- [ ] 选择足够大且文本可压缩的真实 endpoint。
- [ ] 固定 endpoint、身份、数据、build、网络位置和请求方式。
- [ ] 记录 cold/warm connection。
- [ ] 验证 Accept-Encoding 与服务端协商。
- [ ] 保存解码后 body hash 或语义比较。
- [ ] 从代理或 Server 记录 wire bytes。
- [ ] 同时记录 upstream duration，排除后端计算变化。
- [ ] 报告 n、p50、p95、max，不只报告平均值。
- [ ] 做独立复测；跨境场景最好交替 A/B。
- [ ] 验证 cache、header、错误响应和取消语义没有回归。
- [ ] 对小响应和高 CPU 场景单独评估，不全局套结论。

## 追溯

- [PR #206](https://github.com/lilhammerfun/clumsies/pull/206)
- source commit：53fabaf0bcfbaee6aa597768b2a340ea7830bb2b
- squash merge：133fa5a7c784a314dcc85f4da0c613432e08557f
- main CI：32919506973
- Server Delivery：32920318605
- 最终生产镜像：sha256:98454460ba43353a21791bf9fffd708c314a7559bfd1d9394cd4ad63810e03f0

这些构建事实只证明证据截止时的交付状态；其后是否仍为当前生产版本不能由本文推断。未闭环项见[验证证据台账](./evidence-ledger.md)。
