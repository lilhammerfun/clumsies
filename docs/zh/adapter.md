# Adapter

> 文档属性：详细设计型｜L3｜当前权威。

Adapter 是 daemon 管理的宿主集成层，让 Codex、Claude Code、
opencode、dsh 与 Antigravity 在同一套 Clumsies Agent
runtime 上可用。每个 Harness 针对本机 macOS 用户配置一次：
Codex 使用 Clumsies Plugin，其余宿主使用各自的用户级配置文件。
两种交付方式都提供 MCP 注册与 Memory 指引，
不会创建第二份 memory 或 runtime 实现。

## 运行边界

macOS App bundle 只包含一份签名的 Rust 可执行文件：

```text
Clumsies.app/Contents/Resources/clumsiesd
```

launchd 把它作为常驻 daemon 运行；
Adapter 把同一 App 内绝对路径写入每个受管 MCP entry，
并以两种短进程 proxy 模式之一启动：

```text
clumsiesd mcp serve
clumsiesd mcp serve --host codex --delivery host-plugin
```

安装器要求规范路径以 `Contents/Resources/clumsiesd` 结尾，
校验 macOS code signature，
并在 Adapter manifest 中记录路径与 SHA-256。
它不搜索 checkout 构建、`PATH`、环境变量
或复制的 helper。

每个 proxy 在转发 XPC 前比较自身与常驻 daemon 的 Agent
runtime protocol revision 和
build identity。替换 App 会更新之后启动的 proxy；
若 resident 仍是旧版本，请求会明确
报错并要求重启，而不是混用协议。

## 宿主交付面

| Host | MCP 注册 |
| --- | --- |
| Codex | 全局 `clumsies@clumsies-local` Plugin，包含启动 Skill |
| Claude Code | `~/.claude.json` → `mcpServers.clumsies` |
| opencode | `~/.config/opencode/opencode.json` → `mcp.clumsies` |
| dsh | 用户维护的 profile 中的 MCP entry |
| Antigravity | `~/.gemini/config/mcp_config.json` → `mcpServers.clumsies` |

首次打开 App 时会出现 Harness 选择界面，默认勾选 Codex。
之后在 **Settings → Agents**
修改同一组选择。这些选择属于本机 macOS 用户，与登录账号、
Server 或 Project 无关。
App 在后续启动时会重新核对已保存的选择，退出登录时也一样；被关闭的集成在 App 更新后
仍保持关闭。

