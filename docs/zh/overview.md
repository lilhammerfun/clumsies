# 认识 Clumsies

Clumsies 为团队提供一套编码 Agent 可以查找、使用和共同维护的知识库。每篇 Memory 都是一份 Markdown 文档，可以是部署检查单、编码约定，也可以是系统设计说明。修改先保存为草稿，经过人工 Review 后，才成为团队共享的正式版本。

初次接触时，不必先读数据库表或 MCP 协议。这一页先用一个例子说明产品怎样工作，再介绍后面会反复出现的概念。

## 从一篇文档开始

假设团队有一篇 **部署回滚检查单**，路径是 `operations/deployment-rollback.md`。

开发者正在 Payments 仓库中处理部署任务，需要用到这份检查单。团队先把它选入 Payments Project。Agent 开始工作时，Clumsies 根据任务找出相关段落；需要完整上下文时，Agent 再读取全文。

任务中，开发者发现检查单少了一个验证步骤，明确要求 Agent 更新文档。这次修改会保存为 Draft，可以先在当前 Project 中使用。组织管理员检查并批准修改，合并成功后产生新的正式版本，再同步到选择了这篇文档的 Project。

这就是主要工作过程：**找到知识 → 使用知识 → 提出修改 → 审查 → 发布**。[完整流程](/zh/flows)会继续追踪每一步的数据变化。

## 先认识六个概念

| 概念 | 含义 | 例子 |
| --- | --- | --- |
| **Memory** | 一篇有稳定身份、路径、摘要和 Markdown 正文的文档 | 部署回滚检查单 |
| **Organization（组织）** | 团队及其共享的正式 Memory 库 | 拥有这份检查单的公司 |
| **Project（项目）** | 一个工作上下文，包含成员、仓库绑定和所选组织 Memory | Payments 选择了回滚检查单和编码约定 |
| **Draft（草稿）** | 一组尚未发布的修改，由一个 Project 承载 | 增加回滚后的验证步骤 |
| **Review（审查）** | 按顺序组织一个或多个 Draft，供讨论和授权决定 | 一起审查检查单和相关运行手册 |
| **Commit（提交版本）** | 发布时创建的不可变快照 | 新版部署回滚检查单所在的版本 |

Project 选择的是原始 Memory 的 ID，不会复制出另一篇独立文档。同一篇 Memory 后续更新后，选择了它的 Project 会收到更新。

规则、流程、系统背景都使用同一种当前内容模型；用途由路径和正文表达，不是三套独立的发布系统。

## 同一篇 Memory，为什么会看到不同内容？

| 视图 | 读到的内容 | 由谁维护 |
| --- | --- | --- |
| **Organization Memory** | 已正式发布的检查单 | Server |
| **Project projection（项目投影）** | Payments 选中的正式文档集合 | Server 根据选择集和组织内容生成 |
| **Effective Memory（实际读取视图）** | 本机的 Project 投影，再叠加当前 Project 尚未发布的 Draft 修改 | 本机常驻 daemon |

“投影”可以理解为从完整知识库中选出的一份视图。“Effective”表示此刻本机读取和检索实际使用的内容。

因此，Agent 可能在组织正式发布之前，就读到你刚提出的检查单修改。变化发生在当前绑定 Project 的本地 Effective Memory 中，尚未发布到所有 Project。

## 哪些组件在工作？

Clumsies 有三个主要的常驻组件：

- **Desktop** 是人使用的 macOS 界面，负责登录、选择 Project、阅读和编辑 Memory、审查修改。
- **daemon** 名为 `clumsiesd`，是同一台 Mac 上的后台进程，负责保存 Draft 操作、与 Server 同步、执行本地检索。
- **Server** 把共享身份、权限、Draft、Review 和已发布历史保存在 PostgreSQL 中。

Agent 通过一个很小的 **MCP 代理**访问 daemon。MCP 是 Agent 宿主调用工具的协议。这个代理负责转发请求，不会再维护一套数据库或搜索引擎。

组件关系见[系统架构](/zh/architecture)，对象之间的关系见[数据模型](/zh/data-model)。

## “保存成功”分三个阶段

| 阶段 | 已经完成什么 | 尚未完成什么 |
| --- | --- | --- |
| **本地已受理** | daemon 已把 Draft 操作提交到本机 SQLite，并排入同步队列 | Server 可能还没有收到 |
| **已同步** | Server 已接受 Draft 和相关操作 | 正式检查单仍未改变 |
| **已发布** | 获得授权的 Review 合并创建了组织 Commit，并移动当前版本指针 | 其他 Mac 可能仍在下载新 Project 快照或准备索引 |

这里的“当前版本指针”叫 **Ref**。Commit 本身不变，Ref 指向下一个 Commit。

Agent 调用 `memory.store` 成功，只表示**本地已受理**，不能作为发布成功的证明。成员可以提出修改，也不意味着拥有审批权限。

## 接下来读什么？

| 你想知道 | 下一篇 |
| --- | --- |
| 怎样在自己的仓库里使用？ | [成员使用流程](/zh/guides/how-to-use-clumsies) |
| 从检索到发布，数据经历了什么？ | [完整流程](/zh/flows) |
| 哪些组件运行在哪里？ | [系统架构](/zh/architecture) |
| 核心记录、字段和版本关系是什么？ | [数据模型](/zh/data-model) |
| 有哪些领域操作和接口？ | [领域接口地图](/zh/reference/domain-api) |
| 从哪里开始读代码？ | [代码库地图](/zh/repos) |

## 本文的范围

这里介绍当前的“组织统一发布”模型。兼容代码和部分界面里仍能看到旧 Project scope、Context / Rule / Workflow 名称；它们不能被理解为新增 Memory 的其他发布通道。

详细字段约定和已知实现差异放在[数据模型](/zh/data-model)与[接口参考](/zh/reference/domain-api)中。入门页先解释正常工作流程。

实现入口：[MCP 契约](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/agent_runtime/mcp_contract.rs)、[本地 Draft 持久化](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/state.rs)、[Review 发布](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/changes/postgres.rs)。
