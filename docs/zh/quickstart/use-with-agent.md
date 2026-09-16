---
description: 从绑定的仓库开始 Codex 任务，验证它能按任务检索项目已经选择的组织 Memory。
prev:
  text: 从组织选择 Memory
  link: /zh/quickstart/select-memory
next:
  text: 让 Codex 修改 Memory
  link: /zh/quickstart/update-memory
---

# 3. 在 Codex 中使用项目记忆

前一篇已经把组织的部署回滚检查单选入 **Payments**。现在从绑定的仓库开始一个 Codex 任务，让它用这份知识帮助你工作。

## 开始前

- 在 Payments 中能打开 `deployment-rollback.md`。
- 当前 Mac 上的 `clumsies-demo` 仓库已经绑定到 Payments。
- 已安装 Codex，并在 Clumsies 的 **Settings → Agents** 中启用 **Codex**。

Codex 下方应显示 **Plugin installed and enabled**。如果显示尚未安装或需要修复，按[接入编码 Agent](../guides/agent-runtime)处理。第一次使用还需要等待检索模型下载和索引准备完成。

## 1. 从绑定的仓库开始任务

Plugin 首次安装或更新后，重启 Codex，再从 `clumsies-demo` 开始一个新任务。

集成按本机用户安装一次，各个仓库的绑定决定使用哪个 Project。仅在 Clumsies 窗口里选中 Payments，不会把任意 Codex 任务都切换到这个项目。

## 2. 像平时一样描述工作

在新任务里输入：

> 请帮我整理 Payments 的发布前检查和回滚计划。先根据项目已有约定列出检查项，并注明依据的记忆文档。本次只制定计划。

Clumsies 集成附带的使用说明会要求 Codex 在开始实质任务时调用 `memory.activate`，按任务检索相关段落；需要完整上下文时再调用 `memory.load`。你不需要先复制整份知识库到对话中，也不需要手写工具参数。

这里的“自动检索”指 Agent 按集成说明调用工具。是否已经检索，应以实际工具调用及其结果为准。

## 3. 确认它使用了已有知识

检查 Codex 的工具调用与回答：

1. 任务开始后，出现 Clumsies **memory** 工具的 `activate` 调用。
2. 返回的相关内容包含 Payments 选中的检查单，回答据此解释发布或回滚要求。
3. 让 Codex 再读取这份检查单全文，确认返回的路径是 `deployment-rollback.md`，包含上一页核对过的原文：**发布前先确认上一版可以恢复。**

不要只凭“我已读取记忆”这句话判断成功。来源路径和实际返回内容，才说明任务读到了项目知识。第一次检索不一定把这篇文档排在首位；可以补充“部署回滚检查单”这个任务线索，但仍应检查调用结果。

## 没有读到时

| 现象 | 先检查什么 |
| --- | --- |
| 任务没有 Clumsies 工具 | 确认 Plugin 已安装并启用；重启 Codex 后新建任务。 |
| 工具可用，但没有发生检索 | 明确请 Codex 使用 Clumsies 查找部署回滚约定，再检查实际调用；不要把普通回答当成检索结果。 |
| 提示仓库未绑定 | 检查任务实际使用的目录。Git worktree 可以复用主仓库绑定；无法解析时再检查路径和绑定。 |
| 提示检索正在准备 | 等待模型和索引准备；下载或同步失败见[排查问题](../guides/troubleshooting)。 |
| 能检索，却找不到检查单 | 确认这篇 Memory 已选入 Payments；组织中存在并不表示所有项目都使用它。 |

**完成结果：** Codex 从项目已有知识中找到并使用了检查单。

接下来模拟工作中发现遗漏的情况：[让 Codex 修改 Memory](./update-memory)。
