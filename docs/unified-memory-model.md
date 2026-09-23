# Unified Memory design

This page explains the constraints and tradeoffs behind the [core data model](/data-model). Read it when implementing or reviewing a feature. It describes current code behavior; see [Project authority cutover](/project-authority-migration) for migration history.

::: warning Historical Org-only design
The 2026-09-23 [Project and Organization Memory ownership decision](/project-org-memory-ownership) replaces the Organization-only target below. Project and Organization each own published Memory, with one target per Review and independent Organization contributions. This page preserves the historical Org-only design; use the ownership decision for current publication behavior.
:::

## Why there is one Memory type

Deployment rollback checklists, coding constraints, and architecture notes are all Markdown Memory with stable IDs. A path under `rules/` or `workflow/` does not grant a different type, permission, or execution capability.

This separates changes to a team's knowledge organization from protocol changes across Server, daemon, MCP, and clients. The system governs identity, versions, permissions, and publication. Retrieval judges relevance to a task; it does not approve content or promote a Draft to published authority.

The current Memory content contract carries a body and optional description. It has no Category/Tag, `content_type`, `content_format`, `agent_instruction`, or `invocable_skill` fields. Historical `ctx_`, `rul_`, and `wfl_` IDs remain valid; their prefixes do not determine current behavior.

## One publication source, multiple Project projections

Publication authority determines the current official shared content. The Organization Ref points to the current Organization Commit; a Review merge updates that version.

Project Org Selection stores Memory IDs. Server uses them to generate a Project Commit from current Organization content and update the Project Ref. Such a purpose-specific view of existing data is a **projection**. The Project Ref is a synchronization unit, not another publication entry point.

Why keep a Project Commit instead of filtering the Organization on every Agent call? The daemon can download a specific Project snapshot, install and index it locally, and track which version it is using. Selection changes and relevant upstream resource changes refresh the projection.

Two boundaries matter:

- Removing a Project selection changes that Project's baseline; it does not delete Organization Memory.
- A Draft originates from a Project but targets Organization publication. Newly created Memory is automatically selected for the originating Project at merge; changes to existing Memory must target resources already selected by that Project.

## Local views and publication views

```text
Installed Project projection
  + this Project's open/submitted Draft operations
  = Effective Memory
  → Index Revision matching content and model versions
  → memory.activate / memory.load
```

Successful `memory.store` means the daemon persisted the proposal and scheduled synchronization. It does not mean Server has received it, a Review has approved it, or the Organization Ref has advanced. Successful synchronization is also different from publication.

Overlays preserve Commit or Draft provenance. Search indexes are derived from this view. When an index is behind, readiness must be handled explicitly; an index for another version cannot be presented as current content.

The local daemon may not yet have downloaded the latest Project Commit. “Currently effective locally” can therefore differ from “currently published on Server.” Investigations should examine the installed Ref, Draft state, and index version together.

## Three independent Draft state dimensions

| Dimension | Values | Question |
|---|---|---|
| Lifecycle `status` | `open`, `submitted`, `merged`, `discarded` | Where is the proposal in its lifecycle? |
| `freshness` | `current`, `behind` | Does Base Commit equal the current upstream Commit? |
| `reconciliation` | `unknown`, `clean`, `conflicts` | Is a comparison available, and can the changes be combined without conflicts? |

A Draft can be `submitted + behind + clean`: submitted for review, behind upstream, but reconcilable. Treating behind as failure or conflicts as a terminal lifecycle state obscures recovery paths.

Server compares Base, Current, and Draft Result. A candidate is bound to Draft version and current Ref; generating it does not modify the Draft. Rebase saves the previous Draft revision, advances Base, and expresses operations against that new baseline. It does not publish.

Each behind Draft in a single- or multi-Draft submission can carry its own confirmed candidate. Server applies the candidates within the create/resubmit Review transaction. Missing candidates return reconciliation information; stale candidates require rereading and comparing again.

