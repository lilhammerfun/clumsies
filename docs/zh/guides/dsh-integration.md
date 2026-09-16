# DeepSeek Harness 集成

在 dsh profile 的 `cordis.patch.yml` 注册 MCP，`cwd` 必须指向已绑定 Project 的工作目录：

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
```

MCP 根据工作目录解析 Project，提供 `memory.activate`、`load` 和 `store`。
Activity 直接读取 DSH 的会话日志；不需要事件转发插件。

升级时，在用户维护的 profile 中移除旧 `clumsies-hook` 注册项及其复制的脚本。
App 会清理清单中仍未被修改的旧 `.dsh/clumsies.json`，不会改写用户的 profile。
详见[工作目录绑定](/zh/guides/workspace-binding)。
