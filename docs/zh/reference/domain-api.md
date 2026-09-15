# 领域接口

Clumsies 有四类接口边界，分别服务于不同调用方。即使它们参与同一次操作，读取 Memory、编辑本地 Draft 和发布 Organization Memory，也仍是不同的能力。

如果你知道要做什么，却不知道该找哪个组件，从这一页开始。对象定义见[数据模型](/zh/data-model)，完整过程见[核心流程](/zh/flows)。

## 先选对接口

| 调用方 | 接口 | 能做什么 | 调用位置 |
| --- | --- | --- | --- |
| Coding Agent | stdio MCP，只有一个 `memory` 工具 | 找相关片段、加载完整 Memory、提出 Draft 变更 | App 内置代理 → 常驻 daemon |
| macOS App | 本地 XPC，使用有类型定义的请求和响应 | 目录绑定、本地编辑、同步、检索诊断、带认证的 Server 请求 | App → 常驻 daemon |
| 产品客户端，通常是 daemon | Public HTTP `/api/v1/...` | 读取共享数据、同步 Draft、提交和审阅变更、按权限发布 | 客户端 → Server |
| Organization 管理员 | Admin HTTP `/api/v1/admin/...` | 管理组织、成员、Project、令牌和审计记录 | 原生 Administration → daemon → Server |

**Public API 指产品接口，并不表示匿名可访问。** 常规 Public 和 Admin 请求都使用 `Authorization: Bearer …`。Admin 路由额外要求 Organization 的 `owner` 或 `admin` 角色；部分 Public 路由，例如发布接口，也要求这个角色。

首次安装是单独的例外：`/api/v1/setup/...` 使用短期 setup cookie 和 CSRF token。OIDC 入口与回调、令牌交换、setup 入口以及 `/api/v1/admin/health` 各有启动阶段的访问规则。health 虽然在 `admin` 路径下，却是公开的健康检查。详细过程见[认证与会话](/zh/reference/auth)。

## 按领域查找能力

下面的路由表用来说明职责。花括号表示调用方提供的值，例如 `{project_id}`；所有路径都相对于配置的 Server 地址。

### 身份与组织

这个领域回答：**谁在调用，他能做哪些事？** Project 是协作和 Memory 选择的边界，Organization 拥有已发布 Memory 的权威内容。

| 能力 | 代表性 HTTP 操作 | 权限规则 |
| --- | --- | --- |
| 登录和刷新令牌 | `GET /oauth2/authorization/oidc`、`GET /login/oauth2/code/oidc`、`POST /api/v1/auth/token` | OIDC 授权码 + PKCE，或轮换的 refresh token |
| 读取当前身份、退出登录 | `GET /api/v1/me`、`DELETE /api/v1/auth/session` | 当前已认证会话 |
| 查找 Project 和成员 | `GET /api/v1/projects`、`GET /api/v1/projects/{project_id}`、`GET /api/v1/projects/{project_id}/members` | Server 按权限过滤或检查 |
| 创建 Project | `POST /api/v1/projects` | 已登录的组织成员；要求 `Idempotency-Key`；创建者成为 Project admin |
| 修改、删除 Project | `PATCH` / `DELETE /api/v1/projects/{project_id}` | 该 Project admin 或具备成员访问权限的 Organization owner/admin；要求版本 `If-Match` |
| 配置 Project 和成员 | `/api/v1/admin/projects/{project_id}`、`/members` 及成员子路由 | Project 成员可读；该 Project admin 或 Organization owner/admin 可修改，组织管理员也可管理未加入的项目 |
| 查找待添加的成员 | `GET /api/v1/admin/projects/{project_id}/member-candidates` | 该 Project admin 或 Organization owner/admin；支持 `q`、`limit`、`cursor`，仅返回未加入项目且未禁用的用户资料 |
| 管理组织 | `/api/v1/admin/org`、`/members`、`/projects`、`/tokens`、`/audit-events` | Organization owner/admin；此行所有路径均以 `/api/v1/admin` 开头 |

`GET /api/v1/me` 的 `projects[].role` 返回当前用户在各 Project 中的角色；组织成员拥有 `project:create` capability。组织全部项目列表 `/api/v1/admin/projects` 仍仅供 Organization owner/admin 使用。

Project 内的角色不会自动变成 Organization 管理员。Server 会分别检查“能看到这个 Project”和“能发布组织内容”。

