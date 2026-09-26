# 本地运行时

先读[系统架构](/zh/architecture)与[端到端流程](/zh/flows)了解整
体设计。本页讲持久化的本地状态、同步、检索与恢复。
现象与恢复步骤见[排查问题](/zh/guides/troubleshooting)。

## 本地 daemon

`clumsiesd` 是 owner 作用域的 macOS launchd 服务，
由 macOS App 安装并启动。Desktop 通过 XPC 直接连接；
Agent 宿主以短时 MCP proxy 的方式启动同一个 App 内置可执行文件，
再由它通过 XPC 连接常驻进程。daemon 用一个中心 SQLite
数据库保存持久客户端状态，并在每个 Project 的活动本地存储里各有一个派生检索数据库。

该数据库当前保存：

- 安装身份与 schema 版本；
- Server URL 与所选 Project 配置；
- 来自 Server 权威的本地 Project 绑定，
  以及 workspace root 到 `project_id` 的映射；
- 本地 Draft 与有序操作；
- 同步状态、失败信息与 Server Draft 身份；
- 不可变的 Blob、Tree、Commit 元数据；
- 已安装的 Organization 权威与 Project 投影 Ref；
- 每个 Project 的活动检索 head 与存储位置 revision。

每个 Project 的检索数据库保存派生检索 revision、
完整的 Effective Memory 资源、Markdown 单元、FTS5 行与向量。
embedding 与 rerank 模型留在共享 daemon 缓存中，
不按 Project 复制。

文件权限仅 owner 可访问。access 与 refresh token 作为绑定
Server 的单个 generic-password 条目保存在 macOS 钥匙串。
SQLite 从不持久化任何 token，daemon 也没有明文凭据兜底。

## 检索模型准备

daemon 在第一个 MCP 请求之前就在后台准备模型。
embedding 使用锁定的 int8 版
`intfloat/multilingual-e5-small`，
rerank 使用 `Xenova/bge-reranker-base`。
当前下载量为 431,831,479 字节；revision、
产物大小与 SHA-256 校验和定义在
[模型清单](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/search/models.rs)。

下载支持续传。产物在 ONNX 加载之前校验，并缓存以供离线复用。
检索状态以 `preparing` 报告已下载字节与总字节。
准备完成之前 activation 立即返回 `search_model_preparing`；
它不会为一个未上报的下载长期占住 MCP 请求，也不会悄悄改用更弱的检索路径。

## 请求路径

原生 Swift 客户端通过 XPC 序列化类型化的能力请求。daemon 执行本地操作，
或向 Server 发送需认证的 HTTP 请求。

```text
SwiftUI/AppKit -> XPC -> daemon -> HTTPS -> Server
```

这样凭据留在 daemon 与 macOS 钥匙串中，不依赖 WebView 或 CORS。

## 客户端诊断

App 把 JSON lines 写入其配置的 daemon 日志目录中的
`client.log`（稳定安装为 `~/Library/Logs/ai.clumsies`）。
从 Finder 启动与从开发脚本启动都有效。常驻 daemon 把结构化事件写入
`daemon.log`，不再把每个事件复制到 launchd stderr。
每个主日志与 `clumsiesd.crash.log` 保留当前文件加三个归档，
每个上限 4 MiB。升级前遗留的超大文件会被裁剪到尾部。管理命令不与常驻写入者共用文件。
写入失败会报告给 OSLog（App）或 stderr（daemon）；
stderr 还保留 daemon 日志启动之前发生的 bootstrap 错误。

Desktop 为 XPC 调用生成 `request_id`。
daemon 把它保留在错误信封中，并在 HTTP 请求上同时发送
`x-clumsies-request-id` 与 `x-request-id`。
Server 入口日志同时保留客户端 ID 与 Server 自己的 ID，
因为代理可能替换 `x-request-id`。HTTP 日志区分请求开始、
收到 headers、body 与解码失败、以及完成。
一次已完成的 XPC 调用仍可能携带不成功的 HTTP 状态。
XPC 超时不会取消已经派发给 Server 的写入；重试之前请先核对匹配的 ID。
只缺少完成日志不能证明请求从未到达。旧客户端可能不带该 ID，
旧 daemon 可能不带错误 `details`；信封仍可读。

请求诊断记录 method、API 资源族（不含嵌套 Memory 路径或查询串）、耗时、状态、
字节与 Draft 计数，以及安全的传输原因与 OS 码。它们不记录 body、
authorization header、cookie 或原始错误来源字符串。原生登录、
初始化与恢复的请求及响应解码共用同一个 App 失败边界。
sync 与 reaper worker 记录首次失败、相同安全错误签名的 2 的幂次重复，
以及恢复事件；错误签名变化会立即记录；陈旧 HTTP 缓存兜底会显式标注。
每次单独的 HTTP 尝试仍然可见。

