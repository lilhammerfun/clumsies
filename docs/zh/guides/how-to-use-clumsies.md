# 在仓库中使用 Clumsies

这篇指南带团队成员完成登录、使用 Memory、提出修改和 Review 发布。例子仍是[概览](/zh/overview)中的 **部署回滚检查单**。

开始前需要 macOS App、组织的 Server 地址、已获准加入的账号，以及一个有权限访问的 Project。组织 owner 或 admin 负责成员准入和 Project 授权。首次部署 Server 请先读[组织部署指南](/zh/guides/deploy-for-an-org)。

## 1. 登录组织

打开 Desktop，输入管理员提供的 Server origin，然后在系统浏览器中完成 SSO 登录。

Server origin 类似 `https://memory.example.com`，不附带页面路径、查询参数或内嵌账号密码。远程连接要求 HTTPS；本地开发支持 loopback HTTP。

尚未初始化的 Server 会显示 Setup Code 设置流程，第一位验证成功的身份成为组织 owner。普通成员加入已有组织，不需要再初始化一套服务。

登录后，确认 Desktop 显示的是预期的组织和账号。

## 2. 选择 Project，绑定仓库

选择本次工作的 Project。如果没有可选 Project，需要组织管理员授予访问权限或创建 Project。

在 Project 的 **Repositories** 区域，通过 **Add Repositories…** 添加本地仓库。这是在当前 Mac 上建立绑定；其他 Mac 的仓库路径可能不同，需要分别绑定。

Project 和仓库是不同的对象：

- Project 保存共享的工作上下文、成员权限和 Memory 选择集。
- 本地绑定告诉 daemon：某个仓库目录应该使用哪个 Project。
- Desktop 中选择 Project，控制的是当前浏览内容，不能代替托管 Agent 集成使用的仓库绑定。

## 3. 确认所选 Memory

打开 **Memory**，比较两个视图：

| 视图 | 应该看到什么 |
| --- | --- |
| **Organization** | 已发布的共享 Memory |
| **当前 Project** | 选中的组织 Memory，以及本机尚未发布的 Draft 修改 |

本例需要 Project 包含 `operations/deployment-rollback.md`。组织 owner/admin 可以更改 Project 的组织 Memory 选择集；成员通过 Project Draft 提议修改所选 Memory。

如果 Project 中缺少某篇共享文档，先检查选择集，再排查搜索。能在组织知识库中找到，不代表所有 Project 都选中了它。

**Bundle** 是个人保存的一组共享 Memory ID，方便重复使用同一组文档。它不会发布另一份内容，也不会覆盖 Project 选择集。

## 4. 接入 Agent 宿主

在 Desktop 的 Agent 集成设置中安装对应宿主的集成。宿主差异见 [Agent 运行时指南](/zh/guides/agent-runtime)。

集成会启动 App 内置的 MCP 代理，由常驻 daemon 提供当前 Project 的 Memory。Agent 不需要再维护一套 Clumsies 数据库、Server token 或手动同步的 Markdown 文件夹。

从已经绑定的仓库开始任务，让 Agent 查找团队部署回滚要求。通常依次使用：

```text
activate：按当前任务查找相关 Memory 片段
load：需要完整细节时，读取整篇检查单
store：只有用户要求维护 Memory 时，才提出修改
```

实际 MCP 只暴露一个名为 `memory` 的工具，包含这三个操作。调用格式见 [MCP](/zh/mcp)。

托管 host-plugin 运行时在绑定缺失或任务期间绑定变化时会停止处理请求。修正绑定后重新开始任务。手动启动的普通 `mcp serve` 仍保留回退到 Desktop 当前 Project 的兼容行为；日常使用应完成托管绑定，避免 Project 来源不明确。

首次使用时，本地检索模型和索引可能仍在准备。查看进度，就绪后重试。

## 5. 提出一处修改

在 Project 的 Memory 视图中打开检查单，增加缺失的验证步骤；也可以明确要求 Agent 更新这篇 Memory。Agent 修改前需要读取当前全文，并使用返回的 hash 和精确原文。

修改会创建或复用一个 **Draft**。保存时，操作进入 daemon 的本地持久化队列，随后自动同步到 Server。

