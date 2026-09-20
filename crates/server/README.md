# Server code organization

The backend is one Rust crate. Business resources live under `src/app/`; HTTP
paths do not determine the directory hierarchy. Organization membership belongs
to `organization`, project membership belongs to `project`, and a memory's
organization or project scope does not create another copy of the resource.

```text
src/
├── main.rs                  # CLI dispatch
├── bootstrap.rs             # Configuration, connections, listener and shutdown
├── config.rs                # Validated process configuration
├── state.rs                 # Dependencies shared by handlers
├── routes.rs                # Resource router assembly and authorization groups
├── http.rs                  # HTTP preconditions and error responses
├── middleware.rs            # Authentication and security headers
├── telemetry.rs             # Request IDs and tracing
├── dto.rs                   # The shared deletion response
├── pagination.rs            # Shared page parameters and page metadata
├── identity.rs              # Random IDs and secret hashing
├── error.rs                 # Errors shared across resource operations
├── app/
│   ├── mod.rs               # build_app: the production Router constructor
│   ├── auth/                # Login, current user and sessions; oidc.rs is its adapter
│   ├── installation/        # First-run setup
│   ├── organization/        # Organization settings and members
│   ├── project/             # Projects and project members
│   ├── memory/              # Memory content and organization selections
│   ├── bundle/              # Personal memory bundles
│   ├── commit/              # Snapshots, history and references
│   ├── draft/               # Drafts, operation batches and reconciliation
│   ├── review/              # Review, decisions and publication
│   ├── inbox/               # Personal notifications and versioned receipts
│   ├── token/               # Administrator token management
│   ├── audit_event/         # Administrator audit feed
│   └── health/              # Dependency status
├── infra/
│   └── database.rs          # PostgreSQL pool construction and SQL migrations
└── maintenance/
    └── project_authority.rs # Explicit legacy-data maintenance command
```

Within a resource, add a file only when that responsibility exists:

| File | Responsibility |
| --- | --- |
| `mod.rs` | Declare the resource and its deliberately exposed interface. |
| `routes.rs` | Register paths and methods in the public, authenticated or organization-administrator group. |
| `handler.rs` | Extract HTTP input, obtain the principal, check preconditions and construct responses. |
| `dto.rs` | Request bodies, query parameters and response shapes. A shared shape has one definition. |
| `service.rs` | Application operations, authorization checks and transaction ownership. |
| `repository.rs` | Private SQL queries, row decoding and database locking operations. |
| `model.rs` | Internal state, pure validation and reconciliation calculations. |
| `error.rs` | Resource-specific errors where an independent error boundary exists. |

Handlers call resource operations rather than a shared `ServerRepository`.
Services take concrete dependencies such as `&PgPool`; the stateful authentication
and installation services retain their provider/configuration state. Database
access does not need a repository trait for its single implementation.

Resource operations enforce their authorization policy before changing state.
Handlers do not perform a separate permission check that another caller could
omit. A direct service regression test verifies both rejected writes and the
absence of audit side effects, then verifies project-local administrator access.
`service` and `repository` modules are private; `mod.rs` exposes the operations
needed by callers, with transaction participants restricted to the crate.

Cross-resource publication and reconciliation must use the caller's transaction.
The resource interface explicitly exposes the required transaction participants;
simple typed persistence operations can be re-exported without a forwarding
function. SQL row types and row-decoding helpers stay in repositories. Only the
outer operation begins and commits the
transaction. In particular, `CommitOutcome` preserves the existing behavior that
persists a reconciliation candidate before reporting a conflict to the client.

Route groups preserve authorization semantics. `/api/v1/admin/projects/{id}` is
authenticated and checked against project permissions in the resource operation;
the `/admin` prefix alone does not imply an organization-administrator gate.

The OIDC HTTP client belongs to `auth/oidc.rs` because authentication is its only
consumer. PostgreSQL connection and migration code lives in `infra/database.rs`.
`config.rs` parses deployment credentials and redirect settings; `bootstrap.rs`
performs provider discovery and supplies the configured dependencies. Service
constructors receive explicit values and do not read environment variables.
The legacy authority migration remains an explicit, plan-hash-guarded CLI command;
it does not run during normal startup or an HTTP request. There are no background
business workers to configure in the current server.

## Verification

`build_app(pool, auth, installation)` builds the production Router without reading
environment variables, connecting to a database, binding a port or spawning tasks.
Both server API tests and daemon/server TCP scenarios use this constructor.

Server API and SQL scenarios use real PostgreSQL containers. Each scenario owns
its pool and container, discovers the Docker host and mapped port, and explicitly
closes/removes them with `TestPostgres::shutdown`. Container drop remains the
fallback if a scenario panics. The Router itself does not own a background task.
Daemon/server TCP scenarios use a shared `TestServer` that owns the listener task,
pool, and database container; its shutdown signals graceful draining and awaits
the task before removing the database. The OIDC adapter has separate HTTP protocol tests; API scenarios use the existing
narrow identity-provider substitute. Test response bodies have a finite limit.

```sh
cargo fmt --all --check
cargo clippy -p server --all-targets -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc -p server --no-deps --document-private-items
cargo test -p server
cargo test -p daemon --test server_integration
cargo test --workspace
```

Docker is required for database scenarios. A missing Docker service is a test
failure, not a skipped successful run. Route registration still supplies the
operation metadata used to compare all registered paths/methods against the
public and administrator OpenAPI contracts.

The selection/review concurrency scenario waits until PostgreSQL reports the
operation blocked on the held advisory lock, then releases the transaction. It
uses a bounded deadline rather than treating a fixed sleep as proof of contention.

The server denies missing public/private documentation and missing `# Errors`
sections, including private functions. CI runs all-target Clippy, strict private
API documentation, and the workspace tests, which include server doctests.