**Settings → Support → Diagnostics → Export**、
菜单栏的 **Export Diagnostics…**、
以及启动错误页都会生成一个本地诊断文件夹。其 manifest 标明实例、收集时间、
App 可执行文件哈希、App 与 daemon 版本，以及缺失或不可读的文件。
只包含指定的 App、daemon、crash 与 bootstrap 及开发日志文件，
每个最多 4 MiB；数据库、Memory 与钥匙串数据不会被收集。
既有的 crash 与历史记录按日志复制，不重新脱敏。不会自动上传任何内容。
如果某个文件写入不可用，请查看 manifest。

回归证据位于
`crates/clumsiesd/tests/client_diagnostics.rs`、
daemon 诊断与 XPC 单元测试、Server telemetry 测试，
以及 macOS 的 `ClientDiagnosticsTests` 与
`NativeServerBootstrapTests`。改动请求或日志边界时，除了成功请求，
还必须验证失败证据、关联、脱敏与保留。运行
`cargo test -p clumsiesd --test client_diagnostics`、
`cargo test -p clumsiesd --lib --bins`、
`cargo test -p server telemetry::tests --lib` 与
`just test-macos`；这些都由既有的 Rust 与 macOS CI 作业覆盖。
超时探针使用 118 个假 Draft、一个 loopback server、
临时存储与内存凭据存储。

## Draft 同步

每个本地操作都会先持久化，再尝试同步。队列支持统一模型中 Memory 资源的 create、
update、rename、delete 与 discard。

删除一个权威资源会保留一个待合并的删除 Draft，直到 Review 合并。
删除一个仅由当前 Draft 创建的资源则取消失效：daemon 记录一次 discard，
该 Draft 直接离开 Effective Memory，
不会为一个从未在 Ref 中存在过的资源生成删除提案。

每个 Draft 携带：

- `project_id`；
- 权威 `scope`（每个新 Draft 都是 `org`；
  `project` 只在丢弃历史本地行时接受）；
- 统一的 Memory 身份（id 或 path；没有三类型 kind）；
- `base_commit_id`；
- 当前已安装的目标 Ref Commit；
- 派生出的 freshness 与 Server 对账投影；
- 本地 Draft ID 与可选的 Server Draft ID；
- 有序操作历史。

sync worker 自动启动，在新操作到达或配置变化时唤醒，并重试失败的工作。
本地 Draft 会在连续编辑之间复用，因此重复写入不会为每次按键创建一个 Server
Draft。

## MCP 写入路径

由适配器管理的 MCP 入口是 `clumsiesd mcp serve`。
该进程只负责有界的 JSON-RPC 组帧、类型化的 `memory` 契约、
Project 绑定与 XPC 转发。在接受 Agent 流量之前，
它先核对自己的 Agent runtime 协议 revision 与 build
身份是否与常驻 daemon 一致。它不会初始化 `DaemonState`、
打开 SQLite、加载模型或启动后台 worker。

MCP 保持公开的 `memory`（`op.store`）工具形状。
内部它把当前绑定的 Project 作为 Draft 载体，
并把 Organization 权威标记为提案目标。这是两条独立的轴：
Project 拥有合并前的 overlay，`org` 描述一次获批 Review
最终可能推进的 Ref。进程启动时，MCP 把自己的当前工作目录交给 daemon；
daemon 规范化该路径并在 SQLite 中解析最近的已绑定祖先。
Codex 宿主插件 proxy 会在每次 `tools/call` 之前重复该解析，
并要求它保持同一个 Project；全局 Plugin 不要求存在 project
Adapter 行。MCP 永远不把旧的 Workspace ID 当作 Project ID。
Rust MCP 契约测试会在这些信封被映射为类型化 daemon 请求之前，
先验证它们确切的面向 Agent 形状。

来自 Agent 的更新是精确文本替换，不是整篇文档写入。MCP 转发资源 ID、
`load` 返回的完整资源哈希，以及一对或多对 `old_text` 与
`new_text`。在 Draft 与 Commit 同步被排除期间，
daemon 解析当前 Effective Memory 资源，校验哈希与唯一的非重叠匹配，
然后原子地应用整批替换。只有物化后的完整结果会作为普通的 Draft update 操作持久化，
因此 Server 同步与 Commit 存储保持独立于面向 Agent 的编辑协议。