应根据状态判断完成了哪一步：

| 状态 | 含义 |
| --- | --- |
| 本地已保存 / queued | 提案在这台 Mac 上已经持久化，可能还在等待上传 |
| Draft 已同步 | Server 已保存提案，正式组织 Memory 尚未改变 |
| Review 已合并 | 提案已经成为新的组织版本 |

本地读取和索引准备完成后，修改可通过当前 Project 的 Effective Memory 使用，此时仍不属于共享的正式指导。

## 6. 处理共享更新，提交 Review

Desktop 如果提示 **A shared update is available**，使用 **Merge latest version** 比较 Base、Current 和 Draft，阅读结果后确认。有重叠编辑时，在同一界面解决冲突。

单篇文档使用 **Request Review**；一组相关修改可以使用 **Request Review for All Project Changes…**。提交前检查文件列表、修改结果和说明。

提交流程会同步待上传操作，并用有效比较候选协调落后的 Draft。如果确认期间又出现更新，应刷新并检查新的比较结果。

提交后可以进入 Review 讨论。提交成功表示 Review 已存在，详情页仍需要获取显示 diff 所需的数据。

## 7. 由有权限的审查者发布

组织 owner 或 admin 检查完整提案后可以：

- **Approve and merge**：把有序 Draft 集合作为一个组织 Commit 发布。
- **Reject**：退回 Draft，供作者继续修改和重新提交。

当前 Desktop 的批准操作会同时合并。API 也支持独立的 Approved 状态；停留在这个状态的 Review 尚未发布，仍需合并。

组织 Ref 如果已经前进，审查者必须针对更新并协调后的提案做决定，不能继续使用过期页面中的确认结果。

合并后，发起提案的 Project 和其他受影响的 Project 会收到新快照。各台 Mac 继续同步快照与检索索引。本地就绪后，让 Agent 再次读取；对话里已经存在的旧文本不会自行更新。

## 遇到问题时先检查什么？

| 现象 | 首先处理 |
| --- | --- |
| 没有 Project 访问权限 | 请组织管理员检查成员授权 |
| Agent 提示绑定缺失或改变 | 检查已添加的仓库，并重新开始任务 |
| 搜索仍在准备 | 查看模型/索引进度，避免反复发起同一调用 |
| Draft 一直 queued | 查看同步状态和登录状态，重试已有操作流程 |
| Agent 编辑前内容已经变化 | 重新 load，根据最新原文生成精确替换 |
| Review 提示需要处理共享更新 | 检查并确认最新 Base/Current/Draft 比较 |
| Review 已合并，本机内容仍旧 | 查看 Project 同步和索引就绪状态，再读一次 |

请求返回失败时，也可能是 Server 已处理、响应却丢失，操作结果暂时不确定。先刷新已有 Draft 或 Review，再决定是否创建新提案。[完整流程](/zh/flows)进一步解释了这些边界。

## 本地存储与管理

Settings 中的 Project 本地存储设置会显示缓存位置、大小和可用性。使用 **Choose…** 更换位置，或使用 **Reset** 返回标准位置。Clumsies 在所选目录下管理一个隐藏子目录，它不是手动编辑 Memory 的工作目录。

**Clear Cache…** 只清理可重建的 Commit generation 和 Project 搜索索引，保留 Draft、待同步操作、设置与无关文件。外部磁盘不可用时，应重新连接或恢复访问权限。Clumsies 不会悄悄在其他位置新建缓存；checkout 和 MCP 检索会等待配置的位置恢复可用。

Owner/admin 通过 **Administration** 管理成员、Project、token、审计和健康状态。daemon 启动失败时，可通过 **Administrator Recovery** 建立临时的直连 Server 管理会话进行修复。正常 Memory 工作仍需要 daemon。

## 实现参考

[Project 界面](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/apps/macos/Sources/Features/ProjectManagementView.swift)、[工作区操作](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/apps/macos/Sources/Domain/WorkspaceStore.swift)和 [Server 授权处理](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/changes/http.rs)定义了上述工作流程。想了解背后的设计，可以继续读[系统架构](/zh/architecture)和[数据模型](/zh/data-model)。
