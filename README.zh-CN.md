# Clumsies

<p align="center">
  <img src="https://raw.githubusercontent.com/lilhammerfun/clumsies/main/docs/public/logo.svg" width="72" height="72" alt="Clumsies Logo" />
</p>

<p align="center">
  <b>面向 Agent 编程的团队协同记忆平台</b><br>
  <i>像管理代码一样，在研发团队与 AI 编程智能体之间共享、评审与沉淀组织级记忆资产。</i>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="https://github.com/lilhammerfun/clumsies/actions/workflows/ci.yml"><img src="https://github.com/lilhammerfun/clumsies/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/lilhammerfun/clumsies/blob/main/LICENSE"><img src="https://img.shields.io/github/license/lilhammerfun/clumsies?label=License" alt="License: MIT"></a>
  <a href="https://github.com/lilhammerfun/clumsies/releases"><img src="https://img.shields.io/github/v/release/lilhammerfun/clumsies?label=Release" alt="Release"></a>
</p>

---

## 时代范式转移

AI 编程智能体（Coding Agents）正在彻底重构软件开发的控制面（Control Plane）。

过去，研发组织只需要通过 Git 仓库管理源代码；而在智能体协同时代，团队必须同时管理**指导 Agent 编写代码的架构规则、业务约束与项目上下文**。

然而，传统的 Agent 记忆往往被困在单次会话或单机本地文件中：它既无法在团队成员之间共享，也无法接受同行评审，更无法跨会话同步。一旦上下文超出窗口，关键的项目约束就会被模型无声丢弃。

**Clumsies 是面向 Agent 编程的团队协同记忆平台。** 它将智能体记忆视作一等公民的版本化组织资产，让人类工程师与自主编程 Agent 能够无缝构建、评审和激活同一套共享大脑。

---

## 核心特性

- **记忆即团队资产（Git 语义上下文管理）**：将架构规则、工作流规范与项目上下文收敛为统一的 Markdown 组织记忆。Project 只选择要使用的组织记忆并承载项目内可见的 Draft overlay；变更经过团队 Review 后原子合入组织 Commit 历史，杜绝静默覆盖。
- **混合检索与按需精准激活**：深度融合 SQLite FTS5 BM25 全文检索、本地向量嵌入、倒数排名融合（RRF）与交叉编码器（Cross-Encoder）重排。Agent 通过 `activate` 按需动态召回最相关切片，避免上下文窗口浪费。
- **MCP + 非阻塞生命周期集成**：原生支持 Google Antigravity、Claude Code、OpenAI Codex、opencode 与 DeepSeek Harness (dsh)，由统一签名的 Rust 守护进程（`clumsiesd`）提供代理。Codex 使用 App 自动维护的用户级 Plugin，项目维护的 Skill 留在 Memory Space 并按需加载；Plugin 变化后需重启 Codex 并新建 task，Plugin Hook 首次运行前还需要用户在 `/hooks` 中审查并信任。纳管适配器不安装正常根 `Stop` Hook；Issue 关闭由可选 skill 或人工维护的工作流显式决定。
- **私有化权威部署**：在自有基础设施中运行 Rust Server 与 PostgreSQL（支持组织 OIDC 鉴权），本地守护进程独立管理高速本地缓存与 XPC 通信。

---

## 适配智能体矩阵

| 智能体宿主 | 协议表面 | 纳管文件 | 支持的生命周期 |
| :--- | :--- | :--- | :--- |
| **Google Antigravity** | MCP + 生命周期 Hook | `.mcp.json`, `.agents/hooks.json` | `PreInvocation`；无根 `Stop` |
| **Claude Code** | MCP + 生命周期 Hook | `.mcp.json`, `.claude/settings.json` | prompt、subagent、失败与会话事件；无根 `Stop` |
| **OpenAI Codex** | Plugin：MCP + Hook + 启动 Skill | App 纳管的用户级 Plugin；无项目文件 | Hook 获得信任后的 prompt、subagent 与会话事件；无根 `Stop` |
| **opencode** | MCP + Plugin | `opencode.json`, `.opencode/plugins/clumsies.ts` | prompt、失败与会话事件；不转发正常根 `Stop` |
| **DeepSeek Harness (dsh)** | MCP + Hook 桥接 | `.dsh/clumsies.json` | prompt、失败与会话事件；不转发正常根 `Stop` |

