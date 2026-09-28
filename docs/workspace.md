# Project

> 文档属性：概念定义型 / 详细设计型｜L2–L3｜当前权威。

Project 是仓库绑定、授权边界、Organization Semantic Memory 选择和 Draft carrier；它
不是 Semantic Memory 的发布命名空间。Project 同时是 Episodic Memory 的归属与授权
边界，但 Episode 不参与 Organization Draft/Review。本文保留历史路由 `/workspace` 以兼容
旧链接；当前 API 与运行时只使用 `project_id`。

## 身份与本地绑定

Server 签发规范 Project 身份。本机目录只是安装级绑定：

```text
normalized Server authority
  + canonical workspace root
  -> canonical project_id
```

常驻 daemon 把绑定保存在中心 SQLite，规范化调用方当前目录并选择最长的已绑定祖先；
Git worktree 没有独立绑定时，还可解析主 checkout 的仓库根。文件夹名称、旧 `ws_id` 和
请求参数都不能充当 Project 身份。当前运行时不再读取或迁移
`~/.clumsies/config.toml`。

两种 Agent 入口的兼容策略不同：

- 纳管 host-plugin 必须由当前目录解析绑定，启动及每次 `tools/call` 都校验仍为同一
  Project；绑定缺失或变化时关闭式失败。
- 手工启动的普通 `mcp serve` 先解析目录；没有绑定时可兼容使用 daemon 中 Desktop
  当前选中的 Project。该回退不是调用方可写的 `project_id`，也不适用于 host-plugin。

因此多个已绑定 MCP 进程可同时服务不同仓库，Desktop 选中项不会重定向纳管进程。

## Organization 权威与 Project 投影

Organization 是唯一 active Memory 权威。Project 保存 Organization Memory ID 选择集合，
并持有由“选择 + 当前 Organization Commit”生成的 Project Ref/Commit。Project Ref 是可
同步、可安装的物化投影，不是第二个发布目标：Review merge 推进 Organization Ref，选择
变化或上游权威变化刷新 Project Ref。

```text
Organization Ref + Project Org Selection
  -> Project Ref / Commit projection
  -> installed generation
  + Project-carried open/submitted Org Drafts
  -> Effective Memory
```

仓库专属知识也先作为由该 Project 携带的 Organization-scoped Draft。合并前 overlay 只在
该 Project 可见；批准后成为 Organization 权威，并自动加入携带 Project 的选择集合。

`GET /api/v1/projects/{project_id}/memories` 及详情路由只读取遗留的
Project-scoped authority 数据，不返回 Project selection、投影或 Effective Memory。当前
客户端不得用这两个接口构建 Project 视图。

## 本机状态

常驻 daemon 持有 Project 的安装级状态：

- 目录绑定和 Server authority；
- 已安装的 Organization Ref 与 Project 投影 Ref；
- 不可变 Commit generation；
- 当前 Draft、操作队列和同步状态；
- 与 Effective Memory hash 匹配的派生检索索引；
- 结束 AgentRun 的 durable Episode outbox、Server 当前 summary 副本与 corpus revision；
- 可选 Project Local Storage 位置及 move 状态；
- 仓库级 direct-file Adapter 记录；
- Server Issue 的本地副本和本机 AgentRun 投影。

Server 仍对 Project membership、Org Selection、Organization Memory、Review/merge、
Project Episode/Evidence/摘要策略、共享 Kanban Issue/claim 有权威。本地绑定、缓存或
AgentRun 都不会自行创建远端权威；Episode 只有在 Evidence finalize 后才成为共享对象。

## Effective Memory

Agent 读取的不是某个 HTTP Project-memory 列表，而是 daemon 合成的当前视图：

```text
installed Project projection + current open/submitted Draft operations
  -> Effective Memory hash
  + current Server Episode summaries
  -> MemoryCorpus hash
  -> matching unified Index Revision
  -> memory.activate
```

`memory.load` 仍只读取完整 Semantic resource；Episodic 命中通过 `memory.evidence` 按
`episode_id` 从 Server 分页下钻。联合索引不改变 Effective Memory 自身的发布语义。

`memory.store` 先在 daemon 中持久化一个由绑定 Project 携带、以 Organization 为发布目标
的 Draft，并加入同步队列。成功只表示本地接受；它不会更新 Organization Ref、审批
Review 或让其他 Project 立即看到提案。

## Project Local Storage

每个 Project 可为本机选择可重建 generation 与检索数据库的位置。设置以 Server
authority 和 `project_id` 为键，只属于当前安装，不进入 Server Project 元数据。

用户选择的目录只是 daemon 托管子树的父目录。中心 Draft、操作队列、Episode outbox、
凭据、Server Evidence 和共享模型不会随之移动；这里只有可重建的 summary 投影与统一
索引。自定义位置不可用时，daemon 明确返回错误，不会悄悄在默认位置创建第二份活动
缓存。迁移、ownership marker、CAS 和恢复语义见
[本地运行时](/runtime#project-local-storage)。

## 相关文档

- [Organization Memory](/artifact)：唯一内容权威与 Bundle；
- [统一 Memory 数据模型](/unified-memory-model)：字段、投影、Draft 与多 Draft Review；
- [本地运行时](/runtime)：目录解析、存储、同步和失败行为；
- [Adapter](/adapter)：各 Agent Host 的纳管方式；
- [系统架构](/architecture)：跨组件不变量。
