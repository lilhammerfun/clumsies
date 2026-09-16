# Glossary

Use this page to look up a term. For a first introduction, read [Meet Clumsies](/overview) and the [core data model](/data-model); you do not need to memorize the vocabulary first.

## Organization

The organization owning shared Memory publication history. Organization owners/admins can make publication decisions; Projects select its content for their work.

## Project

A Server-identified project that manages members, Organization Memory selection, and proposed Drafts. Local directories bind to it but are not its identity. Workspace is a former name; current APIs use `project_id`.

## Memory

A Markdown knowledge resource with a stable ID. SQL calls it `resources`; HTTP uses `memory_id`. Active published resources currently belong only to Organization. Rules, procedures, and background notes all use the same Memory object.

ID identifies the resource, path locates it in the namespace, and revision identifies a resource revision. Renaming preserves ID. `name` comes from the filename; the daemon's display title comes from a Markdown heading or filename. See the [data model](/data-model).

## Organization authority

The official publication boundary expressed by the Organization Ref and its Commit history. Authority means content passed the Organization publication process, not that it is necessarily correct. Drafts and retrieval rankings cannot grant themselves this status.

## Project Org Selection

The explicit set of Organization Memory IDs selected for a Project's published baseline. The collection has its own revision. Selection neither copies Memory nor represents a user's personal Bundle.

## Projection

A purpose-specific view generated from existing authoritative data. A Project Commit is a versioned projection of Organization content through Org Selection. It supports synchronization without becoming another publication source.

## Effective Memory

Content the daemon assembles from the installed Project projection and that Project's `open` / `submitted` Drafts. Active Draft results are computed from their own Base and operations. The view can contain unpublished content and can lag Server publication while synchronization catches up.

## Draft

A proposal carried by a Project and targeting Organization publication. It records Base Commit, version, and ordered create/update/rename/delete operations. Lifecycle states are `open`, `submitted`, `merged`, and `discarded`. A local Draft may not yet be synchronized and is not disposable cache data.

## Base / Current / Draft Result

The three inputs to comparison: Base is the publication snapshot the change started from; Current is today's published state; Draft Result applies proposal operations to Base. Their comparison determines whether upstream changes and the proposal can be combined.

## Freshness / Reconciliation

Freshness says whether a Draft matches the current Ref: `current` or `behind`. Reconciliation records comparison availability or result: `unknown`, `clean`, or `conflicts`. Behind does not necessarily mean conflicted; neither dimension is a Draft lifecycle state.

## Reconciliation candidate / Rebase

A candidate is a Server-generated comparison bound to Draft version and Base/Current Commits. Rebase confirms and applies it, retaining a previous revision, updating Base, and expressing operations against the new baseline. It does not publish Memory. Stale candidates require another comparison.

## Review

A Server object for coordinating and publishing an ordered group of Drafts. Merge publishes the group in one transaction. Approval binds to a result hash; changed results cannot use an old approval. Authorized users can merge open or approved Reviews.

## Blob / Tree / Commit / Ref

- **Blob:** immutable text that several snapshots can reference.
- **Tree:** entries connecting Memory IDs, paths, and provenance to Blobs.
- **Commit:** an immutable complete snapshot referencing a Tree and parent; not a Git commit in the code repository.
- **Ref:** a movable pointer to the current Commit. Organization Ref identifies publication; Project Ref identifies a selection projection.

## Revision / Version / ETag / CAS

Revision and version identify changes or concurrency state for different objects and are not interchangeable. ETag is an HTTP representation of version identity, such as resource detail `"rev-3"`. CAS, compare-and-swap, accepts a write only if its expected version still holds. Commit IDs and body hashes also have distinct purposes; they are not universal version tokens.

## Content hash / Effective Memory hash / Index Revision

Content hash identifies body content. Effective Memory hash identifies the local effective-content input. Index Revision identifies an index built from content plus model, parser, and other retrieval versions. The index must match the effective content being queried. None of these is an approval or permission credential.

## Bundle

A user's Server-stored set of Memory IDs, similar to a personal collection of shared knowledge. Changing or deleting a Bundle does not modify Memory or automatically change Project Org Selection.

## Project binding

The local mapping from Server URL and canonical directory to `project_id`. A managed host-plugin must resolve and revalidate this binding. Plain manually started `mcp serve` can fall back to the Desktop-selected Project when no directory binding exists. See [Project](/workspace).

## Generation / Project Local Storage

A generation is an immutable snapshot directory installed by the daemon. Project Local Storage is where an installation keeps a Project's rebuildable generations and search database. It is not a Server project-directory setting and does not relocate central Drafts, queues, or credentials.

## Daemon / Runtime proxy / XPC

The daemon is the resident local process that persists data, synchronizes, and retrieves Memory. A proxy translates Agent protocols into local requests. XPC is the macOS interprocess communication mechanism. An MCP proxy owns neither a separate business database nor a model instance. See [Architecture](/architecture).

## Server

The shared HTTP service governing identity, Organization Memory, Project selection, Drafts/Reviews, and snapshots. PostgreSQL holds its state. Local directory bindings, retrieval models, and retrieval history are not Server Memory publication data.

## Adapter / Agent Host

An Agent Host is the product running the coding Agent. An Adapter makes Clumsies available in that host by installing MCP configuration. Codex uses a managed global Plugin; other supported hosts have their own integration mechanisms. See [Adapter](/adapter).

## MCP

The protocol Agents use to invoke tools. Clumsies exposes one `memory` tool with `activate`, `load`, and `store` actions. It exposes no Review approval, merge, or arbitrary Server request tool.

## Retrieval Run / Evaluation Case / Corpus

A Retrieval Run records a local `memory.activate` query, data/index identity, candidates, results, and latency. An Evaluation Case freezes a successful run's query, complete corpus of resources, and human evidence judgments into an evaluation sample. These explain and test retrieval; they are not additional MCP tools and are not uploaded with Memory publication.

## Issue / Assignee / Claim

These terms occur in earlier work-coordination designs and migrations: Issue means a work item, assignee its owner, and claim a temporary execution lease. Current Server routes expose no corresponding shared Issue API. Historical tables alone do not establish an available domain capability.

## Rule / Workflow / Context and historical names

Rule, Workflow, and Context were closed Memory types. They now describe document purposes rather than three Server/MCP content types; some old macOS UI classification remains.

Artifact was the Organization Memory management surface; Hub and Local were UI labels; Manifest was a runtime term; Attestation belonged to a retired client event stream. Check historical documents against their implementation period instead of treating old terms as current objects or interfaces.
