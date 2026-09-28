# 附录：调试与分发的签名边界

被 macOS 拒绝的辅助可执行文件会触发重试，看起来像延迟回归。在把这些样本当作性能问题之前，
先诊断身份。

临时签名（ad-hoc）提供本地结构完整性与 designated requirement，
但不提供分发身份。开发、ad-hoc 测试、Developer ID 与 App Store
分发各自有不同的有效要求。

## 诊断

1. 校验 bundle 与嵌套代码结构。
2. 检查 identifier、team 与签名标志。
3. 检查 designated requirement。
4. 只在发布模式需要时检查证书。
5. 回到调用方与辅助程序的边界。

回归检查为每种发布模式保护身份契约；本地调试构建不需要生产证书。