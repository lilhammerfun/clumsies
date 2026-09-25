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

Skeleton. The Memory screen is real-shaped (tree, Markdown, draft diff) but
every document is compiled in and nothing is loaded from `clumsiesd` yet;
`engine.rs` is the only file that changes when it is.
