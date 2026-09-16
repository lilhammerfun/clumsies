---
description: 为没有现成知识的组织准备新的 Memory，通过草稿和人工审阅发布后供项目选用。
prev:
  text: 使用指南
  link: /zh/guides/
next:
  text: 选择已有 Memory
  link: /zh/quickstart/select-memory
---

# 创建新的组织 Memory

当组织还没有合适的知识时，可以先创建并发布一份 Memory，再让各项目选用。本页适合准备初始内容的组织管理员，也适合需要提议新知识的项目成员。

如果 `deployment-rollback.md` 已经存在，请直接[选择已有 Memory](/zh/quickstart/select-memory)。日常使用通常从团队已有的知识开始。

## 开始前

你已登录 App，并可以访问一个项目，例如 **Payments**。如果还没有项目，先[创建项目](/zh/quickstart/create-project)，再回到本页。项目成员可以创建草稿；发布到组织需要 **owner/admin**。如果你是普通成员，请提前安排有权限的同事审阅。

这里创建的是 Clumsies Memory，不会在 `clumsies-demo` 仓库中自动生成同名文件。当前 App 没有与 **Export as ZIP…** 对应的 Memory 导入按钮；少量新内容可以用下面的编辑流程准备。

## 创建并填写草稿

1. 打开 **Memory**，在顶部项目筛选器中选择 **Payments**。若项目设置仍打开，点击 **Project Settings** 齿轮返回内容。
2. 选择 macOS 菜单 **File → New Memory**，或按 **⌘N**。空列表中的 **Propose New Organization Memory** 也可创建草稿。
3. 文件树中出现默认命名的草稿，通常为 `untitled.md`。右键选择 **Rename…**。
4. 在 **Rename Draft** 的 **File name** 中输入 `deployment-rollback.md`，点击 **Rename**。本例使用根目录文件名，不要输入 `/`。
5. 右键文件选择 **Open Source**，或在已打开文件的 **Document View** 中选择 **Source**，将正文替换为：

```markdown
# 部署回滚检查单

发布前先确认上一版可以恢复。

- 发布前：确认自动化检查通过；记录当前版本与回滚步骤。
- 发布后：检查关键页面和错误率；异常时恢复上一版并记录原因。
```

停止输入后，App 会自动保存并在后台同步草稿。可以切换到 **Preview** 检查排版，或用 **Diff** 检查新增内容。

## 提交并发布

1. 等待草稿同步完成，右键文件选择 **Request Review…**。
2. **Title** 填写 `新增部署回滚检查单`，按需补充 **Description**，点击 **Request**。使用创建草稿的账号提交。
3. 在 **Reviews** 中检查新增文件的完整内容。组织 owner/admin 确认后，点击提示为 **Approve and merge this Review** 的勾选按钮，或选择菜单 **Review → Approve**。
4. 等待 Review 状态变为 **Merged**。

更多审阅说明见[审阅并发布修改](/zh/quickstart/review-and-publish)。这里审阅的是新增文件，主流程中的例子则是修改已有文件。

## 确认完成

将 Memory 顶部筛选器切到 **Org**，应能找到 `deployment-rollback.md`，并读到 **发布前先确认上一版可以恢复。**。只在 Payments 草稿中看到正文，尚不能证明发布成功。

## 常见阻碍

- **New Memory 不可用**：先选中一个项目。新知识通过项目承载草稿，Org 视图用于浏览已发布内容。
- **Request Review… 不可用**：等待草稿同步；有同步错误时先处理错误。
- **没有发布权限**：请组织 owner/admin 审阅并发布；项目管理员身份本身不授予这项权限。
- **已经存在同名检查单**：使用已有 Memory；需要补充时按[让 Codex 修改 Memory](/zh/quickstart/update-memory)提出更新。

准备完成后，回到[选择已有 Memory](/zh/quickstart/select-memory)，让需要这份知识的项目选用它。
