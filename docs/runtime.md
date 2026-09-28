# 本地运行时

> 文档属性：详细设计型｜L3 具体规范｜面向工程实现、运营保障与质量审计。

`clumsiesd` 是当前用户作用域的 macOS launchd 常驻服务。Desktop 负责原生交互；daemon 在 Desktop 关闭后继续持久化 Draft、同步 Commit、构建检索索引并服务 Agent。Agent Host 启动同一个 App 内签名二进制的短进程 `mcp serve` 或 `_agent issue-run-event` 代理，再通过 XPC 调用常驻 daemon。

## 本地状态所有权

daemon 使用一个中心 SQLite 数据库；当前 schema version 为 `41`。它保存：

- 安装身份、schema version、Server URL 和 Desktop 当前选中的 Project；
- 规范化的工作目录到 `project_id` 绑定；
- 本地 Draft、有序操作、同步状态和 Server Draft 身份；
- Blob、Tree、Commit 元数据以及已安装的 Organization / Project Ref；
- Project Local Storage 位置、revision 和 move 状态；
- `native_issues` 本地看板副本、依赖/阻塞事实、AgentRun 与生命周期事件；
- Project Episode outbox manifest、待传 Evidence BLOB、Server 当前摘要副本和每 Project corpus revision；
- Retrieval Run、Evaluation Case 和相关诊断状态。

每个 Project 的派生检索数据库位于该 Project 的活动 Local Storage 中，保存 Semantic
Effective Memory、当前 Episodic summary、统一检索 unit、FTS5 行、vector 和 search
revision。embedding/reranking 模型只保存在 daemon 共享缓存，不按 Project 复制。

本地文件权限为 owner-only。access/refresh token 只作为绑定 Server URL 的一个 generic-password 条目保存在 macOS Keychain；SQLite 和文件系统没有明文凭据兜底。

## 请求路径

```text
Desktop (Swift) -> typed XPC -> resident daemon -> HTTPS -> Server
Agent Host -> stdio MCP / Hook -> signed short proxy -> typed XPC -> resident daemon
```

短进程只负责有界 framing、两项 MCP tool（`memory`、`kanban`）、Project 选择与 XPC
转发。`memory` 的 `evidence` operation 也只是注入当前 Project 并转发稳定
`episode_id`；短进程不读取宿主文件。它不初始化 `DaemonState`，不打开 SQLite，不加载
模型，也不启动后台 worker。启动时，代理必须验证自身协议 revision 和 build identity 与
常驻 daemon 一致。

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

因此成功的本地 `store` 会改变下一次 Semantic corpus hash，并触发相匹配的 search
revision。Server 当前 Episode summary 变化也会改变统一 `MemoryCorpus` hash；两种来源
共用一个 Project index，但各自保留 `memory_system`、authority 和 provenance。Commit
sync 可以更新 current Commit、freshness 和候选有效性，但不能改写 Draft Base、操作、
正文或 lifecycle。

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

自定义位置不可用时，daemon 不回退默认缓存，也不在 generation 不完整时推进 Ref。Draft
和同步队列仍在中心 SQLite 运行，但 `activate`、`load` 和 checkout 返回明确的
storage/search readiness 错误。Clear Cache 只删除 marker 所属 generation、search 数据和
staging，不删除 Draft、Episode 上传 outbox、设置、模型或管理子树外的文件。Server 已
ACK 的 Episode summary 可再次同步，Evidence 详情始终从 Server 读取。

## Kanban 同步与 AgentRun

共享边界如下：

| 对象 | 权威与用途 |
| --- | --- |
| Kanban Issue、assignee、`content_revision` | Server `kanban_issues` 共享权威 |
| Issue claim | Server `issue_claims` 短期 lease 权威 |
| `native_issues` | daemon 本地副本、离线执行状态和 UI/MCP 投影输入 |
| AgentRun / lifecycle event | daemon 本地遥测；不直接推进 Issue |

第一次把本地看板接入共享 Server 时，daemon 只在 Server Issue 集合为空时导入本地 Issue；Server 已有数据时不反向覆盖。正常 list 先取 Server snapshot 并同步 `native_issues`，再叠加本机 AgentRun、latest run 和 stale 投影。没有 ready Server 配置时可使用本地副本；已配置 Server 但网络请求失败时，操作会显式失败，不能把旧副本伪装成已同步结果。

Issue 内容更新使用 Server `content_revision` CAS。begin/resume 在 Server ready 时先取得 claim，再修改本地 workflow；本地步骤失败会尽力释放 claim。pause、unclaim 和 request-closure 在本地变化后发布 Issue，并释放相应 claim。AgentRun 始终由 Host lifecycle Hook 创建和续租，Server claim 只做跨安装互斥。

