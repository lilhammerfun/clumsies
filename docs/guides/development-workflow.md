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

### Ready-to-review test instance

Use `just dev-macos-reviews` when handing a Review UI build to someone for testing.
It completes local Server setup, signs in the fake-OIDC owner through the daemon,
skips agent selection, and seeds 12 numbered Reviews. The tester does not need a
setup code, credentials, or terminal commands. Repeating the command signs in
again and preserves the existing playground.

Open **Reviews** in the Dev App. Each Review description explains its expected
behavior. Scenarios include mixed current/conflicting files, automatic merges,
multiple text conflicts, rename conflicts, deletion on either side, discarded
members, ready-to-approve changes, an already-updated Review, a rejected Review,
and two independent additions at the same path. Show **Rejected** or **All** to
find scenario 11. Inspect scenarios before publishing: publishing advances Remote
for this shared test organization, so other Reviews may need updating again.

To test an update becoming stale while its editor is open, run
`python3 dev/seed-review-playground.py --advance-remote`, then apply the old result.
The editor should preserve your input and offer to check the latest version.
The instance's `review-playground.json` records Review IDs and expectations;
it contains no credentials. This fixture command accepts only the current
worktree's loopback Local instance, never a Preview or production Server.

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

### Downloadable previews

Run the **Release** workflow with `distribution=preview` (the manual default)
and matching workflow/source refs. For example, after merging into `main`:

```sh
gh workflow run release.yml --ref main -f distribution=preview -f ref=main
```

CI builds an Apple Silicon App and daemon, ad-hoc signs them, creates and mounts
the DMG, verifies its contents, signing, and architecture, then publishes
a GitHub pre-release tagged `macos-preview-<run-number>` with a DMG, SHA-256
checksum. Downloadable Preview DMGs need no Apple or Sparkle secrets. Intel
previews are not available because the current ONNX Runtime dependency has no
prebuilt `x86_64-apple-darwin` library. They do not change the latest stable release.

The primary update action is **Update** beside the user's account at the bottom
of the sidebar. It shares the updater with Settings → General → Check for Updates.
Sparkle downloads and verifies the update in the App, then **Install and Relaunch**
replaces the App and restarts it. The account menu remains available by clicking
the avatar or name; its old trailing chevron is removed.

To offer Preview releases through this flow, configure the repository Actions
secret `SPARKLE_PRIVATE_KEY` with the key matching the App's `SUPublicEDKey`.
CI signs the existing DMG and publishes `preview-appcast.xml` on the dedicated
`macos-updates` release. Debug/Preview apps use this fixed feed, and verify the
archive signature before extraction. A missing key leaves DMG publishing available
and the existing update feed unchanged; an invalid key fails feed generation.
No informational/“Learn More” update is generated. The signing key is maintained
by developers once, not configured by users.

Release apps use `appcast.xml` on the same dedicated release, so unrelated CLI
releases cannot redirect the update feed. Existing apps still using
`releases/latest/download/appcast.xml` need the new Preview DMG installed once.

The preview uses the Debug runtime contract, as `just install-macos` does,
with the regular `ai.clumsies.desktop` App identity and bundled daemon.
Release runtimes still require Developer ID signing for Agent installation;
an ad-hoc Release build would fail that check. The preview does not create a
Dev Instance. It is not Apple-notarized; first launch may require the user's
Privacy & Security exception, which managed Macs can restrict.

### Distribution signing

Tagged releases build a universal Developer ID-signed app, notarize and staple
it, verify the App and bundled Agent runtime share the expected signing team
and hardened-runtime identity, then create a signed and notarized DMG for
downloads. GitHub Actions publishes `Clumsies-<version>-macos-universal.dmg`,
the Sparkle-signed ZIP update archive, and `appcast.xml`. Sparkle only scans
the ZIP, so the DMG cannot create a duplicate update for the same version.
The workflow can also be dispatched
with `distribution=notarized` from the current default-branch tip to produce a signed candidate. Manual
candidates include both DMG and ZIP, without publishing a GitHub Release or appcast.
Tagged releases also update the fixed `macos-updates/appcast.xml` feed and are
explicitly marked Latest for older clients still using the original feed URL.

The DMG contains `Clumsies.app` and an `Applications` shortcut. Users drag the
App into Applications, eject the disk image, and open the installed App.
The ZIP is also usable for manual installation: unzip it and move the App
to Applications before opening it. Neither format requires build tools.
Developer ID signing and notarization let Gatekeeper validate the download
without the preview's manual exception. Switching to ZIP does not remove
those checks. This distribution path does not use App Store Review.

`just test-macos-package` also creates and mounts a temporary DMG, verifies
the copied App signature and binaries, and checks ZIP-only appcast selection.
These local checks use ad-hoc signing and do not submit to Apple. A local
packaging preview can be created from an existing App with
`sh apps/macos/Scripts/create-dmg.sh /path/to/Clumsies.app /tmp/Clumsies.dmg`;
that command does not sign or notarize a distribution package.

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
