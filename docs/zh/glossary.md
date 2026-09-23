# 术语表

遇到名词时用本页查含义。第一次认识项目，建议先读[认识 Clumsies](/zh/overview)和[核心数据模型](/zh/data-model)，不用按顺序背术语。

## Organization

拥有共享 Memory 发布历史的组织。Organization owner/admin 可以作发布决定；Project 选择组织里的内容用于具体项目。

## Project

Server 签发身份的项目对象，管理成员、项目自有正式 Memory、Organization Memory 选择，并承载待发布 Draft。本机目录可以绑定到 Project，但目录不是 Project 身份。`Workspace` 是旧称，当前接口使用 `project_id`。

## Memory

一份具有稳定 ID 的 Markdown 知识。数据库称为 `resources`，HTTP 使用 `memory_id`。正式资源属于某个 Project 或 Organization；规则、流程和背景都使用同一种 Memory。

ID 标识“哪一份知识”，path 表示“目前放在哪个路径”，revision 表示“资源的哪次修订”。重命名不换 ID。`name` 来自文件名；daemon 展示标题来自 Markdown 标题或文件名。详细字段见[数据模型](/zh/data-model)。

## Organization authority

Organization 的正式内容发布权威，由 Organization Ref 及其 Commit 历史表达。“有权威”不是说内容一定正确，而是说它已经通过系统的组织发布边界。未发布 Draft 与检索排序不能自行获得这个身份。

## Project Org Selection

Project 明确选择的 Organization Memory ID 集合。它决定 Project 发布基线的内容；整个集合有独立 revision。Selection 不复制 Memory，也不是用户的个人 Bundle。

## Projection（投影）

从已有权威数据按用途生成的视图。Project Commit 组合项目自有 Memory 和选用的 Organization 内容。选用部分是读取投影，项目自有内容则有独立发布历史。

## Effective Memory（有效内容）

daemon 将已安装的 Project 投影与该 Project 当前 `open` / `submitted` Draft 组合得到的可读内容。活动 Draft 使用自己的 Base 与操作计算结果。它可能包含未发布内容，也可能因本机同步进度而晚于 Server 最新版本。

## Draft

待发布提案：由 Project 携带，以该 Project 或 Organization 为目标，保存 Base Commit、版本及有序 create/update/rename/delete 操作。状态为 `open`、`submitted`、`merged`、`discarded`。本地 Draft 可能尚未同步，不能当作可清理缓存。

## Base / Current / Draft Result

三方比较的三个输入：Base 是修改开始时的发布快照，Current 是现在的发布状态，Draft Result 是把提案操作应用到 Base 后得到的结果。它们用于判断上游更新与本次修改能否组合。

## Freshness / Reconciliation

Freshness 表示 Draft 是否跟上当前 Ref：`current` 或 `behind`。Reconciliation 表示比较进度或结果：`unknown`、`clean`、`conflicts`。落后不一定冲突；两者都不是 Draft 生命周期状态。

## Reconciliation candidate / Rebase

candidate 是 Server 生成的比较结果，绑定 Draft version 和 Base/Current Commit。rebase 是确认并应用候选：保存旧 revision、更新 Base、重新表达操作。它不发布 Memory；候选过期后需要重新比较。

## Review

Server 中用于协调和发布一组 Draft 的对象。Draft 集合有顺序，merge 时整组修改在一个事务中发布。批准结果由哈希绑定；内容结果变化后不能继续使用旧批准。有权限的用户可合并 open 或 approved Review。

## Blob / Tree / Commit / Ref

- **Blob：** 不可变正文，同样的内容可被不同快照引用。
- **Tree：** 一个版本里的条目集合，将 Memory ID、路径和来源连接到 Blob。
- **Commit：** 指向 Tree、记录父版本的不可变完整快照；不是代码仓库的 Git commit。
- **Ref：** 指向当前 Commit 的可移动指针。Organization Ref 代表发布，Project Ref 指向组合项目自有内容和选择结果的快照。

## Revision / Version / ETag / CAS

revision 和 version 是不同对象的修订或并发版本，不能跨对象互换。ETag 是 HTTP 表达版本身份的一种方式，例如资源详情的 `"rev-3"`。CAS（compare-and-swap）要求写入时看到的版本仍然成立，否则拒绝旧写入。Commit ID 和正文哈希各有用途，也不能充当任意对象的 version。

## Content hash / Effective Memory hash / Index Revision

content hash 标识正文内容；Effective Memory hash 标识本机有效内容输入；Index Revision 标识基于内容及检索模型、解析器等构建的索引版本。索引必须与要查询的有效内容匹配。它们都不是用户审批或权限凭据。

## Bundle

一个用户保存在 Server 的 Memory ID 集合，类似个人共享知识收藏夹。改变或删除 Bundle 不修改 Memory 本身，也不自动改变 Project Org Selection。

## Project binding

本机“Server 地址 + 规范目录 → project_id”的映射。纳管 host-plugin 必须解析并复查目录绑定；普通手工 `mcp serve` 无绑定时可兼容回退到 Desktop 当前 Project。见 [Project](/zh/workspace)。

## Generation / Project Local Storage

generation 是 daemon 安装的不可变快照文件目录。Project Local Storage 是这台安装为某个 Project 保存可重建 generation 和检索数据库的位置。它不是 Server 的项目目录设置，也不搬走中心 Draft、队列或凭据。

## Daemon / Runtime proxy / XPC

daemon 是本机常驻后台进程，负责持久化、同步和检索；proxy 是把 Agent 协议转成本地请求的短进程；XPC 是 macOS 进程间通信机制。MCP proxy 不拥有另一份数据库或模型实例。见[系统架构](/zh/architecture)。

## Server

共享 HTTP 服务，负责身份授权、Organization Memory、Project 选择、Draft/Review 和版本快照。PostgreSQL 保存服务端状态。本机目录绑定、检索模型和检索历史不属于 Server Memory 发布数据。

## Adapter / Agent Host

Agent Host 是运行编码 Agent 的产品。Adapter 是让 Clumsies 在这个宿主中可用的集成层，负责安装 MCP 配置。Codex 使用纳管全局 Plugin；其他支持的宿主采用各自集成方式，见 [Adapter](/zh/adapter)。

## MCP

Agent 调用工具的协议。Clumsies 当前只暴露一个 `memory` 工具，支持 `activate`、`load`、`store` 三种 action。它不暴露 Review 审批、merge 或任意 Server 请求。

## Retrieval Run / Evaluation Case / Corpus

Retrieval Run 是一次 `memory.activate` 的本地检索记录，保存 query、数据与索引身份、候选、结果和延迟。Evaluation Case 将成功记录对应的查询、完整 corpus（资源集合）和人工证据判断冻结成评测样本。它们用来解释和检验检索，不属于 MCP 的新工具，也不会随 Memory 发布上传到 Server。

## Issue / Assignee / Claim

这些词出现在早期任务协作设计和数据库迁移中：Issue 表示工作事项，assignee 表示负责人，claim 表示临时执行租约。当前 Server 路由没有提供对应的共享 Issue API，不能仅因历史表仍存在就当成可用领域能力。

## Rule / Workflow / Context 与历史名称

Rule、Workflow、Context 是旧版封闭 Memory 类型。现在它们可以描述文档用途，不是 Server/MCP 的三种内容类型；macOS 尚有旧 UI 分类残留。

Artifact 是 Organization Memory 管理面的旧称；Hub、Local 是旧 UI 标签；Manifest 是旧运行时术语；Attestation 属于已退役客户端事件流。阅读历史文档时先确认其日期与实现范围，不要据此创建当前不存在的对象或接口。
