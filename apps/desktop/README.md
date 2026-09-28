# Clumsies Desktop

The Windows and Linux client. macOS keeps its own native Swift app in `apps/macos`.

- UI toolkit: [GPUI Kit](https://gpui-kit.com) (`gpui-kit`), the same stack Zed is built on.
- Engine: the local `clumsiesd` engine, shared with the macOS client.
- Design rules: [DESIGN.md](DESIGN.md).

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

What is still missing is screen coverage, not data: Inbox, Bundles and Activity
are not translated, and Reviews is missing comments, resubmission and the
permission checks macOS makes from its own capabilities.
