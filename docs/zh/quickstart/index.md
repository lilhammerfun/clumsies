---
title: 快速开始
description: 选择团队知识，让 Codex 使用并提出修改，再由人审阅发布。
prev: false
next:
  text: 创建项目
  link: /zh/quickstart/create-project
---
# 让 Codex 使用并更新团队 Memory

这组教程使用组织中已有的 **部署回滚检查单**，路径为 `deployment-rollback.md`。你会把它选入 **Payments** 项目，让 Codex 在 `clumsies-demo` 中使用它，明确要求 Codex 提出改进，再由人审阅并发布。

最后回到 Codex，重新读取 Memory，确认发布的修改已经可以使用。

## 开始前

**WIP：** 当前 App 获取方式见[获取 App](/zh/quickstart/install)，登录步骤见[连接组织](/zh/quickstart/connect)。先完成这些准备工作，再进入五步教程。

你需要已登录的组织账号、名为 `clumsies-demo` 的练习仓库，以及 Codex。创建项目一篇会说明如何绑定仓库，Codex 一篇会说明托管集成如何把 Memory 提供给任务。

组织中应当已有正式发布的 `deployment-rollback.md`。它是文档使用的示例，**不是产品内置内容**。如果组织中还没有合适的 Memory，先完成[第 1 步：创建项目](/zh/quickstart/create-project)，再按[创建并发布 Memory](/zh/guides/create-memory)准备内容，由有权限的人审阅发布，之后进入第 2 步。选择已有 Memory 是把它的引用加入 Project，不会复制出一份独立文档。

## 每个环节由谁完成

| 操作 | 所需角色 |
| --- | --- |
| 创建 Project | 组织成员；创建者成为该项目的管理员（Project admin） |
| 为项目选择组织 Memory | Project admin，或组织所有者（owner）/管理员（admin） |
| 发布 Review | 组织 owner/admin |

如果需要别人发布 Review，开始最后一步前先确定审查者。在 Codex 中操作不会增加组织权限。

## 按顺序完成五步

| 步骤 | 要做的事 | 应确认的结果 |
| --- | --- | --- |
| 1. [创建项目](/zh/quickstart/create-project) | 创建 Payments，绑定 `clumsies-demo` | 仓库关联到正确的 Project |
| 2. [选择 Memory](/zh/quickstart/select-memory) | 选入组织中已有的检查单 | 项目包含这份共享文档 |
| 3. [让 Codex 使用它](/zh/quickstart/use-with-agent) | 开始一个需要检查单的任务 | Codex 检索并读取相关 Memory |
| 4. [让 Codex 提出修改](/zh/quickstart/update-memory) | 明确要求修改这篇 Memory | Draft 中出现提议的修改，组织正式版本未变 |
| 5. [审阅并发布](/zh/quickstart/review-and-publish) | 人提交 Review，授权审查者发布，再让 Codex 读取 | 共享版本已包含修改，Codex 能检索到它 |

任务中的检索和读取由 Codex 完成，跟随教程不需要手写 MCP 请求。普通编码任务也不等于授权修改团队 Memory；第 4 步会明确提出这项要求。

`deployment-rollback.md` 是 Memory 路径，不需要复制到练习仓库。五步始终使用同一个 Project、仓库和 Memory。

## 完成后应当理解什么

你应能解释：选择 Memory 为什么不等于复制文档，保存 Draft 为什么不等于发布，以及发布为什么需要人的 Review 决定。想了解背后的设计，可以继续读[完整流程](/zh/flows)和[核心数据结构](/zh/data-model)。

现在开始[创建项目](/zh/quickstart/create-project)。如果只想查某项日常操作，可以进入[任务指南](/zh/guides/)。
