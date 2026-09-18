# Find your way around the codebase

The repository contains the macOS App, the resident daemon, the authority Server, and this documentation site. Start with the [architecture](/architecture) and [end-to-end flow](/flows); then choose a source path based on the question you are trying to answer.

The two Rust workspace members are `crates/server` and `crates/daemon`. Swift owns the native interface. Bun runs the VitePress documentation tooling.

## Directory map

| Path | Responsibility | Read it when you want to… |
| --- | --- | --- |
| `apps/macos/Sources/App/` | Startup, windows, menus and dependency composition | Follow the App lifecycle |
| `apps/macos/Sources/Features/` | Views, state and operations grouped by product feature | Find a complete screen workflow |
| `apps/macos/Sources/Services/` | Shared workspace coordination, persistence operations and platform clients | Trace shared state and I/O |
| `apps/macos/Sources/Libraries/` | Shared values, diff algorithms, diagnostics and UI building blocks | Reuse a specific building block |
| `crates/daemon/src/agent_runtime/` | MCP contract and short-lived agent proxies | Understand the agent-facing tool boundary |
| `crates/daemon/src/state.rs`, `draft.rs` | Local state, durable Draft writes, and synchronization | Understand what “queued” means |
| `crates/daemon/src/commit_sync.rs`, `project_storage.rs` | Commit installation, local generations, and cache locations | Trace published data reaching a Mac |
| `crates/daemon/src/search/` | Effective Memory, chunking, indexing, and retrieval | Understand how relevant fragments are selected |
| `crates/server/src/` | HTTP routing and domain modules | Understand shared data and authorization |
| `crates/server/migrations/` | PostgreSQL schema history | Inspect persistent records and constraints |
| `crates/server/openapi/` | Public and Admin HTTP contracts | Look up request/response schemas |
| `packages/clumsies/` | Host integration assets and the Clumsies plugin | See how hosts launch the bundled runtime |
| `dev/`, `apps/macos/Scripts/` | Local development and build utilities | Run an isolated development environment |
| `docs/`, `docs/zh/` | English and Chinese documentation | Improve this site |

There is no active `src/client/` standalone client tree. Historical CLI material is [archived](/guides/cli-commands).

## Trace one operation instead of reading every file

For the deployment rollback checklist, these are useful short routes:

| Question | Source route |
| --- | --- |
| What happens when a user edits and requests Review? | `Features/Workspace/WorkspaceView.swift` → `Services/Workspace/WorkspaceStore.swift` → `Services/Daemon/DaemonXPCClient.swift` |
| What does an agent's `memory.store` do? | `agent_runtime/mcp_contract.rs` → `agent_runtime/mod.rs` → `state.rs::store_draft_operation` → local Draft queue |
| What validates and publishes a Review? | Server `http.rs` → `changes/http.rs` → `changes/service.rs` → `changes/postgres.rs` |
| How does publication reach selected Projects? | Server `memory/postgres.rs` → daemon `commit_sync.rs` → `search/` |
| How is repository context resolved? | Daemon `main.rs` → Project-binding XPC methods → daemon state |

Paths in the first row are relative to `apps/macos/Sources/`; the other daemon paths are relative to `crates/daemon/src/`.

The Server pattern is deliberate: HTTP handlers decode requests and enforce access, service methods coordinate domain work, and PostgreSQL code performs state transitions and transactions. Check all three when changing a public operation.

## Read the tests alongside the implementation

| Behavior | Where to find executable examples |
| --- | --- |
| Multi-file Review order and atomic publication | `crates/server/tests/draft_operation_ordering.rs` |
| Draft upload, merge, projection updates, and two-daemon convergence | `crates/daemon/tests/server_integration.rs` |
| Local persistence and process restart | `crates/daemon/tests/daemon_lifecycle.rs` |
| Agent proxy and real XPC boundary | `crates/daemon/tests/agent_runtime_xpc_e2e.rs` |
| Desktop daemon contract and state mapping | `apps/macos/Tests/Services/DaemonContractTests.swift` |

A test tells you what behavior the implementation promises. It does not establish a production latency target; use the [performance documentation](/performance/) for measurement and scope.

## Open the main entry points

[Desktop workspace](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/apps/macos/Sources/Domain/WorkspaceStore.swift) · [MCP contract](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/agent_runtime/mcp_contract.rs) · [daemon state](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/daemon/src/state.rs) · [Server router](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/http.rs) · [Review transactions](https://github.com/lilhammerfun/clumsies/blob/5d038ffb0ad6e170680618a8fcd0e1ff3d760f77/crates/server/src/changes/postgres.rs)

To run and change the project, continue to [Development workflow](/guides/development-workflow). For interface semantics before implementation, use the [domain API map](/reference/domain-api).
