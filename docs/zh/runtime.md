# 本地运行时

本页解释 daemon 的持久状态、同步、检索和恢复边界。初次阅读建议先看[系统架构](/zh/architecture)与[完整流程](/zh/flows)；遇到故障可直接查[排查问题](/zh/guides/troubleshooting)。

`clumsiesd` 是当前用户作用域的 macOS launchd 常驻服务。Desktop 负责原生交互；daemon 在 Desktop 关闭后继续持久化 Draft、同步 Commit、构建检索索引并服务 Agent。Agent Host 启动同一个 App 内签名二进制的短进程 `mcp serve` 或 `_agent agent-run-event` 代理，再通过 XPC 调用常驻 daemon。

## 本地状态所有权

daemon 使用一个中心 SQLite 数据库；当前 schema version 为 `40`。它保存：

- 安装身份、schema version、Server URL 和 Desktop 当前选中的 Project；
- 规范化的工作目录到 `project_id` 绑定；
- 本地 Draft、有序操作、同步状态和 Server Draft 身份；
- Blob、Tree、Commit 元数据以及已安装的 Organization / Project Ref；
- Project Local Storage 位置、revision 和 move 状态；
- `native_issues` 本地看板副本、依赖/阻塞事实、AgentRun 与生命周期事件；
- Retrieval Run、Evaluation Case 和相关诊断状态。

每个 Project 的派生检索数据库位于该 Project 的活动 Local Storage 中，保存 Effective Memory、Markdown unit、FTS5 行、vector 和 search revision。embedding/reranking 模型只保存在 daemon 共享缓存，不按 Project 复制。

本地文件权限为 owner-only。access/refresh token 只作为绑定 Server URL 的一个 generic-password 条目保存在 macOS Keychain；SQLite 和文件系统没有明文凭据兜底。

## 检索模型准备

daemon 在第一次 MCP 请求之前，就开始后台准备模型。当前使用固定版本的 int8 `intfloat/multilingual-e5-small` 做 embedding，使用 int8 `Xenova/bge-reranker-base` 做 reranking。完整下载量为 431,831,479 字节；版本、文件大小和 SHA-256 校验值定义在[模型清单](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/models.rs)中。

下载支持断点续传，文件通过校验后才加载到 ONNX，随后可复用本机缓存。准备期间，搜索状态显示 `preparing` 及已下载、总字节数。Activation 会立即返回 `search_model_preparing`，不会让 MCP 请求一直等待未报告的下载，也不会静默降级到较弱的检索方式。

## 请求路径

```text
Desktop (Swift) -> typed XPC -> resident daemon -> HTTPS -> Server
Agent Host -> stdio MCP / Hook -> signed short proxy -> typed XPC -> resident daemon
```

短进程只负责有界 framing、`memory` MCP tool、Project 选择与 XPC 转发。它不初始化 `DaemonState`，不打开 SQLite，不加载模型，也不启动后台 worker。启动时，代理必须验证自身协议 revision 和 build identity 与常驻 daemon 一致。

## 客户端诊断日志

App 将 JSON 行写入当前实例日志目录的 `client.log`，稳定版默认路径为
`~/Library/Logs/ai.clumsies`。从 Finder 启动也会落盘。常驻 daemon 将结构化事件
写入 `daemon.log`，不再把每条事件重复写入 launchd stderr。两个主日志和
`clumsiesd.crash.log` 各保留当前文件及三个归档，每个最多 4 MiB；升级前超限
文件只保留末尾内容。管理命令不与常驻进程共用文件写入器。文件日志不可用时，
App 向 OSLog 报告，daemon 向 stderr 报告；初始化日志之前的启动错误仍在 stderr。

App 为 XPC 调用生成 `request_id`，daemon 将其保留在错误信封中，并通过 HTTP
的 `x-clumsies-request-id`、`x-request-id` 发送。代理可能替换后者，因此服务端
收到请求时同时记录客户端 ID 与服务端 ID。日志区分请求开始、收到响应头、
响应体读取或解码失败及完成。XPC 调用成功返回也可能携带 HTTP 错误状态。
XPC 超时不会撤销已经发往服务端的修改，重试前应按 ID 核查结果；缺少完成日志
不能证明请求从未到达。旧客户端可以省略 ID，旧 daemon 可以省略错误 `details`，
客户端仍能读取错误信封。

