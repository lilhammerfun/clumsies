# Clumsies

<p align="center">
  <img src="docs/public/logo.png" width="72" height="72" alt="Clumsies Logo" />
</p>

[English](README.md) · [简体中文](README.zh-CN.md) · [技术文档](https://docs.clumsies.ai/zh/)

[![CI](https://github.com/lilhammerfun/clumsies/actions/workflows/ci.yml/badge.svg)](https://github.com/lilhammerfun/clumsies/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/github/license/lilhammerfun/clumsies?label=License)](LICENSE)

> **WIP — 原生 macOS App 仍在开发中。** GitHub 公开 Releases 提供的是旧版 CLI，不是当前 App 的安装包。请按[安装说明](https://docs.clumsies.ai/zh/quickstart/install)从源码构建并安装 App。

Clumsies 帮助团队维护供 Coding Agent 使用的 Markdown 知识，例如架构决策、项目约束和操作流程。组织保存已发布的 **Memory**；每个 **Project（项目）** 选择需要使用的 Memory，再与本地仓库绑定。Agent 在工作时检索相关指导，也可以在用户明确要求后提出修改。

## 怎样使用

[快速开始](https://docs.clumsies.ai/zh/quickstart/)围绕一条完整流程展开：

1. 创建 Project，绑定实际工作的仓库。
2. 从组织中选择这个项目需要的 Memory。
3. 在启用 Clumsies 集成的 Codex 中工作，由它随任务检索相关 Memory。
4. 约定或流程需要调整时，明确要求 Codex 修改 Memory。
5. 在 Clumsies 中检查生成的 Draft（草稿），由人提交 Review，再由组织 owner 或 admin 批准并发布。

检索不会自动改写 Memory。保存草稿表示提出修改，正式发布需要人工审阅。发布前，草稿就可能影响这个 Project 的本地 Memory 视图。

## 安装并开始使用

App 运行在 **macOS 14 及以上版本**。源码安装需要适配当前系统的完整 Xcode 26 或更新版本、Rust stable、Just 和 XcodeGen；构建所需的 macOS 版本高于 App 的运行要求。环境准备、更新和故障处理统一见[安装说明](https://docs.clumsies.ai/zh/quickstart/install)。

环境就绪后，执行：

```sh
git clone --branch main https://github.com/lilhammerfun/clumsies.git
cd clumsies
just install-macos
```

命令会编译 Debug 配置，将包含 daemon 的完整 App 安装到 **`~/Applications/Clumsies.app`** 并打开。它使用常规应用的持久化账号、Memory 和设置。默认 Server 地址为 **`https://app.clumsies.ai`**，登录需要该组织已经准入的账号；接入其他部署时，使用团队提供的 Server 地址。

接下来[连接组织](https://docs.clumsies.ai/zh/quickstart/connect)，再进入[快速开始](https://docs.clumsies.ai/zh/quickstart/)。

### 让 Agent 帮你安装

```text
请帮我在这台 Mac 安装日常使用的 Clumsies，使用以下仓库的 main 分支：
https://github.com/lilhammerfun/clumsies

按照仓库内 docs/zh/quickstart/install.md 操作。检查所需环境，沿用已有的
可用工具，保留仓库修改，以及 Clumsies 的账号、Memory 和设置。
执行 just install-macos，安装到 ~/Applications/Clumsies.app。
首次安装沿用内置 Server 地址，除非我提供其他地址。
不要创建 Dev Instance，也不要启动本地 Server。

命令失败时说明错误，不要自行换一种安装方式。登录或 macOS 授权需要我
操作时，告诉我具体步骤。确认已安装的 App 能打开后，引导我阅读快速开始。
```

## Agent 支持与使用条件

当前实现包含 macOS Codex App、Claude Code、opencode、DeepSeek Harness（`dsh`）和 Google Antigravity 的集成。Agent 宿主需要自行安装。适配器按本机用户安装一次，供所有项目使用；首次设置默认勾选 Codex。仓库绑定决定 Agent 使用哪个 Project 的 Memory。

修改 Codex 集成后，需要重启 Codex 并新建任务。检索需要本地 daemon 运行、索引就绪；首次使用会下载检索模型。各宿主的具体要求见 [Agent 接入](https://docs.clumsies.ai/zh/guides/agent-runtime)。

团队也可以自行部署 Rust Server 和 PostgreSQL，接入自己的 OIDC 身份提供方，详见[组织部署](https://docs.clumsies.ai/zh/guides/deploy-for-an-org)。

## 了解项目设计

[项目概览](https://docs.clumsies.ai/zh/overview) · [系统架构](https://docs.clumsies.ai/zh/architecture) · [数据模型](https://docs.clumsies.ai/zh/data-model) · [领域接口](https://docs.clumsies.ai/zh/reference/domain-api) · [开发流程](https://docs.clumsies.ai/zh/guides/development-workflow)

## 开源协议

[MIT](LICENSE) © 2026 Clumsies Lab