---

## 快速上手

日常使用推荐从 `main` 源码安装。以下步骤会编译 Debug 版本，将它安装到
**`~/Applications/Clumsies.app`** 并打开。Debug 只是编译配置；安装后使用常规的
Clumsies 应用，长期保留账号、记忆和 Agent 集成。

应用已内置默认服务端地址 `https://app.clumsies.ai`，登录后会自动获取该服务端
配置的组织信息。

### 让 Agent 帮你安装

把下面这段提示词发给你的编程 Agent：

```text
请帮我在这台 Mac 安装日常使用的 Clumsies，仓库地址：
https://github.com/lilhammerfun/clumsies

按照 README 的“从源码安装”步骤，使用 main 分支。先检查并补齐适配当前
macOS 的完整 Xcode 26 或更新版本、Rust stable、Just 和 XcodeGen。沿用
已有可用工具，保留仓库中已有的修改，以及 Clumsies 的账号、记忆和配置。
首次安装沿用应用内置的服务端地址和登录配置。

在仓库根目录执行 just install-macos，将包含内置 daemon 的完整
Debug 应用安装到 ~/Applications/Clumsies.app，并打开这个已安装的应用。
不要创建 Dev Instance，也不要启动本地 Server。某一步失败时，请说明
失败的命令和原因，不要自行换成另一种安装方式。登录或 macOS 授权需要
我操作时，请告诉我具体步骤。

完成后确认已安装的应用能够打开，再引导我登录组织、把实际工作的仓库
绑定到 Project，并接入我使用的 Agent。
```

### 从源码安装

1. 安装**完整的 Xcode 26 或更新版本**，按照
   [Apple 的系统要求](https://developer.apple.com/cn/xcode/system-requirements/)
   选择适配当前 macOS 的版本。首次打开 Xcode，接受许可协议并安装所需组件。
   然后选择这套工具链（若安装位置不同，请调整路径）：

   ```sh
   sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
   sudo xcodebuild -runFirstLaunch
   xcodebuild -version
   ```

2. 如果尚未安装 [Homebrew](https://brew.sh/)，先完成安装，再安装
   [Just](https://formulae.brew.sh/formula/just)、
   [XcodeGen](https://formulae.brew.sh/formula/xcodegen) 和
   [Rust](https://formulae.brew.sh/formula/rust)：

   ```sh
   brew install just xcodegen rust
   ```

   已有可用的 Rust stable 工具链时，可从命令中去掉 `rust`。
   确保当前终端能直接运行 `cargo` 和 `rustc`。

3. 编译并安装应用：

   ```sh
   git clone --branch main https://github.com/lilhammerfun/clumsies.git
   cd clumsies
   just install-macos
   ```

   首次构建需要下载依赖，可能耗时数分钟。命令会编译应用及其内置 daemon、
   验证签名、安装到 `~/Applications/Clumsies.app`，然后打开应用。
   覆盖已有应用时会保留账号、记忆和配置。

### 登录并接入 Agent

1. 打开应用，保留预填的 **Server address**，点击 **Continue in Browser**
   完成登录。应用会自动加载组织信息。
2. 选择 Project，通过 **Repositories → Add Repositories…** 添加你实际工作的仓库。
3. 打开 **Settings → Agent**。Codex 由应用自动维护；按需启用其他支持的 Agent。
   使用 Codex 时，等待状态显示 **Ready**，重启 Codex，在绑定的仓库中新建任务，
   然后通过 `/hooks` 审查 Clumsies Hook。
4. 等待首次模型下载和记忆索引完成后，可以这样提问：

   ```text
   请通过 Clumsies 读取这个仓库对应项目的记忆，概括当前任务应遵守的约定。
   ```

完整使用流程见[使用指南](https://docs.clumsies.ai/zh/guides/how-to-use-clumsies)。
如果登录或 Project 访问被拒绝，再联系组织管理员处理权限。
只有接入其他部署时，才需要修改服务端地址。

后续更新时，在这份 `main` 分支的仓库目录中执行：

```sh
git pull --ff-only
just install-macos
```

---

## 技术文档

完整技术文档请访问 [docs.clumsies.ai](https://docs.clumsies.ai)。

---

## 开源协议

[MIT License](LICENSE) © 2026 Clumsies Lab
