# Local runtime

Start with [Architecture](/architecture) and [End-to-end flows](/flows) to understand the overall design. This page covers persistent local state, synchronization, retrieval, and recovery. For symptoms and recovery steps, see [Troubleshooting](/guides/troubleshooting).

## Local daemon

`clumsiesd` is an owner-scoped macOS launchd service. The macOS app installs and
starts it. Desktop connects directly over XPC; Agent hosts start the same
App-bundled executable as a short-lived MCP proxy, which then connects
to the resident process over XPC. The daemon has one central SQLite database for
durable client state and one derived search database inside each Project's
active local storage.

The database currently stores:

- installation identity and schema version
- Server URL and selected project configuration
- canonical local Project bindings from Server authority plus workspace root to `project_id`
- local drafts and ordered operations
- synchronization status, failures, and Server draft identity
- immutable Blob, Tree, and Commit metadata
- installed Organization authority and Project projection Refs
- the active search head and storage-location revision for each Project

Each Project search database stores its derived search revisions, complete
Effective Memory resources, Markdown units, FTS5 rows, and vectors. Embedding
and reranking models remain in the shared daemon cache and are not copied per
Project.

File permissions are owner-only. Access and refresh tokens are stored as one
Server-bound generic-password item in macOS Keychain. SQLite never persists
either token, and daemon has no plaintext credential fallback.

## Retrieval model preparation

The daemon prepares models in the background before the first MCP request. It uses pinned int8 versions of `intfloat/multilingual-e5-small` for embedding and `Xenova/bge-reranker-base` for reranking. The current download is 431,831,479 bytes; the revisions, artifact sizes, and SHA-256 checksums are defined in the [model manifest](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/search/models.rs).

Downloads support resuming. Artifacts are verified before ONNX loading and cached for offline reuse. Search status reports `preparing` with downloaded and total bytes. Activation returns `search_model_preparing` immediately until preparation finishes; it does not hold an MCP request open for an unreported download or silently use a weaker retrieval path.

## Desktop request path

The native Swift client serializes typed capability requests over XPC. Daemon
executes local operations or sends authenticated HTTP requests to Server.

```text
SwiftUI/AppKit -> XPC -> daemon -> HTTPS -> Server
```

This keeps credentials in daemon and macOS Keychain without a WebView or CORS
dependency.

## Client diagnostics

The App writes JSON lines to `client.log` in its configured daemon log directory
(`~/Library/Logs/ai.clumsies` for stable installations). This works when launched
from Finder as well as from development scripts. The resident daemon writes
structured events to `daemon.log`; it no longer duplicates every event to
launchd stderr. Each primary log and `clumsiesd.crash.log` retains the active
file and three archives, each capped at 4 MiB. Oversized pre-upgrade files are
trimmed to their tails. Management commands do not share the resident writer.
File-sink failures are reported to OSLog (App) or stderr (daemon); stderr also
retains bootstrap errors that occur before daemon logging starts.

Desktop generates a `request_id` for XPC calls. Daemon keeps it in error
envelopes and sends it as `x-clumsies-request-id` and `x-request-id` on HTTP
requests. Server ingress logs retain both the client ID and the Server's own
ID, since a proxy can replace `x-request-id`. HTTP logs distinguish request
start, received headers, body/decode failure and completion. A completed XPC
call can still carry an unsuccessful HTTP status. An XPC timeout does not
cancel a mutation already dispatched to Server; inspect the matching IDs
before retrying. Missing completion logs alone do not prove a request never
arrived. Older clients may omit the ID and older daemons may omit error
`details`; the envelope remains readable.

Request diagnostics record method, API resource family (not nested Memory
paths or query strings), elapsed time, status, byte/draft counts and safe
transport causes/OS codes. They do not record bodies, authorization headers,
cookies or raw error source strings. Native login/setup/recovery requests and
response decoding use the same App failure boundary. Sync/reaper workers log
the first failure and power-of-two repeats of the same safe error signature,
plus recovery; a changed error signature is logged immediately; stale HTTP cache
fallbacks are explicit. Individual HTTP attempts remain visible.

**Settings → Support → Diagnostics → Export**, the menu-bar **Export
Diagnostics…** item, and the startup error page create a local diagnostic
folder. Its manifest identifies the instance, collection time, App executable
hash, App/daemon versions and missing or unreadable files. Only named
App/daemon, crash and bootstrap/development log files are included, with at
most 4 MiB per file; databases, Memory and Keychain data are not collected.
Existing crash/legacy records are copied as logs, not re-sanitized. Nothing is
uploaded automatically. Review the manifest if a file sink was unavailable.