这些位置都是各 Harness 文档中的用户级位置：
[Claude Code](https://code.claude.com/docs/en/settings)、
[opencode](https://opencode.ai/docs/config/) 和
[Antigravity](https://antigravity.google/docs/mcp)。
不会写入新的仓库配置；仓库绑定只用于确定使用哪个 Project 的 Memory，
移除绑定不会卸载任何全局 Adapter。dsh 的 profile 设置见
[dsh integration](/zh/guides/dsh-integration)。

所有宿主直接消费 `memory` MCP 工具。
Codex Plugin 附带一个轻量的 `project-memory`
Skill，引导 Agent 在分析、规划或实现前先查阅项目 Memory，
即使用户没有提到 Clumsies。
Clumsies 与宿主原生记忆互补：Agent 遵循适用的宿主记忆政策，
同时查询 Clumsies，
即使已经查阅过宿主记忆。Clumsies 记忆的维护遵循绑定项目的 Memory
Guidelines。
项目维护的 `coding` 等技能只是 Memory Space 中的普通资源：
bootstrap 在相关时通过
`memory.load` 加载它们，绝不复制或安装到宿主 skill 目录。

旧版 Codex 和 Claude Code 安装的、
与 Clumsies 无关的宿主原生 `activate` / `ntmd`
skill 已退役。历史 Codex Adapter 行保留足够的归属元数据，
可在该仓库被移除时精确清理
遗留的 `.codex/config.toml`、
`.codex/hooks.json` 与受管 Hook 片段。direct-file 更新
路径同样会删除此前受管的退役 skill 文件，不触碰用户自有内容。

Codex Plugin 以
`mcp serve --host codex --delivery host-plugin`
启动固定路径的二进制。
`host-plugin` 只标识全局 Plugin 交付方式，不选择或授权 Project。
daemon 在启动时以及
每次 `tools/call` 前解析仓库的 canonical Project
binding，并要求它保持同一个 Project；
绑定缺失或改变都会关闭式失败，不会转去查 Codex 项目 Adapter 行。
所有 MCP proxy 都要求
启动时和每次工具调用前存在 canonical 仓库绑定；未绑定目录绝不使用 App 当前选中的
Project。运行路由见
[工作目录绑定](/zh/guides/workspace-binding)。

## 安装、更新与移除

direct-file Adapter 只合并宿主共享配置中的 Clumsies 段，
生成脚本/plugin 文件则由
manifest 独占管理。manifest 记录每个文件的安装 hash；
更新会删除当前计划中已退役且
仍与记录一致的文件。

Codex 使用独立的 `host_plugin` 交付方式。用户保存 Harness 选择后，
App 会检查 Codex
host、App-owned local marketplace、
安装/启用状态与期望 Plugin 版本；已选中 Harness 的
受管状态缺失或过期时，通过签名 Codex CLI 完成 reconciliation。
检查是只读的；自动
reconciliation 与 **Settings → Agents** 中的
**Repair Selected Integrations** 会物化
marketplace 并安装或更新 Plugin。
两种操作都不写 `project_agent_adapters` 行或仓库文件。
关闭 Codex 时通过签名 CLI 卸载 Clumsies Plugin，
已保存的禁用状态会阻止重新安装。

Plugin 更新后需要重启 Codex 并新建 task。
reconciliation 会清理已退役的受管生命周期
脚本和注册，保留外部配置，并报告被修改文件的冲突。

App Translocation 下的二进制不能被持久化为 runtime path。
Release App 必须先移动到
`/Applications` 或 `~/Applications` 并重新打开，
避免临时 quarantine UUID 进入
LaunchAgent 和宿主配置。

所有交付方式遵守以下安全规则：

- 安装拒绝覆盖无关的 MCP entry 或未受管文件；
- 更新用 Adapter record revision 做乐观并发保护；
- 可以把此前受管的 runtime path 迁移到当前 App 内路径；
- 旧 Zig CLI 直接创建的安装只做只读发现并保持原样。
  缺失的 workspace 保持 pending，
  可达的安装报告可操作的“复查并重装”警告。检查是尽力而为的，有较短的 App 侧截止
  时间，绝不阻塞 daemon 管理的集成的 reconciliation，
  退出登录或离线时也一样。
  它们的外部 manifest 不作为原生 ownership 证明；
- 归档的 `repo` scope generation 报告为不支持。
  请移除它们旧的 Clumsies MCP/Hook
  entry；App 自有的全局 Codex Plugin 取代仓库级 Codex 集成，
  其他宿主也使用用户级
  配置；
- 从 App 重新安装是显式的交接：原生安装器拒绝外部或漂移的 entry，不会静默接管；
- 移除只删除精确匹配的受管 entry 和文件，内容漂移报告冲突而不是覆盖；
- 文件和记录变化写入 journal，中断的安装或迁移由下一次 reconciliation
  确定性恢复。

用户级选择与 manifest 保存在 `host_agent_adapters` 中，
只按 Harness 区分。direct-file
改动写入 `host_adapter_fs_ops`，复用已有的带校验文件 journal。
启用或停用某个 Harness
会清理它在已知 Server 上由 daemon 管理的仓库配置；内容有变化的文件报告冲突，
不可达的
仓库保留记录并在后续启动时重试。归档的 Zig manifest 只用于检查，绝不构成归属证明。

这些限制保证用户自有宿主配置不被静默覆盖，也防止旧 worktree 或 helper
抢占运行时。

## 实现锚点

| 关注点 | 当前路径 |
| --- | --- |
| 全局开关、安装与仓库配置迁移 | `crates/clumsiesd/src/agent_adapter/global.rs` |
| direct-file 安装、合并与 legacy 发现 | `crates/clumsiesd/src/agent_adapter.rs` |
| Codex Plugin 物化和 CLI reconciliation | `crates/clumsiesd/src/agent_adapter/codex_plugin.rs` |
| Codex Plugin 源包 | `packages/clumsies/` |
| MCP proxy | `crates/clumsiesd/src/main.rs` |
| typed MCP contract | `crates/clumsiesd/src/agent_runtime/mcp_contract.rs` |

退役的 Zig Adapter 实现仍可从 Git commit
`4b18f7947a977dbc6b62f560b698dc992597f19d` 恢复；
它不作为安装或兼容路径存在或执行。
原生 daemon 只包含一个有界的、只读的 manifest 发现过程，绝不运行退役代码，
也不把归档
manifest 当成归属数据库。
