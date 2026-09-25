# Clumsies Desktop

The Windows and Linux client. macOS keeps its own native Swift app in `apps/macos`.

- UI toolkit: [GPUI Kit](https://gpui-kit.com) (`gpui-kit`), the same stack Zed is built on.
- Engine: the local `clumsiesd` engine, shared with the macOS client.
- Design rules: [DESIGN.md](DESIGN.md).

## Run

```sh
cargo run -p desktop
```

## Layout

```
src/
├── main.rs         bootstrap: window options, app id, title
├── app.rs          the shell: Project rail beside the active screen
├── engine.rs       the engine seam — fixtures today, daemon calls later
├── ui.rs           spacing steps, the Windows type ramp, radii
├── components/     diff, Markdown, Memory tree
└── screens/        one module per screen
```

## Current state

The Project rail, the Memory tree and the Markdown preview all read from the
local `clumsiesd`: Projects come through its Server proxy, and a Project's
Memory comes from its checkout. Signing in is a daemon concern; on Linux and
Windows `dev/dev-login.py` performs the same authorization a client does and
hands the session to the daemon.

What is still missing is screen coverage, not data: Reviews, Inbox, Settings
and the draft editing surface are not translated yet. `components/diff.rs`
is waiting for the first two.