Regression evidence lives in `crates/daemon/tests/client_diagnostics.rs`,
daemon diagnostics/XPC unit tests, Server telemetry tests, and macOS
`ClientDiagnosticsTests` / `NativeServerBootstrapTests`. Changes to request or
logging boundaries must verify failure evidence, correlation, redaction and
retention, in addition to successful requests. Run `cargo test -p daemon
--test client_diagnostics`, `cargo test -p daemon --lib --bins`, `cargo test -p
server telemetry::tests --lib`, and `just test-macos`. All are covered by the
existing Rust/macOS CI jobs. The timeout probe uses 118 fake Drafts, a loopback
server, temporary storage and an in-memory credential store.

## Draft synchronization

Every local operation is persisted before synchronization is attempted. The
queue supports create, update, rename, delete, and discard for Memory
resources in the unified model.

Deleting an authoritative resource keeps an open deletion Draft until Review
merge. Deleting a resource created only by the current Draft cancels that
creation instead: daemon records a discard, and the Draft leaves Effective
Memory without creating a deletion proposal for a resource that never existed
in the Ref.

Each draft carries:

- `project_id`
- authority `scope` (`org` for every new Draft; `project` is accepted only
  while discarding historical local rows)
- the unified Memory identity (id or path; no three-type kind)
- `base_commit_id`
- the currently installed target Ref Commit
- derived freshness and Server reconciliation projection
- local draft ID and optional Server draft ID
- ordered operation history

The sync worker starts automatically, wakes when a new operation arrives or
configuration changes, and retries failed work. A local draft is reused across
successive edits, so repeated writes do not create one Server draft per
keystroke.

## MCP write path

The adapter-managed MCP entrypoint is `clumsiesd mcp serve`. This process owns
only bounded JSON-RPC framing, the typed `memory` contract, Project binding,
and XPC forwarding. Before accepting Agent traffic it
verifies that its Agent runtime protocol revision and build identity match the
resident daemon. It does not initialize `DaemonState`, open SQLite, load models,
or start background workers.

MCP keeps the public `memory` (`op.store`) tool shape. Internally it adds the
current bound Project as the Draft carrier and marks Organization authority as
the proposal target. These are separate axes: the Project owns the pre-merge
overlay; `org` describes the Ref an approved Review may eventually move. At
process startup, MCP gives its current working directory to daemon; daemon
canonicalizes the path and resolves the nearest bound ancestor in SQLite. A
Codex host-plugin proxy repeats that resolution before every `tools/call` and
requires it to remain the same Project; the global Plugin does not require a
project Adapter row. MCP never treats a legacy Workspace ID as a Project ID.
The Rust MCP contract tests exercise the exact Agent-facing
envelopes before they are mapped to typed daemon requests.

Agent-originated updates are exact text replacements, not complete-document
writes. MCP forwards the resource ID, the complete-resource hash returned by
`load`, and one or more `old_text`/`new_text` pairs. While draft and commit sync
are excluded, daemon resolves the current Effective Memory resource, verifies
the hash and unique non-overlapping matches, and applies the complete batch
atomically. Only the materialized complete result is persisted as the ordinary
Draft update operation, so Server synchronization and Commit storage remain
independent of the agent-facing editing protocol.

An old `~/.clumsies/config.toml` entry is used only when no daemon binding
exists. MCP matches its display name against the signed-in user's Server
Projects, persists the unique canonical `project_id` in daemon, and removes the
migrated path from the old file. Missing or duplicate matches fail explicitly;
the legacy `ws_id` value is never sent to daemon.

When the caller omits `base_commit_id`, daemon reads it from the installed
Organization Ref before creating the local Draft. A missing Ref produces a
Draft with no base; daemon never invents a Commit ID.

MCP does not expose a caller-selectable scope, Review decision, merge, or
publish operation. `store` can only create a Project-carried proposal. Before
merge it affects that Project's Effective Memory overlay; Organization
authority changes only after an Org administrator approves and merges the
Review. Organization is an authority namespace, not a synthetic Project.

The daemon combines the installed authority generation with current
`open`/`submitted` Draft operations before both `activate` and `load`. For a
resource with a Draft, it restores that resource from the Draft Base Commit,
applies the ordered operations, and overlays the complete Draft result on the
latest installed authority. All other resources come from the latest Commit.
This applies equally to create, update, rename, and delete. A successful `store`
therefore changes the next Effective Memory hash and causes the next activation
to build or select a matching search revision.

## Synchronization and reconciliation

Draft sync and Commit sync are independent. Commit sync may update the local Ref,
`current_commit_id`, freshness, and candidate validity, but it never changes a
Draft Base, operations, content, or lifecycle. A behind Draft remains editable,
syncable, restart-safe, and visible to MCP.

Server is the canonical reconciliation executor. A candidate binds Draft ID,
Draft version, Base Commit, and current Commit, and is either `clean` or
`conflicts`. Merely creating or viewing it does not mutate the Draft. Explicit
rebase stores the previous Draft revision and rewrites the Draft as
`base = Current` plus `operations = diff(Current, confirmed result)`. Any Draft
edit or Ref advance invalidates the old candidate.

## Commit synchronization

