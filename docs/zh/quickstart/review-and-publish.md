---
description: 在 App 中提交 Codex 修改后的草稿，由人检查差异并发布到项目 Memory。
prev:
  text: 让 Codex 修改 Memory
  link: /zh/quickstart/update-memory
next:
  text: 继续阅读使用指南
  link: /zh/guides/
---

# 5. 审阅并发布修改

上一页让 Codex 为 `deployment-rollback.md` 补充了回滚后的验证要求。这一页把草稿提交为 **Review**，由人检查差异，再发布到项目的共享 Memory。

Review 是 Clumsies 内的变更审阅单，作用类似代码 PR。这里提交的是 Memory 的变更，不会因此创建 GitHub Pull Request。当前 `memory` 工具可以读取和保存草稿；提交 Review、审阅和合并需要在 App 中完成。

## 开始前

- 已完成[让 Codex 修改 Memory](/zh/quickstart/update-memory)，Payments 中出现了这份文件的修改草稿。
- App 使用的是创建这份草稿的账号，并且草稿已同步到 Server。Codex 保存成功只说明草稿已持久化并排队同步，不等于发布完成。
- 由 Payments **owner/admin** 完成项目发布。可选的 Org 贡献由 Org owner/admin 独立审阅。

## 核对草稿，再提交 Review

1. 打开 **Memory**，将顶部项目筛选器设为 **Payments**。
2. 打开 `deployment-rollback.md`，在 **Document View** 中选择 **Diff**，检查这次修改。
3. 确认原文保留，新增句子正确。下面仅列出本例要核对的句子，原有其他检查条目也应保留。

修改前：

```text
发布前先确认上一版可以恢复。
```

修改后：

```text
发布前先确认上一版可以恢复。
回滚后验证健康检查和关键业务请求，并记录结果。
```

4. 右键文件，选择 **Request Review…**。也可在打开文件后，从右上角 **Memory Actions** 菜单选择 **Request Review**。
5. 在 **Title** 填写 `补充回滚后的验证要求`，在 **Description** 简述为什么要验证健康检查和关键业务请求。
6. 点击 **Request**。成功后，App 会切换到 **Reviews** 并打开这条审阅单。

如果出现共享变更或冲突对比，先检查别人已经发布的内容与自己的草稿如何合并，确认最终结果后再继续。

## 由人审阅并发布

审阅者在 **Reviews** 打开这条 Review，等待详情加载完成，然后核对文件列表、差异和说明。确保新增的是回滚验证要求，原有检查单没有被无意删除或替换。

确认可以发布后，项目 owner/admin 点击工具栏的勾选按钮，提示文字为 **Approve and merge this Review**；也可以使用 macOS 菜单 **Review → Approve**。

**当前 App 中的 Approve 会直接执行批准并合并。** 成功后状态变为 **Merged**，本次修改进入项目正式内容。如果你没有项目发布权限，请由有权限的同事完成这一步。仅保存草稿、提交 Review 或在项目中读到新句子，都不能证明已经发布。

## 确认完成

1. Review 状态显示 **Merged**。
2. 回到 **Memory**，选择 **Payments**，打开 `deployment-rollback.md`，确认已发布正文包含新增句子。
3. 切回 **Payments**，确认原有选择仍然有效，并能读到更新后的检查单。
4. 等待 Payments 同步完成、检索就绪，然后从绑定的 `clumsies-demo` 仓库新建 Codex 任务，要求：“请使用 Clumsies 读取 `deployment-rollback.md` 全文，并引用回滚后的验证要求。”检查 `memory.load` 的实际返回正文，确认包含 **回滚后验证健康检查和关键业务请求，并记录结果。**

先核对前两项的 **Merged** 和 **Project** 正式正文，再进行 Codex 验证。项目草稿在发布前也可能被检索到，所以仅命中新句子不能证明它已经发布。

其他项目仍使用 Org 原文。如需共享改进，可在提交项目 PR 时选择 Org 贡献，再独立审核并合并关联的 Org PR。

## 常见阻碍

- **Request Review… 不可用**：检查是否位于 Payments、文件是否有修改草稿，以及草稿是否已经同步。有 **Retry Draft Sync** 时先重试同步。
- **提示只有作者可提交**：使用创建草稿的账号提交；审阅者账号应在提交后打开 Review。
- **批准按钮不可用**：确认项目发布权限、详情加载状态，以及是否需要先处理最新共享变更。
- **Review 仍为 Open 或被拒绝**：尚未发布。按反馈修改草稿；被拒绝的 Review 由作者重新提交，之后再审阅。

完成以上检查后，你就走完了“选择知识 → Codex 使用 → 明确要求修改 → 人审阅发布 → Codex 读取发布结果”的一轮流程。下一步可阅读[使用指南](/zh/guides/)。
