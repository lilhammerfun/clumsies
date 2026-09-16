# Code Review Instructions

> Development workflow is documented in `docs/guides/development-workflow.md`; reviews should follow its worktree rules.

## Review Priorities

**CRITICAL (Block merge)**
- Security: path traversal, unsafe command construction, unbounded input, exposed secrets, or unnecessary session payload persistence
- Data loss: ignored write/transaction errors, incomplete rollback, or unsafe cross-database publication
- Runtime authority drift: duplicated MCP validation, a proxy that opens daemon state directly, or an Adapter fallback to a retired executable

**IMPORTANT (Requires discussion)**
- Missing test coverage for new behavior and failure paths
- Lock ordering, cancellation, blocking work, and transaction ownership are unclear
- Cross-platform correctness (HOME vs USERPROFILE, path separators, build target vs host OS)

**SUGGESTION (Non-blocking)**
- Naming clarity, code simplification

## Active runtime conventions

- `cargo fmt --all --check`, `cargo test --workspace`, and daemon clippy gates must pass.
- The resident daemon is the only owner of SQLite, model state, and background workers.
- `clumsiesd mcp serve` is a stdio proxy; it uses typed XPC and must not construct `DaemonState`.
- MCP schemas, argument validation, and domain conversion live in Rust. Do not add an untyped JSON tunnel.
- Proxy stdout is protocol-only. Diagnostics go to bounded, privacy-safe logging.
- Adapter entries must point to the signed bundled `clumsiesd`; no PATH, worktree build, or archived CLI fallback is allowed.

## Retired Zig history

The former Zig CLI source is intentionally absent from the active tree. Its
last active snapshot remains recoverable from Git commit
`4b18f7947a977dbc6b62f560b698dc992597f19d`. Do not import or reuse that code
as an active compatibility layer without an explicit product migration.

## Compatibility
- No backward compat layers or deprecated wrappers; modify interfaces directly; delete unused code

## Testing
- E2E tests must isolate HOME to a temp directory with cleanup trap
- Tests must not mutate the installed App, daemon, LaunchAgent, Adapter config, or user database unless the test explicitly owns and restores that environment
