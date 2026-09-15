# Agent Adapter

> 文档属性：详细设计型｜L3｜当前权威。

Adapter 是 daemon 管理的宿主集成层，让 Codex、Claude Code、opencode、dsh 与
Antigravity 使用同一套 Clumsies Agent runtime。Codex 使用全局 Clumsies Plugin；其余
宿主使用用户级配置文件集成。两种交付方式都只注册 MCP 与非阻塞生命周期桥，不
创建第二套 Memory 或运行时实现。

## 运行边界

macOS App bundle 只包含一份签名的 Rust 可执行文件：

```text
Clumsies.app/Contents/Resources/clumsiesd
```

launchd 把它作为常驻 daemon 运行；Adapter 把同一绝对路径写入受管配置，并以短进程模式
启动：

```text
clumsiesd mcp serve
clumsiesd mcp serve --host codex --delivery host-plugin
clumsiesd _agent agent-run-event --host codex|claude-code|opencode|dsh|antigravity
clumsiesd _agent agent-run-event --host codex --delivery host-plugin
```

安装器要求规范路径以 `Contents/Resources/clumsiesd` 结尾，校验 macOS code signature，
并在 Adapter manifest 中记录路径与 SHA-256。它不搜索 checkout 构建、`PATH`、环境变量
或复制的 helper。

每个 proxy 在转发 XPC 前比较自身与常驻 daemon 的 Agent runtime protocol revision 和
build identity。替换 App 会更新之后启动的 proxy；若 resident 仍是旧版本，请求会明确
报错并要求重启，而不是混用协议。

## 宿主交付面

| Host | MCP 注册 | 生命周期接入 |
| --- | --- | --- |
| Codex | `clumsies@clumsies-local` 全局 Plugin 注册 pinned MCP | Plugin Hook；仓库内不写文件 |
| Claude Code | `~/.claude.json` 的 `mcpServers.clumsies` | `~/.claude/settings.json` 与受管 shell Hook |
| opencode | `~/.config/opencode/opencode.json` 的 `mcp.clumsies` | `~/.config/opencode/plugins/clumsies.ts` |
| dsh | profile 中单独注册 MCP | `~/.dsh/clumsies.json`；桥接与 MCP 仍需在 profile 注册 |
| Antigravity | `~/.gemini/config/mcp_config.json` 的 `mcpServers.clumsies` | `~/.gemini/config/hooks.json` 与受管 shell Hook |

首次打开 App 时选择要安装的适配器，默认勾选 Codex。之后在 **Settings → Agents**
修改同一组本机配置。适配器按 macOS 用户全局安装，与账号、Server、Project 无关；
项目只绑定本地目录，用于定位 Memory。移除项目绑定不会卸载全局适配器。

`host_agent_adapters` 按 Harness 保存启用状态和受管 manifest；`host_adapter_fs_ops`
复用原有文件事务机制。用户关闭的适配器不会在下一次启动时自动装回来。启用或关闭时，
安装器精确清理各 Server 下 daemon 管理的旧仓库配置；用户改过的内容会报冲突，暂时
不可达的目录保留记录，之后再重试。

所有宿主直接消费 `memory` MCP 工具。Codex Plugin 只附带一个很薄的 bootstrap Skill，
用于说明何时 `memory.activate`、如何加载 Project skill。`skills/**` 中的项目技能仍是普通 Memory，由 bootstrap 在相关时通过
`memory.load` 读取；Adapter 不把它们复制到宿主 skill 目录，也不从 `workflow/` 路径
自动生成可执行 skill。

旧版本安装的宿主原生 `activate` / `ntmd` skill 已退役。历史 Codex Adapter 行仅用于
精确清理旧 `.codex/config.toml`、`.codex/hooks.json` 与受管 Hook 片段；当前 Plugin 安装、
Project 解析和运行路由不依赖这些行。

## Project 解析

Codex Plugin 使用：

```text
mcp serve --host codex --delivery host-plugin
```

`host-plugin` 只声明交付方式，不选择或授权 Project。proxy 启动时从当前目录解析 canonical
binding，并在每次 `tools/call` 前重新解析；缺少绑定、绑定改变或 delivery 不匹配都会
关闭式失败。全局 Plugin 不要求仓库级 Codex Adapter 行，也不回退 Desktop 当前选中项。

所有 MCP proxy 都要求当前目录已有 Project 绑定，并在每次工具调用前重新校验。
未绑定目录不会回退到 App 当前选中的项目。生命周期桥检查本机 Harness 开关，不要求
仓库级 Adapter 记录。

## 生命周期桥

Codex Plugin、Claude Code 与其他受支持宿主把有限的生命周期事件交给 App 内 proxy：