The daemon synchronizes both the Organization authority Ref and each Project
projection Ref on its background interval and through explicit retry. Its
target set is the union of durable directory bindings, active Draft Projects,
and the Project currently selected by Desktop. Desktop selection is UI state
and cannot redirect an MCP process in another directory.

```text
Server commit-state + ETag
  -> validate Ref identity
  -> download Commit payload
  -> verify Blob addresses and Tree ownership
  -> build an immutable project generation
  -> move the local SQLite Ref
  -> daemon combines that generation with local Drafts
  -> MCP asks daemon to activate fragments or load complete resources
```

Before moving a local Ref, every sync also checks active `open` and `submitted`
Drafts and fetches any missing Base Commit, Tree, and Blob payloads referenced by
their `base_commit_id`. This preserves an old-Base overlay after a cache rebuild
without pinning the rest of the project to that Base.

The generation is built under a temporary directory and renamed before the Ref
transaction commits. A failed download, invalid payload, or incomplete
generation leaves the previous Ref and MCP-visible files unchanged. The
`commit_sync.server_cursor` is the installed project Commit ID, not a fabricated
timestamp or independent revision.

Server currently publishes full Commit payloads, so incremental object transfer
is not implemented. Cached immutable objects are retained for restart and
integrity checks. Active Draft Base references are retention roots and cannot be
garbage-collected.

Cache diagnostics preserve layer boundaries: an unknown local Ref reports
`project_ref_not_synced`; an absent or invalid materialized generation reports
`commit_generation_missing` or `commit_generation_corrupt`; only derived search
index preparation and build failures use search-index error codes.

## Project local storage

Project Local Storage is an installation-local cache setting keyed by normalized
Server authority and canonical `project_id`. It is not part of Server Project
metadata and is not synchronized to another installation. An absent setting
resolves to
`<daemon-cache>/projects/<authority-hash>/<project-id>`. A preexisting
`projects/<project-id>` generation is adopted once into that authority-scoped
layout with its permissions hardened; daemon does not maintain both layouts.

For a custom location, the selected directory is only a parent. Daemon owns the
following subtree and never treats it as an editable working directory:

```text
<selected-root>/.clumsies/cache-v1/<authority-hash>/<project-id>/
  ownership.json
  generations/
  search/index.sqlite
  staging/
```

Changing the location creates a persistent daemon move. Daemon materializes and
verifies the generations and Project search index under destination staging,
then switches the location with `expected_location_revision` CAS while holding
the same synchronization boundary used by Commit installation. Readers retain
the old location until the switch obtains its write gate. A restart resumes any
nonterminal move; cleanup failure after a successful switch is diagnostic and
does not roll back the new active location.

The macOS app uses `NSOpenPanel` to create a one-time ordinary bookmark for the
selected directory. Daemon resolves that handoff while Desktop is running,
creates a security-scoped bookmark under the daemon's own code-signing identity,
and persists only that daemon-owned bookmark. This is required because an
app-scoped bookmark created by Desktop cannot be resolved by a separately signed
LaunchAgent. Daemon holds security-scoped access for each filesystem operation
and refreshes stale persisted bookmark data. It refuses network filesystems,
symbolic links, invalid ownership markers, unsafe nesting, and paths without
capacity or write access. Managed directories and files use `0700` and `0600`.

An unavailable custom location never falls back to the default cache or advances
the local Project Ref without a complete generation. Drafts, operation queues,
and their synchronization continue in central SQLite. `activate`, `load`, and
checkout return an explicit storage/search readiness error until the same
location becomes available again.

The daemon IPC methods are `project_storage`, `replace_project_storage`,
`project_storage_move`, `reset_project_storage`, and `clear_project_cache`.
Clear Cache removes only marker-owned generations, search data, and staging;
Drafts, settings, models, and files outside the managed subtree are preserved.

## Diagnostics

Desktop can read daemon health, bootstrap state, project configuration, sync
status, draft lists, draft details, and operation results through
typed XPC requests. It can request explicit retry without directly mutating queue
rows.

Settings → Support opens the log folder.
Memory Search History lists the active Project's latest Runs and loads one
complete trace on demand. Candidate columns show exact/BM25, vector, RRF, reranker, final rank,
score, exclusion reason, and activation delta action. A successful Run can be
added to the local Evaluation Set, graded from 0–3, supplemented with missed
resource evidence, exported as a versioned fixture, or retained while unpinned
history is cleared.

The local methods are `list_retrieval_runs`, `get_retrieval_run`,
`create_evaluation_case`, `replace_evaluation_judgments`,
`clear_retrieval_runs`, and `export_evaluation_set`. Retrieval history is
central daemon state and remains independent from Project Local Storage.
Retention keeps the latest 500 unpinned Runs per Project; Evaluation Cases pin
their source Runs and immutable corpora. See [Retrieval and evaluation](/retrieval-evaluation).

Server diagnostics are available at `/api/v1/admin/health`. Database, schema,
Commit service, and OIDC are reported as separate components.
