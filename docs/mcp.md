# MCP: the Memory tool

MCP is the interface a Coding Agent uses to work with this Project's Memory. You do not need Server URLs, access tokens or a `project_id` argument in a tool call: the managed integration resolves the current working directory to a Project, and daemon owns the local data and authenticated synchronization.

The normal sequence is **activate → load when needed → store only when asked to maintain Memory**. `activate` helps discover relevant context; `load` gives you complete, exact resources; `store` creates a proposal. See [Domain interfaces](/reference/domain-api) for how this differs from Desktop XPC and Server HTTP.

Clumsies exposes one agent-facing tool:

| Tool | Purpose |
|---|---|
| `memory` | Read the bound Project's Effective Memory or persist Project-carried proposal Drafts (`store`). |

| Operation | Use it when | Result |
| --- | --- | --- |
| `activate` | Starting a substantive task, without knowing which resources matter | Ranked fragments with resource IDs and paths |
| `load` | You need a complete resource, or will edit one | Full resource, stable ID and complete-resource hash |
| `store` | The user explicitly asks to create/update/rename/delete/discard managed Memory | Durable local Draft operation and synchronization status |

The App-bundled Rust `clumsiesd mcp serve` process is a protocol proxy.
Effective Memory construction, indexing, retrieval, exact loading, Draft
persistence belong to the resident `clumsiesd` and
are reached over local XPC. The proxy exposes only the typed `memory` tool; it
cannot pass arbitrary JSON through to daemon methods.

The proxy verifies the resident's Agent runtime protocol revision and build
identity before resolving the current directory's Project binding, and the
resident revalidates that marker on every Agent-scoped dispatch. A missing or
stale resident or proxy fails explicitly instead of mixing releases.

There is no setup call. The removed `retrieve` tool, host-session binding,
`META_PROMPT.md` bootstrap, and MCP attestation path have no compatibility
dispatch. Runtime guidance is delivered by `InitializeResult.instructions` and
the tool descriptions.

The examples below show the tool's **arguments** or the returned domain object, not the outer JSON-RPC envelope. MCP returns a successful domain object in `structuredContent` and as serialized text in `content`, with `isError: false`. Tool failures use `isError: true`; malformed protocol messages can instead fail at the JSON-RPC layer.

## Common input rules

- `op` must contain exactly one of `activate`, `load` or `store`.
- Omit optional fields instead of sending `null`; unknown fields are rejected.
- Field names are case-sensitive. In particular, `knownHashes` is camelCase, while `expected_hash` is snake_case.
- Project binding belongs to the integration. Calls cannot override it with a `project_id` argument.

## Memory

`memory` unifies all memory operations under a single tool with an `op` tagged enum: `activate`, `load`, and `store`.

### Memory Guidelines (`CLUMSIES.md`)

Each Project may define a managed Memory guidelines document, conventionally at the Memory path `CLUMSIES.md`. This is an exact path inside Effective Memory, not an instruction to open an arbitrary file from the operating system. The document establishes:
1. **Taxonomy & Organization**: Standard directory layouts (e.g. `architecture/*`, `decisions/*`, `guides/*`).
2. **Update Rules & Mutation Policy**: When the agent should propose drafts, what descriptions to write, and what not to persist.
3. **Deprecation Policy**: How conflicting or obsolete memories should be superseded.

Before maintaining memory, agents read the complete guide via `memory` with `op: { load: { ids: ["CLUMSIES.md"] } }`, using the exact configured path from MCP guidance when different. A current guide already in context for the same task can be reused. Ordinary read-only tasks do not require loading it in full. User customizations in the guide govern the applicable maintenance conventions; the App's bundled template is only a starting point and grants no additional write authorization.

If the guide returns `memory_resource_not_found`, report the missing path. This establishes absence only in the current Project view, not throughout the Organization. Continue retrieval and fully specified, authorized edits using the user's instructions and existing conventions; clarify decisions that depend on the missing guide. Do not automatically create a guide, substitute a local file, or fall back from a missing custom path.

Users who want to set up guidelines can use **Set Up Guidelines** or **Use Team Guidelines** in the App's empty Project Memory view. The latter selects existing Organization Memory and requires Project administrator access. Adoption is optional; the bundled template is not a Memory resource until adopted. See [Memory Guidelines](/guides/memory-guidelines) for the concept, preview, and research sources.

### Activate

Clumsies complements host-native memory. Follow applicable host memory policies and also query Clumsies for substantive project tasks, even when host memory has already been consulted. A read or write in one store does not fulfill a read or write in the other. Clumsies maintenance follows the bound Project's Memory Guidelines. Respect explicit user requests to skip Clumsies or use only another source or destination.

