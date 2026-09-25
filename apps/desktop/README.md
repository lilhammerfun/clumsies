# Clumsies Desktop

The Windows and Linux client. macOS keeps its own native Swift app in `apps/macos`.

- UI toolkit: [GPUI Kit](https://gpui-kit.com) (`gpui-kit`), the same stack Zed is built on.
- Engine: the local `clumsiesd` engine, shared with the macOS client.

## Run

```sh
cargo run -p desktop
```

## Current state

Layout and interaction skeleton only. The Project list is hard-coded; nothing is
loaded from `clumsiesd` yet.
