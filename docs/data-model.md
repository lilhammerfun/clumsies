# Core data model

Clumsies stores team knowledge and the history of changes to it. Start by separating **published content, proposed changes, and the content a Project currently reads**. They are related, but they are different states.

This page follows a deployment rollback checklist through the system. On a first visit, read the object map, worked example, and concurrency section. Use the storage tables when locating implementation code. See [Architecture](/architecture) for components and [API reference](/reference/) for requests.

## Six objects to know first

| Object | Question it answers | Example |
|---|---|---|
| Organization | Who publishes this shared knowledge? | The Acme team |
| Project | Which knowledge does this project use, and where are changes proposed? | Payments |
| Memory | Which piece of knowledge is this? | `operations/deployment-rollback.md` |
| Draft | What change is being proposed? | Add rollback steps to the checklist |
| Review | Which changes will people consider and publish together? | Checklist and alerting Drafts |
| Commit / Ref | What did a complete version contain, and which version is current? | Immutable snapshot C2 and a pointer to C2 |

Organization is the only Memory publication authority. Projects select Organization Memory and hold versioned snapshots of that selection. Selecting a Memory preserves its identity: two Projects can select the same `memory_id`.

```text
Organization
  ├─ published Memory ───────────────────┐
  └─ Organization Ref → Commit          │
                                         ↓ select by Memory ID
Project → Org Selection → Project Ref → Project Commit
  └─ Draft → Review → merge ────────────→ new Organization version

Content available locally = installed Project Commit + this Project's Draft changes
```

The last line is **Effective Memory**, the view assembled by the local daemon. “Effective” means this Project can read it; it does not mean every item has been approved and published.

## A complete worked example

The IDs and versions below are illustrative. `C1` and `P1` are readable labels; real Commit IDs are content-addressed strings.

1. **Published baseline.** Organization has Memory `mem_example_rollback`, at `operations/deployment-rollback.md`, containing a deployment rollback checklist. Its Ref points to Commit `C1`.
2. **Project selection.** Payments adds `mem_example_rollback` to its Org Selection. Server creates Project Commit `P1`. Its Tree entry still has ID `mem_example_rollback`, with `source: selected_org`. The Project Ref points to `P1`.
3. **Local installation.** The daemon downloads and installs `P1`. An Agent finds relevant passages through `memory.activate` and reads content through `memory.load`.
4. **Proposed change.** The Agent uses `memory.store` to add a rollback check. The daemon persists a Draft and its operations before syncing them. The Draft's `project_id` is Payments, its publication target `resource.scope` is `org`, and its `base_commit_id` is Organization Commit `C1`. Project snapshot `P1` and Draft base `C1` are different Commits.
5. **Use within this Project.** While the Draft is `open` or `submitted`, its changes overlay Payments' local Effective Memory. Other Projects do not receive this unpublished proposal.
6. **Review submission.** The author submits this Draft, or an ordered group of Drafts, to a Review. Server validates authorship, Project, Draft versions, upstream state, and targets.
7. **Publication.** An authorized Organization owner/admin merges the Review. One transaction applies all changes, creates Organization Commit `C2`, advances the Organization Ref, and refreshes affected Project projections. A failed publication check does not publish only part of the group.
8. **New snapshot installation.** The daemon installs the updated Project Commit. Merged Drafts stop contributing unpublished overlays; Agents read the new published content.

**For a new Memory:** before merge, it uses a provisional Draft identity. At merge, Server allocates a `mem_…` resource ID and automatically selects it for the originating Project. A create Draft's ID is not the final Memory ID.

**For a rename:** `operations/deployment-rollback.md` can become `runbooks/deployment-rollback.md` while `mem_example_rollback` remains unchanged. Selections continue to reference that ID.

## Memory: identity, path, and content

The domain calls it Memory; the database table is still `resources`. An HTTP detail response is `{ memory, content, etag }`: metadata is inside `memory`, and the body is the outer `content` field. Database column names are not automatically JSON field names.

