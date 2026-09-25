# Server

本页面向需要部署或修改 Server 的开发者。先读[系统架构](/zh/architecture)和[领域接口](/zh/reference/domain-api)，再查这里的服务端职责与运行方式。

Server 是 clumsies 可部署的共享权威服务。`Hub` 只是 Desktop 早期对 Organization 作用域的历史称呼，不是另一项服务；Rust 二进制和容器统一称为 Server。

## 职责与边界

Server 负责：

- Organization、Project、成员、角色和会话授权；
- Organization Memory 权威、Project 的 Organization Memory 选择及其 Commit 投影；
- 个人 Bundle（`resource_ids`）；
- Draft、操作历史、有序多 Draft Review、决定、评论和原子合并；
- 不可变 Blob、Tree、Commit、Organization 权威 Ref 与 Project 投影 Ref；
- 管理配置、token 撤销、审计事件和健康检查。

Server 不负责本机工作目录、目录到 Project 的绑定、macOS bookmark、检索模型、检索历史和 Project Local Storage。这些状态属于 daemon。Desktop 和 MCP 只能先把 Draft 写入 daemon，再由 daemon 同步；客户端不能绕过 Draft/Review 直接修改 Memory 权威。

## Memory 权威与版本模型

权威图使用与 Git 相同的不可变对象关系：

```text
Ref -> Commit -> Tree -> entries -> Blob
         ^
Draft(base_commit_id)
```

每个 Organization 有一个权威 Ref。每个 Project 有独立版本的投影 Ref，内容由该 Project 的 Organization Memory 选择和对应的 Organization 权威版本生成。Project 元数据 revision、Organization Ref 和 Project Ref 是三条不同的并发边界，不能互相替代。

Project 当前选择通过 `/api/v1/projects/{project_id}/org-selections` 管理。daemon 通过 `/api/v1/projects/{project_id}/commit-state` 和 Commit payload 安装 Project 投影，再在本地叠加该 Project 的 `open`/`submitted` Draft，才得到 Agent 实际读取的 Effective Memory。

需要区分读取边界：`GET /api/v1/projects/{project_id}/memories` 及其详情路由读取项目拥有的已发布 `scope=project` 资源，不是 Project 选择投影，也不是包含 Draft overlay 的 Effective Memory。当前主链不能用这组接口解释 Project 的有效视图。

Organization 管理员可用 `GET /api/v1/admin/memory-export` 导出全部 Organization Memory（包括历史 `issues/` 路径）、Draft 及原始操作、Project 选择和个人 Bundle，作为可重复验证的迁移输入。

## Draft、Review 与合并

Draft 生命周期（`open`、`submitted`、`merged`、`discarded`）与 freshness（`current`、`behind`）以及 reconciliation（`unknown`、`clean`、`conflicts`）相互独立。Ref 前进时，Server 不修改 Draft Base 和操作。

一个 Review 可以按顺序包含多个 Draft。创建或重新提交 Review 时，Server 校验每个 Draft 的所有者、状态、版本和候选；合并时在同一事务中按顺序应用全部 Draft，只生成一个结果 Commit 并推进目标 Ref。任何 Draft 不可发布都会使整次决定失败，不会留下部分合并。

Project 成员可以创建、提交、查看和评论 Review。只有 Organization owner/admin 可以批准或拒绝 Organization 发布。Desktop 的 Approve 调用 merge 路由，记录批准并原子推进 Ref。独立 HTTP decision 的 `approved` 只记录决定，不发布；merge 路由支持 `open` 或 `approved` Review。

reconciliation 候选绑定 Draft ID、Draft version、Base Commit 和 Current Commit：

- 查看候选不修改 Draft；
- `clean` 候选只能应用 Server 计算出的结果；
- `conflicts` 候选允许用户提交完整的已解决结果；
- rebase 先保存不可变 Draft revision，再改写为 `base = Current` 与 `diff(Current, confirmed result)`；
- Draft 编辑或 Ref 前进都会使旧候选失效。

合并持有目标 Ref 锁，并以 `If-Match`/CAS 作为最终并发保护。版本冲突、候选失效或任一校验失败时，事务不推进 Ref，原 Draft 仍可继续检查和协调。

## HTTP 契约

| 契约 | 范围 |
| --- | --- |
| `crates/server/openapi/clumsies.public.v1.yaml` | Desktop 与 daemon 使用的产品 API |
| `crates/server/openapi/clumsies.admin.v1.yaml` | Administration API、公开健康检查与首次安装配置 |

本地 daemon IPC 不是 HTTP，不再维护 OpenAPI 副本。其可执行契约由
`crates/clumsiesd/src/types.rs` 的请求/响应类型、`crates/clumsiesd/src/state.rs` 的分派表以及
Rust/macOS 契约测试共同定义。

OpenAPI 是 HTTP wire contract 来源，但当前存在一个已知实现缺口：Public OpenAPI 的 `TreeEntry.type` 仍声明 `rule/context/workflow/project_org_selection`，且没有 `description`；Rust/数据库当前实际模型是 `memory/project_org_selection` 并携带 `description`。在契约修复前，不能把这部分 schema 视为实现的准确描述。

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

复制 `.env.example` 为 `.env`，配置 Organization OIDC，并启动 `compose.production.yml`。`CLUMSIES_PUBLIC_ORIGIN` 必须是 Server 的规范 HTTPS origin；在 IdP 注册由它派生的 `/login/oauth2/code/oidc`。同一 origin 提供 Public API、Admin API 和 OIDC callback。

OIDC 变量为空时，Server 为基础设施诊断仍可启动，但 health 会把 OIDC 标为 `down`，登录不可用；这不是可用的生产状态。

## 验证

```bash
cargo test -p server --lib axum_routes_match_public_and_admin_openapi
cargo test -p server
cargo test -p clumsiesd
```

Server 与 daemon 集成测试使用真实 PostgreSQL Testcontainer。发布流程还必须验证生产镜像和 Compose health；仅有文档构建不证明认证或 Review 并发语义正确。
