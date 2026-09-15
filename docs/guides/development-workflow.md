# Development Workflow: Worktrees

Clumsies development uses Git worktrees so concurrent changes do not share a
working tree, build output, or development runtime. A session writes to only one worktree at a time and reuses it for follow-up work. Before switching, finish or explicitly hand off existing changes and stop writing to the old directory. A new task does not automatically require a new worktree.

## Core loop

1. Check existing work and reuse its worktree when appropriate. When a new one is needed, create it from the main checkout and an appropriate baseline:

   ```sh
   git worktree add .worktree/<name> -b codex/<name> main
   ```

2. Develop and verify inside that worktree.
3. Open and merge one focused PR.
4. Stop the worktree's Dev Instance before removing it:

   ```sh
   just dev-macos-reset
   cd /absolute/path/to/main-checkout
   git worktree remove .worktree/<name>
   git branch -d codex/<name>
   ```

## Conventions

| Thing | Convention |
|---|---|
| Worktree | `.worktree/<slug>` |
| Agent branch | `codex/<slug>` |
| Commit subject | `<area>: <summary>`, at most 72 characters |

Keep unrelated changes on separate branches. Do not rewrite a pushed branch
that is under review; create a new branch and cherry-pick the relevant commits.

## Dev Instance isolation

For docs-only changes, build and preview VitePress; no macOS runtime is needed. To run the product, each worktree can use a complete isolated Dev Instance with `just dev-macos`.
The complete instance requires Just, XcodeGen, Xcode, Rust, Bun, and Docker Desktop.
Its canonical path determines the App identity, daemon service, runtime
directories, Keychain service, Compose project, dynamic ports, and isolated
`CODEX_HOME`.

```sh
just dev-macos-status
just dev-macos-logs
just dev-macos-down   # preserve instance data
just dev-macos-reset  # delete instance data and credentials
just dev-macos-preview path/to/preview.json
```

`down` stops only that instance and preserves its data. `reset` removes its
data and test credentials and must run before deleting the worktree. Only
`just install-macos` may replace the stable Debug installation.

## Validation

| Layer | Command |
|---|---|
| daemon | `cargo test -p daemon --lib` and `cargo test -p daemon --test daemon_lifecycle` |
| macOS app | `just test-macos` |
| Dev lifecycle | `just test-dev-macos` |
| public docs | `bun run build` |

## macOS build and packaging

Run these commands from the repository root. `just test-macos` runs the
regular suite without a Server dependency; `just test-macos-live` requires
a running, authenticated Dev Instance and uses only that instance's daemon
and Server.

`just build-macos` creates an unsigned Release build for the current Mac's
architecture under `/private/tmp/clumsies-macos-build`. The build embeds
`clumsiesd` in the app bundle. Set `CLUMSIES_SKIP_DAEMON_BUILD=1` only when
iterating on UI code with an already installed daemon.

To update the long-lived Debug installation, run `just install-macos`.
The App reconciles the global Codex Plugin after launch; restart Codex and
create a new task to use the updated plugin. Debug builds use Xcode's local
ad-hoc signature. The embedded daemon receives an explicit, stable designated
requirement so rebuilding it does not invalidate its file-keychain access
control entry.

Regenerate the Xcode project with:

```sh
xcodegen generate --spec apps/macos/project.yml
```

The generated `Clumsies.xcodeproj` is ignored; only its SwiftPM lockfile is tracked.

### Distribution signing

Tagged releases build a universal Developer ID-signed app, notarize and staple
it, verify the App and bundled Agent runtime share the expected signing team
and hardened-runtime identity, then publish a Sparkle-signed update archive
and `appcast.xml` through GitHub Actions. The workflow can also be dispatched
from the current default-branch tip to produce a signed candidate. Manual
candidates do not publish a GitHub Release or appcast.

Keep the Apple certificate, certificate passphrase, notarization account,
app-specific password, team ID, temporary keychain password, and Sparkle
private key in the protected `macos-signing` GitHub environment, using the
secret names referenced by `release.yml`. Restrict that environment to the
default branch and release tags and require a reviewer before its secrets
are exposed. The Sparkle private key is required only for a tagged release;
only its public key is committed in `project.yml`.

A Debug ad-hoc runtime is accepted at the installation boundary and supports
managed Coding Agent integrations. Release packages must carry an accepted
team identity and the hardened-runtime flag.
