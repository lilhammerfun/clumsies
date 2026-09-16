# 接入编码 Agent

本篇介绍怎样在 Mac 上启用或修复 Agent 集成。想跟着实例走完整流程，请读[在 Codex 中使用项目记忆](/zh/quickstart/use-with-agent)。

集成让 Agent 能调用 Clumsies 工具，仓库绑定决定这些工具可以访问哪个 Project。两项都完成后，Agent 才能使用项目知识。

## 开始前

- 打开 Clumsies，登录组织。
- 安装需要使用的 Agent 宿主。Codex 集成需要 macOS Codex App。
- [把仓库绑定到 Project](/zh/quickstart/create-project)，再[选择项目需要的组织 Memory](/zh/quickstart/select-memory)。

## 启用集成

打开 **Settings → Agents**，在 **Agents on This Mac** 中勾选要使用的宿主。Clumsies 为当前 macOS 用户安装集成，各个 Project 共用，无需重复安装。

首次启动时，**Connect Your Agents** 提供相同的选择，默认勾选 Codex。点击 **Install and Continue** 完成设置，也可以选 **Set Up Later**，以后再从 Settings 进入。

Codex 下方应显示 **Plugin installed and enabled**。如果显示 **Will install when Codex is available**，先安装 Codex App；如果显示 **Plugin needs repair** 或 **Plugin not installed**，点击 **Repair Selected Integrations**，再检查状态。

## 开始一个新任务

安装或更新 Codex 插件后，重启 Codex，从绑定的仓库开始新任务。已有任务仍使用之前的插件快照。

Activity 直接读取 Codex 和 DSH 会话日志，将 Memory 调用关联到本地检索历史，不需要生命周期 Hook。

Clumsies 会在任务启动时和每次工具调用时检查目录绑定。未绑定目录不能使用 App 窗口里当前选中的 Project。Git worktree 可以通过主仓库的绑定解析 Project。MCP 连接保留启动目录，Shell 中的 `cd` 不会切换它。路由和升级细节见[工作目录绑定](/zh/guides/workspace-binding)。

## 确认连接成功

针对 Project 已选中的一篇文档，向 Agent 提出具体问题。检查 Clumsies **memory** 工具的实际结果，确认来源路径和内容符合预期。

集成说明会要求 Codex 在开始实质工作时检索相关知识。如果没有发生检索，可以明确请它使用 Clumsies，再检查工具调用。仅有“已使用记忆”的回答不足以证明连接成功。[Codex 教程](/zh/quickstart/use-with-agent)提供了示例和预期结果。

首次使用会下载本地检索模型，并准备 Project 索引。准备期间可能返回 `search_model_preparing`；等待完成后重试。模型会保存在本机，供后续复用。

## 连接失败时

| 现象 | 检查方法 |
| --- | --- |
| Codex 任务没有 Clumsies 工具 | 检查插件状态，重启 Codex，再创建新任务。 |
| 提示仓库未绑定 | 把当前任务实际使用的目录绑定到目标 Project。 |
| 更新后提示运行时版本不匹配 | 重启 Clumsies 和 Agent 宿主，让 App 内运行时与常驻 daemon 使用同一版本。 |
| 检索仍在准备，或找不到某篇文档 | 检查模型准备、同步状态，以及 Project 是否选中了该 Memory。 |
| 检索成功，但没有活动记录 | 检查绑定目录是否有受支持的 Codex 或 DSH 会话日志，且其中包含 Clumsies 检索调用。 |

仍然失败时，按[排查问题](/zh/guides/troubleshooting)收集对应诊断信息。

## 其他宿主

Settings 也提供 Claude Code、opencode、Antigravity 和 dsh。dsh 显示 **Enabled; MCP profile setup required** 时，还需要在用户维护的 profile 中注册 MCP，详见 [DSH 集成](/zh/guides/dsh-integration)。

[适配器参考](/zh/adapter)列出了各宿主的 MCP 配置和 profile 要求；维护集成时查阅该页。Memory 工具本身的契约见 [MCP 参考](/zh/mcp)。
