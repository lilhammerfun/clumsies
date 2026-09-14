# HTTP 契约与调用示例

这一页用一个变更说明：已经同步的 Draft，怎样成为已发布 Memory。先读[领域接口](/zh/reference/domain-api)，确定要使用哪类接口。这里展示 Server 请求；Coding Agent 通常使用 [MCP](/zh/mcp)，Desktop 则通过 daemon 发送带认证的请求。

路径相对于你配置的 Server 地址。下文的 ID、哈希、版本和时间仅用于示例。发送写请求前应读取真实值，不要拿另一个资源的版本拼出 ETag。

## 通用规则

| 项目 | 当前行为 |
| --- | --- |
| 传输 | HTTPS 上的 JSON；本地开发支持 loopback HTTP |
| 认证 | 常规 Public/Admin 请求使用 `Authorization: Bearer <access_token>`；daemon 负责注入和刷新令牌 |
| 权限 | Server 按操作检查 Organization 角色、Project 访问权和 Draft 所有权；登录成功不等于有权执行所有操作 |
| 时间 | 时间字段序列化为 RFC 3339 |
| 标识 | 资源 ID 和 cursor 按不透明值处理；Commit ID 是 64 字符的内容寻址哈希 |
| 请求追踪 | 排错时保留响应头 `X-Request-ID` 和 `error.request_id` |
| API 命名空间 | `/api/v1`；OpenAPI 文档版本为 `1.0.0`，与 App/daemon 的构建标识各自独立 |

setup cookie 和 CSRF 仅用于首次安装，不替代常规 Administration 的 Bearer 认证。详见[认证与会话](/zh/reference/auth)。

## 三种并发控制值

它们分别保护不同的数据，不能混用。

| 控制值 | 示例 | 使用位置 |
| --- | --- | --- |
| 可变对象的 version/revision | `If-Match: "4"` | 按接口分别用于 Draft/Project 修改、Bundle 修改、Project 选择集合替换 |
| JSON 中的预期对象版本 | `"expected_draft_version": 4`、`"expected_review_version": 2` | 批量操作、reconciliation、Review 操作 |
| 权威 Ref 的 ETag | `If-Match: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"` | Review 创建/重新提交、Draft rebase、发布 |

空 Ref 使用固定的强 ETag `"ref-none"`。Ref 前置条件必须带引号；`W/"…"` 这样的弱 ETag 会被拒绝。Organization scope 的 Draft 必须使用 **Organization 权威 Ref**，不能使用携带它的 Project 投影 Ref。即使 Project 显示的是相同 Memory，这两个 Commit ID 也可能不同。

MCP 的 `expected_hash` 又是另一种值：它保护精确文本替换所依据的完整资源正文，不是 Draft 版本，也不是 HTTP Ref 前置条件。

## 示例：发布一个资源变更 {#walkthrough}

假设 Project `prj_example` 使用了 `operations/deployment-rollback.md`。作者修改了这份资源，daemon 已将 Draft `drf_example` 同步到 Server。接下来由 Organization owner/admin 审阅并发布。

### 1. 读取 Draft 和权威版本

```http
GET /api/v1/drafts/drf_example
Authorization: Bearer <access_token>
```

响应是 `DraftDetail`，包含 `draft`、按顺序排列的 `operations` 和 `sync_state`。后续步骤需要关注这些字段：

| 字段 | 调用方为什么需要它 |
| --- | --- |
| `draft.project_id` | 携带提案的 Project |
| `draft.resource.scope` | 发布的权威目标；当前可写 scope 是 `org` |
| `draft.base_commit_id` | 起草时依据的快照 |
| `draft.version` | 下一次操作需要带上的 Draft 版本 |
| `draft.status` | 创建 Review 前必须为 `open` |
| `draft.coordination.freshness` | `current` 或 `behind`，与 Draft 生命周期独立 |
| `draft.coordination.current_commit_id` | 这次详情读取所见的当前权威版本 |

再读取当前权威 Ref：

```http
GET /api/v1/org/commit-state
Authorization: Bearer <access_token>
```

`200` 响应带 `ETag` 头。JSON 包含 `ref`、`latest`、`update_available`、`download_url` 和 `incremental_supported`。可传 `?local_commit_id=<本地_commit_id>`，比较本地快照和当前版本。Project 下的对应接口返回的是投影状态，不是 Organization 的发布基线。

读取对象不会为之后的请求持有锁。这两步之间可能有其他作者发布变更，所以最终写入仍必须带前置条件。

### 2. 仅在 Draft 落后时进行协调

如果 Draft 的 base 与当前权威版本不同，请 Server 比较旧基线、当前已发布资源和提案资源：

```http
POST /api/v1/drafts/drf_example/reconciliation-candidates
Authorization: Bearer <access_token>
Content-Type: application/json

{"expected_draft_version":4}
```

响应是 `DraftReconciliationCandidate`：

