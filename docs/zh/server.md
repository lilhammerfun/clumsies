# Server

Server 是可部署的服务，负责共享的 Clumsies 状态和发布。
本页面向需要部署或修改 Server 的开发者。
整体设计先读[系统架构](/zh/architecture)和
[领域接口](/zh/reference/domain-api)。

## 职责与边界

Server 负责：

- Organization 成员、Project 成员、角色和 token 会话
- Organization 和 Project Memory 的发布、选择与组合快照
- 个人 Bundle（`resource_ids`）
- Draft、Draft 操作历史、Review、决定、评论和合并
- 不可变的 Commit 历史、Tree、Blob、
  Organization Ref 与 Project Ref
- 管理配置、token 撤销、审计事件和健康检查

Desktop 和 MCP 通过 daemon 写入本地 Draft，
再由 daemon 同步到 Server。本机目录到 Project 的绑定属于 daemon
SQLite；Server 只提供并授权规范 `project_id`。
Project Local Storage 路径、macOS bookmark、
每个 Project 的检索数据库和存储迁移任务也完全属于 daemon，
永远不会进入 Public 或 Admin Server 端点。
任何客户端都不能直接更新权威 Memory。

## Memory 权威与版本模型

快照图使用与版本控制相同的概念。这些对象描述 Memory 历史，
不是源代码仓库的 Git commit：

```text
Ref -> Commit -> Tree -> entries -> Blob
         ^
Draft(base_commit_id)
```

Organization 和每个 Project 都有独立版本的 Ref。
合并会锁定发布者的 Ref，校验 `If-Match` 和 Draft 基线，
创建不可变 Commit，并推进该 Ref。Project 快照组合项目自有 Memory
与选中的 Org 内容；选择和上游 Org 变更也会刷新这些快照，
同时保留 Project 适配。Project 元数据 revision 与这两段历史相互独立。

已发布的 Organization Memory 通过
`GET /api/v1/org/memories` 及其 `{memory_id}`
详情路由读取。`/api/v1/projects/{project_id}/memories`
路由读取已发布的、项目拥有的资源；它们不返回当前选择投影或 Effective Memory。
Project 投影使用 org-selections 和 commit-state，
本地 Effective Memory 使用 MCP。
org-admin 的 `GET /api/v1/admin/memory-export`
会导出全部 Memory（包括 `issues/` 路径）、所有 Draft 及其原始操作、
Project org 选择和个人 bundle，作为可重复、可验证的迁移导出。

Draft 生命周期（`open`、`submitted`、`merged`、
`discarded`）与 freshness（`current`、`behind`）
以及 reconciliation（`unknown`、`clean`、`conflicts`）
相互独立。Ref 前进时，Server 保持 Draft Base 和操作不变。只有在被请求时，
它才计算规范的 Base/Current/Draft 候选，
并通过按作者作用域的 auto-rebase 端点应用 clean 候选。
conflicts 保持基线和操作不变，并通知作者。
rebase 先保存不可变 Draft revision，
再原子改写 `base_commit_id` 和操作。
应用 clean 候选始终使用 Server 的规范提案结果；
只有 conflicts 候选接受用户提交的完整已解决状态。

创建或重新提交 Review，以及批准它发布，是两个协作边界。Project 成员可以提出、
提交、查看和评论。Project owner/admin 发布 Project Review；
Organization owner/admin 发布 Org Review。
可选的 Org 贡献是从固定的已合并 Project commit 创建的独立 Review。
Desktop 的 Approve 调用 merge 端点，
在一个事务中记录决定并推进目标 Ref。该端点接受 Open 或 Approved 的
Review。独立的 HTTP `approved` 决定只记录批准，不发布。
Review 的创建/提交可以在同一个 Ref 锁定事务中应用每个 Draft 的已确认候选。
发布从不把首次过期检查当作常规工作流；它保留 `If-Match`/CAS 作为最终并发保护。

