# 一篇 Memory 从检索到发布的完整流程

本文继续使用虚构的 **部署回滚检查单**，路径为 `deployment-rollback.md`，逐步说明：用户做什么、数据在哪里变化，以及每一步“成功”的含义。

如果还不熟悉 Memory、Draft、Project，请先读[认识 Clumsies](/zh/overview)。本文解释系统怎样工作；实际开始使用的步骤见[快速开始](/zh/quickstart/)。

## 先看全程

[![Memory 生命周期：为 Project 选择组织 Memory，本地检索，保存并同步 Draft，Review 合并，再同步新的 Project 快照。](/diagrams/memory-lifecycle.png)](/diagrams/memory-lifecycle.png)

图的文字说明：

```text
组织发布回滚检查单
  → 管理员把它选入 Payments Project
  → daemon 安装 Project 快照
  → Agent 检索相关段落并读取全文
  → 用户明确要求的修改保存为本地 Draft 操作
  → daemon 把操作上传到 Server Draft
  → 作者把 Draft 提交为 Review
  → 管理员批准并合并
  → Server 创建组织 Commit，更新受影响的 Project 投影
  → 各台 Mac 的 daemon 安装新快照并准备检索
```

保存 Draft 和发布 Commit 是两件事，中间有网络同步；发布之后，各台 Mac 还要准备自己的读取视图。

## 1. 让 Project 可以使用检查单

**用户操作。** Project admin，或能访问 Payments 的组织 owner/admin，把检查单选入该项目，本地仓库绑定到这个 Project。

**Server 中的数据。** Project 选择集记录所选组织 Memory 的 ID。Server 根据选择集生成 Project 快照，把 Project Ref 移到新快照。组织检查单的正文没有因此变化。

**本机数据。** daemon 保存这台 Mac 上的“仓库 → Project”绑定，下载 Project Commit、Tree 和引用的正文，再安装为一份完整的本地 generation。

其中，**Tree** 描述快照里有哪些资源，**Blob** 保存不可变内容，**generation** 表示 daemon 在本机完整安装的一份快照。

**完成后的结果。** 快照和所需搜索索引就绪后，这篇检查单可以参与当前 Project 的本地检索。其他 Project 不会因为组织里存在这篇文档，就自动获得它。

注意这两个设置的区别：Project 选择集由 Server 保存，仓库绑定由本机 daemon 保存。

## 2. 找到相关指导，再读取全文

**用户操作。** 开发者让 Agent 准备一次部署回滚。

**Agent 调用。** 宿主调用唯一的 MCP 工具 `memory`，传入 `activate` 操作：

```json
{
  "op": {
    "activate": {
      "query": "准备部署回滚，查找团队的部署回滚检查单"
    }
  }
}
```

App 内置的 MCP 代理通过 macOS XPC，把请求转交给常驻 daemon。daemon 使用当前绑定 Project 的 Effective Memory，也就是已安装快照加上当前本地 Draft 操作。

Activation 检索并排序相关片段，返回来源身份和内容，Agent 再判断哪些文档需要完整阅读。每个任务不需要把整个组织知识库都塞进上下文。

接着，Agent 调用：

```json
{
  "op": {
    "load": {
      "ids": ["deployment-rollback.md"]
    }
  }
}
```

`load` 按已知 ID 或精确路径读取完整资源，返回稳定 ID 和内容 hash。hash 用来标识 Agent 实际读到的正文版本。

**完成后的结果。** Agent 获得本次任务所需的背景，Draft 和正式记录都没有变化。

Activation 的可选 `state` 用来记录已经返回过的片段。只有旧片段仍在 Agent 上下文中时才能复用；上下文压缩或开始新任务后应省略。它不是登录凭证，也不是永久会话存档。

## 3. 保存用户明确要求的修改

**用户操作。** 开发者要求：“在这份检查单中增加回滚后的验证步骤。”

