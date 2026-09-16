# 在仓库中使用 Clumsies

本页是已有工作区成员的任务索引。选择当前需要的环节即可，不必从头按顺序读完。初次使用请进入[快速开始](/zh/quickstart/)。

## 登录组织 {#_1-登录组织}

按照[连接组织](/zh/quickstart/connect)完成登录，确认进入预期的组织。账号准入和凭据规则见[认证与会话](/zh/reference/auth)。

## 选择 Project，绑定仓库 {#_2-选择-project-绑定仓库}

[创建项目](/zh/quickstart/create-project)说明了 Project 和仓库绑定步骤。团队已经分配 Project 时，直接复用它。[Project 选择与绑定](/zh/workspace)解释了共享项目与本地目录的区别。

## 确认所选 Memory {#_3-确认所选-memory}

组织中已经有项目需要的文档时，按照[选择 Memory](/zh/quickstart/select-memory)把它选入项目。Project admin 或组织 owner/admin 可以调整选择集。如果文档还不存在，使用[创建新的 Memory](/zh/guides/create-memory)指南。

## 接入 Agent 宿主 {#_4-接入-agent-宿主}

正常使用 Codex 时，先看[让 Codex 使用 Memory](/zh/quickstart/use-with-agent)。配置或排查宿主连接时，查 [Agent 接入](/zh/guides/agent-runtime)。

## 提出一处修改 {#_5-提出一处修改}

按照[让 Codex 提出修改](/zh/quickstart/update-memory)，明确授权维护 Memory，并检查得到的 Draft。保存或同步提案都不代表发布；[完整流程](/zh/flows)解释了各状态的边界。

## 处理共享更新，提交 Review {#_6-处理共享更新-提交-review}

使用[审阅并发布](/zh/quickstart/review-and-publish)检查当前提案，再提交给人审阅。如果共享内容变化或请求失败，先按照[排查问题](/zh/guides/troubleshooting)确认结果，再决定是否创建另一份提案。

## 由有权限的审查者发布 {#_7-由有权限的审查者发布}

发布由组织 owner/admin 完成。[审阅并发布](/zh/quickstart/review-and-publish)包含审查决定，以及回到 Codex 确认共享内容的步骤。

## 遇到问题时先检查什么？

从[排查问题](/zh/guides/troubleshooting)按可见现象查找。检索或集成失败时，也可以查 [Agent 接入](/zh/guides/agent-runtime)。

## 本地存储与管理

缓存恢复和已保存编辑的边界见[排查问题](/zh/guides/troubleshooting)，本地数据归属见[本地运行时](/zh/runtime)。组织初始化与访问权限分别见[组织部署](/zh/guides/deploy-for-an-org)和[认证与会话](/zh/reference/auth)。

## 实现参考

先用[领域接口地图](/zh/reference/domain-api)确认操作涉及的边界，再通过[代码库地图](/zh/repos)找到实现。[系统架构](/zh/architecture)和[数据模型](/zh/data-model)解释各部分的关系。
