# Activity

Activity shows **memory use inside local agent sessions**.
It projects provider-specific logs into one small hierarchy:

> agent activity → user request → memory search → memory chunks made available to the agent

This is intentionally not a general transcript model or chat-history importer.
Assistant prose, unrelated tools, and ChatGPT `conversations.json` exports are out
of scope.

## What it renders

Three columns, newest session first:

| Column | Shows |
| --- | --- |
| Sidebar | The existing global sidebar, with Activity selected. |
| Content | Agent activity for bound workspaces. Each row shows a **DSH** or **Codex** host badge, title, and time. |
| Detail | The user request, the exact memory query written by the agent, search duration, and the selected memory chunks shown as three-line previews of their original source. |

The host badge is part of session identity: session ids only need to be unique
within a host.

Each memory search offers **Retrieval Process** when a run identity is available.
It opens that run's summary and full candidate table in the workspace, temporarily
hiding the session list while keeping the global sidebar. Native stack navigation
provides the toolbar Back button and returns to the same session and search.
The detail header shows the agent's query without repeating the session title or
user prompt. Its navigation container fills the available detail width so it
does not push content beyond the window when resized. This drill-down belongs to one activation; a request
can contain several searches. The detail also reuses **Report Inaccurate** and
**Review Evidence** to capture and review an evaluation case for that run. The
shared diagnostics view also supports a cross-session retrieval-run list. Click the
Final, BM25, Vector, RRF, or Rerank column header to sort by that stage’s numeric
rank in either direction; candidates without a rank stay last.

## What a memory search means

The user request and memory query are different data:

- **User request** is the human message recorded by the agent harness.
- **Memory query** is the exact text the agent sent to `memory.activate`. The
  daemon trims it but does not perform LLM query rewriting, intent extraction,
  conversation expansion, or hidden filter inference.

The raw query goes through exact id/path/title matching, BM25 full-text search,
semantic vector search, reciprocal-rank fusion, and cross-encoder reranking.
The final assembly removes overlapping chunks and applies relevance, per-file,
fragment-count, and token-budget limits. Activity shows only the chunks made
available to the agent. Model scores and excluded candidates appear in the
Retrieval Process detail when requested, keeping the session's reading flow
focused on requests and selected chunks.

The Result column's info button explains all delivery actions and exclusion
reasons; hovering a result explains that row. `Not Reranked` means no reranking
result was recorded, either because the candidate missed the shortlist or the
run stopped before reranking completed. It does not imply low relevance.

The delivery state is a context delta, not a memory edit:

- `add`: the chunk was sent to the agent because it was not in the previous
  activation state;
- `replace`: a newer version replaced the version the agent already had;
- `reuse`: the unchanged chunk was already available to the agent, so its text
  could be omitted from the wire response.

The UI translates these values to plain-language delivery labels and never
presents `add` as creating or modifying stored memory.

## Session sources

### DSH

DSH sessions live at
`~/.dsh/sessions/<encoded-workspace>/<session>/session.jsonl.zstd`.
The file is an append-only, **multi-frame zstd** archive: DSH writes each JSONL
record as an independent frame. A reader must consume all complete frames; decoding
only the first frame returns the `session` header and makes every task count zero.

The projection consumes:

- `session` and `session/title` for identity, workspace, time, and title;
- `user/message` only when `source.kind == "user"` for human tasks;
- `tool/call` for the current `mcp__clumsies__memory` activation shape,
  `{"op":{"activate":{"query":"...","state":"..."}}}`, plus the legacy
  `mcp__clumsies__activate` / top-level `query` shape;
- `tool/result`, paired to the call by `callId`, for the structured activation
  result or error.

### Codex Desktop/App

Codex rollouts are discovered under `~/.codex/sessions` and
`~/.codex/archived_sessions`. `session_meta.payload.cwd` binds a rollout to a
workspace; active and archived copies are de-duplicated by session id.

The projection consumes structured human `response_item` records, legacy
`user_message` events, and completed MCP calls. A current activation is
recorded atomically as:

```text
event_msg.payload.type = "item_completed"
payload.item = {
  type: "McpToolCall",
  id: "...",
  server: "clumsies",
  tool: "memory",
  arguments: { op: { activate: { query: "...", state?: "..." } } },
  result?: { structuredContent: { run_id?, fragments, ... }, isError? },
  error?: { message: "..." }
}
```

Legacy `mcp_tool_call_end` events and the earlier dedicated tool names
`memory/activate` and `activate`, including JSON-encoded arguments, are
accepted as compatibility input. Other MCP servers and other `memory`
operations are ignored.

## Projection and retrieval-run identity

