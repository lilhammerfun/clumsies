# Domain interfaces

Clumsies has four interface boundaries. They serve different callers, even when they participate in the same action. Reading Memory through MCP, editing a local Draft, and publishing Project or Organization Memory are separate capabilities.

Start here if you know what you want to do but do not know which component owns it. For object definitions, read [Data model](/data-model); for a complete example, read [Core flows](/flows).

## Choose the interface

| Caller | Interface | What it can do | Where it runs |
| --- | --- | --- | --- |
| Coding Agent | MCP over stdio: one `memory` tool | Find relevant fragments, load complete Memory, propose Draft changes | App-bundled proxy → resident daemon |
| macOS App | Local XPC with typed request/response payloads | Workspace binding, local editing, sync, search diagnostics, authenticated Server requests | App → resident daemon |
| Product client, normally daemon | Public HTTP `/api/v1/...` | Read shared data, sync Drafts, submit/review/publish changes subject to role checks | Client → Server |
| Organization administrator | Admin HTTP `/api/v1/admin/...` | Manage organization, membership, Projects, tokens and audit records | Native Administration → daemon → Server |

**“Public API” means the product API surface; it does not mean anonymous access.** Normal Public and Admin requests use `Authorization: Bearer …`. Organization administration requires the Organization `owner` or `admin` role; project administration routes also accept the relevant Project roles. Publication checks the Review target: Project owner/admin for Project Memory, Organization owner/admin for Org Memory.

First-installation setup is a separate exception: `/api/v1/setup/...` uses a short-lived setup cookie and CSRF token. OIDC entry/callback, token exchange, setup entry points, and `/api/v1/admin/health` have their own bootstrap access rules. The health URL is public despite its `admin` path. See [Authentication and sessions](/reference/auth).

## The domains

The route families below are a map of responsibilities. Braces identify a value supplied by the caller, such as `{project_id}`; paths are relative to the configured Server origin.

### Identity and organization

This domain answers **who is calling, and which actions may they perform?** A Project is a collaboration and Memory-selection boundary. Organizations and Projects own published Memory.

| Capability | Representative HTTP operations | Access rule |
| --- | --- | --- |
| Sign in and refresh | `GET /oauth2/authorization/oidc`, `GET /login/oauth2/code/oidc`, `POST /api/v1/auth/token` | OIDC authorization code + PKCE, or rotating refresh token |
| Read identity; sign out | `GET /api/v1/me`, `DELETE /api/v1/auth/session` | Current authenticated session |
| Discover Projects and members | `GET /api/v1/projects`, `GET /api/v1/projects/{project_id}`, `GET /api/v1/projects/{project_id}/members` | Server filters/checks access |
| Create Projects | `POST /api/v1/projects` | Authenticated organization members; requires `Idempotency-Key`; the creator becomes the Project owner |
| Update/delete Projects | `PATCH` / `DELETE /api/v1/projects/{project_id}` | That Project's admin or an Organization owner/admin with membership access; requires version `If-Match` |
| Configure Projects and membership | `/api/v1/admin/projects/{project_id}`, `/members`, and member subroutes | Project members can read; that Project's admin or an Organization owner/admin can mutate, including organization administrators managing projects they have not joined |
| Find members to add | `GET /api/v1/admin/projects/{project_id}/member-candidates` | That Project's admin or an Organization owner/admin; supports `q`, `limit`, `cursor`; returns user profiles excluding disabled users and existing project members |
| Administer the organization | `/api/v1/admin/org`, `/members`, `/projects`, `/tokens`, `/audit-events` | Organization owner/admin; each path here is under `/api/v1/admin` |

`GET /api/v1/me` returns the caller’s membership role in `projects[].role`; organization members receive the `project:create` capability. The organization-wide directory `/api/v1/admin/projects` remains restricted to Organization owners/admins.

A role in a Project is not automatically an Organization administrator role. Server checks publication authority independently of whether the caller can see a Project.

### Memory and selection