当前 Kanban 没有像 Draft 那样的持久 outbox。这意味着部分 mutation 会先成功写入 `native_issues`，随后因 Server PUT/DELETE 失败而向调用方报错；后续 list 会重新以 Server snapshot 投影本地状态，失败的本地变化不会自动补发。claim release 失败时，租约可能保留到 `lease_expires_at`。这是当前已知恢复限制，不应描述为强离线同步保证。

## AgentRun Episode 摄取与联合检索

AgentRun 仍由 lifecycle Hook 记录。后台 worker 对已绑定 Project 的结束或恢复结束 root
Run 扫描宿主源数据，以 `run_id` 为 Episode 边界；当前 Codex 路径使用
`thread_history_1.sqlite` 中与 Hook `host_run_key=root:<turn_id>` 精确匹配的
`thread_turns/thread_items`，保留 provider-native 有序 item，不另造 ProductionEvent。
没有稳定映射的 subagent 不会被猜测性摄取。

摄取把 manifest 与不超过 1 MiB 的 canonical Evidence BLOB 原子写入中心 SQLite 的同一
outbox 行。manifest 和 Server 摘要副本都绑定 canonical Server URL 与 `project_id`，防止
切换 Server 或 Project 时串读。Evidence hash 使用与 Server 相同的 canonical JSONL；网络
中断、进程重启和重复扫描只会重试同一 `project_id + run_id + evidence_hash`。只有 Server
已耐久 ACK 后才在同一行清空正文并保留 manifest；摘要暂不可用不会阻止 Evidence ACK。

daemon 按 Project corpus revision 同步 Server 当前摘要。`NO_MEMORY` 与已删除 Episode 不
进入索引；其他当前摘要和 Effective Memory 通过 `MemoryCorpus` 一次装配、使用同一物理
索引。索引 fingerprint 只包含 corpus、解析/分块、embedding/tokenizer 等会改变物理索引
的输入；BM25/RRF/reranker、候选数和预算属于 retrieval profile，单独变化不重算
embedding。`memory.activate` 的结果以 `semantic|episodic`、`organization|project`
区分来源；Episode 还返回 `episode_id`、源 `run_id`、`activity_at` 和 `evidence_hash`。

`memory.evidence` 只接受 `episode_id` 和可选 cursor；daemon 从当前 binding 注入
`project_id`，绕过通用 Server response cache，并让 Server 每次重验当前 membership。
响应分页且限制字节，正文标记为不可信历史数据，不会被自动当作 Agent 指令执行。

## Activity / Recall 隐私边界

Activity 是只读的本地诊断视图。daemon 只为已绑定工作目录读取：

- `~/.dsh/sessions/<encoded-workspace>/<session>/session.jsonl.zstd`；
- `~/.codex/sessions` 与 `~/.codex/archived_sessions` 下的 Codex rollout。

投影会读取真实用户消息、`memory.activate` 的精确 query、tool result 和已返回 fragment，并可从本地 Retrieval Run 冻结快照打开当时的完整 fragment。它不导入通用聊天历史、assistant prose 或其他 tool，不修改日志、Memory、Issue 或 Retrieval Run。

Activity 投影本身仍只通过本机 XPC 暴露给 Desktop，不上传 Server，也不进入 Organization
Memory。与之不同，Episode 摄取只截取已绑定 Project 的已结束 AgentRun Evidence，经过
durable outbox 后进入受组织治理的 Server 长期数据；这不是把 Activity 页面或 whole
Session 上传。正常 App 列表从 binding 表枚举 workspace，完整历史片段读取也校验
Project；但底层 `ListRecallsRequest.workspace_root` 当前只规范化显式路径而未验证该路径
已绑定，本机 XPC 调用方仍可读取对应日志投影。移除 binding 会让目录离开正常 Activity
列表，但在修复前不能视为所有底层列表调用都已撤销读取能力。完整格式、兼容解析和缺口
见 [Activity](/recall)。

## 诊断与验证

Desktop 可通过 typed XPC 查看 daemon health、bootstrap、Project 配置、binding、Draft/Commit sync、MCP、Kanban、Retrieval Run 与 storage move 状态，并显式触发 retry；Desktop 不直接修改队列表。

Server health 位于 `/api/v1/admin/health`。本地实现变更至少运行对应最小测试；覆盖本页主要边界的检查为：

```bash
cargo test -p daemon
bun run api:check
bun run build
```

其中文档构建只验证站点和链接，不能替代 XPC、Keychain、Commit 原子切换或 Kanban 并发测试。
