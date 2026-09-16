---
title: 从这里开始
description: 按使用产品、理解设计或开发运维的目标，选择 Clumsies 文档阅读路线。
---
# 从这里认识 Clumsies

Clumsies 为团队维护一组可复用的知识，称为 **Memory**，供编码 Agent 查找和使用。Project 选择当前项目需要的知识；修改先成为 Draft 草稿，经人审阅并发布后，才改变团队共享的正式版本。

下面有三条阅读路线。你可以先了解设计，也可以先完成一次操作，遇到疑问再查原理。

## 我想在团队中使用 Clumsies

从[快速开始](/zh/quickstart/)进入：创建项目、选择已有的组织 Memory、让 Codex 使用它、明确要求 Codex 提出修改，最后由人审阅并发布。

**完成后：** Codex 能使用项目选中的知识，你能分清“提案已保存”和“团队版本已发布”。教程贯穿使用 Payments 项目、`clumsies-demo` 练习仓库和 `deployment-rollback.md`。

如果已经在项目中工作，可以直接打开[任务指南](/zh/guides/)，只查当前要做的事。

## 我想理解项目设计

这条路线适合想了解项目如何实现的读者。先读[认识 Clumsies](/zh/overview)，再按下面的顺序深入：

| 阅读内容 | 回答的问题 |
| --- | --- |
| [系统架构](/zh/architecture) | App、本地 daemon、Server 和 Agent 集成各自负责什么？ |
| [核心数据结构](/zh/data-model) | Memory、Draft、Review 和版本如何表示、怎样关联？ |
| [完整流程](/zh/flows) | 数据如何经过检索、本地修改、同步和发布？ |
| [领域接口地图](/zh/reference/domain-api) | 一个操作跨越哪些边界，由哪个接口处理？ |

**读完后：** 你能说明数据存在哪里、读到的是哪个版本，以及本地保存、同步和发布为什么是不同结果。遇到不熟悉的词，可以查[术语表](/zh/glossary)；侧栏也保留了各子系统的详细设计。

## 我想开发集成、部署服务或修改代码

按你负责的部分选择入口：

| 要做的工作 | 阅读路线 | 预期结果 |
| --- | --- | --- |
| 开发集成 | [领域接口](/zh/reference/domain-api) → [MCP](/zh/mcp) 或 [HTTP](/zh/reference/http-api) | 找到合适的操作、输入、权限和错误处理方式 |
| 维护团队服务 | [组织部署](/zh/guides/deploy-for-an-org) → [认证与会话](/zh/reference/auth) → [排查问题](/zh/guides/troubleshooting) | 配置访问权限，并能定位运行问题 |
| 修改实现 | [代码库地图](/zh/repos) → [开发流程](/zh/guides/development-workflow) | 找到相关源码，运行独立开发实例 |

[接口参考](/zh/reference/)用于查契约，[任务指南](/zh/guides/)用于完成具体工作。历史方案和带日期的性能证据放在“开发与维护”下面。

## 使用 App 前

**WIP：** 原生 App 仍在开发。当前获取方式见[获取 App](/zh/quickstart/install)，登录步骤见[连接组织](/zh/quickstart/connect)。它们是动手教程的准备工作；阅读设计文档不需要先安装 App。