**Agent 调用。** Agent 先读取完整文档，再使用返回的稳定 ID、hash 和精确原文，提交替换操作：

```json
{
  "op": {
    "store": {
      "update": {
        "id": "mem_example_rollback",
        "expected_hash": "替换为-load-实际返回的-hash",
        "replacements": [
          {
            "old_text": "确认旧版本已经运行。",
            "new_text": "确认旧版本已经运行，并验证健康检查和一条示例请求。"
          }
        ]
      }
    }
  }
}
```

上面的 ID、hash 和原文都是示例。实际操作必须使用真实 `load` 结果中的值。

**本地校验。** daemon 检查目标是否允许修改、正文是否仍匹配 `expected_hash`、替换原文是否匹配。如果文档已经变化，会返回错误，不会把编辑套到另一份内容上。

**持久化。** daemon 在一个 SQLite 事务中创建或复用 Draft，把操作写入 `local_draft_operations`，标记 `sync_status = queued`，并排入 Project 索引更新任务。事务完成后，再唤醒后台工作进程。

**完成后的结果。** 返回 `queued: true` 表示本地已受理，不能证明 Server 已经收到。Project 的 Effective Memory 会通过本地读取和索引流程纳入 Draft；匹配的索引尚未就绪时，检索可能返回准备状态。

Desktop 编辑也使用同一个持久化 Draft 队列。Agent 无需一直保持 MCP 进程运行，同步也能继续。

## 4. 把提案同步到 Server

**后台动作。** daemon 创建或复用对应的 Server Draft，上传队列中的操作，并拉取更新后的 Draft 状态。

Draft 记录提案基于哪个组织 Commit、作者是谁、由哪个 Project 承载，以及操作历史和版本。daemon 同时记录本地 Draft 与 Server Draft 的对应关系。

**完成后的结果。** Server 已经保存提案。所需操作同步完成后，作者可以提交 Review。组织中正式发布的检查单仍未改变。

Server 不可达时，已持久化的本地队列仍然保留。修复连接或登录问题后，让同步重试即可；反复创建同一份提案不能替代检查现有 Draft 状态。

如果请求发出后响应丢失，仅凭报错无法判断 Server 是否已经执行。应先刷新已有 Draft 或 Review，再决定是否重试。

## 5. 处理其他人已经发布的新版本

开发者编辑期间，另一位管理员可能已经发布了新版检查单。此时 Draft 会变成 **behind**：它的 base Commit 与当前组织 Ref 不同。

Clumsies 比较三份内容：

| 内容 | 含义 |
| --- | --- |
| **Base** | Draft 开始时所依据的正式内容 |
| **Current** | 当前组织 Ref 指向的正式内容 |
| **Draft** | 把作者操作应用到 Base 后的内容 |

**reconciliation candidate（协调候选）**保存针对某个 Draft 版本和当前 Commit 的比较结果。请求或查看候选，不会把结果写回 Draft。

Desktop 提供 **Merge latest version**。用户检查结果并确认；存在重叠修改时，需要手动解决冲突。即使比较结果是 clean，也不会仅凭查看就无声改写作者的 Draft。

已经跟上当前版本的 Draft 可以直接提交。落后的 Draft 可以携带有效候选和必要的冲突解决结果提交，Server 在创建 Review 的事务中完成整组协调。候选必须仍然匹配当前 Draft 版本和组织 Ref。

**失败后的状态。** 如果 Draft 或共享 Ref 再次变化，旧确认会被拒绝，或需要重新比较。正式内容不会被覆盖；刷新后检查新的候选即可。

## 6. 提交、讨论、批准与发布

**作者操作。** 选择一个或多个 Draft，填写 Review 标题和修改说明，并按确定的顺序提交。这些 Draft 必须属于同一个 Project、同一个作者，并满足当前发布规则。

**Server 校验。** 检查每个 Draft 版本，以及请求预期的组织 Ref。Review 保存有序的 Draft ID 集合。评论和决定也关联具体 Review 版本，避免悄悄作用于另一版提案。

