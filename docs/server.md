# Server

> 文档属性：详细设计型｜L3 具体规范｜面向工程实现、运营保障与质量审计。

Server 是 clumsies 可部署的共享权威服务。`Hub` 只是 Desktop 早期对 Organization 作用域的历史称呼，不是另一项服务；Rust 二进制和容器统一称为 Server。

## 职责与边界

Server 负责：

- Organization、Project、成员、角色和会话授权；
- Organization Memory 权威、Project 的 Organization Memory 选择及其 Commit 投影；
- 个人 Bundle（`resource_ids`）；
- Draft、操作历史、有序多 Draft Review、决定、评论和原子合并；
- 不可变 Blob、Tree、Commit、Organization 权威 Ref 与 Project 投影 Ref；
- Server 共享的 Kanban Issue、成员 assignee、短期 lease claim；
- Project Episode、不可变 Evidence、版本化摘要与 Project 摘要策略；
- 管理配置、token 撤销、审计事件和健康检查。

Server 不负责本机工作目录、目录到 Project 的绑定、macOS bookmark、检索模型和 Project Local Storage。这些状态属于 daemon。Desktop 和 MCP 只能先把 Draft 写入 daemon，再由 daemon 同步；客户端不能绕过 Draft/Review 直接修改 Memory 权威。AgentRun 仍是本地执行遥测；只有结束或恢复结束的已绑定 root AgentRun 被摄取后，才会派生出独立的 Server `ProjectEpisode`，二者不能混用。

## Memory 权威与版本模型

权威图使用与 Git 相同的不可变对象关系：

```text
Blob -> Tree -> Commit -> Ref
                    ^
Draft(base_commit_id)
```

每个 Organization 有一个权威 Ref。每个 Project 有独立版本的投影 Ref，内容由该 Project 的 Organization Memory 选择和对应的 Organization 权威版本生成。Project 元数据 revision、Organization Ref 和 Project Ref 是三条不同的并发边界，不能互相替代。

Project 当前选择通过 `/api/v1/projects/{project_id}/org-selections` 管理。daemon 通过 `/api/v1/projects/{project_id}/commit-state` 和 Commit payload 安装 Project 投影，再在本地叠加该 Project 的 `open`/`submitted` Draft，才得到 Agent 实际读取的 Effective Memory。

需要特别区分旧接口：`GET /api/v1/projects/{project_id}/memories` 及其详情路由只读取遗留的 `scope=project` 权威行，不是 Project 选择投影，也不是包含 Draft overlay 的 Effective Memory。当前主链不能用这组接口解释 Project 的有效视图。

Organization 管理员可用 `GET /api/v1/admin/memory-export` 导出全部 Organization Memory（包括历史 `issues/` 路径）、Draft 及原始操作、Project 选择和个人 Bundle，作为可重复验证的迁移输入。

## Draft、Review 与合并

Draft 生命周期（`open`、`submitted`、`merged`、`discarded`）与 freshness（`current`、`behind`）以及 reconciliation（`unknown`、`clean`、`conflicts`）相互独立。Ref 前进时，Server 不修改 Draft Base 和操作。

一个 Review 可以按顺序包含多个 Draft。创建或重新提交 Review 时，Server 校验每个 Draft 的所有者、状态、版本和候选；批准时在同一事务中按顺序应用全部 Draft，只生成一个结果 Commit 并推进目标 Ref。任何 Draft 不可发布都会使整次决定失败，不会留下部分合并。

Project 成员可以创建、提交、查看和评论 Review。只有 Organization owner/admin 可以批准或拒绝 Organization 发布。批准记录决定并原子推进 Ref；历史 `Approved` Review 仍可走兼容 merge 路由。

reconciliation 候选绑定 Draft ID、Draft version、Base Commit 和 Current Commit：

