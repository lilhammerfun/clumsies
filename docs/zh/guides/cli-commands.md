# 已归档的 Zig CLI

独立的 `clumsies` CLI 不是当前的产品面或运行时面。
它最后一次活跃的源码仍可从 Git 提交
`4b18f7947a977dbc6b62f560b698dc992597f19d` 恢复，
并且不参与构建、测试、打包、安装或发布。

当前的人工工作流使用 macOS Desktop。
受支持的 Agent 宿主在 **Settings → Agent** 中接入 App 内置的
Rust 运行时：

```text
clumsiesd mcp serve
```

这些是由适配器管理的 proxy 模式，不是通用 CLI 的替代品。
见 [Agent runtime](/zh/guides/agent-runtime) 与
[Adapter](/zh/adapter)。