Call `memory` with `op: { activate: ... }` once at the beginning of each substantive task:

```json
{
  "op": {
    "activate": {
      "query": "adjust the MCP hybrid retrieval interface",
      "state": "optional-opaque-state"
    }
  }
}
```

| Field | Required | Meaning |
|---|---:|---|
| `query` | yes | A non-empty natural-language representation of the current task or cue. |
| `state` | no | The preceding `next_state`, but only while its earlier fragments remain in the model context. |

The daemon performs BM25 and dense-vector recall, RRF fusion, Cross-Encoder
reranking, resource diversity limits, token budgeting, and fragment delta
calculation in one operation. `kind`, `group`, `limit`, model names, and ranking
parameters are deliberately not agent-facing inputs.

Retrieval parameters belong to daemon; callers describe the task rather than tuning a search engine. [Retrieval evaluation](/retrieval-evaluation) explains the ranking and diagnostic details.

The response contains:

```json
{
  "index_revision": "search_...",
  "profile": "agent_activation.v2",
  "next_state": "opaque-state",
  "fragments": [
    {
      "action": "add",
      "unit_key": "mem_123/memory-delta/0/0",
      "content_hash": "sha256:...",
      "resource_id": "mem_123",
      "scope": "org",
      "kind": "memory",
      "path": "architecture/retrieval.md",
      "heading_path": ["MCP", "Memory Delta"],
      "content": "..."
    }
  ],
  "removed": []
}
```

The response may also contain a local diagnostic `run_id`. `add` and `replace` include content. `reuse` identifies content already present
in the caller's context and omits it. `removed` invalidates units that have been
deleted, lost permission, or disappeared after reparsing. A unit that is merely
irrelevant to the current query is not removed.

Omit `state` after context compaction, after old tool output is dropped, or
when starting fresh. Invalid or unsupported state returns
`invalid_activation_state`; it is never silently treated as an empty state.

The daemon prepares its pinned local models in the background. Until they are
ready, activation returns `search_model_preparing` with current and total byte
counts instead of holding the MCP request open. Model preparation retries in
the background and has no lexical-only fallback.

### Load

Use `memory` with `op: { load: ... }` for complete resources already identified by ID or exact path (including project guidelines like `CLUMSIES.md`):

```json
{
  "op": {
    "load": {
      "ids": ["mem_123", "CLUMSIES.md", "architecture/retrieval.md"],
      "knownHashes": {
        "mem_123": "sha256:..."
      }
    }
  }
}
```

`ids` is required, non-empty, unique, and contains strings. `knownHashes` is
optional. When a known hash matches the current complete resource,
`changed=false` and content is omitted. A missing requested resource returns
`memory_resource_not_found` instead of being silently dropped.

`load` reads the same Effective Memory as `activate`, including current local
Draft overlays. It does not perform fuzzy search, embedding, or reranking.

A response without a matching known hash looks like this; the ID and hash are illustrative:

```json
{
  "resources": [
    {
      "resource_id": "mem_123",
      "scope": "org",
      "kind": "memory",
      "path": "architecture/retrieval.md",
      "title": "Retrieval",
      "description": "How the project retrieves Memory",
      "content_hash": "sha256:example",
      "changed": true,
      "content": "# Retrieval\n\nLoad the complete resource before editing."
    }
  ]
}
```

Use the returned `resource_id` for an update and the returned `content_hash` for `expected_hash`. An activation fragment hash identifies a fragment; it is not the complete-resource hash required by `store.update`.

### Store

Call `memory` with `op: { store: ... }` only when the user explicitly asks to create, update, rename,
delete, or discard managed memory.

MCP creates Project-owned Drafts in the Project bound to the current directory.
Updating or renaming a selected Org reference creates an explicit Project adaptation
with a separate identity and a fixed source version. Deleting an Org reference
requires removing its selection in the App or making an explicit Org proposal.
Existing Org Drafts keep their original publication target. Saving changes only the
Project's local Effective Memory; publication requires a Review. MCP exposes no
Review decision or merge operation. A Project PR and an optional Org contribution
are independent Reviews with separate permissions and outcomes.

Operations:

| Operation | Required fields | Optional fields |
|---|---|---|
| `create` | `path`, `body` | `description` |
| `update` | `id`, `expected_hash`, `replacements` | `description` |
| `rename` | `id`, `new_path` | `description` |
| `delete` | `id` | `description` |
| `discard` | `id` | none |

`resource` is an optional field on `store`, alongside `create` or `update`, not inside that operation. Its only allowed value is `memory`, which is also the default. IDs may be `mem_`-prefixed or legacy `ctx_` / `rul_` / `wfl_` values; legacy
IDs stay stable and opaque and are never rewritten.