请求日志记录方法、API 资源类别、耗时、状态、字节数、草稿数量及安全的传输
原因和 OS 错误码；不记录 Memory 子路径、查询参数、请求或响应正文、令牌、
Cookie 及原始错误链字符串。原生登录、初始化和管理员恢复的请求与解码失败也
进入 App 公共边界。同步和运行回收任务记录首次失败、同类错误次数为 2 的幂次的重复失败
及恢复，错误类别或安全的原因字段变化时立即记录；HTTP 缓存回退有独立记录，每次实际 HTTP 尝试仍可查询。

设置的 **Support → Diagnostics → Export**、菜单栏的 **Export Diagnostics…**
和启动失败页会导出本地诊断目录。清单包含实例、收集时间、App 可执行文件哈希、
App/daemon 版本，以及缺失或不可读文件。只收集指定名称的 App、daemon、崩溃、
启动及开发日志，每个文件最多取末尾 4 MiB，不收集数据库、Memory 或 Keychain，
也不会自动上传。已有崩溃和旧日志按文件复制，不重新脱敏；日志文件不可用时应
检查导出清单。

请求或日志边界改动需要验证失败证据、关联、脱敏及保留上限，成功请求不能替代
这些检查。回归命令为 `cargo test -p daemon --test client_diagnostics`、
`cargo test -p daemon --lib --bins`、`cargo test -p server telemetry::tests --lib`
和 `just test-macos`，现有 Rust/macOS CI 会运行这些测试。超时测试使用 118 条
假草稿、回环服务器、临时目录和内存凭据存储，不接触真实账号或 Keychain。

## Project 绑定与两种 MCP 启动语义

daemon 以规范化工作目录查找当前 Server 下最具体的已绑定祖先；Git worktree 没有独立绑定时，还会尝试主 checkout 的仓库根。绑定只使用规范 `project_id`，不会把旧 `ws_id` 当作 Project ID。当前运行时不再读取或迁移 `~/.clumsies/config.toml`；目录绑定由 Desktop/daemon 明确维护。

两种启动方式不能混为一谈：

| 启动方式 | Project 解析与失败语义 |
| --- | --- |
| managed host-plugin：`mcp serve --host <host> --delivery host-plugin` | 启动时必须解析目录绑定并满足对应 delivery；失败即拒绝启动。每次 `tools/call` 前重新解析并要求仍是同一 Project，否则返回 `project_binding_changed`。Codex 的全局 Plugin 不要求仓库级 Adapter 行，但仍要求目录已绑定。 |
| 普通 `mcp serve` | 启动时先尝试目录绑定；解析失败时回退到 daemon 中 Desktop 当前选中的 Project。若两者都没有，代理没有有效 Project carrier，后续 Project 作用域调用不能正常完成。 |

普通入口的兼容 fallback 不能削弱 managed host-plugin 的严格边界。Desktop 当前选择也不能重定向一个已经由目录绑定到其他 Project 的 managed Agent 进程。

## Draft 写入与 Effective Memory

所有本地 Draft 操作先进入中心 SQLite，再尝试同步。队列支持 Memory 的 create、update、rename、delete 和 discard；连续编辑会复用同一 Draft，不会为每次按键创建一个 Server Draft。

删除权威资源会留下待 Review 的 deletion Draft；删除只由当前 Draft 新建的资源则折叠为 discard，因为 Ref 中从未存在该资源。每个 Draft 记录 Project carrier、目标 authority scope、Memory 身份、Base Commit、当前目标 Ref、freshness/reconciliation、本地与可选 Server Draft ID，以及有序操作。

MCP 的 update 不是整篇覆盖。Agent 必须提交 `load` 返回的完整资源 hash 与一个或多个精确 `old_text/new_text` 替换。daemon 在排除 Draft/Commit sync 的临界区内验证 hash、唯一匹配与不重叠，整批原子应用，最终只持久化 materialized 完整结果。

调用方未给 `base_commit_id` 时，daemon 从已安装 Organization Ref 读取；本地尚无 Ref 时保留空 Base，不伪造 Commit。`store` 只能创建 Project 承载、以 Organization 为发布目标的提案，不能选择 scope、决定 Review 或发布。

`activate` 与 `load` 使用同一 Effective Memory：

1. 读取最新安装的权威 generation；
2. 对每个 `open`/`submitted` Draft，从其 Base Commit 恢复资源并顺序应用操作；
3. 把完整 Draft 结果 overlay 到最新权威；
4. 其他资源保持最新 Commit 内容。

因此成功的本地 `store` 会改变下一次 Effective Memory hash，并触发相匹配的 search revision。Commit sync 可以更新 current Commit、freshness 和候选有效性，但不能改写 Draft Base、操作、正文或 lifecycle。

## Commit 同步与恢复

