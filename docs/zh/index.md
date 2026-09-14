---
title: 阅读路线
description: 初次了解 Clumsies，从产品概念到架构、核心数据结构、领域接口和完整流程。
---
# 从这里认识 Clumsies

Clumsies 让团队把可复用的知识保存为 **Memory**，供编码 Agent 在任务中查找和使用。修改先成为草稿，由人审阅并发布，团队才会共享新的正式版本。

这套文档面向第一次接触项目的成员、开发者和维护者。你不需要先读代码，也不需要了解 Clumsies 的历史版本。

## 先花半小时建立整体认识

时间是阅读建议，不包含动手操作。以下章节使用同一个“部署回滚清单”示例。

| 顺序 | 阅读内容 | 读完应该能回答 |
| --- | --- | --- |
| 1 · 约 5 分钟 | [认识 Clumsies](/zh/overview) | 产品解决什么问题？Organization、Project、Memory 分别是什么？ |
| 2 · 约 7 分钟 | [系统架构](/zh/architecture) | Desktop、daemon、Server 怎样配合？数据在本地还是服务端？ |
| 3 · 约 10 分钟 | [核心数据结构](/zh/data-model) | 一篇内容、一个 Draft、一个 Commit 有哪些关键字段，如何关联？ |
| 4 · 约 8 分钟 | [完整流程](/zh/flows) | 一次修改如何保存、同步、审阅、发布，再被 Agent 读到？ |

然后打开[领域接口地图](/zh/reference/domain-api)，把每一步对应到 MCP、本地 XPC 或 HTTP 接口。查到不熟悉的名词时，用[术语表](/zh/glossary)补充，不必提前背下所有术语。

## 按当前任务阅读

| 我想做什么 | 从哪里开始 |
| --- | --- |
| 先用起来 | [第一次使用](/zh/guides/how-to-use-clumsies) → [接入 Agent](/zh/guides/agent-runtime) |
| 理解设计和数据 | [系统架构](/zh/architecture) → [数据结构](/zh/data-model) → [Memory 详细设计](/zh/unified-memory-model) |
| 开发调用方或排查接口 | [领域接口](/zh/reference/domain-api) → [MCP](/zh/mcp) / [HTTP 契约](/zh/reference/http-api) |
| 部署和维护组织服务 | [组织部署](/zh/guides/deploy-for-an-org) → [认证与会话](/zh/reference/auth) → [排查问题](/zh/guides/troubleshooting) |
| 修改这个项目的代码 | [代码库地图](/zh/repos) → [本地开发](/zh/guides/development-workflow) |
| 理解为什么慢或查不到内容 | [排查问题](/zh/guides/troubleshooting) → [检索与评测](/zh/retrieval-evaluation) → [性能专题](/zh/performance/) |

## 阅读时先分清三件事

- **本地保存成功：** 编辑已经写到这台设备，网络失败不应让这次已保存的编辑消失。
- **同步成功：** Server 收到了提案，它仍然需要审阅。
- **发布成功：** 授权合并推进了 Organization 的正式版本；各设备随后准备新的可读视图。

这三个状态是理解 Clumsies 数据和接口的起点。详细设计放在侧栏“深入设计”，退役方案放在“维护与历史”；它们都不是入门的前置条件。

开始阅读：[认识 Clumsies](/zh/overview)。
