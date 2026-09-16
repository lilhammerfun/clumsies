# DeepSeek Harness integration

Register MCP in the dsh profile's `cordis.patch.yml`. Set `cwd` to a workspace bound to a Project:

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

MCP resolves the Project from this directory and provides `memory.activate`, `load`, and `store`.
Activity reads DSH session logs directly; no event-forwarding plugin is required.

When upgrading, remove the old `clumsies-hook` registration and copied script from your
user-managed profile. App reconciliation removes unchanged owned `.dsh/clumsies.json`
files but does not edit the profile. See [Workspace binding](/guides/workspace-binding).