- 查看候选不修改 Draft；
- `clean` 候选只能应用 Server 计算出的结果；
- `conflicts` 候选允许用户提交完整的已解决结果；
- rebase 先保存不可变 Draft revision，再改写为 `base = Current` 与 `diff(Current, confirmed result)`；
- Draft 编辑或 Ref 前进都会使旧候选失效。

合并持有目标 Ref 锁，并以 `If-Match`/CAS 作为最终并发保护。版本冲突、候选失效或任一校验失败时，事务不推进 Ref，原 Draft 仍可继续检查和协调。

## Kanban 共享权威

`kanban_issues` 是 Project 看板的共享持久权威，保存稳定 Issue 身份、1–999 的 Project 内编号、Project 成员 assignee、内容快照和 `content_revision`。更新使用 revision CAS；assignee 必须仍是该 Project 成员。

`issue_claims` 是带到期时间的执行租约，以 `(project_id, issue_id)` 唯一。Server 只允许当前 claimant/run 续租或释放；未过期的其他 claim 会阻止并发认领。daemon 的 `native_issues` 是本地副本和离线执行状态，AgentRun 保持本地。共享 Issue、claim 与本地投影的具体运行语义见 [Issue 看板设计](/issue-board-design)。

## Project Episodic Memory

`ProjectEpisode` 以 `project_id + run_id` 为身份边界，保存宿主、Session 分组、源活动时间、
Evidence 格式、hash、状态及当前摘要 revision。`EpisodeEvidence` 按源顺序作为 PostgreSQL
`TEXT` 记录保存，由 PostgreSQL TOAST 管理大值压缩；它不是 Project Local Storage 或某台
daemon 的私有归档。每个 Episode 最多 50,000 条、canonical JSONL 最多 1,000,000 bytes。

Finalize 使用 `project_id + run_id + evidence_hash` 幂等：相同内容返回同一 Episode；同一
Run 的身份、格式或 hash 冲突会显式失败。Server 先事务性保存 Evidence，再运行摘要。
摘要 revision 永远绑定该次 `evidence_hash`、固定 `summary_algorithm_revision` 和 Project
policy revision；无长期检索价值时保存 `NO_MEMORY`，Evidence 仍可核验或以后重建。

摘要引擎的 system constraints 固定在 Server，Evidence 始终作为不可信输入；Project
policy 只能补充重点、详细程度和术语，不能覆盖安全与事实约束。修改 policy 只影响以后
生成的摘要；预览不持久化，旧摘要只有显式 rebuild 才更新，`activity_at` 始终来自源活动。
启用 Responses-compatible executor 会把该 Episode Evidence 发送到组织配置的 provider；
请求携带 `store: false`，但组织仍须把该 provider 纳入自身的数据处理与合规边界。

Project 成员可列出 Episode 和分页读取 Evidence；默认列表是带删除 tombstone 的增量
change feed，`recent=true` 返回按活动时间倒序的当前 Episode。policy 修改、预览、重建和
删除使用 Project 管理权限。Evidence cursor 是 `sequence:byte_offset`，每页限制 1–200 个
segment，`max_bytes` 约束最终序列化 JSON 响应的 256–262,144 bytes；巨型 UTF-8 记录跨页
继续而不静默截断。每次读取留下不含正文的审计记录，响应明确携带 `untrusted: true`。删除
在同一事务内写 tombstone、推进 corpus
revision 并清除 Evidence/摘要正文，随后详情返回 404；Project 删除通过外键级联。
第一版的保留策略是明确的长期保留：在 Organization 管理员删除 Episode 或删除其 Project
之前不自动过期。它不另设尚无实际策略需求的定时清理器；数据库备份与恢复覆盖这段完整
生命周期。

## HTTP 契约

| 契约 | 范围 |
| --- | --- |
| `packages/api-contract/openapi/clumsies.public.v1.yaml` | Desktop 与 daemon 使用的产品 API |
| `packages/api-contract/openapi/clumsies.admin.v1.yaml` | Web Admin API |
| `packages/api-contract/openapi/clumsies.daemon.v1.yaml` | 本地 daemon IPC 能力清单 |

