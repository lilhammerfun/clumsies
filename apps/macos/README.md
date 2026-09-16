# Clumsies for macOS

The native Clumsies desktop app uses AppKit and SwiftUI and includes the Rust
daemon that provides local Memory and Agent integration.

## Install

Follow the [Quick Start](../../README.md#quick-start)
([简体中文](../../README.zh-CN.md#快速上手)) for prerequisites and a ready-to-copy
agent installation prompt. From the repository root, the installation command is:

```sh
just install-macos
```

This builds the complete Debug app, verifies signing, installs it at
`~/Applications/Clumsies.app`, and opens it. The app manages its bundled daemon
and Codex integration automatically. Account, Memory, and configuration data
are retained when replacing an existing installation.

## Start using Memory

Keep the prefilled Server address (`https://app.clumsies.ai` on a first install)
and click **Continue in Browser** to sign in. The app loads the Server's
configured organization automatically. Choose a Project and add your working
repository through **Repositories → Add Repositories…**. Then connect your
agent in **Settings → Agent**. For Codex, wait for **Ready**, restart Codex, start
a new task in the bound repository.

See the [usage guide](../../docs/guides/how-to-use-clumsies.md) for the full
workflow.