**审查者操作。** 组织 owner 或 admin 可以驳回提案，或批准并合并。普通 Project 成员可以提案、参与讨论，但不能发布。

当前 Desktop 的批准操作调用 merge 接口，在一个事务内批准并发布 Open Review。API 也保留了独立的 Approved 状态，支持合并已经批准的 Review。单独“已批准”不等于“已发布”。

**发布事务。** Server 按顺序应用完整 Draft 集合，创建组织 Commit，移动组织 Ref，更新受影响的 Project 投影，并把 Review 和 Draft 标记为已合并。新创建的组织 Memory 还会自动选入发起提案的 Project。

整组 Draft 原子发布。Ref 过期或冲突尚未解决时，发布不会进行，不会只发布前几个文件。Review 被驳回后，其 Draft 会重新开放，供作者继续编辑和再次提交。

## 7. 让各台 Mac 能读到新版本

**Server 的结果。** 组织已经有一个新的正式版本。选择了受影响 Memory 的 Project 会得到更新后的投影快照。

**本机后续动作。** 各台 Mac 的 daemon 拉取 Project Ref 和 Commit 内容，准备完整的本地 generation，并更新派生搜索索引。daemon 保护 generation 切换边界，避免一次读取混用前后两份快照。

**完成后的结果。** 本地视图和所需索引就绪后，下一次 activation 或 load 可以使用新版检查单。发布不会把新正文主动塞进已经保留旧内容的 Agent 对话，Agent 仍需再次检索或读取。

关闭 Desktop 不会停止常驻 daemon 的后台任务；结束短时 MCP 代理进程也不会丢掉本地队列。

## 按失败发生的位置排查

| 现象 | 应检查的边界 | 下一步 |
| --- | --- | --- |
| Agent 无法确定 Project | 仓库绑定 / 宿主运行时 | 检查绑定；绑定改变后重新开始 Agent 任务 |
| Activation 提示模型或索引准备中 | 本地检索准备 | 查看就绪状态和进度，完成后重试 |
| 更新返回 `memory_content_changed` | 内容并发校验 | 重新 load，以最新原文制定替换 |
| Draft 一直 queued | 本地到 Server 的同步 | 检查同步状态、网络和登录，重试已有 Draft |
| Review 要求 reconciliation | Draft base 与当前组织 Ref | 检查并确认 Base/Current/Draft 比较 |
| 提交 Review 成功，页面仍在加载 | Desktop 的 Review 详情和 diff 加载 | 单独检查后续读取与页面就绪，不能把它等同于提交失败 |
| Review 已合并，Agent 仍读到旧内容 | Project 快照/索引同步，或 Agent 已有上下文 | 检查本地就绪状态，再次检索 |
| Project 存储不可用 | 配置的本地存储位置 | 接回磁盘或恢复权限，不要编辑托管缓存文件 |

组件职责见[系统架构](/zh/architecture)，具体请求契约见[领域接口](/zh/reference/domain-api)。

## 实现依据与深入阅读

本文依据仓库中 `5d038ff` 版本的实现编写，不假设其他分支中尚未发布的改动已经生效。

| 行为 | 源码或可执行验证 |
| --- | --- |
| MCP 校验与操作格式 | [MCP 契约](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/agent_runtime/mcp_contract.rs) |
| hash 更新校验、本地持久化队列及确认 | [daemon 状态层](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/state.rs) |
| 检索中的本地 Draft 叠加 | [Search overlay](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/search/overlay.rs) |
| 候选校验、Review 创建和原子发布 | [Review 持久化](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/changes/postgres.rs) |
| 多 Draft 合并保留操作顺序 | [Draft 操作顺序测试](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/tests/draft_operation_ordering.rs) |
| 发布变化到达两个 daemon，且重启后保留 | [Server 集成测试](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/tests/server_integration.rs) |
