# Agent 运行时

Agent host 通过 App 内同一份签名的 Rust `clumsiesd` 使用两个短进程入口：

```text
clumsiesd mcp serve
clumsiesd _agent agent-run-event --host <host>
```

- launchd 常驻模式独占数据库、模型、同步与检索 worker。
- MCP / Hook proxy 启动时校验常驻 daemon 的协议版本和 build identity，再通过 XPC 转发。
- Adapter 固定使用 App 内路径，不搜索 `PATH`、worktree 构建产物或旧 helper。
- MCP 只提供 `memory` 工具，包含 `activate` / `load` / `store` 操作。
- AgentRun 只记录 lifecycle，不向 Agent 注入工作管理协议。

## 本机适配器选择

首次启动时选择 Harness，默认勾选 Codex；之后在 Settings → Agents 修改。
适配器全局安装，项目仅绑定本地目录。所有 MCP 入口都要求目录已有绑定，
每次工具调用都会复核绑定，未绑定时不会读取 App 当前选中的项目。