```text
host event
  -> managed Hook / plugin
  -> clumsiesd _agent agent-run-event --host <host>
  -> typed XPC record_agent_run_event
  -> resident daemon AgentRun
```

Codex Plugin Hook 在用户通过 `/hooks` 审查并信任当前 hash 后才运行；Adapter 不绕过这个
宿主信任边界。Plugin 更新不会热加载到已打开 task，更新后应重启 Codex 并新建 task。

当前事件策略：

- Codex 与 Claude Code 记录 root prompt、subagent 与 session 生命周期；Claude Code 还
  记录 `StopFailure`；
- Antigravity 使用 `PreInvocation`；
- opencode 映射用户消息、失败 completion 与 session 删除；
- dsh client bridge 非阻塞转发生命周期；
- 纳管集成都不注册或合成正常 root `Stop`。

proxy 最多读取 1 MiB，并只保留有界 session/turn/agent 标识、父子关系、工作目录、显示
标签、事件类型与 outcome。prompt、transcript、assistant message、tool payload 和原始错误
正文不跨 XPC。桥接失败必须 fail-open，不能阻止宿主继续工作。

root/subagent 事件成功后，桥接只记录有界 AgentRun 遥测，不向宿主注入工作上下文，也不在
正常完成时注入阻塞式提醒。

为了清理旧安装，私有桥仍接受 legacy/手工 root `Stop`，但只记录终止遥测，不能阻塞宿主。
完整映射见 [AgentRun 生命周期](/zh/guides/agent-run-injection)。

## 安装、更新与移除

direct-file Adapter 只合并宿主共享配置中的 Clumsies 段，生成脚本/plugin 文件则由
manifest 独占管理。manifest 记录每个文件的安装 hash；更新会删除当前计划中已退役且
仍与记录一致的文件。

Codex `host_plugin` 使用独立流程：

1. 用户保存适配器选择后，App 只读检查 Codex host、App-owned local marketplace、安装/启用状态与期望版本；
2. 已选中的 Plugin 缺失或过期时，通过签名 Codex CLI 物化 marketplace 并安装/更新 Plugin；
3. **Repair Selected Integrations** 执行同一 reconciliation；
4. 关闭时通过签名 Codex CLI 卸载 Plugin，并保存禁用状态；
5. 不写 `project_agent_adapters` 或仓库文件。

“已安装并启用”只证明 Plugin 交付，不证明 Hook 已受信任或当前 task 已加载新快照。因此
Settings 仍需提示 `/hooks` 信任、Codex 重启和新 task。

App Translocation 下的二进制不能被持久化为 runtime path。Release App 必须先移动到
`/Applications` 或 `~/Applications` 并重新打开，避免临时 quarantine UUID 进入
LaunchAgent 和宿主配置。

所有交付方式遵守以下安全规则：

- 安装拒绝覆盖同名但不归 Clumsies 管理的 MCP entry 或文件；
- 更新用 Adapter revision 做乐观并发保护；
- 可以把已归属的旧 runtime path 迁移到当前 App 内路径；
- 旧 Zig CLI 安装只做有界、只读 manifest 发现，不执行 archive 代码，也不把外部
  manifest 当作原生 ownership；
- archived `repo` scope 集成只报告为不支持，重新安装是显式 ownership handoff；
- 移除只删除精确匹配的受管配置和文件，内容漂移会报告冲突；
- 文件和记录变化写入 journal，中断后由下一次 reconciliation 确定性恢复。

这些限制保证用户自有宿主配置不被静默覆盖，也防止旧 worktree 或 helper 抢占运行时。

## 实现锚点

| 关注点 | 当前路径 |
| --- | --- |
| 全局开关、安装与仓库配置迁移 | `crates/daemon/src/agent_adapter/global.rs` |
| direct-file 安装、合并与 legacy 发现 | `crates/daemon/src/agent_adapter.rs` |
| Codex Plugin 物化和 CLI reconciliation | `crates/daemon/src/agent_adapter/codex_plugin.rs` |
| Codex Plugin 源包 | `packages/clumsies/` |
| MCP / Hook proxy | `crates/daemon/src/main.rs` |
| typed MCP contract | `crates/daemon/src/agent_runtime/mcp_contract.rs` |
| Hook 归一化 | `crates/daemon/src/agent_runtime/hook.rs` |
| Codex Hook 模板 | `packages/clumsies/scripts/agent-run-event.sh.tpl` |
| direct-file Hook 模板 | `assets/adapters/*/runtime/hooks/agent-run-event.sh.tpl` |
| opencode plugin | `assets/adapters/opencode/runtime/plugin.ts` |

退役 Zig Adapter 的最后一份活动源码可从 Git commit
`4b18f7947a977dbc6b62f560b698dc992597f19d` 恢复，不保留在当前构建、安装或运行路径中。