This domain answers **which published resources exist, and which ones does this Project use?** A Memory has stable identity and Markdown content. A selection contains resource IDs; it does not copy the content into a new authority.

| Capability | HTTP operations | Result or constraint |
| --- | --- | --- |
| Browse published Organization Memory | `GET /api/v1/org/memories` and `/{memory_id}` | Metadata list or complete resource |
| Read published Project-owned Memory | `GET /api/v1/projects/{project_id}/memories` and `/{memory_id}` | `scope=project` resources; these routes do not return the selected Organization projection |
| Read/change Project selection | `GET` / `PUT /api/v1/projects/{project_id}/org-selections` | `resource_ids` input; replacement needs that Project’s admin or an Organization owner/admin with membership access, plus selection revision `If-Match` |
| Save a personal selection Bundle | `GET` / `POST /api/v1/me/bundles`; `GET` / `PATCH` / `DELETE /api/v1/me/bundles/{bundle_id}` | Owned by the current user; editing/deletion uses Bundle revision `If-Match` |
| Export managed organization data | `GET /api/v1/admin/memory-export` | Admin export of Memory, Drafts, selections and Bundles |

For the current selected Organization view, read `/org-selections` under the Project, or its `/commit-state` and referenced Commit snapshot. To answer “what will my Agent read **right now**?”, use local MCP `load` or `activate`. The daemon constructs Effective Memory from the Project's published projection plus its local Draft overlays. A Server Memory GET alone cannot answer that question.

### Drafts and synchronization

This domain answers **what changes have been proposed, and have they reached Server?** A Draft is carried by a Project and targets exactly one owner, Project or Organization. Local persistence, Server synchronization and publication are separate milestones.

| Capability | Interface | Important input/output |
| --- | --- | --- |
| Make an Agent proposal | MCP `memory.store` | Exact replacements for updates; returns local operation ID, Draft ID and sync status |
| Create/read/edit a Server Draft | `POST` / `GET /api/v1/drafts`; `GET` / `PATCH` / `DELETE /api/v1/drafts/{draft_id}` | Creation needs Project and daemon installation IDs; Draft edits are author-scoped and version checked |
| Append a full materialized operation | `POST /api/v1/drafts/{draft_id}/operations` | `action`, `resource`, content/path fields; integer Draft `If-Match` |
| Upload queued operations | `POST /api/v1/draft-operation-batches` | Each item has `local_operation_id`, `draft_id`, `expected_draft_version`, `operation` |
| Consume changes for sync | `GET /api/v1/draft-events` | `after_cursor`, `limit`; returns events and next cursor for the current author's Drafts |
| Compare against a newer base | `POST /api/v1/drafts/{draft_id}/reconciliation-candidates` | Expected Draft version → Base/Current/Draft comparison |
| Automatically reconcile a clean Draft | `POST /api/v1/drafts/{draft_id}/auto-rebases` | Author only; expected Draft version; conflicts preserve the baseline and notify the author |
| Apply a confirmed comparison | `POST /api/v1/drafts/{draft_id}/rebases` | Candidate ID + expected Draft version + authority Ref `If-Match`; saves the previous Draft revision |

The HTTP operation format is not the MCP operation format. MCP `update` accepts `expected_hash` and exact replacements. The daemon validates these against complete Effective Memory and turns them into the full content operation used for synchronization.

### Review and publication

This domain answers **which proposed changes are being reviewed, and who can publish them?** One Review can contain several Drafts, including a large batch of files, but every Draft must target the same publication owner.

| Capability | HTTP operation | Concurrency and permission |
| --- | --- | --- |
| Submit Drafts | `POST /api/v1/reviews` | Author-owned, open Drafts from one Project and one scope; each Draft version + authority Ref `If-Match` |
| Read Review/detail/comments | `GET /api/v1/reviews`, `GET /api/v1/reviews/{review_id}`, `GET /api/v1/reviews/{review_id}/comments` | Authorized Review readers |
| Comment | `POST /api/v1/reviews/{review_id}/comments` | Expected Review version; optional paired `anchor_path` and one-based `anchor_line` |
| Resubmit after changes | `POST /api/v1/reviews/{review_id}/submissions` | Draft author; expected Review version, each Draft version, authority Ref `If-Match` |
| Record a decision | `POST /api/v1/reviews/{review_id}/decisions` | Publication owner/admin; `approved` or `rejected`, expected Review version |
| Approve and publish | `POST /api/v1/reviews/{review_id}/merges` | Publication owner/admin; expected Review version + authority Ref `If-Match` |