只有在不存在 daemon 绑定时，才会使用旧的
`~/.clumsies/config.toml` 条目。
MCP 用显示名匹配已登录用户在 Server 上的 Project，
把唯一的规范 `project_id` 持久化到 daemon，并从旧文件中移除已迁移的路径。
匹配缺失或重复都会显式失败；旧的 `ws_id` 值永远不会发送给 daemon。

当调用方省略 `base_commit_id` 时，daemon 在创建本地 Draft 之前，
从已安装的 Organization Ref 读取它。
Ref 缺失会产生一个没有 base 的 Draft；
daemon 从不凭空编造 Commit ID。

MCP 不暴露可由调用方选择的 scope、Review 决定、
merge 或 publish 操作。`store` 只能创建由 Project 承载的提案。
合并之前它只影响该 Project 的 Effective Memory overlay；
Organization 权威只在 Org 管理员批准并合并该 Review 之后才变化。
Organization 是权威命名空间，不是合成的 Project。

daemon 在 `activate` 与 `load` 之前，
把已安装的权威 generation 与当前 `open` 或 `submitted` 的
Draft 操作组合起来。对于带 Draft 的资源，
它从该 Draft 的 Base Commit 恢复该资源、应用有序操作，
并把完整的 Draft 结果覆盖到最新的已安装权威之上；其余资源来自最新 Commit。
对 create、update、rename 与 delete 一视同仁。
因此一次成功的 `store` 会改变下一次的 Effective Memory 哈希，
并让下一次 activation 构建或选择匹配的检索 revision。

## 同步与对账

Draft 同步与 Commit 同步相互独立。Commit 同步可以更新本地 Ref、
`current_commit_id`、freshness 与候选有效性，
但绝不改变 Draft 的 Base、操作、内容或生命周期。
behind 的 Draft 仍可编辑、可同步、可安全重启，并对 MCP 可见。

Server 是规范的对账执行者。候选绑定 Draft ID、Draft version、
Base Commit 与当前 Commit，状态为 `clean` 或
`conflicts`。创建或查看候选本身不会改动 Draft。
显式 rebase 会保存先前的 Draft revision，
并把 Draft 改写为 `base = Current` 加上
`operations = diff(Current, confirmed result)`。
任何 Draft 编辑或 Ref 前进都会让旧候选失效。

## Commit 同步

daemon 按后台间隔并通过显式重试，同时同步 Organization 权威 Ref
与每个 Project 投影 Ref。目标集合是持久目录绑定、
有活动 Draft 的 Project，以及 Desktop 当前所选 Project 的并集。
Desktop 选择属于 UI 状态，不能让另一个目录中的 MCP 进程改道。

每个同步周期先读取实时的完整 `/api/v1/me` 成员列表。
目标集合在发送 Draft 操作或获取 Project Commit 之前与该列表求交。
身份读取失败或格式错误会中止该周期；daemon 绝不替换为空列表或陈旧的 HTTP 缓存。
成员关系限定在当前登录会话。Organization Ref 使用该身份的
Organization ID，即使没有任何 Project 也会继续同步。

已本地绑定或存在活动 Draft、但不在成员关系中的 Project 会通过
`sync_status.unavailable_projects`
报告（`project_id`、`bindings`、`draft_count`）。
它们的操作被保留、不计入活动队列与错误计数，并在访问恢复后自动继续。
不可用的 Desktop 选择会被清除；目录绑定、Draft 历史与缓存文件都会保留。
来自不可用 Project 的 Draft 事件会被推迟，且不会阻塞作者的 feed。
一个持久标记会让成员关系恢复后重放一次 feed，因此推进游标不会在重启后丢失这些事件。
MCP 绑定解析在核查可用性之后报告 `project_binding_unresolved`。
Desktop 的 Inbox 提供本地绑定移除与保留 Draft 的 JSON 导出，
且不依赖远端项目目录。

仅凭一个 Project 的 HTTP 404 不能断定它已被删除：鉴权同样可能把它隐藏。
Commit-state 处理会在要求 ETag 之前先确认 HTTP 成功，
因此 HTTP 错误保留其真实状态；而没有 ETag 的成功响应仍是协议错误。

```text
Server commit-state + ETag
  -> validate Ref identity
  -> download Commit payload
  -> verify Blob addresses and Tree ownership
  -> build an immutable project generation
  -> move the local SQLite Ref
  -> daemon combines that generation with local Drafts
  -> MCP asks daemon to activate fragments or load complete resources
```

在移动本地 Ref 之前，每次同步还会检查活动的 `open` 与 `submitted`
Draft，并抓取它们的 `base_commit_id` 所引用而尚缺的 Base
Commit、Tree 与 Blob 载荷。这样在缓存重建之后仍能保留旧 Base 的
overlay，而不必把项目其余部分钉在那个 Base 上。