| 字段 | 含义 |
| --- | --- |
| `candidate_id`、`draft_id`、`draft_version` | 比较结果的身份，以及它对应的准确 Draft 版本 |
| `base_commit_id`、`current_commit_id` | 参与比较的两个不可变快照版本 |
| `base_state`、`current_state`、`draft_state` | 三个位置的资源存在性、引用和完整正文 |
| `status` | `clean` 或 `conflicts` |
| `proposed_state` | 无冲突时由 Server 算出的规范合并结果 |
| `conflicts` | 正文、路径、存在性或路径被占用等冲突字段 |
| `valid` | 这个 candidate 是否仍匹配当前 Draft 和权威版本 |

对于 `clean`，确认结果后，在下一步的 Draft 条目里传入 `candidate_id` 即可。对于 `conflicts`，需要明确解决冲突，同时提供包含 `exists`、`resource` 和 `content` 的 `resolved_state`。clean candidate 不接受自定义 `resolved_state`；conflicts candidate 则必须提供它。

创建 candidate 不会改写 Draft。应用它时，Server 才会保存旧 Draft revision，再修改 base 和 operations。这可以在 Review 提交事务内完成，因此这个流程不需要先单独调用 `/rebases`。如果你想先更新基线、继续编辑，则单独调用 `/rebases`，提供 candidate ID、预期 Draft 版本和权威 Ref `If-Match`。

### 3. 提交 Review

对于版本为 `4` 的 current Draft，完整请求体可以是：

```http
POST /api/v1/reviews
Authorization: Bearer <access_token>
Content-Type: application/json
If-Match: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

{
  "title": "补充部署回滚检查",
  "description": "部署失败时先确认上一稳定版本，再执行回滚。",
  "drafts": [
    {"draft_id": "drf_example", "expected_draft_version": 4}
  ]
}
```

对于已确认 **clean** candidate 的 behind Draft，改用以下数组条目：

```json
{
  "draft_id": "drf_example",
  "expected_draft_version": 4,
  "candidate_id": "rcn_example"
}
```

请求头必须使用之前读取的真实 ETag。所有 Draft 必须由当前作者拥有、处于 open、有操作内容，并属于同一个 Project 和发布 scope。已有 Organization 资源必须在这个 Project 允许使用的选择范围内。current Draft 必须省略 reconciliation 数据。

`200` 响应为 `ReviewDetail`，包含 `review`、主 `draft` 及其 `operations`、完整 `drafts` 数组和 `comments`。读取所有文件时使用 `drafts`，单数形式只代表主 Draft。创建 Review 会把 Draft 改成 `submitted`，并记录一个 `open` Review，不会前移权威 Ref。

多个文件也使用这个数组，每项带自己的 Draft 版本，必要时带自己的 candidate。Ref 校验、已确认的 rebase 和 Review 提交在一个事务中完成。

### 4. 阅读 Review，再批准发布

```http
GET /api/v1/reviews/rev_example
Authorization: Bearer <access_token>
```

审阅内容并保留 `review.version`。通过 `GET /api/v1/commits/{commit_id}` 获取所需快照。每次响应都是完整 `CommitPayload`，包括 `commit`、`tree`、`blobs`、`project_org_selection`；多个文件共用一个 Commit 时应复用快照。

Organization owner/admin 发布审阅过的版本：

```http
POST /api/v1/reviews/rev_example/merges
Authorization: Bearer <admin_access_token>
Content-Type: application/json
If-Match: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

{"expected_review_version":1}
```

成功的 `200` 响应包含：

| 字段 | 含义 |
| --- | --- |
| `review` | 更新后的 Review，包含 `status: "merged"` 和新版本 |
| `commit_id` | 新发布的权威 Commit |
| `applied_operation_count` | 实际应用的资源操作数量 |

这就是当前 Desktop **Approve** 的路径：从 `open` 直接到 `merged`。Server 在事务内记录审批、应用正文、创建 Commit、前移 Organization Ref，并更新受影响的 Project 投影。Draft 随之成为 `merged`。

单独的 `/decisions` 接口接受 `decision`、`expected_review_version` 和可选 `body`。`approved` 只记录批准而不发布；`/merges` 也支持这种 `approved` 状态，并验证获批结果哈希。`rejected` 会重新打开 Draft。之后的 `/submissions` 请求包含 `expected_review_version` 和新的 `drafts` 数组，Ref 前置条件规则与首次提交相同。

### 5. 同步并读取发布结果

daemon 检查 Project 的 `/commit-state`，下载新快照，重建本地 Effective Memory。Project 的投影 Commit 不必等于 merge 返回的 Organization Commit。同步完成后，MCP `load` 读取本地生效结果；merge 成功不代表所有设备已经下载完毕。

## 同步、分页与重试

Draft 上传使用 `POST /api/v1/draft-operation-batches`，请求格式为：

```json
{
  "daemon_installation_id": "dmi_example",
  "operations": [
    {
      "local_operation_id": "lop_example",
      "draft_id": "drf_example",
      "expected_draft_version": 4,
      "operation": {
        "action": "update",
        "resource": {"scope": "org", "id": "mem_example_rollback", "path": "operations/deployment-rollback.md"},
        "content": {"content": "# 部署回滚清单\n\n确认上一稳定版本，再执行回滚。"},
        "new_path": null
      }
    }
  ]
}
```

