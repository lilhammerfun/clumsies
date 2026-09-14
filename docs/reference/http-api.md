# HTTP contracts and examples

This page follows one change from a synchronized Draft to published Memory. Read [Domain interfaces](/reference/domain-api) first to choose the right API. The examples explain Server requests; a Coding Agent normally uses [MCP](/mcp), and Desktop sends authenticated requests through daemon.

Paths are relative to your configured Server origin. IDs, hashes, versions and timestamps below are illustrative. Read the real values before sending a write; do not manufacture an ETag from an unrelated resource.

## Common rules

| Concern | Current behavior |
| --- | --- |
| Transport | JSON over HTTPS; loopback HTTP is supported for local development |
| Authentication | Normal Public/Admin requests use `Authorization: Bearer <access_token>`; daemon supplies and refreshes the token |
| Permissions | Server checks Organization role, Project access, and Draft ownership as applicable; being authenticated alone is insufficient |
| Time | Serialized timestamps use RFC 3339 |
| Identifiers | Treat resource IDs and cursors as opaque values; Commit IDs are 64-character content-addressed hashes |
| Request tracing | Retain the `X-Request-ID` response header and `error.request_id` when diagnosing failures |
| API namespace | `/api/v1`; OpenAPI documents are versioned `1.0.0`, independently of the App/daemon build identity |

The setup cookie and CSRF flow is limited to first installation. It does not replace Bearer authentication for normal Administration. See [Authentication](/reference/auth).

## Three different concurrency values

These values protect different data. They cannot be substituted for each other.

| Value | Example | Use |
| --- | --- | --- |
| Mutable object version/revision | `If-Match: "4"` | Draft/Project changes, Bundle changes, Project selection replacement, depending on the endpoint |
| Expected object version in JSON | `"expected_draft_version": 4`, `"expected_review_version": 2` | Batch operations, reconciliation and Review actions |
| Authority Ref ETag | `If-Match: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"` | Review creation/resubmission, Draft rebase and publication |

An empty Ref uses the literal strong ETag `"ref-none"`. Ref preconditions require quotes; weak ETags such as `W/"…"` are rejected. For an Organization-scoped Draft, use the **Organization authority Ref**, not the carrying Project's projection Ref. Their Commit IDs can differ even when the Project displays the same Memory.

An `expected_hash` from MCP is different again: it protects the complete resource content used for an exact text replacement. It is not a Draft version or HTTP Ref precondition.

## Walkthrough: publish one changed resource {#walkthrough}

Suppose Project `prj_example` uses `operations/deployment-rollback.md`. An author has changed that resource, and daemon has synchronized Draft `drf_example` to Server. The Organization owner/admin will review and publish it.

### 1. Read the Draft and authority head

```http
GET /api/v1/drafts/drf_example
Authorization: Bearer <access_token>
```

The response is `DraftDetail`: `draft`, its ordered `operations`, and `sync_state`. The fields needed for the next steps are:

| Field | Why the caller needs it |
| --- | --- |
| `draft.project_id` | Project carrying the proposal |
| `draft.resource.scope` | Publication authority; current writable scope is `org` |
| `draft.base_commit_id` | Snapshot on which the Draft was authored |
| `draft.version` | Expected version for the next Draft operation |
| `draft.status` | Must be `open` to create a Review |
| `draft.coordination.freshness` | `current` or `behind`; separate from lifecycle status |
| `draft.coordination.current_commit_id` | Current authority head known to this detail |

Read the current authority Ref as well:

```http
GET /api/v1/org/commit-state
Authorization: Bearer <access_token>
```

The `200` response carries an `ETag` header. Its JSON includes `ref`, `latest`, `update_available`, `download_url` and `incremental_supported`. Supplying `?local_commit_id=<your_commit_id>` lets the caller compare its local snapshot with the current head. The response's Project equivalent describes a projection, not the Organization publication base.

Reading these objects does not lock them for a later request. The final write must still send preconditions, because another author may publish between these steps.

### 2. Reconcile only if the Draft is behind

If the Draft's base differs from the current authority head, ask Server to compare the old base, the current published resource and the proposed resource:

```http
POST /api/v1/drafts/drf_example/reconciliation-candidates
Authorization: Bearer <access_token>
Content-Type: application/json

{"expected_draft_version":4}
```