后台同步目标是目录绑定、活动 Draft Project 和 Desktop 当前 Project 的并集。它分别同步 Organization authority Ref 与 Project projection Ref：

```text
Server commit-state + ETag
  -> 校验 Ref
  -> 下载完整 Commit payload
  -> 校验 Blob address 与 Tree 所有权
  -> 在 staging 构建不可变 generation
  -> 原子 rename
  -> SQLite Ref transaction
  -> 叠加本地 Draft
  -> 构建/选择 Project 搜索索引
```

移动本地 Ref 前，daemon 还会补齐活动 Draft Base 所引用的 Commit、Tree 和 Blob，使旧 Base overlay 在缓存重建后仍可恢复。下载失败、payload 无效或 generation 不完整时，旧 Ref 和 Agent 可见文件保持不变。

Server 当前返回完整 Commit payload，不支持增量对象传输。不可变对象为重启和完整性校验保留；活动 Draft Base 是垃圾回收 root。错误码保持层次边界：Ref 未同步为 `project_ref_not_synced`，generation 缺失/损坏为 `commit_generation_missing`/`commit_generation_corrupt`，只有派生索引失败使用 search-index 错误。

Server 是 reconciliation 的规范执行者。候选绑定 Draft version、Base 和 Current；查看不修改 Draft，显式 rebase 才保存旧 revision 并重写操作。Draft 编辑或 Ref 前进会使旧候选失效。

## Project Local Storage

Project Local Storage 是由规范 Server authority 和 `project_id` 定位的本机缓存设置，不属于 Server Project 元数据，也不跨安装同步。未配置时使用：

```text
<daemon-cache>/projects/<authority-hash>/<project-id>
```

自定义目录只是父目录；daemon 只管理带 ownership marker 的子树：

```text
<selected-root>/.clumsies/cache-v1/<authority-hash>/<project-id>/
  ownership.json
  generations/
  search/index.sqlite
  staging/
```

更换位置会创建持久 move：在目标 staging 完成 generation 与索引构建和校验后，以 `expected_location_revision` CAS 切换。读取方在 write gate 成功前继续使用旧位置；重启会恢复未完成 move。切换后的清理失败只产生诊断，不回滚新位置。

Desktop 通过 `NSOpenPanel` 交付普通 bookmark；daemon 在自身签名身份下生成并持久化 security-scoped bookmark。它拒绝网络文件系统、符号链接、不安全嵌套、无效 marker、容量不足或不可写路径，目录/文件权限分别为 `0700`/`0600`。

自定义位置不可用时，daemon 不回退默认缓存，也不在 generation 不完整时推进 Ref。Draft 和同步队列仍在中心 SQLite 运行，但 `activate`、`load` 和 checkout 返回明确的 storage/search readiness 错误。Clear Cache 只删除 marker 所属 generation、search 数据和 staging，不删除 Draft、设置、模型或管理子树外的文件。

## AgentRun

AgentRun 与 lifecycle event 只保存在 daemon 本机，用于 Activity 与诊断。Host Hook 创建、续租和结束 run；过期 lease 会恢复为 ended。生命周期数据不上传 Server，也不改变 Memory。

## Activity / Recall 隐私边界

Activity 是只读的本地诊断视图。daemon 只为已绑定工作目录读取：

- `~/.dsh/sessions/<encoded-workspace>/<session>/session.jsonl.zstd`；
- `~/.codex/sessions` 与 `~/.codex/archived_sessions` 下的 Codex rollout。

投影会读取真实用户消息、`memory.activate` 的精确 query、tool result 和已返回 fragment，并可从本地 Retrieval Run 冻结快照打开当时的完整 fragment。它不导入通用聊天历史、assistant prose 或其他 tool，不修改日志、Memory 或 Retrieval Run。

这些内容只通过本机 XPC 提供给 Desktop。所有列表过滤都限制在绑定目录内；指定 Project 时同时校验目录归属。完整格式见 [Activity](/zh/recall)。

## 诊断与验证

Desktop 可通过 typed XPC 查看 daemon health、bootstrap、Project 配置、binding、Draft/Commit sync、MCP、Retrieval Run 与 storage move 状态，并显式触发 retry；Desktop 不直接修改队列表。

Server health 位于 `/api/v1/admin/health`。本地实现变更至少运行对应最小测试；覆盖本页主要边界的检查为：

```bash
cargo test -p daemon
cargo test -p server --lib axum_routes_match_public_and_admin_openapi
bun run build
```

其中文档构建只验证站点和链接，不能替代 XPC、Keychain 或 Commit 原子切换测试。
