# 使用指南

先选择你要完成的事情。指南负责讲清操作步骤；架构和参考页解释这些操作背后的设计。

## 第一次了解这个项目？

建议按下面的顺序阅读，再进入源码：

1. [认识 Clumsies](/zh/overview)：解决什么问题，六个核心概念是什么。
2. [系统架构](/zh/architecture)：Desktop、daemon、Server 和 Agent 集成分别运行在哪里。
3. [数据模型](/zh/data-model)：Memory、Draft、Review 和版本快照怎样关联。
4. [完整流程](/zh/flows)：跟随部署回滚检查单，从检索一直走到发布。
5. [领域接口地图](/zh/reference/domain-api)：把产品操作对应到 MCP、XPC 和 HTTP。
6. [代码库地图](/zh/repos)：按你的问题选择实现入口。

如果想先用起来，直接阅读下面的成员使用流程，遇到设计问题再回到上述页面。

## 选择要完成的任务

| 我想…… | 对应指南 | 完成后的结果 |
| --- | --- | --- |
| 在仓库中使用 Memory 并提出修改 | [成员使用流程](/zh/guides/how-to-use-clumsies) | 绑定仓库，准备可提交 Review 的 Draft |
| 为团队部署 Server | [组织部署](/zh/guides/deploy-for-an-org) | 配置服务并建立首位 owner |
| 接入 Agent 宿主 | [Agent 运行时](/zh/guides/agent-runtime) | 让宿主通过托管集成访问常驻 daemon |
| 理解 Agent 生命周期事件 | [AgentRun 生命周期](/zh/guides/agent-run-injection) | 知道记录哪些事件，以及事件的作用 |
| 接入 DeepSeek Harness | [DSH 集成](/zh/guides/dsh-integration) | 注册 MCP 并转发生命周期事件 |
| 在本地开发 Clumsies | [开发流程](/zh/guides/development-workflow) | 建立隔离的 worktree 和 Dev Instance |
| 理解缓存和 Draft 叠加 | [Memory 存储边界](/zh/guides/rule-store-unification) | 区分正式来源与可重建数据 |

[归档 CLI 页面](/zh/guides/cli-commands)解释历史命令，不是当前的上手路径。

## 哪些步骤需要管理员？

有 Project 访问权限的成员可以使用其 Memory、创建自己的 Draft、提交 Review，并参与权限范围内的 Review 讨论。组织 owner/admin 管理 Project 和组织 Memory 选择集，并授权发布。安装 Agent 集成不会增加 Server 权限。

[成员指南](/zh/guides/how-to-use-clumsies)会展示这些角色如何完成同一流程；[领域接口地图](/zh/reference/domain-api)说明权限在哪些边界生效。