The current Desktop **Approve** action calls `/merges`: an `open` Review becomes `merged`, with decision metadata and the new authority Commit recorded in the same transaction. The separate `/decisions` API remains implemented: `approved` records approval without publishing, and `/merges` can subsequently publish an `approved` Review if its approved content still matches. A rejection reopens the Drafts for editing.

A Project Review can include an explicit `org_contribution` selection. After Project merge, it creates a separate Org Review from that fixed commit; `POST /api/v1/reviews/{review_id}/org-contribution` retries creation idempotently. Failure or rejection leaves the Project publication intact.

See the [HTTP walkthrough](/reference/http-api#walkthrough) for exact request shapes, including stale Draft reconciliation.

### Snapshots and local retrieval

This domain answers **which version is published, and what content belongs to that version?**

| Capability | Interface | Meaning |
| --- | --- | --- |
| Check published head | `GET /api/v1/org/commit-state` or `GET /api/v1/projects/{project_id}/commit-state` | Current Ref, latest Commit, update availability and strong ETag |
| Read history | `GET /api/v1/org/commits` or `GET /api/v1/projects/{project_id}/commits` | Organization history or combined Project publication/selection history |
| Download one snapshot | `GET /api/v1/commits/{commit_id}` | Full `commit`, `tree`, `blobs`, optional Project selection |
| Retrieve relevant fragments | MCP `memory.activate` → XPC `activate_memory` | Task query → ranked fragments from local Effective Memory |
| Load a known resource | MCP `memory.load` → XPC `load_memory` | IDs/exact paths → complete resources and content hashes |
| Inspect retrieval | Private XPC diagnostic methods | Local Retrieval Runs, evaluation and index state; see [Retrieval evaluation](/retrieval-evaluation) |

A Commit download is a **whole snapshot**, not a single-file diff. Multiple files can share the same base/current Commit. Clients should load each required Commit once per operation, then derive file changes locally.

## Local XPC is a separate contract

The XPC envelope contains `method`, `payload`, `request_id`, and an optional `agent_runtime` marker. Responses contain `ok`, `payload`, and an optional structured `error`. The daemon dispatches each method to its request type. This is local macOS IPC, not an HTTP service on a localhost port.

The App can use the private `server_request` method with an HTTP method, relative path, headers and body. Daemon supplies the configured Server origin and credentials. The MCP proxy cannot use this general bridge: it exposes only its three typed Memory operations. Agent runtime calls also carry a protocol revision/build identity, checked before binding and on dispatch.

Workspace directory binding, Keychain credentials, local storage paths, search indexes, and Draft queue state belong to daemon. They are not Server resources and do not acquire Public API routes merely because Desktop displays them.

## Contracts and compatibility

- [HTTP examples and failure handling](/reference/http-api) explain wire fields, preconditions and current implementation gaps.
- [MCP reference](/mcp) defines the single Agent-facing tool.
- [Public OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml) and [Admin OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.admin.v1.yaml) describe HTTP methods and schemas.
- [Server routes](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/routes.rs), [Draft/Review types](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/draft/dto.rs), [Review DTO](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/review/dto.rs), [Memory types](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/memory/dto.rs), and [daemon types](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/types.rs) show the implementation behind those contracts.

Both `project` and `org` scopes can publish. Project editing defaults to `project`; existing Org Drafts retain their target. A Project Ref combines Project-owned content with selected Org content. The removed MCP `retrieve` tool and old separate rule/workflow/context APIs are not current integration entry points. Preserve returned identifiers as opaque values instead of inferring their meaning from a prefix.