| Field | Location / type | Meaning and nullability |
|---|---|---|
| `memory_id` / `resource_id` | HTTP / database, string | The same stable identity. New resources use `mem_`; historical IDs remain valid. |
| `org_id` | Database, string | Owning Organization; the org HTTP route derives it from the authenticated context. |
| `scope` | Both, string | Published Memory is currently `org`. `project` remains for historical reads and cleanup. |
| `project_id` | Both, string or null | Null for Organization resources. Projects that select it are represented in a separate relation. |
| `path` | Both, string | Relative path, unique among active resources in an Organization. It can change; it is neither identity nor a guaranteed local file location. |
| `name` | Both, string | Server derives it from the final path component, such as `deployment-rollback.md`. |
| `description` | Both, string | Semantic summary. Non-null in SQL, but may be empty; merge does not yet reliably preserve Draft summaries. See [implementation boundaries](/unified-memory-model#current-implementation-boundaries). |
| `content` / `body` | Outer HTTP field / database, string | Markdown content. There is no separate `content_format` field. |
| `content_hash` | Both, string | `sha256:…` hash of the body. A rename without a body change can leave it unchanged. |
| `revision` | Database, integer | Resource revision. HTTP `MemoryMeta` does not expose it directly; detail `etag` looks like `"rev-3"`. |
| `status` | Both, string | `active`, `deprecated`, or `archived`. Delete currently archives the resource; old snapshots retain its historical body. |
| `created_at` / `updated_at` | Database timestamps; HTTP metadata returns `updated_at` | Creation and latest modification time. They are not concurrency tokens. |

Three names have different jobs: `name` comes from the path; the daemon's display `title` comes from the first Markdown heading, falling back to the filename; Draft `title` describes the proposal. Editing a Markdown heading does not rename the resource.

## Selection and Bundle: ID sets with different jobs

| | Project Org Selection | Bundle |
|---|---|---|
| Owner | One Project | One user |
| Contents | Organization Memory IDs | Organization Memory IDs |
| Purpose | Define the Project's published baseline | Collect shared knowledge for reuse |
| Changes the Project Ref? | Replacing the selection rebuilds its projection | No |
| Copies Memory bodies? | No | No |

A selection replacement request uses `resource_ids: [...]`. Reads return `memories` metadata and the whole collection's `revision`. An empty array means an empty selection, not all Memory. Saving a Memory in a Bundle does not enable it for a Project.

## Draft: a baseline and ordered changes

A Draft records the published version a proposal started from, its target, and its changes. Its Project carries the proposal and local view; its Organization scope identifies the publication target.

| Field | Meaning |
|---|---|
| `draft_id` | Draft identity. The daemon may also store a corresponding remote Draft ID for synchronization. |
| `project_id` | Required Project carrying the proposal. |
| `resource.scope` | Must be `org` for a currently publishable proposal. |
| `resource.id` / `resource.path` | Resource locator. Use stable identity for an existing Memory; a create needs a path before its final ID exists. DTO nullability does not waive action-specific validation. |
| `base_commit_id` | Organization snapshot used as the baseline; nullable when no initial snapshot exists. |
| `operations` | Ordered create/update/rename/delete operations. Server persists ordering in `draft_operations.ordinal`. |
| `version` | Draft concurrency version. Writers supply their expected version. |
| `status` | `open`, `submitted`, `merged`, or `discarded`. |
| `coordination` | Computed upstream relationship: freshness, current Commit, resource changes, and reconciliation candidate. This is separate from lifecycle status. |

Create/update operations carry `content: { content: "Markdown…", description?: "Summary" }`; rename uses `new_path`; delete identifies a target to remove. Server stores the resulting content for an update. The daemon can accept text replacements and turn them into synchronized operations. A create followed by update/rename is materialized into the final new resource at publication.

`behind` means the Base Commit differs from the current Organization Ref. Someone publishing an unrelated document can make a Draft behind without creating a content conflict. Server compares **Base**, **Current**, and **Draft Result** to produce a reconciliation candidate. Applying a confirmed candidate through rebase updates the Draft baseline; it does not publish content.

## Review: the publication boundary

A Review contains at least one Draft. The `review_drafts` relation records membership and order; one Draft cannot belong to different Reviews simultaneously. API `draft_ids` and detail `drafts[]` describe the full group. Retained singular fields `draft_id`, `draft`, and `operations` refer to the first Draft; they are not the complete multi-file change.

Each submitted Draft includes `expected_draft_version`. A behind Draft can also provide a `candidate_id`, with a complete `resolved_state` when conflicts need resolution. Server can apply these confirmed candidates within the create/resubmit Review transaction.

Review states are `open`, `approved`, `rejected`, and `merged`. Its `version` guards operations on the Review. `approved_result_hash` binds approval to the full proposed result. Changed results cannot use an old approval; a rebase that preserves the result can retain approval. An authorized publisher can also merge an open Review directly, with Server recording the decision and actor.

Comments carry `review_version`. Line comments require both `anchor_path` and `anchor_line`; both are null for comments without a line anchor. This identifies the version and location under discussion.

## Blob, Tree, Commit, and Ref

These borrow version-control concepts to describe Clumsies Memory snapshots. They are **not Git commits in the project repository**.

| Object | Contents | Why it exists |
|---|---|---|
| Blob | Immutable text; `blob_id`, `content` | Several versions can reference identical content. |
| Tree | Entries linking Memory ID, path, source, description, and Blob | Records the resources and paths in a snapshot. A rename can reuse the Blob while changing the Tree. |
| Commit | `tree_id`, nullable `parent_commit_id`, scope, version, timestamp | Adds history and Organization/Project ownership to a complete snapshot. Root Commits have null parents. |
| Ref | Named current pointer; nullable `commit_id` | Answers which version is current. It may have no target before an initial snapshot exists. |

Server uses SHA-256 with an object-type prefix in the hash input to content-address Blob, Tree, and Commit objects. Consumers should use returned IDs. Memory `content_hash` and Blob ID use different hash inputs and are not interchangeable.

Selected Memory entries in a Project Tree have `type: memory` and `source: selected_org`. A `type: project_org_selection` entry stores the selection snapshot. It is system configuration, not Agent-readable Memory.

`GET /api/v1/commits/{commit_id}` returns `commit`, `tree`, `blobs`, and nullable `project_org_selection`: a **complete snapshot**, not one file. When several files share a Commit, consumers should reuse that snapshot during a load instead of downloading it once per file.

## Versions and concurrency

A writer states which version it read. Server accepts the change only if that precondition still holds. This is commonly called compare-and-swap, or CAS. A conflict requires rereading and coordinating the change, not blindly retrying the stale request.

| Value | What it identifies or protects | What it cannot replace |
|---|---|---|
| Memory `revision` / detail `etag` | A published resource revision | Draft version or current Organization Ref |
| Selection `revision` | The Project's complete selection | Memory revision |
| Draft `version` | Proposal content and lifecycle | Base Commit |
| Review `version` | Review coordination and decision state | Every contained Draft's version |
| Organization Ref `commit_id` | Upstream snapshot for publication/reconciliation | Project Ref `commit_id` |
| `content_hash` | Equality of body bytes | Path, approval, permission, or publication time |
| Effective Memory hash / Index Revision | Whether local content and retrieval index match | Server publication version or human approval |

See [API reference](/reference/) for exact request headers, errors, and examples.

## Where the data lives

### Server: PostgreSQL

Server transactions maintain these tables. Integrations should use APIs instead of writing tables directly.

| Domain object | Actual tables | Key relationship |
|---|---|---|
| Organization, user, Project | `orgs`, `users`, `projects`, `project_members` | An Organization has Projects; membership is keyed by `(project_id, user_id)`. |
| Current Memory | `resources` | `resource_id` primary key; `(org_id, path)` unique for active org resources. |
| Project selection | `project_org_selection_states`, `project_org_resource_selections` | Collection revision plus `(project_id, resource_id)` membership. |
| Bundle | `personal_bundles`, `personal_bundle_items` | User ownership and resource membership with ordering positions. |
| Draft and synchronization events | `drafts`, `draft_operations`, `draft_events` | Ordered operations belong to a Draft; event sequences support incremental sync. |
| Upstream coordination | `draft_reconciliation_candidates`, `draft_revisions`, `draft_rebases` | Candidate bound to Draft version and Base/Current; rebase retains a previous revision. |
| Review | `reviews`, `review_drafts`, `review_comments`, `review_merges` | Ordered Draft group, versioned comments, and final merge record. |
| Published snapshots | `blobs`, `trees`, `tree_entries`, `commits`, `refs` | Ref → Commit → Tree → entry → Blob. |

### Local daemon: SQLite and files

Local state includes both unsynchronized user changes and rebuildable caches. Drafts must not be treated as disposable cache entries.

| State | Main storage | Ownership / durability |
|---|---|---|
| Directory → Project binding | Central SQLite `project_bindings` | Installation configuration keyed by Server URL and canonical directory. |
| Local Drafts and queued operations | `local_drafts`, `local_draft_operations` | Durable proposals. Operation records carry sync state and may not yet exist on Server. |
| Synchronization and HTTP cache | `remote_draft_events`, `sync_retries`, `server_response_cache` | Event progress, retries, and read copies; not publication authority. |
| Downloaded immutable snapshots | `cached_blobs`, `cached_trees`, `cached_commits`, `cached_refs`; installed generation files | Local copies of Server snapshots. |
| Effective Memory | Assembled from installed snapshots and Draft operations | Derived view; no remote authoritative Effective Memory table. |
| Retrieval index | Project SQLite `search_revisions`, `search_resources`, `search_units`, `search_units_fts`, `search_heads`, etc. | Rebuildable. Retains Commit/Draft provenance and must match effective content and model versions. |

Project Local Storage moves only managed generations and retrieval data. Central Drafts, queues, and credentials stay in central storage; see [Runtime](/runtime). Retrieval evaluations have separate boundaries described in the [glossary](/glossary); they are not part of Memory publication snapshots.

## Continue reading and implementation sources

- [Organization Memory](/artifact): publication authority and Bundles.
- [Project](/workspace): selection, directory binding, and local views.
- [Unified Memory design](/unified-memory-model): invariants, transactions, and implementation boundaries.
- [Memory API types](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/api.rs) and [change API types](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/api.rs): actual JSON fields, nullable values, and enums.
- [Database migrations](https://github.com/lilhammerfun/clumsies/tree/main/crates/server/migrations): read in order; the original schema contains subsequently removed fields.
- [Snapshot/resource persistence](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/memory/postgres.rs), [Review transactions](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/changes/postgres.rs), [Draft overlay](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/overlay.rs), and [index schema](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/index.rs): behavior at each layer.