generation 在临时目录中构建，并在 Ref 事务提交之前完成重命名。下载失败、
载荷无效或 generation 不完整时，先前的 Ref 与 MCP 可见文件保持不变。
`commit_sync.server_cursor` 是已安装的 Project
Commit ID，不是伪造的时间戳或独立 revision。

Server 目前发布完整的 Commit 载荷，因此尚未实现增量对象传输。
已缓存的不可变对象会保留用于重启与完整性检查。活动 Draft 的 Base 引用是保留根，
不能被垃圾回收。

缓存诊断保持层级边界：未知的本地 Ref 报告
`project_ref_not_synced`；
缺失或无效的物化 generation 报告
`commit_generation_missing` 或
`commit_generation_corrupt`；
只有派生检索索引的准备与构建失败才使用检索索引错误码。

## Project 本地存储

Project Local Storage 是按规范化的 Server 权威与规范
`project_id` 索引的安装本地缓存设置。
它不属于 Server 的 Project 元数据，也不会同步到其他安装。
设置缺失时解析为
`<daemon-cache>/projects/<authority-hash>/<project-id>`。
既有的 `projects/<project-id>` generation
会被一次性迁入该按权威划分的布局，并收紧其权限；daemon 不会同时维护两种布局。

对自定义位置而言，所选目录只是一个父目录。daemon 拥有下面这棵子树，
绝不把它当作可编辑的工作目录：

```text
<selected-root>/.clumsies/cache-v1/<authority-hash>/<project-id>/
  ownership.json
  generations/
  search/index.sqlite
  staging/
```

变更位置会触发一次持久的 daemon 迁移。daemon 在目标 staging
下物化并校验各 generation 与 Project 检索索引，
然后在与 Commit 安装相同的同步边界内，用
`expected_location_revision` CAS 切换位置。切换取得写入门之前，
读取方仍使用旧位置。重启会继续任何未完成的迁移；切换成功后的清理失败只是诊断信息，
不会回滚新的活动位置。

macOS App 使用 `NSOpenPanel` 为所选目录创建一次性普通
bookmark。daemon 在 Desktop 运行期间解析该交接，
以 daemon 自己的代码签名身份创建 security-scoped bookmark，
并只持久化这个 daemon 自有的 bookmark。这是必需的：
Desktop 创建的 app-scoped bookmark 无法被单独签名的
LaunchAgent 解析。daemon 在每次文件系统操作时持有
security-scoped 访问权，并刷新过期的持久 bookmark 数据。
它拒绝网络文件系统、符号链接、无效的 ownership 标记、不安全的嵌套，
以及没有容量或写权限的路径。受管目录与文件使用 `0700` 与 `0600` 权限。

不可用的自定义位置永远不会回退到默认缓存，也不会在没有完整 generation
的情况下推进本地 Project Ref。Draft、
操作队列及其同步仍继续在中心 SQLite 中进行。同一个位置恢复可用之前，
`activate`、`load` 与 checkout 都会返回显式的存储或检索就绪错误。

daemon IPC 方法有 `project_storage`、
`replace_project_storage`、
`project_storage_move`、`reset_project_storage`
与 `clear_project_cache`。
Clear Cache 只删除由标记拥有的 generation、检索数据与 staging；
Draft、设置、模型以及受管子树之外的文件都会保留。

## 诊断

Desktop 可以通过类型化 XPC 请求读取 daemon 健康、bootstrap 状态、
Project 配置、同步状态、Draft 列表、Draft 详情与操作结果。
它可以请求显式重试，但不会直接改队列行。

Settings → Support 打开日志文件夹。
Memory Search History 列出活动 Project 最近的 Run，
并按需加载一次完整 trace。候选列显示 exact/BM25、vector、RRF、
reranker、最终排名、分数、排除原因与 activation delta 动作。
成功的 Run 可以加入本地 Evaluation Set、按 0–3 打分、
补充遗漏的资源证据、导出为带版本的 fixture，或在清除未固定历史时保留。

本地方法有 `list_retrieval_runs`、`get_retrieval_run`、
`create_evaluation_case`、
`replace_evaluation_judgments`、
`clear_retrieval_runs` 与
`export_evaluation_set`。检索历史属于中心 daemon 状态，
并与 Project Local Storage 保持独立。
保留策略为每个 Project 保留最近 500 个未固定 Run；
Evaluation Case 会固定其来源 Run 与不可变语料。
见[检索与评估](/zh/retrieval-evaluation)。

Server 诊断位于 `/api/v1/admin/health`。数据库、schema、
Commit 服务与 OIDC 分别作为独立组件报告。
