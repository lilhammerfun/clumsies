# Organization Memory

Organization Memory 是团队正式发布的知识库。一份 Memory 可以被多个 Project 选择，但它的身份和发布历史只维护一套。先读[核心数据模型](/zh/data-model)，可以看到从一份文档到 Review 发布的完整例子。

本页沿用历史地址 `/artifact`。Artifact 和 Hub 是旧产品名称，不是当前系统里的额外对象或服务。

## 组织拥有内容，Project 决定使用范围

假设组织有三份 Memory：编码约定、支付接口说明和部署回滚检查单。Payments 可以选择全部三份，Website 只选择编码约定和部署回滚检查单。这些 Project 引用同一批 Memory ID，不各自复制一份正文。

组织的已发布状态由 Organization Ref 指向的 Commit 记录。Project 的选择会生成各自的 Project Commit；上游选中资源发生变化时，Server 刷新相关 Project 投影，本机再同步新版本。

| 动作 | 改变什么 |
|---|---|
| 在 Project 中选择 / 移除 Memory | Project 的选择与投影 |
| 修改 Memory | 先形成该 Project 携带的 Organization Draft |
| 提交 Review | 把一个或多个 Draft 交给人协调 |
| merge Review | 更新 Organization 发布内容及相关 Project 投影 |
| 重命名 Memory | 发布后改变路径；稳定 ID 不变 |
| delete Memory | 发布后归档当前资源；已有不可变快照保留历史 |

普通成员在授权范围内提出、提交和评论变更。Organization owner/admin 有发布决定权限。Agent 的 `memory.store` 只保存提案，MCP 不提供审批或 merge 工具；具体接口权限见[接口参考](/zh/reference/)。

## 一份 Memory 包含什么

每份 Memory 有稳定 ID、组织内路径、由路径生成的 `name`、摘要 `description`、Markdown 正文和状态。完整字段与可空性见[Memory 字段表](/zh/data-model#memory-身份、路径与正文)。

正文可以表达规则、流程或背景知识，但没有对应的三种系统内容类型。路径和标题供人组织知识，不赋予额外权限，也不会自动把普通文档变成 Agent Host 的可执行 Skill。

摘要当前允许为空，Server merge 还存在未完整持久化 Draft 摘要的限制。阅读有摘要的文档时可以利用它理解内容，但不能假设所有已发布 Memory 都有可靠摘要，见[实现边界](/zh/unified-memory-model#当前实现边界)。

## Bundle：个人收藏的一组 Memory

Bundle 是用户保存在 Server 的 Memory ID 集合，用于归组、发现和复用。例如一个人可以创建“新同事入门”Bundle，其中包含编码约定和部署回滚检查单。

- Memory 不必属于任何 Bundle，也可同时属于多个 Bundle。
- Bundle 调整只改个人集合，不修改正文、资源 ID 或发布状态。
- 收藏进 Bundle 不会自动加入某个 Project 的 Org Selection。
- 删除 Bundle 不会删除其中的 Memory。

数据表为 `personal_bundles` 与 `personal_bundle_items`。Project 选择使用另外两张表，见[数据存储映射](/zh/data-model#数据实际存在哪里)。

继续阅读：[Project](/zh/workspace)、[统一 Memory 设计](/zh/unified-memory-model)。实现依据：[Memory API](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/memory/dto.rs)、[Memory 持久化](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/memory/repository.rs)、[Bundle 持久化](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/bundle/repository.rs)。