成功响应包含 `accepted_operations`（本地操作 ID 数组）和 `cursor`。这些 ID 用来把确认结果对应到 daemon 本地队列。当前 handler 会回传 ID，但没有把它们持久化成 Server 幂等键；重复写入会受到预期 Draft 版本检查的约束。创建 Project 则单独要求 `Idempotency-Key`，Review 写入依赖状态和版本前置条件。网络中断导致写入结果不明确时，先读取资源现状，再决定是否重新写入。

`GET /api/v1/draft-events?after_cursor=123&limit=50` 返回 `events`、`next_cursor`、`has_more`。limit 默认 `50`，有效范围为 `1`–`200`。消费事件后保存返回的 cursor，`has_more` 为 true 时继续读取。这个事件流只包含当前作者的 Draft。

Admin 列表使用 `cursor` 和 `limit`，同样默认 `50`，limit 范围为 `1`–`200`。当前 cursor 编码的是 offset，客户端仍应原样回传。不要把 Draft event cursor 用在 Admin 列表中。

## 失败时如何处理

Server 领域错误使用以下结构；此例是对象版本过期：

```json
{
  "error": {
    "code": "version_conflict",
    "message": "draft version conflict: expected 4, actual 5",
    "request_id": "req_example",
    "details": {"entity": "draft", "expected_version": 4, "actual_version": 5}
  }
}
```

程序根据 `code` 判断，不要解析面向人的 message。HTTP 解析错误、代理错误和传输失败可能使用其他格式，甚至没有 JSON 响应。

| HTTP 状态 / code | 含义 | 调用方处理 |
| --- | --- | --- |
| `401` | 缺少会话或会话失效 | daemon 尝试刷新一次并重试；否则重新登录 |
| `403 forbidden` | 角色或访问权不足 | 使用有权限的账号；重试不能获得权限 |
| `404 not_found` | 对象不存在或不可见 | 刷新可见列表；部分访问检查会有意返回 not found |
| `400 invalid_request` | 字段、状态转换或前置条件不合法 | 修正请求或当前操作 |
| `409 version_conflict` | Draft、Review 或 revision 已变化 | 重新读取现状，重新判断要提交的变更 |
| `412 precondition_failed` | 权威 Ref 已变化 | 读取新版本，重新检查 Draft 是否落后 |
| `409 reconciliation_required` | Draft base 已落后 | 加载返回的 `candidate_id`，确认结果后提交协调数据 |
| `409 candidate_invalid` | candidate 不再匹配 Draft 或权威版本 | 创建新 candidate，不直接套用旧的冲突解决结果 |
| `409 draft_already_current` | 已不需要协调 | 刷新 Draft，省略协调数据后继续 |
| `5xx` 或传输失败 | Server 或网络故障 | 尽可能保留 request ID；重试写入前检查现状 |

## OpenAPI 与实现边界 {#contract-limits}

完整 schema 可查仓库中的 [Public OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml) 和 [Admin OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.admin.v1.yaml)。其中的 `servers` URL 是示例，不是服务发现地址。

路由一致性测试检查的是 **method/path 覆盖**，不等于所有字段和运行时行为都经过验证。以下是会影响调用方的当前差异：

| 文档声明 | 已核对的实现 |
| --- | --- |
| 部分 Public 列表声明 `limit` / `cursor` | Project、Memory、Bundle、Draft、Review、评论和 Commit 列表未实现通用 cursor 分页。部分查询有固定条数限制（Memory 列表为 200），却返回终止态 `page_info`；`has_more: false` 不保证数据完整。需要完整数据时应使用有权限的快照或导出路径。Draft events 和 Admin 分页有独立实现。 |
| Memory、Bundle 详情声明 `If-None-Match` / `304` | 当前 handler 返回 `200` JSON，正文包含 `etag` 字段，未实现这些条件读取。 |
| `TreeEntry.type` 仍列出 `rule/context/workflow/project_org_selection`，缺少 `description` | 当前 Rust/数据库使用 `memory/project_org_selection` 并携带 `description`。生成客户端时需要核对这部分 schema；字段语义见[数据结构](/zh/data-model)。 |
| scope 枚举保留 `project` | 用于历史记录和投影；新 Draft 的可写和发布 scope 为 `org`。 |

针对具体发布版本接入时，应读取同一个 release/tag 下的 OpenAPI 和实现。仅凭 `/api/v1`，不能推断旧客户端或旧文档中的所有行为仍然有效。

仓库的路由覆盖检查命令是：

```bash
cargo test -p server http::tests::axum_routes_match_public_and_admin_openapi
```

实现入口：[路由、前置条件和错误](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/http.rs)、[Draft/Review 请求响应](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/api.rs)、[Review 事务](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/postgres.rs)、[Memory handler](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/http.rs)、[Admin 分页](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/organization/http.rs)。