The response is a `DraftReconciliationCandidate`:

| Field | Meaning |
| --- | --- |
| `candidate_id`, `draft_id`, `draft_version` | Identity and exact Draft version of this comparison |
| `base_commit_id`, `current_commit_id` | The two immutable snapshot versions compared |
| `base_state`, `current_state`, `draft_state` | Resource existence, reference and complete content at each point |
| `status` | `clean` or `conflicts` |
| `proposed_state` | Server's canonical merged state for a clean comparison |
| `conflicts` | Conflicting content, path, existence, or occupied-path fields |
| `valid` | Whether the candidate still applies to the current Draft and head |

For `clean`, confirm the proposed result and pass only `candidate_id` with the Draft in the next step. For `conflicts`, explicitly resolve the result and also provide `resolved_state` with `exists`, `resource` and `content`. A clean candidate rejects an override `resolved_state`; a conflicts candidate requires it.

Creating the candidate does not rewrite the Draft. Applying it saves the old Draft revision and changes the base and operations. This can happen inside Review submission, so a separate `/rebases` request is unnecessary for this flow. If you want to rebase while continuing to edit, call `/rebases` with the candidate ID, expected Draft version and authority Ref `If-Match` instead.

### 3. Submit the Review

For a current Draft at version `4`, the complete request body can be:

```http
POST /api/v1/reviews
Authorization: Bearer <access_token>
Content-Type: application/json
If-Match: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

{
  "title": "Clarify deployment rollback checks",
  "description": "Confirm the previous stable release before rollback.",
  "drafts": [
    {"draft_id": "drf_example", "expected_draft_version": 4}
  ]
}
```

For a behind Draft with a confirmed **clean** candidate, use this item in the `drafts` array instead:

```json
{
  "draft_id": "drf_example",
  "expected_draft_version": 4,
  "candidate_id": "rcn_example"
}
```

The request must use the actual head ETag read earlier. All Drafts must be author-owned, open, contain operations, and share the same Project and publication scope. Existing Organization resources must belong to the Project's allowed selection. A current Draft must omit reconciliation data.

The `200` response is `ReviewDetail`: `review`, the primary `draft` and `operations`, the complete `drafts` array, and `comments`. Use `drafts` for all files; the singular fields describe only the primary Draft. Review creation makes the Drafts `submitted` and records an `open` Review. It does not advance the authority Ref.

The same array supports multiple files. Each item carries its own Draft version and, when necessary, its own candidate. Ref validation, confirmed rebases and Review submission occur in one transaction.

### 4. Read the Review, then approve and publish

```http
GET /api/v1/reviews/rev_example
Authorization: Bearer <access_token>
```

Review the content and retain `review.version`. Fetch required snapshots with `GET /api/v1/commits/{commit_id}`. Each response contains a whole `CommitPayload` (`commit`, `tree`, `blobs`, `project_org_selection`), so reuse a snapshot when several files reference the same Commit.

An Organization owner/admin publishes the reviewed version:

```http
POST /api/v1/reviews/rev_example/merges
Authorization: Bearer <admin_access_token>
Content-Type: application/json
If-Match: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

{"expected_review_version":1}
```

On success, the `200` response is:

| Field | Meaning |
| --- | --- |
| `review` | Updated Review with `status: "merged"` and its new version |
| `commit_id` | The new published authority Commit |
| `applied_operation_count` | Number of materialized resource operations applied |

This is the current Desktop **Approve** path: from `open` directly to `merged`. Server records the decision, applies the content, creates a Commit, advances the Organization Ref and updates affected Project projections transactionally. Drafts become `merged`.

The separate `/decisions` API takes `decision`, `expected_review_version` and optional `body`. `approved` records approval but does not publish; `/merges` also supports that `approved` state and verifies the approved result hash. `rejected` reopens the Drafts. A later `/submissions` request includes `expected_review_version` and a new `drafts` array, with the same Ref precondition rules as initial submission.

### 5. Synchronize and read the published result

Daemon checks the Project's `/commit-state`, downloads its new snapshot and rebuilds the local Effective Memory view. The Project projection Commit need not equal the Organization Commit returned by merge. MCP `load` then reads the local effective result after synchronization; a successful merge response alone does not mean every device has already downloaded it.

