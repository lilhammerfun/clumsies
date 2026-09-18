---
description: 使用可编辑的记忆维护规范，约定 Agent 怎样组织、更新和清理知识。
---

# 记忆维护规范

**记忆维护规范（Memory Guidelines）告诉 Agent：什么值得记、怎样组织，以及何时更新和清理。** 它保存在一份可编辑的 Memory 文档中，默认文档名为 `CLUMSIES.md`。

你可以直接采用 Clumsies 的默认规范，也可以使用团队已有的约定。采用后可以随时修改，并通过组织现有审阅流程共享；App 升级会保留你的修改。

## 采用默认规范

1. 打开 **Memory**，选择一个尚无记忆的项目。
2. 点击 **Preview guidelines and their sources** 查看完整英文模板及研究出处；也可以直接点击 **Set Up Guidelines**。
3. Clumsies 一起创建起始文档草稿，并打开 `CLUMSIES.md`。以后需要调整任何文档时，切换到 **Source** 编辑即可。

```text
CLUMSIES.md
knowledge/README.md
procedures/README.md
lessons/README.md
```

三份目录说明告诉用户什么内容适合放在哪里，不包含虚构的项目事实。预览中可分别查看每份文档。已有目录和自定义规范会保留。新文档在同一个本地事务中保存，初始化失败不会留下半套骨架。

这些草稿在发布前已参与当前项目的有效记忆。其他项目需要共用时，按现有流程[审阅并发布](/zh/quickstart/review-and-publish)，再在那些项目中选择这些资源。

采用默认规范是可选操作。你也可以通过 **File → New Memory** 创建自己的内容。

## 沿用已有规范

初始化前，Clumsies 会检查配置路径、当前项目草稿和组织记忆，保留已有规范。组织已有对应文档时，**Use Team Guidelines** 会将同一资源加入当前项目的选择，不复制一份新文档；这个操作需要项目管理员权限。

如果配置了自定义路径，但文档不存在，App 会提示恢复文档或修正配置。它不会静默改用另一处默认内容。若草稿正在删除或重命名规范文档，需要先处理该草稿。

这里的路径属于 Clumsies 记忆空间，不是仓库或插件缓存中的文件。当前自定义路径设置属于本机 daemon，App 尚未提供每项目独立的路径设置。

## 默认约定包含什么

- 保存会影响未来行动的决定、约束、经过验证的操作方法与经验。
- 保留已有组织方式；新空间可按需使用 `knowledge/`、`procedures/` 和 `lessons/`。
- 写清适用范围，并将原因、证据和例外放在对应的指导旁边。
- 同一主题优先更新原文，可以独立维护的新主题才新建文档。
- 根据范围和证据处理矛盾，从当前指导中移除过期要求。

这份文档仍然是普通 Memory。它不增加权限，不自动授权写入，也不安装宿主 Skill。操作和草稿状态仍遵守 [Memory 工具契约](/zh/mcp)。

## 这些约定的依据

默认规范结合 Clumsies 的产品模型，参考了公开研究和工程实践：

| 设计选择 | 参考来源 |
|---|---|
| 保持信息相关，同时留下正确行动所需的上下文 | [Anthropic：Effective context engineering for AI agents](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents) |
| 检查已有笔记，整理知识，避免不断追加重复内容 | [LangChain：How we built Agent Builder's memory system](https://www.langchain.com/blog/how-we-built-agent-builders-memory-system) |
| 优先增量修订，保留具体经验与有效细节 | [Agentic Context Engineering，v3](https://arxiv.org/abs/2510.04618v3) |

这些来源支持设计取舍，不代表这份模板的效果已经得到验证。可以根据团队实际任务修改默认约定，观察更新是否保留了有用知识、减少了重复。
