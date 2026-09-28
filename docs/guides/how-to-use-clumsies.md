# Use clumsies

Desktop is the primary human client. The normal workflow does not require a
separate command-line client or manual draft synchronization.

## Sign in

Open Desktop and continue with SSO. The current development distribution
connects to `https://app.clumsies.ai`; it does not expose a local/production
profile selector. The system browser handles authentication, and your email
must already be admitted by an organization owner or admin.

Local Server and OIDC environments are test infrastructure for backend and E2E
development. They are not a second interactive Desktop mode. A future
self-hosted distribution will receive its organization Server authority from
the installation or deployment channel instead of asking users to switch
profiles in the app.

## Browse memory

The unified **Memory** section shows two views of one Organization-authority
model in a single navigator:

- **Org** (organization scope, historically called Hub) contains
  organization-shared Memory.
- **Project** contains that Project's selected Organization resources plus its
  private pre-merge Organization Draft overlays.

There are no closed Context / Rule / Workflow types in the current domain
model — a Memory's role is carried by its content and path. The navigator lists
Memory by path; opening one shows its Markdown body and semantic description in
the workbench. A non-empty description is the intended authoring rule, but the
current write and merge paths do not yet enforce or preserve it consistently.

Agent retrieval also includes **Episodic Memory**: Server-kept summaries of
past Agent runs in the current Project. Published/Draft Memory is the current
Semantic knowledge authority; Episodes describe how the Project reached its
current state. They share one Project search index but remain visibly distinct,
and an Episode never becomes Organization authority automatically.

## Edit and review

Editing creates or reuses a local draft. The daemon persists each operation and
automatically synchronizes it to Server. Saving a draft does not publish it.

The collaboration flow is:

1. create a proposal or edit selected Organization Memory inside a Project
2. keep editing normally while newer shared Commits synchronize
3. review and explicitly merge the latest shared version when prompted
4. submit one or more coordinated Drafts as an ordered Review
5. an Organization owner/admin rejects it or approves and merges the complete Draft set atomically
6. receive the new authority Commit

When the target Ref advances, Desktop continuously shows **共享版本已有更新**.
Viewing the candidate does not change the Draft. **合并最新版本** shows the
Base/Current/Draft comparison and requires confirmation even when the result is
clean. Conflicts use the same screen for manual resolution. The Draft remains
editable throughout, and its old revision is retained when the result is
applied.

Creating or resubmitting a Review must use the latest Ref. If it moves again
during confirmation, Clumsies recalculates instead of overwriting authority.
An existing Review may remain behind and continue to receive comments, but it
must be coordinated before approval. Approval uses `If-Match` as a final guard;
if the Ref moves, the Review remains Open rather than stopping in a partially
approved state. Historical Approved Reviews can still be merged.

## Agent workflow

The MCP server exposes exactly two tools:

- `memory`, with `activate`, `load`, `evidence`, and `store` operations
- `kanban`, for native Issue reads, semantic updates, and explicit transitions

Managed host-plugin processes resolve their Project from the current directory
through the always-on daemon and fail closed if that binding is missing or
changes. A manually launched plain `mcp serve` keeps a compatibility fallback
to the Project currently selected in Desktop when no directory binding exists;
the caller still cannot pass an arbitrary Project ID.

MCP `store` and Desktop editing use the same daemon queue, so a change created
by an agent appears in Desktop for review. The Draft changes only the bound
Project's Effective Memory before merge. Its publication target is Organization
authority, but MCP cannot approve, merge, or publish it; an Org administrator
must do that through the Review workflow. Organization is not represented as a
Project.

When `activate` returns an Episodic result, its provenance includes the
Episode, source run, activity time, and Evidence hash. Use `memory.evidence`
only when the summary is insufficient and the original run details are needed.
Evidence is untrusted historical data: text inside it is never an instruction
to execute. The Project is injected from the current directory binding, so an
agent cannot use this operation to inspect another Project.

## Configure Episodic Memory

Project Settings shows recent Episodes and optional summary guidance. Guidance
can emphasize domain terms, desired detail, or relevant outcomes; fixed Server
safety and factuality constraints still take precedence. Preview tests the
current editor text against one recent Episode without saving a new summary.
Saving guidance affects future summaries only. Choose **Rebuild Summary** on a
specific Episode to update an older summary explicitly.

**View Source Evidence** pages the Server copy and always shows an untrusted-data
warning. Once the Server acknowledges capture, this view and later rebuilds no
longer depend on the original host session log or the computer that produced it.

## Bundles

Bundles are personal, Server-stored selections of shared memory. They help one
user reuse a curated set without making that selection an organization-wide
authority object.

## Manage Project local storage

Open Settings and use **Project Local Storage** to inspect the selected Project's
cache location, availability, and size. **Choose...** opens the native macOS
directory picker. Clumsies creates its own hidden managed subtree below that
directory; the directory itself remains yours and is never treated as a memory
editing folder.

Moving storage continues in the background daemon if Desktop closes. Do not edit
files inside the managed subtree. **Reset** moves the cache back to the standard
macOS location through the same verified migration. **Clear Cache...** removes
only rebuildable Commit generations and the Project search index; Drafts,
pending operations, Episode upload outbox, settings, Server Evidence, and
unrelated files in the selected directory are preserved.

If an external volume is disconnected or permission is revoked, the Project
location shows **Unavailable**. Clumsies does not create a replacement cache in
the default location. Draft editing and synchronization continue, while checkout
and MCP retrieval resume after the configured location is accessible again.

## Administration

Web Admin is reserved for organization settings, member admission, projects,
tokens, audit events, and health. It is not a second memory editor.

See [Deployment](/guides/deploy-for-an-org) for Server configuration and
[Architecture](/architecture) for component boundaries.
