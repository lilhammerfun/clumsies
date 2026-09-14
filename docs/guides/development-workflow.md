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
Its canonical path determines the App identity, daemon service, runtime
directories, Keychain service, Compose project, dynamic ports, and isolated
`CODEX_HOME`.

`down` stops only that instance and preserves its data. `reset` removes its
data and test credentials and must run before deleting the worktree. Only
`just promote-debug-macos` may replace the stable Debug installation.

## Validation

| Layer | Command |
|---|---|
| daemon | `cargo test -p daemon --lib` and `cargo test -p daemon --test daemon_lifecycle` |
| macOS app | `just test-macos` |
| Dev lifecycle | `just test-dev-macos` |
| public docs | `bun run build` |