`delete` removes the addressed item from Local Effective Memory. When the item
is an unpublished Create Draft, daemon normalizes the operation to `discard`;
only deletion of an authoritative resource remains an open deletion Draft that
can be submitted for Review.

Example Create:

```json
{
  "op": {
    "store": {
      "create": {
        "path": "release/RELEASE.md",
        "description": "Release procedure for the project",
        "body": "# Release\n\nRun verification before publishing."
      }
    }
  }
}
```

Before updating a resource, call `load` and pass its complete-resource
`content_hash` as `expected_hash`. An update contains one or more exact text
replacements:

```json
{
  "op": {
    "store": {
      "update": {
        "id": "mem_123",
        "expected_hash": "sha256:...",
        "replacements": [
          {
            "old_text": "The original exact text.",
            "new_text": "The replacement text."
          }
        ]
      }
    }
  }
}
```

Every `old_text` must occur exactly once in the current Effective Memory
resource. Replacements in one update must not overlap and are applied
atomically against the same original content. A stale hash, missing match,
ambiguous match, or overlap rejects the complete update without creating a
Draft operation. `new_text` may be empty to delete text.

Paths address the managed Memory namespace, not arbitrary local filesystem files. The create
`body` is the complete resource content; updates never accept a complete body
from the agent — daemon materializes the verified replacements into the
complete Draft result. Memory bodies are Markdown; whether a resource reads as
a rule, workflow, or context is expressed by its content and path, not by a
wire type. `description` is an optional semantic summary and retrieval field. Its publication currently has a Server persistence gap; see [Data model implementation notes](/data-model). Metaprompt and `mpf` are not valid wire values.

A successful result contains the local operation ID, Draft ID, queue status,
and sync status. It means the operation is durably stored locally and queued
for automatic synchronization. It does not mean a Review was merged or an
authority Ref moved. Ordinary Project members may propose and submit changes,
Project owners/administrators decide and merge Project Reviews. Org Reviews
require Org owner/administrator authority.

For example, a queued local write can return:

```json
{
  "local_operation_id": "lop_example",
  "draft_id": "drf_example",
  "queued": true,
  "sync_status": "queued"
}
```

`sync_status` can be `queued`, `syncing`, `retrying`, `synced` or `failed`. Even `synced` means that a Draft reached Server, not that it was approved or published.

## Errors and recovery

| Error or condition | What to do |
| --- | --- |
| `search_model_preparing` | Wait for background model preparation; the response reports progress |
| `invalid_activation_state` | Discard stale state and activate with a fresh context; never claim missing fragments are still available |
| `memory_resource_not_found` | Check the exact ID/path and current Project; a load does not silently drop missing targets |
| `memory_content_changed` | Load the complete resource again and reassess the intended edit |
| `text_replacement_not_found` / `text_replacement_ambiguous` / `text_replacement_overlap` | Correct exact replacement spans using the freshly loaded content; the update is rejected atomically |
| `agent_runtime_mismatch` | Align the managed proxy and resident daemon builds; restarting a stale host integration may be necessary |

A failed update does not permit a fallback full-body overwrite. A failed activation does not mean that no relevant Memory exists. Treat those as failed operations, then recover using the explicit error.

## Daemon operations

| XPC method | Consumer |
|---|---|
| `activate_memory` | MCP `activate` |
| `load_memory` | MCP `load` |
| `store_draft_operation` | MCP `store`, Desktop, and other clients |
| `search_index_status` | Desktop diagnostics and tests |
| `rebuild_search_index` | Recovery, tests, and development tooling |

Every valid `activate_memory` call also writes one local Retrieval Run from the
same ranked candidate trace used for the response. This does not add fields to
the MCP `activate` schema. Retrieval history, Evaluation Cases, and B1–B4
exports are daemon/Desktop diagnostic APIs described in
`docs/retrieval-evaluation.md`; they are not additional MCP tools and are never
sent to Server.

The default retrieval profile has no silent BM25-only, old substring-search,
or fallback to an incompatible index. While a compatible replacement index is
building, the previous ready generation remains queryable; the scheduler
atomically publishes the new generation when it is complete. Model, vector,
generation, and state failures remain
explicit protocol errors.

## Implementation references

The executable input contract and advertised tool schema live in [mcp_contract.rs](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/agent_runtime/mcp_contract.rs). [mcp.rs](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/agent_runtime/mcp.rs) handles stdio/JSON-RPC and result wrapping; [search response types](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/search/mod.rs) and [daemon operation types](https://github.com/lilhammerfun/clumsies/blob/main/crates/clumsiesd/src/types.rs) define the returned data.