OpenAPI 是预期的 wire contract 来源，但当前存在一个已知实现缺口：Public OpenAPI 的 `TreeEntry.type` 仍声明 `rule/context/workflow/project_org_selection`，且没有 `description`；Rust/数据库当前实际模型是 `memory/project_org_selection` 并携带 `description`。在契约修复并重新生成客户端前，不能把生成的 TypeScript 类型视为这部分实现的准确描述。

## 身份与凭据

Desktop 在系统浏览器发起 Organization OIDC 登录，以临时 `127.0.0.1` 回调和 PKCE S256 接收授权码；Server 负责 provider discovery、JWKS、issuer、audience、nonce、签名和过期校验。Server 只在 PostgreSQL 保存 opaque access/refresh token 的哈希，refresh token 每次使用都会轮换。

原生 Swift 客户端完成授权码交换并短暂取得 token pair，然后通过 XPC 交给 daemon。daemon 将 pair 作为绑定 Server URL 的单个 generic-password 条目写入 macOS Keychain；SQLite 和 Project Local Storage 不保存 token。之后由 daemon 为 Server 请求注入 bearer token；遇到 `401` 时最多轮换一次 refresh token 并重试一次。daemon API 返回的配置只暴露凭据是否存在，不回传凭据正文。

## 本地运行

本地开发由 Docker 运行 PostgreSQL 和确定性的 fake OIDC provider，Rust Server 原生运行：

```bash
bun run dev:server
```

| 服务 | 地址 |
| --- | --- |
| Server | `http://127.0.0.1:18080` |
| PostgreSQL | `127.0.0.1:5432` |
| Fake OIDC | `http://127.0.0.1:18081/clumsies` |
| Health | `http://127.0.0.1:18080/api/v1/admin/health` |

fake provider 使用锁定为 `4.0.0` 的 NAV mock OAuth2 server，默认身份为 `owner@clumsies.local`。它仍覆盖 discovery、授权码、PKCE、签名 ID token、JWKS 与 nonce 校验，但不会进入 `compose.production.yml`。停止本地依赖使用：

```bash
bun run dev:infra:down
```

## 生产运行

复制 `.env.example` 为 `.env`，配置 Organization OIDC，并启动 `compose.production.yml`。`CLUMSIES_PUBLIC_ORIGIN` 必须是 Server 的规范 HTTPS origin；在 IdP 注册由它派生的 `/login/oauth2/code/oidc`。同一 origin 提供 Public API、Admin API、Web Admin 和 OIDC callback。

OIDC 变量为空时，Server 为基础设施诊断仍可启动，但 health 会把 OIDC 标为 `down`，登录不可用；这不是可用的生产状态。

Episode Evidence 不需要额外对象存储服务，随 PostgreSQL 一起备份恢复。启用内置摘要
执行器时配置：

| 变量 | 含义 |
| --- | --- |
| `CLUMSIES_EPISODE_SUMMARY_API_KEY` | Responses-compatible API key；缺失或为空时禁用自动摘要，finalize 仍耐久 ACK 并保持 `pending_summary` |
| `CLUMSIES_EPISODE_SUMMARY_MODEL` | 启用摘要时必填的模型名 |
| `CLUMSIES_EPISODE_SUMMARY_BASE_URL` | 可选，默认 `https://api.openai.com/v1`；Server 固定调用 `<base>/responses` |

未配置摘要执行器不会丢弃 Evidence；配置恢复后可从 Desktop 对 pending Episode 显式
rebuild。第一版没有模型选择器、prompt/storage plugin 或多阶段摘要 DAG。

## 验证

```bash
bun run api:check
cargo test -p server
cargo test -p daemon
```

Server 与 daemon 集成测试使用真实 PostgreSQL Testcontainer。发布流程还必须验证生产镜像和 Compose health；仅有文档构建不证明认证、Review 或 Kanban 并发语义正确。