### Memory 与选择集合

这个领域回答：**组织发布了哪些资源，这个 Project 使用哪些资源？** Memory 有稳定身份和 Markdown 正文。选择集合保存资源 ID，不会把正文复制成另一份独立权威内容。

| 能力 | HTTP 操作 | 结果或约束 |
| --- | --- | --- |
| 浏览已发布的 Organization Memory | `GET /api/v1/org/memories` 及其 `/{memory_id}` | 元数据列表或完整资源 |
| 读取历史 Project scope Memory | `GET /api/v1/projects/{project_id}/memories` 及其 `/{memory_id}` | 历史 `scope=project` 记录；这两个路由不返回已选 Organization Memory 的投影 |
| 读取、替换 Project 选择集合 | `GET` / `PUT /api/v1/projects/{project_id}/org-selections` | 输入 `resource_ids`；替换要求该 Project admin 或具备成员访问权限的 Organization owner/admin，以及 selection revision `If-Match` |
| 保存个人 Bundle | `GET` / `POST /api/v1/me/bundles`；`GET` / `PATCH` / `DELETE /api/v1/me/bundles/{bundle_id}` | 属于当前用户；修改和删除使用 Bundle revision `If-Match` |
| 导出组织受管数据 | `GET /api/v1/admin/memory-export` | 管理员导出 Memory、Draft、选择集合与 Bundle |

要读取当前已选的 Organization 视图，使用 Project 下的 `/org-selections`，或者 `/commit-state` 及其指向的 Commit 快照。要知道“Agent **此刻**能读到什么”，使用本地 MCP 的 `load` 或 `activate`。daemon 把 Project 的已发布投影与本地 Draft 叠加，得到 Effective Memory。单独调用 Server 的 Memory GET 不能回答这个问题。

### Draft 与同步

这个领域回答：**提出了哪些变更，它们是否已经到达 Server？** Draft 由 Project 携带，目标是 Organization 权威内容。本地保存、同步成功和正式发布是三个独立阶段。

| 能力 | 接口 | 关键输入和输出 |
| --- | --- | --- |
| Agent 提出变更 | MCP `memory.store` | update 使用精确替换；返回本地操作 ID、Draft ID 和同步状态 |
| 创建、读取、修改 Server Draft | `POST` / `GET /api/v1/drafts`；`GET` / `PATCH` / `DELETE /api/v1/drafts/{draft_id}` | 创建需要 Project 和 daemon installation ID；修改受作者权限和版本控制 |
| 追加完整操作 | `POST /api/v1/drafts/{draft_id}/operations` | `action`、`resource`、正文或路径字段；整数 Draft `If-Match` |
| 上传本地操作队列 | `POST /api/v1/draft-operation-batches` | 每项包含 `local_operation_id`、`draft_id`、`expected_draft_version`、`operation` |
| 拉取同步事件 | `GET /api/v1/draft-events` | `after_cursor`、`limit`；返回当前作者 Draft 的事件和下一游标 |
| 与更新后的基线比较 | `POST /api/v1/drafts/{draft_id}/reconciliation-candidates` | 输入预期 Draft 版本，返回 Base/Current/Draft 比较 |
| 应用已确认的比较结果 | `POST /api/v1/drafts/{draft_id}/rebases` | candidate ID + 预期 Draft 版本 + 权威 Ref `If-Match`；保存旧 Draft revision |

HTTP 和 MCP 的操作格式不同。MCP `update` 接受 `expected_hash` 和精确替换；daemon 根据完整 Effective Memory 验证替换，再生成用于同步的完整正文操作。

### Review 与发布

这个领域回答：**哪些提案正在被审阅，谁有权发布？** 一个 Review 可以包含多个 Draft，也可以一次提交上百个文件。