## Synchronization, pagination and retries

Draft upload uses `POST /api/v1/draft-operation-batches`. Its request shape is:

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
        "content": {"content": "# Deployment rollback checklist\n\nConfirm the previous stable release before rollback."},
        "new_path": null
      }
    }
  ]
}
```

A successful response contains `accepted_operations` (local operation IDs) and `cursor`. The IDs correlate acknowledgments with daemon's local queue. The current handler echoes them but does not persist them as Server idempotency keys; stale expected Draft versions reject repeated writes. Project creation separately requires an `Idempotency-Key`, while Review writes use state/version preconditions. After a network failure with an uncertain write result, read the resource state before issuing a new write.

`GET /api/v1/draft-events?after_cursor=123&limit=50` returns `events`, `next_cursor`, `has_more`. The limit defaults to `50` and accepts `1`–`200`. Persist a returned cursor after consuming the events; continue while `has_more` is true. This stream is scoped to the current author's Drafts.

Admin list endpoints use `cursor` and `limit`, also defaulting to `50` with a `1`–`200` limit. Their current cursor encodes an offset; clients should still pass it back unchanged. Do not reuse a Draft event cursor for an Admin list.

## Failure handling

Server domain failures use this envelope; this example shows a stale object version:

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

Branch on `code`, not the human-readable message. HTTP parsing, proxy and transport failures may have different bodies or no JSON response.

| HTTP status / code | Interpretation | Caller action |
| --- | --- | --- |
| `401` | Missing/invalid session | Daemon attempts one refresh and retry; otherwise sign in |
| `403 forbidden` | Insufficient role or access | Use an authorized account; retries cannot grant permission |
| `404 not_found` | Missing or inaccessible object | Refresh the visible list; some access checks deliberately return not found |
| `400 invalid_request` | Invalid fields, transition or missing/malformed precondition | Correct the request or current action |
| `409 version_conflict` | Draft/Review/revision changed | Read current state and reassess the change |
| `412 precondition_failed` | Authority Ref changed | Read the new head and re-evaluate Draft freshness |
| `409 reconciliation_required` | Draft base is behind | Load the returned `candidate_id`, review the result, then submit confirmed reconciliation |
| `409 candidate_invalid` | Candidate no longer matches Draft or head | Create a new candidate; do not reuse the old resolution blindly |
| `409 draft_already_current` | Requested reconciliation is no longer needed | Refresh the Draft and proceed without reconciliation data |
| `5xx` or transport failure | Server or network failure | Preserve request ID when available; inspect state before repeating a write |

## OpenAPI and implementation limits {#contract-limits}

Use the checked-in [Public OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml) and [Admin OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.admin.v1.yaml) for schema lookup. Their `servers` URL is an example, not service discovery.

The route parity test checks **method/path coverage**, not every field or runtime behavior. Current gaps that matter to callers are:

| Declared surface | Verified implementation |
| --- | --- |
| `limit` / `cursor` on several Public lists | Project, Memory, Bundle, Draft, Review, comment and Commit list handlers do not implement generic cursor pagination. Several queries use fixed limits (Memory lists: 200) yet return terminal `page_info`; `has_more: false` does not guarantee a complete export. Use authorized snapshot/export paths when completeness matters. Draft events and Admin pagination are implemented separately. |
| `If-None-Match` / `304` on Memory and Bundle detail reads | Current handlers return `200` JSON with an `etag` body field; they do not implement those conditional reads. |
| `TreeEntry.type` still lists `rule/context/workflow/project_org_selection` and omits `description` | Rust/storage use `memory/project_org_selection` with `description`. Check this schema before generating a client; see [Data structures](/data-model). |
| `project` in scope enums | Retained for historical records/projections. New writable/publication Draft scope is `org`. |

For a release-specific integration, read the OpenAPI and implementation at the same release/tag. `/api/v1` alone is not a promise that every behavior in an older client or document remains active.

The repository's route coverage check is:

```bash
cargo test -p server http::tests::axum_routes_match_public_and_admin_openapi
```

Implementation references: [route registration, preconditions and errors](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/http.rs), [Draft/Review payloads](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/api.rs), [Review transactions](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/postgres.rs), [Memory handlers](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/http.rs), [Admin pagination](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/organization/http.rs).