## Review transactions and approval

A Review holds a nonempty, ordered, deduplicated set of Drafts. They must belong to one Project and publication scope and be owned by the submitting author. Both operation ordering within a Draft and Draft ordering within a Review are part of the data semantics. UUID ordering and asynchronous response order cannot substitute for them.

The publication transaction performs five main steps:

1. Lock coordination data, the Review, and its Drafts; check Review version, Draft states, and current Organization Ref.
2. Ensure each Draft Base matches the current Ref. Behind proposals require reconciliation.
3. For an approved Review, verify the complete result hash so approval of old content cannot publish changed content.
4. Materialize and apply operations in order, create one Organization Commit, and advance the Organization Ref once.
5. Refresh affected Project projections, record the merge, and mark the Review and Drafts merged.

Publication changes roll back together on failure. Some failure paths retain a generated reconciliation candidate so the client can proceed with coordination; that is not partial Memory publication.

An owner/admin may approve then merge, or directly merge an open Review. Direct merge still requires authorization and full validation and records the decision actor. Approval binds to the result: a rebase preserving the complete result may preserve approval; a changed result invalidates it. A timestamp, title, or previous approval is not a substitute for checking the result.

## Snapshot read costs

A Commit payload contains the complete Tree and Blob content. Commit-state currently returns `incremental_supported: false`. Several Review files may reference the same Base/Current Commit; they share a snapshot rather than each owning an independent remote file version.

Clients should organize a load around unique Commit IDs, then map snapshot contents to files. File count, unique snapshot count, response size, and page readiness are separate measurements. A successful HTTP request proves that request completed, not that the whole Review page is ready.

## Current implementation boundaries

These limitations remain in the checked implementation. Design intent must not be presented as a completed capability.

| Boundary | Current behavior | Implication |
|---|---|---|
| Stale OpenAPI TreeEntry declaration | Rust/SQL use `memory` and `project_org_selection`, and runtime may return `description`; public OpenAPI still lists the old three kinds and omits description | Verify actual DTOs when integrating; old generated types are insufficient |
| Incomplete description persistence | Drafts can carry descriptions and `resources.description` is non-null, but merge create/update SQL does not write it | Published descriptions may be empty or retain old values; end-to-end description retrieval is not guaranteed |
| Remaining macOS classification | `MemoryKind` still participates in some UI, path, and display behavior | UI labels are not Server domain types or Agent execution capabilities |
| Historical Project Memory routes | `/projects/{project_id}/memories` queries historical project-scoped resources | It is not Selection + Draft Effective Memory and must not build the current Project view |

Update these boundaries alongside the corresponding code fixes. Historical read compatibility neither creates a new Project publication authority nor restores retired Rule/Workflow/Context write contracts.

## Checks when reviewing an implementation

- Does Memory identity survive renames, projections, and migrations? Are path collisions checked in the correct namespace?
- Are Organization publication, Project selection, and local Draft provenance distinct? Is local store success mistakenly presented as publication?
- Does each operation use its own required version and Ref, without mixing resource revision, Draft version, and Review version?
- Is multi-Draft publication atomic, with every Draft checked rather than only the first?
- Does the derived index match Effective Memory, parser, and model versions?

Implementation sources: [Memory DTOs](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/memory/dto.rs), [Review/Draft DTOs](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/draft/dto.rs), [Review DTO](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/review/dto.rs), [publication and reconciliation](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/review/repository.rs), [snapshot generation and resource writes](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/memory/repository.rs), [Commit persistence](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/commit/repository.rs), [commit-state](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/commit/service.rs), [index implementation](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/index.rs), [public OpenAPI](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/openapi/clumsies.public.v1.yaml), and [macOS MemoryKind](https://github.com/lilhammerfun/clumsies/blob/main/apps/macos/Sources/Libraries/Models/MemoryModels.swift).