| 能力 | HTTP 操作 | 并发与权限 |
| --- | --- | --- |
| 提交 Draft | `POST /api/v1/reviews` | 同一个 Project 下、当前作者拥有的 open Draft；每个 Draft 版本 + 权威 Ref `If-Match` |
| 查看 Review、详情和评论 | `GET /api/v1/reviews`、`GET /api/v1/reviews/{review_id}`、`GET /api/v1/reviews/{review_id}/comments` | 有权查看该 Review 的用户 |
| 评论 | `POST /api/v1/reviews/{review_id}/comments` | 预期 Review 版本；可选的 `anchor_path` 和从 1 开始的 `anchor_line` 必须一起提供 |
| 修改后重新提交 | `POST /api/v1/reviews/{review_id}/submissions` | Draft 作者；预期 Review 版本、每个 Draft 版本、权威 Ref `If-Match` |
| 记录审批结论 | `POST /api/v1/reviews/{review_id}/decisions` | Organization owner/admin；`approved` 或 `rejected`，预期 Review 版本 |
| 批准并发布 | `POST /api/v1/reviews/{review_id}/merges` | Organization owner/admin；预期 Review 版本 + 权威 Ref `If-Match` |

当前 Desktop 的 **Approve** 按钮调用 `/merges`：`open` Review 直接成为 `merged`，审批信息与新的权威 Commit 在同一个事务中写入。单独的 `/decisions` API 仍然存在：`approved` 只记录批准，不发布；之后 `/merges` 可以在获批内容未改变时发布 `approved` Review。拒绝则会重新打开 Draft，供作者继续修改。

具体请求格式和过期 Draft 的处理见 [HTTP 调用示例](/zh/reference/http-api#walkthrough)。

### 快照与本地检索

这个领域回答：**当前发布到哪个版本，这个版本包含哪些内容？**

| 能力 | 接口 | 含义 |
| --- | --- | --- |
| 检查已发布版本 | `GET /api/v1/org/commit-state` 或 `GET /api/v1/projects/{project_id}/commit-state` | 当前 Ref、最新 Commit、是否需要更新，以及强 ETag |
| 查看历史 | `GET /api/v1/org/commits` 或 `GET /api/v1/projects/{project_id}/commits` | Organization 权威历史或 Project 投影历史 |
| 下载一个快照 | `GET /api/v1/commits/{commit_id}` | 完整 `commit`、`tree`、`blobs` 和可选的 Project selection |
| 检索相关片段 | MCP `memory.activate` → XPC `activate_memory` | 任务 query → 本地 Effective Memory 的相关片段 |
| 加载已知资源 | MCP `memory.load` → XPC `load_memory` | ID / 精确路径 → 完整资源和内容哈希 |
| 诊断检索过程 | 私有 XPC 诊断方法 | 本地 Retrieval Run、评测与索引状态；见[检索评测](/zh/retrieval-evaluation) |

Commit 下载的是**整个快照**，不是一个文件的 diff。多个文件可能共用同一个 base/current Commit。客户端应在一次操作中按 Commit ID 去重加载，再在本地生成各文件的变化。

## 本地 XPC 有独立契约

XPC 请求包包含 `method`、`payload`、`request_id`，以及可选的 `agent_runtime` 标记。响应包含 `ok`、`payload` 和可选的结构化 `error`。daemon 按方法分发并解码具体请求类型。这是 macOS 本地 IPC，不是在 localhost 端口上运行的 HTTP 服务。

App 可以调用私有 `server_request` 方法，提供 HTTP method、相对路径、headers 和 body；daemon 补入已配置的 Server 地址和凭据。MCP 代理不能使用这个通用桥接，只暴露三种有明确类型的 Memory 操作。Agent runtime 请求还会携带协议修订和构建标识，在解析 binding 前和实际分发时进行检查。

工作目录绑定、Keychain 凭据、本地存储路径、检索索引和 Draft 队列状态属于 daemon。它们不是 Server 资源，不会因为 Desktop 展示了它们就自动对应一条 Public API。

## 契约与兼容边界

- [HTTP 示例与失败处理](/zh/reference/http-api)说明字段、前置条件和当前实现缺口。
- [MCP 参考](/zh/mcp)定义唯一的 Agent 工具。
- [Public OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml) 与 [Admin OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.admin.v1.yaml)描述 HTTP 方法和 schema。
- [Server 路由](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/http.rs)、[Draft/Review 类型](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/api.rs)、[Memory 类型](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/api.rs)和 [daemon 类型](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/types.rs)对应实际实现。

部分类型枚举为历史数据保留了 `project` scope 和旧资源 ID。当前 Draft 创建和发布以 `org` 为目标，Project Ref 表示投影。已移除的 MCP `retrieve` 工具和旧的 rule/workflow/context 分立接口都不是当前接入入口。请把返回的 ID 当作不透明标识保存，不要仅凭前缀推断含义。
