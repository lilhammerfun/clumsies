# 附录：本地 Debug 与正式分发的签名边界

| 文档属性 | 取值 |
|---|---|
| 文档角色 | 旁路工程纠错 |
| 关联实现 | [PR #205](https://github.com/lilhammerfun/clumsies/pull/205) |
| 关联专题 | [端到端延迟优化专题](./index.md) |
| 证据截止 | 2026-08-26 |

> 这次签名告警与性能根因无关。把它单独成篇，是为了避免再次用正式分发身份解释本地 Debug 构建，或在只读发现边界验证一个根本不会被使用的 runtime。现场签名输出与交付身份均为截至 2026-08-26 的证据；只读检查忽略 runtime、写边界继续验签的代码契约已于 2026-08-29 交叉核对。

## 现场事实

证据采集时安装的 Debug runtime 实测：

~~~text
Signature=adhoc
TeamIdentifier=not set
Identifier=ai.clumsies.daemon
designated requirement: identifier "ai.clumsies.daemon"
codesign --verify --strict: passed
~~~

没有创建或导入 Developer ID 证书。

这些事实可以同时成立：

- runtime 是 ad-hoc 签名；
- TeamIdentifier 为空；
- identifier 与 designated requirement 稳定；
- codesign 结构校验通过；
- 本地 Debug App 和 daemon 可以正常运行。

因此，“TeamIdentifier=not set”不能被自动翻译成“忘记 codesign”，更不能被翻译成“本地必须申请 Developer ID”。

## 真正的缺陷发生在哪里

报错来自 archived Zig CLI integration store 的只读检查。

旧逻辑在扫描归档清单之前：

1. 接收 bundled runtime path；
2. canonicalize 该路径；
3. 对 runtime 做 release signing verification；
4. 然后执行 archived store inspection；
5. inspection 过程中并不使用这个 runtime 去执行、安装或更新任何内容。

这是一种边界错位：只读发现流程验证了一个不会被消费的对象。路径不存在、签名是 ad-hoc 或 team 不满足 Release 规则时，流程会把无关状态包装成：

~~~text
project_agent_adapter_invalid_runtime:
The bundled Agent runtime does not have the required release signing identity
~~~

错误文本看似明确，实际把“legacy 只读扫描耦合了无用 runtime”误诊成“本地 runtime 缺少正式证书”。

## PR #205 如何修复

PR #205 将 archived Zig read-only inspection 与 runtime signing 解耦：

- 只读扫描不再 canonicalize 或验证 bundled runtime；
- 相关 request 字段仅为新旧 App/daemon wire compatibility 保留并明确忽略；
- missing runtime 也能完成 legacy inspection 的回归测试，防止耦合重新出现；
- macOS 提示、测试、README 与 OpenAPI 不再把 Debug ad-hoc 和正式分发签名混为一谈。

修复没有放宽真正的写边界。managed integration 的安装或更新仍验证实际将被消费的 App/runtime：

- bundle 路径；
- identifier；
- team 匹配；
- Release 场景所需的非 ad-hoc 与 hardened runtime 等约束。

原则不是“少做安全检查”，而是：

> 只在真正消费受保护对象的边界验证该对象，并让验证强度匹配操作风险。

## 四种场景不能混用同一规则

| 场景 | 是否消费 runtime | 正确验证 |
|---|---|---|
| archived Zig store 只读发现 | 否 | 验证清单可读性与格式；不要求无用 runtime |
| 本地 Debug App/daemon | 是，本地开发运行 | ad-hoc + 稳定 identifier/designated requirement；codesign 结构有效 |
| managed install/update | 是，会改变受管集成 | 对实际 bundle/runtime 做 identifier、team、hardened-runtime 等严格校验 |
| 正式 macOS 分发 | 是，对外发布 | Developer ID、hardened runtime、notarization、staple、Sparkle 等完整发布门禁 |

最后一行是分发流水线的职责。runtime validator 的某个函数没有直接检查 Apple anchor、notarization 或 staple，不代表正式发布可以省略这些步骤；同样，正式发布需要 Developer ID，也不代表本地 Debug 必须具备它。

## 为什么 ad-hoc 仍然叫“签名”

macOS code signing 不只表示“Apple 认可的开发者身份”。ad-hoc signing 仍会生成 CodeDirectory，并允许系统验证代码结构和 designated requirement；它只是没有可验证的开发者证书链和 Team ID。

所以应区分：

- **unsigned / 签名结构无效**；
- **ad-hoc signed**；
- **由具体 Apple Team 签名**；
- **Developer ID 分发签名**；
- **已 notarize 并 staple 的分发产物**。

把这五种状态压成“签了/没签”会直接制造错误修复方向。

## 推荐的排查顺序

### 1. 验证结构

~~~text
codesign --verify --strict /path/to/clumsiesd
~~~

通过表示签名结构自洽，不表示存在 Developer ID。

### 2. 查看 identifier、team 和 flags

~~~text
codesign -dvvv /path/to/clumsiesd
~~~

重点区分 Signature、Identifier、TeamIdentifier 和 hardened runtime flags。

### 3. 查看 designated requirement

~~~text
codesign -dr - /path/to/clumsiesd
~~~

该 Debug runtime 样本的 designated requirement 是 identifier ai.clumsies.daemon。

### 4. 只在需要正式身份时查证书

~~~text
security find-identity -v -p codesigning
~~~

本地没有 Developer ID 只说明无法在此处直接产出正式分发候选，不说明 ad-hoc Debug runtime 无效。

### 5. 回到调用边界

如果一个操作只是读取归档清单，应先问：

- 它会执行这个 runtime 吗？
- 它会把 runtime 安装到受管位置吗？
- 它会更新外部集成吗？
- 签名结果是否真的被后续逻辑使用？

四个答案都是否时，release signing verification 很可能放错了边界。

## 回归应守住什么

只读 legacy inspection 的关键回归不是“给测试 runtime 做一个更强证书”，而是：

- 传入不存在的 runtime path；
- archived store 仍能扫描；
- scanned、deferred、conflicts 按清单事实返回；
- 不产生 invalid_runtime；
- managed install/update 的安全测试继续严格失败于错误 identifier、team 或 Release 签名。

这同时证明“读边界被解耦”和“写边界没有降级”。

## 与性能专题的关系

签名误诊不应进入性能根因链。它没有解释 Reviews 的 2.5 s upstream、17.765 s first-ready，也没有解释 62,632 B 未压缩传输。

它值得作为旁路教训保留，是因为诊断方法相同：

1. 明确操作真正消费什么对象；
2. 找到错误发生的边界；
3. 检查验证结果是否被后续使用；
4. 不根据报错文案直接跳到证书、代理或网络结论；
5. 修复边界，而不是绕过真正需要的安全检查。

## 追溯

- PR #205：https://github.com/lilhammerfun/clumsies/pull/205
- main commit：408de49048021a3fdf53622157f519a88b50392d
- main CI：32868219409
- Server Delivery：32869884040
- 中间生产镜像：sha256:4c747a1e64afc915cb12733767a73d0b0646394a4e76fff14219ec57242244d1

证据截止时的最终性能组合版本和生产 digest 见[验证证据台账](./evidence-ledger.md)；本文不声明其后仍在生产运行。
