# Clumsies Desktop

The Windows and Linux client. macOS keeps its own native Swift app in `apps/macos`.

- UI toolkit: [GPUI Kit](https://gpui-kit.com) (`gpui-kit`), the same stack Zed is built on.
- Engine: the local `clumsiesd` engine, shared with the macOS client.
- Design rules: [DESIGN.md](DESIGN.md).

## Shipping

A release ships packages from one tag: the macOS DMG (its own workflow),
`Clumsies-<version>-linux-x86_64.tar.gz`, and
`Clumsies-<version>-windows-x86_64-Setup.exe` (recommended), and the portable
`Clumsies-<version>-windows-x86_64.zip`. Desktop packages are built by `release.yml`
on `v*` tags and attached to the same GitHub release, each with a SHA-256 file.

The Windows installer adds Start menu and optional desktop shortcuts, installs
to `%LOCALAPPDATA%\Programs\Clumsies` without elevation, and registers an
uninstaller. Close the client before reinstalling or uninstalling; the installer
stops only its own resident engine and preserves `%LOCALAPPDATA%\ai.clumsies`.
Packaging uses Inno Setup 6 and the Visual Studio x64 CRT redistributables,
included on the Windows CI runner. Runtime DLLs are deployed beside the programs
so users do not need a separate VC++ runtime installation. Windows 10 version
2004 or later is required by the DirectML system dependency. CI exercises
installation, shortcuts, installed daemon IPC, reinstall, the running-client
guard and uninstall with isolated daemon state. `windows-installer.yml` can
attach an installer to an already published stable release using its verified
ZIP, without rebuilding or replacing the released binaries.

Each package contains the two programs — `clumsies-desktop` and the engine it
starts, `clumsiesd` — plus the launcher entry and icons on Linux, a README, and
an `install.sh` that puts them in `~/.local` (or `--prefix /usr/local`). Nothing
else is needed: the client starts the engine itself, from beside its own
executable.

The client reports the product version, so `apps/desktop/Cargo.toml` is bumped
with `MARKETING_VERSION` in the release commit; the packaging job refuses to
build a package whose tag and versions disagree. `--smoke-test` is what the job
runs against the assembled package before uploading it: it starts the bundled
engine and waits for its socket, which is the one thing a package can break that
compiling cannot catch.

## Run

```sh
cargo run -p desktop --release
```

**The client runs in release even while it is being worked on.** It paints,
shapes text and lays out on every frame, and an unoptimized GPUI cannot keep a
60Hz window: on a 1181×1296 window one frame costs 49ms of CPU in the dev
profile against 5ms in release, where the frame budget is 16.7ms. A dev build
scrolls visibly worse than the product does, which is not something to review a
screen through. The daemon and the Server stay in debug — there a rebuild
matters more than a frame.

## Layout

```
src/
├── main.rs         bootstrap: window options, app id, title
├── app.rs          the shell: Project rail beside the active screen
├── engine.rs       the engine seam — the daemon, and the Server through it
├── ui.rs           spacing steps, the Windows type ramp, radii
├── timestamps.rs   the reader's own time zone and calendar
├── components/     diff, Markdown, Memory tree
└── screens/        one module per screen
```

## Current state

The Project rail, the Memory tree, the document editor, the Reviews queue and
the Dashboard all read from the local `clumsiesd`: Projects and Reviews come
through its Server proxy, a Project's Memory comes from its checkout, and its
drafts, retrieval telemetry and session belong to the daemon. Signing in is a
daemon concern; on Linux and Windows `dev/dev-login.py` performs the same
authorization a client does and hands the session to the daemon.

**The Dashboard** is the one screen with two owners: the Server counts the
published inventory and owns the calendar, and the daemon reports the retrieval
it retained on this machine, so the client asks for both and draws them
together. It is the one section with no navigator beside it — macOS's Dashboard
is a sidebar next to a single page — so its six metric cards, its six panels and
the period picker all live on that page, and the Project filter travels in the
page's own header.

A Dev Instance may keep a `fixtures/dashboard.json` beside its daemon root, as
`dev/seed-dashboard.py` writes one: the client then draws that sample instead of
the live statistics and badges the page "Demo data". A month of history takes a
month to accumulate, so this is how the screen is looked at while it is built.

**Signing in** is the macOS page in the state it reached when it gained invited
password accounts: the brand mark over a title, then whatever the Server says it
offers — a local password, an identity provider, both, or neither — with an
invitation and a password reset behind the same fields, and the four first-run
fields when the Server has never been configured. The Server address is folded
away at the bottom, because it is set once and then read. Nothing here is
guessed from the client's side: the Server's own `/api/v1/auth/methods` decides
which controls exist, and the four shapes this client mirrors are pinned by
tests.

**Settings** uses the shared GPUI settings component with Account, General,
Agents and Support pages. It initially opens General, like macOS. General
contains the app version; account credentials and their inline Cancel/Save
actions stay in Account. Password fields clear after each submission, and
closing a dirty account form asks before discarding it. Credential changes
install the fresh session in the daemon before reporting success.

Agents are machine-wide integrations, separate from project work folders.
Codex status reflects plugin inspection, including a missing host or a plugin
needing repair. Support opens the logs folder and exports a diagnostic ZIP
containing build metadata and bounded, allowlisted logs, excluding symlinks.
Organization administration, language selection, automatic updates and macOS
settings navigation history are not yet ported.

**Settings and the account menu** are the macOS client's own two: the identity
at the foot of the rail opens Settings — a dialog here, because this client has
one window — and ends the session from the same menu. Signing out stores what
the panes still hold, tells the Server to revoke the session, leaves the daemon
with a Server address and nothing else, and puts the form back.

What is still missing is screen coverage, not data: Inbox, Bundles and Activity
are not translated, and Reviews is missing comments, resubmission and the
permission checks macOS makes from its own capabilities.

### Linux Memory UI test data

With the local instance running, execute `just seed-memory-ui`. Select
**Memory UI 验收** from the project filter (restart the client to refresh its
project list if needed). The seed creates a separate project with 35 published
documents and three open drafts, without changing existing projects. Repeating
the command preserves edits and deletions made during testing.

- `00-从这里开始.md`: test checklist.
- `01-目录层级`: nested folders; collapse, switch projects, and restart to check state restoration.
- `02-长名称`, `03-同名文件`, `05-特殊名称`: truncation, tooltips, duplicate basenames and Unicode paths.
- `04-草稿状态`: compare unchanged, edited (amber) and unpublished (green) files; inspect preview, edit and diff.
- `06-滚动与批量`: 24 disposable documents for scrolling, Ctrl/Shift selection, row menus, rename and delete.
- `07-仅草稿目录`: a folder inferred entirely from an unpublished file.

- `08-空目录` and `09-未发布空目录`: persisted and draft-only empty directories.
- `04-草稿状态/待删除.md`: a published file with a deletion draft (red filename).

These are synthetic, disposable documents. The seed preserves existing edits on rerun.
The ignored `engine::memory::tests::local_memory_roundtrip` integration test,
explicitly enabled with `CLUMSIES_MEMORY_UI_TEST=1` against the isolated daemon,
covers publication, rename, deletion, clean rebase, manual content-conflict resolution,
and organization selection/proposal/removal. It creates disposable test projects.
Permission-denied accounts and native Windows interaction still need platform testing.

## Connect a work folder and AI tool

1. Select the project, then open **More → Project settings**.
2. Choose **Link folder…** and select the local work folder. The folder is routed
   to this project; its files are not copied into Memory. A folder already bound
   to another project is refused instead of silently reassigned.
3. Open the account menu → **Settings → Agents**, enable your tool, then
   start a new AI session in the linked folder. Connections are shared across
   the machine's linked projects. Codex uses the existing managed plugin
   installer. Windows locates the registered Codex desktop app and its bundled
   CLI, falling back to PATH; Linux requires the CLI on PATH. Opening Settings
   or retrying a failed refresh completes a pending Codex installation only
   when the integration was explicitly enabled. Other Linux adapters use their
   existing daemon-managed configuration.
4. **Unlink** in project settings removes the folder routing. Disabling the agent in **Settings → Agents** removes the managed
   tool integration. The daemon preserves unrelated user configuration and
   reports conflicting configuration instead of replacing it.

Distributions place `clumsiesd` (`clumsiesd.exe` on Windows) beside the desktop
executable. Developers may set `CLUMSIES_AGENT_RUNTIME_BINARY`; `just dev-linux`
sets it automatically. Agent installation is disabled in isolated development
instances; non-Codex Windows integrations are not yet supported.
The client starts the bundled daemon automatically when none is reachable.
The daemon remains available for Agent sessions after the window closes.

The explicit local connection test binds a temporary folder, rejects a
conflicting rebind, invokes a real MCP process with the generated configuration,
checks retrieval and full-document loading, then unlinks the folder:
`cargo test -p desktop local_connection_routes_real_mcp_and_unlinks -- --ignored`.
It requires `CLUMSIES_MEMORY_UI_TEST=1`, the isolated daemon root and runtime
binary environment variables, and the seeded Memory UI fixture.