Each genuine human message starts a task. Subsequent clumsies activations belong
to that task until the next human message. DSH call/result records are paired by
`callId`; a Codex `McpToolCall` completion already contains both sides.

For new logs, `structuredContent.run_id` is the authoritative link to
`retrieval_runs`; selected candidates from that exact run supply stable chunk
identity, heading, result order, and preview text. The query text is display
data, not an identity key. The same lookup supplies optional `total_us` for a
terminal run; it stays absent while the run is running or unavailable. This is
the run's recorded retrieval duration, not model response time or a sum of all
stage timings. Chunk counts include `reuse` and do not mean newly sent content.

Retrieval history stores only a bounded preview in each candidate row, but it
also retains the complete resource body used by that run. Expanding **Show source** on a truncated or empty chunk preview
loads its complete text without rendering Markdown using `run_id + unit_key`
and the candidate's frozen locator. It never reads the current Memory document,
which may have changed since the activity occurred. Description-only units have no body byte
range and therefore fall back honestly to their stored preview. Missing or
cleared run records also keep the tool result's recorded preview; an unavailable
snapshot does not mean no search took place.

Older logs may not contain `run_id`. They remain visible with any fragments or
error embedded in their tool result. The daemon uses `(project_id, query)` only
when it identifies exactly one retrieval run; it never chooses the newest among
duplicates. Ambiguous logs keep `run_id` and run status absent.

## Loading and pagination

Activity restores the last concrete project selected for this server, organization,
and account before the first request. If it is unavailable, it uses the current
Memory project or the first accessible project. **All Projects** is an explicit
choice for the current session, not the launch default. No projects means no
initial Activity request.

`list_recalls` returns summaries only, in pages of 20 (maximum 100). Discovery
reads bounded log headers/previews on the blocking pool, not complete transcripts
or retrieval records. Its opaque cursor continues an immutable ordering without
rescanning directories. Explicit refresh discovers a new snapshot.

`get_recall_session` takes a summary's opaque `session_token` and returns one
page of tasks. Only a selected session is parsed in full; later pages reuse its
raw snapshot and enrich only that page's retrievals. The old 500-task/100-activation
truncation is removed. Complete memory chunks are read only when the user expands a
truncated or empty preview.

The daemon retains eight list snapshots and four selected session bodies in
memory; it does not persist another transcript archive. Evicted handles or a
restarted daemon require **Refresh Activity**. Binding changes invalidate access
to an old snapshot. Corrupt individual log headers are skipped; selected-file
failures appear in the detail region with retry.

First loads use one native progress indicator in the waiting region. Refreshes
retain content and selection; continuation progress stays at the list/task footer.
Old responses cannot replace another project's list or another session's detail.
Apple's [Loading](https://developer.apple.com/design/human-interface-guidelines/loading)
and [Progress indicators](https://developer.apple.com/design/human-interface-guidelines/progress-indicators)
guidance informs this choice; the shared view no longer combines a spinner with
decorative skeleton rows.

## Scope

- Sources are DSH and Codex Desktop/App rollouts for bound workspaces.
- The model is a memory-activity projection, not a reusable normalization of the
  full transcript and not a ChatGPT data-export parser.
- Session browsing does not change Memory, Issues, or session files. Reporting
  an inaccurate run and reviewing evidence update retrieval evaluation records
  and retain the source run for that evaluation.

## Implementation map

| Concern | Path |
| --- | --- |
| Shared projection and DSH reader | `crates/clumsiesd/src/recall.rs` |
| Summary snapshots and task pagination | `crates/clumsiesd/src/recall/paging.rs` |
| Codex rollout reader | `crates/clumsiesd/src/recall/codex.rs` |
| XPC dispatch | `crates/clumsiesd/src/state.rs` (`list_recalls`, `get_recall_session`, `get_recall_fragment`) |
| XPC client and models | `apps/macos/Sources/Services/Daemon/DaemonXPCClient.swift`, `apps/macos/Sources/Libraries/Models/DaemonModels.swift` |
| Sidebar section | `apps/macos/Sources/Libraries/Models/MemoryModels.swift` (`WorkspaceSection.sessions`) |
| Activity UI, host badge, and chunk detail | `apps/macos/Sources/Features/Activity/ActivityView.swift`, `ActivityModel.swift` |
| Workspace wiring | `apps/macos/Sources/Features/Workspace/WorkspaceView.swift` |

## Local UI preview

Start this worktree’s isolated Dev Instance with `just dev-macos`, then run
`python3 dev/seed-activity.py`. Reopen the Dev App and select Activity →
**Activity Preview**. The fixture covers raw headings, tables, code, long source,
reused chunks, missing history, empty/failed searches, and sortable numeric ranks.
`python3 dev/seed-activity.py --check` validates the data without a running instance.
