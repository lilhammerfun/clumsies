# DeepSeek Harness 集成

DeepSeek Harness 通过宿主 Adapter 接入统一的 clumsies runtime，不拥有独立的 Memory、Draft 或 Issue 语义。安装与升级必须使用当前发布的签名 runtime，并保持工具 schema 与 Server 合同一致。

通用边界见 [Adapter](/zh/adapter)，Agent 执行语义见 [Agent 运行时](/zh/guides/agent-runtime)。

## 本机安装

首次启动时勾选 dsh，或在 **Settings → Agents** 中启用。App 安装
`~/.dsh/clumsies.json`，不向项目仓库写配置。
本机配置只记录签名 runtime 路径，项目由每个会话的工作目录绑定决定。

先把 `dev/dsh/clumsies-hook.mjs` 复制到 `~/.dsh/clumsies-hook.mjs`；从旧版迁移时更新这份桥接。
仍需在 dsh profile 的 `cordis.patch.yml` 注册 MCP 和生命周期桥，使用自己的绝对路径：

```yaml
- insert:
    - id: mcp-clumsies
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: clumsies
        transport: stdio
        command: /Users/your-name/Applications/Clumsies.app/Contents/Resources/clumsiesd
        args: [mcp, serve]
        cwd: /path/to/bound/repository
    - id: clumsies-hook
      name: /Users/your-name/.dsh/clumsies-hook.mjs
```

App 不覆盖 dsh profile。关闭适配器会移除受管本机配置，单独注册的桥接随后停止转发。旧仓库配置仅在确认归 daemon 管理且未被修改时清理；不可达目录稍后重试。