状态模型、请求示例和失败语义见[数据模型](/zh/data-model)、
[端到端流程](/zh/flows)和
[HTTP 契约](/zh/reference/http-api)。

## HTTP 契约

仓库中检入的 HTTP 规范列在下面。它们与实现存在已知的 payload 和行为差异；
生成客户端前请查阅 [HTTP 契约限制](/zh/reference/http-api)。
仅靠路由覆盖测试不能验证 payload：

| 契约 | 范围 |
| --- | --- |
| `crates/server/openapi/clumsies.public.v1.yaml` | Desktop 与 daemon 使用的产品 API |
| `crates/server/openapi/clumsies.admin.v1.yaml` | 使用 bearer 认证的组织 Administration API、公开健康检查和首次安装 bootstrap |

本地 daemon IPC 不是 HTTP，没有 OpenAPI 文档。
其可执行契约由 `crates/clumsiesd/src/types.rs`
的请求/响应类型、`crates/clumsiesd/src/state.rs` 的分派表以及
Rust/macOS 契约测试共同定义。

认证使用组织在系统浏览器中的 OIDC provider。
原生 macOS App 校验 Server origin，
并持有临时 loopback callback、state 和 PKCE verifier。
首次安装配置使用原生 URLSession 请求，
携带 HttpOnly setup cookie 和 CSRF token，
然后走与普通登录相同的授权码和 PKCE 路径。
App 把签发的 token pair 直接交给 daemon；
SwiftUI 展示状态永远拿不到 bearer 或 refresh token。
daemon 执行带认证的 Server 请求，
在 `401` 之后轮换 refresh token，持久化替换后的 pair，并重试一次。
认证的 Admin 路由只接受 bearer 凭据；
Server 不提供任何管理 HTML 或 JavaScript。

## 本地运行

本地开发在 Docker 中运行 PostgreSQL 和确定性的 fake OIDC
provider，再原生运行 Rust Server，以获得快速的编辑-运行循环。
它不需要企业身份配置：

```bash
bun run dev:server
```

默认端点：

| 服务 | 地址 |
| --- | --- |
| Server | `http://127.0.0.1:18080` |
| PostgreSQL | `127.0.0.1:5432` |
| Fake OIDC | `http://127.0.0.1:18081/clumsies` |
| Health | `http://127.0.0.1:18080/api/v1/admin/health` |

该栈使用
[NAV 的 mock OAuth2 server](https://github.com/navikt/mock-oauth2-server)，
锁定为 `4.0.0`。它会自动认证 `owner@clumsies.local`，
与原生 setup 和登录 fixture 一致。它仍然覆盖 discovery、
授权码和 PKCE 处理、签名 ID token、JWKS 校验以及 nonce 校验。
fake provider 永远不会进入 `compose.production.yml`。

停止本地服务使用：

```bash
bun run dev:infra:down
```

## 生产运行

复制 `.env.example` 为 `.env`，配置企业 OIDC 值，
并启动 `compose.production.yml`。
把 `CLUMSIES_PUBLIC_ORIGIN` 设为 Server 的规范 HTTPS
origin，并在组织 IdP 注册由它派生的
`/login/oauth2/code/oidc` URL。
同一 origin 提供 Public API、bearer Admin API、
公开健康检查端点、memory export 和 OIDC callback；
它不提供管理 UI。

OIDC 变量被有意留空时，Server 仍会启动，以便 health 和数据库诊断仍然可用。
health 会把 OIDC 组件标为 `down`，登录不可用。该状态用于基础设施冒烟测试，
不是可用的部署。

## 验证

```bash
cargo test -p server --lib axum_routes_match_public_and_admin_openapi
cargo test -p server
cargo test -p clumsiesd
```

Server 与 daemon 集成测试使用 Testcontainers 和真实
PostgreSQL 实例。仓库还会验证生产 Docker 镜像和 Compose
健康检查路径。
