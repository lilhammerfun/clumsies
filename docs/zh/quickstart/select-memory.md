---
description: 从 Org 中选取已有的部署回滚检查单，让 Payments 使用这份组织知识。
prev:
  text: 创建项目并绑定仓库
  link: /zh/quickstart/create-project
next:
  text: 让 Codex 使用 Memory
  link: /zh/quickstart/use-with-agent
---

# 2. 选择已有 Memory

组织的 **Org** 视图保存已发布的共享 Memory。每个项目可以选择其中与自己有关的内容；选入之后，项目成员和在绑定仓库中工作的 Codex 才能通过这个项目使用它。

本例把组织中已有的 `deployment-rollback.md` 选入 **Payments**，不需要为每个项目重新写一份检查单。

## 开始前

- 已完成[创建项目并绑定仓库](/zh/quickstart/create-project)，可以在 App 中选择 Payments。
- 组织已发布 `deployment-rollback.md`，其中包含 **发布前先确认上一版可以恢复。**
- 你是 Payments 的 **Project admin**，或组织 **owner/admin**。普通项目成员可以使用管理员已选入的内容；需要增加选择时，请上述管理员完成。

如果组织还没有内容，请管理员先按[创建新的组织 Memory](/zh/guides/create-memory)准备并发布检查单，再回到这里。准备第一批知识是一次单独的工作，不是每个新成员的必经步骤。

## 从 Org 选入 Payments

1. 打开左侧 **Memory**。如果仍停在项目设置，点击 **Project Settings** 齿轮返回内容。
2. 打开顶部项目筛选器，选择 **Org**。
3. 在文件树中找到并打开 `deployment-rollback.md`，确认正文含有 **发布前先确认上一版可以恢复。**
4. 右键点击这个文件，选择 **Add to Project → Add to Payments**。
5. 等待操作完成，再把顶部筛选器切换为 **Payments**。

这里的实际操作名是 **Add to Project**。它修改项目选用哪些组织 Memory 的列表，文件仍是同一份组织知识；这一步不修改正文，也不需要提交 Review。

## 确认完成

在 **Payments** 的文件树中打开 `deployment-rollback.md`，应能读到同一句正文。**Org** 中的原文件也仍然存在。

现在项目已经有了可用的知识。下一步会从绑定的 `clumsies-demo` 仓库启动 Codex，验证它在处理任务时能实际检索到这份 Memory。

## 常见阻碍

- **Org 中找不到文件**：确认当前组织和搜索条件。文件可能尚未发布；项目草稿不会作为已发布文件出现在 Org 中。
- **Add to Payments 灰色或不可用**：需要 Payments 的项目管理员或组织 owner/admin 权限，并且当前没有阻止操作的文档同步。
- **菜单中没有 Payments**：先确认你可以访问该项目。项目管理员可添加成员；如果确实尚未创建，返回上一步。
- **Payments 中仍没有文件**：确认选择操作没有报错，并且顶部筛选器确实是 Payments；有加载错误时先重试。不要新建同名文件代替排查选择结果。

下一步：[让 Codex 使用 Memory](/zh/quickstart/use-with-agent)。
