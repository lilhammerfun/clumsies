# Project

Project 把一个项目的成员、使用的知识和待发布修改组织在一起。本机仓库目录可以绑定到这个 Project，让 Agent 在工作时找到正确的 Memory。Project 的身份由 Server 签发，不能用文件夹名称代替。

本页保留历史地址 `/workspace`；当前 API 使用 `project_id`。Project 与 Organization 的关系见[核心数据模型](/zh/data-model)。

## 一个 Project 管理什么

| 内容 | 保存在哪里 | 用途 |
|---|---|---|
| 名称、描述、成员 | Server | 标识项目及访问权限 |
| Org Selection | Server | 决定该 Project 使用哪些已发布 Organization Memory |
| Project Ref / Commit | Server，并同步到 daemon | 给选择结果建立一个可安装的版本 |
| Project 携带的 Draft | 先在本机持久化，再同步到 Server | 保存以 Organization 为发布目标的提案 |
| 本机目录绑定 | daemon | 把当前仓库目录解析为 `project_id` |
| 安装 generation 与检索索引 | daemon 管理的文件 / SQLite | 支持 Agent 在本机读取和检索 |

成员授权与内容选择是两件事。被选中表示内容进入该 Project 的基线，不意味着个人 Bundle、目录绑定或某个 Agent 请求可以改变成员权限。

## 选择怎样变成当前可读内容

```text
Organization 当前 Memory + Project Org Selection
  → Server 生成 Project Commit / Ref
  → daemon 安装快照
  + 该 Project 的 open/submitted Draft 修改
  → Effective Memory
```

以 Payments 为例，它选择部署回滚检查单后，所有绑定到这个 Project 的工作目录都指向同一个 Server Project。某次编辑生成由 Payments 携带的 Draft；同步到另一台安装后，那台安装也能呈现这个 Project 的提案。它不是某个目录中独立发布的文件，也不会变成另一个 Project 的未发布修改。

未发布修改仅叠加到承载它的 Project。merge 后，Organization 内容更新，选中受影响资源的 Project 才会获得新的投影。对于新建 Memory，Server 还会把它自动加入发起 Project 的选择。

`GET /api/v1/projects/{project_id}/memories` 是历史 Project-authority 数据的读取接口。它不包含当前选择投影和本地 Draft，不能用它代替 Effective Memory。

## 目录绑定怎样工作

绑定关系是：

```text
规范化的 Server 地址 + 本机规范目录 → Server project_id
```

daemon 把关系保存在中心 SQLite 的 `project_bindings` 中。调用者位于子目录时，优先匹配最长的已绑定祖先目录；Git worktree 没有自身绑定时，还可以通过主 checkout 的仓库根解析。移动或重新绑定目录改变的是本机定位关系，不改变 Server Project 身份。

两种 Agent 入口的规则不同：

| 入口 | 选择 Project 的方式 |
|---|---|
| 纳管 host-plugin | 必须从工作目录解析绑定；启动及每次 `tools/call` 校验绑定，丢失或改变时请求失败 |
| 手工普通 `mcp serve` | 先解析工作目录；无绑定时可兼容使用 daemon 中 Desktop 当前选中的 Project |

因此，两个已经绑定的 Agent 进程可以同时服务不同仓库，切换 Desktop 选中项不会重定向纳管进程。工具请求本身不能随意指定另一个 `project_id` 来绕过绑定。

当前 runtime 不再读取或迁移 `~/.clumsies/config.toml`，旧 `ws_id` 也不是 Project 身份。安装与入口细节见[Adapter](/zh/adapter)和[本地运行时](/zh/runtime)。

## Project Local Storage

一台安装可以为某个 Project 选择 generation 与检索数据库的保存位置。这个设置以 Server 地址和 `project_id` 为键，仅作用于本机，不属于 Server Project 元数据。

用户选中的目录是 daemon 托管子目录的父目录。中心 Draft、同步操作、凭据、快照缓存对象和共享模型不会跟着移动。自定义目录不可用时，daemon 返回明确错误，不会悄悄建立另一份活动缓存。具体迁移和恢复流程见[本地运行时](/zh/runtime#project-local-storage)。

## 排查“为什么没读到这份 Memory”

按数据流依次检查：当前目录绑定的 Project、该 Project 的 Org Selection、daemon 已安装的 Project Commit、是否有 Draft 覆盖、检索索引是否就绪。发布成功但本机尚未同步、项目未选择、Draft 覆盖旧基线，是不同的问题。

继续阅读：[Organization Memory](/zh/artifact)、[核心数据模型](/zh/data-model)、[系统架构](/zh/architecture)。实现依据：[绑定解析](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/state.rs)、[MCP 入口](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/agent_runtime/mcp.rs)、[Project 投影](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/postgres.rs)、[本地存储](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/project_storage.rs)。
