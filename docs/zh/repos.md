# 按问题找到代码入口

这个仓库包含 macOS App、常驻 daemon、负责共享数据的 Server，以及当前文档站。建议先读[系统架构](/zh/architecture)和[完整流程](/zh/flows)，再按具体问题进入源码。

Rust workspace 有两个成员：`crates/server` 和 `crates/daemon`。Swift 负责原生界面，Bun 运行 VitePress 文档工具。

## 目录地图

| 路径 | 职责 | 什么时候读 |
| --- | --- | --- |
| `apps/macos/Sources/Features/` | SwiftUI 产品页面与用户操作 | 想知道用户看见什么、可以做什么 |
| `apps/macos/Sources/Domain/` | App 模型、工作区状态与流程协调 | 想跟踪一次点击后的加载、校验和状态变化 |
| `apps/macos/Sources/Infrastructure/` | 类型化 daemon XPC 客户端等平台接入 | 想知道 Desktop 怎样访问本地运行时 |
| `crates/daemon/src/agent_runtime/` | MCP 契约和短时 Agent 代理 | 想理解 Agent 工具的边界 |
| `crates/daemon/src/state.rs`、`draft.rs` | 本地状态、Draft 持久化与同步 | 想知道 queued 具体意味着什么 |
| `crates/daemon/src/commit_sync.rs`、`project_storage.rs` | Commit 安装、本地 generation 和缓存位置 | 想追踪已发布数据怎样到达 Mac |
| `crates/daemon/src/search/` | Effective Memory、分块、索引和检索 | 想知道相关片段怎样被选出来 |
| `crates/server/src/` | HTTP 路由与领域模块 | 想理解共享数据和权限 |
| `crates/server/migrations/` | PostgreSQL schema 演进 | 想检查持久化记录和约束 |
| `crates/server/openapi/` | Public 与 Admin HTTP 契约 | 想查请求和响应结构 |
| `packages/clumsies/` | 宿主集成资源和 Clumsies 插件 | 想知道宿主如何启动内置运行时 |
| `dev/`、`apps/macos/Scripts/` | 本地开发与构建工具 | 想运行隔离开发环境 |
| `docs/`、`docs/zh/` | 英文与中文文档 | 想改进文档站 |

当前没有活跃的 `src/client/` 独立客户端目录。历史 CLI 内容见[归档页](/zh/guides/cli-commands)。

## 沿一条操作读，不必通读所有文件

继续以部署回滚检查单为例，可以选择这些较短的路径：

| 问题 | 源码路径 |
| --- | --- |
| 用户编辑并请求 Review 后发生什么？ | `Features/WorkspaceView.swift` → `Domain/WorkspaceStore.swift` → `Infrastructure/DaemonXPCClient.swift` |
| Agent 的 `memory.store` 做了什么？ | `agent_runtime/mcp_contract.rs` → `agent_runtime/mod.rs` → `state.rs::store_draft_operation` → 本地 Draft 队列 |
| Review 如何校验并发布？ | Server `http.rs` → `changes/http.rs` → `changes/service.rs` → `changes/postgres.rs` |
| 发布怎样到达选择了文档的 Project？ | Server `memory/postgres.rs` → daemon `commit_sync.rs` → `search/` |
| 怎样确定当前仓库所属的 Project？ | daemon `main.rs` → Project binding XPC 方法 → daemon 状态 |

第一行路径相对于 `apps/macos/Sources/`，其余 daemon 路径相对于 `crates/daemon/src/`。

Server 的分层有明确作用：HTTP handler 解码请求并检查权限，service 协调领域操作，PostgreSQL 代码执行状态转换和事务。修改一个公开操作时，需要同时确认这三层。

## 实现和测试一起读

| 行为 | 可执行例子所在位置 |
| --- | --- |
| 多文件 Review 的顺序与原子发布 | `crates/server/tests/draft_operation_ordering.rs` |
| Draft 上传、合并、投影更新和两个 daemon 收敛 | `crates/daemon/tests/server_integration.rs` |
| 本地持久化与进程重启 | `crates/daemon/tests/daemon_lifecycle.rs` |
| Agent 代理与真实 XPC 边界 | `crates/daemon/tests/agent_runtime_xpc_e2e.rs` |
| Desktop daemon 契约和状态映射 | `apps/macos/Tests/DaemonContractTests.swift` |

测试说明代码承诺了哪些行为，不自动构成生产延迟指标。性能测量及适用范围见[性能文档](/zh/performance/)。

## 直接打开主要入口

[Desktop 工作区](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/apps/macos/Sources/Domain/WorkspaceStore.swift) · [MCP 契约](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/agent_runtime/mcp_contract.rs) · [daemon 状态](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/state.rs) · [Server 路由](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/http.rs) · [Review 事务](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/changes/postgres.rs)

准备运行和修改项目时，继续读[开发流程](/zh/guides/development-workflow)；想先理解接口含义，再看实现，可以从[领域接口地图](/zh/reference/domain-api)开始。
