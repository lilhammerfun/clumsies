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

## Source organization

The app follows Slack's [feature organization](https://slack.engineering/happiness-is-a-freshly-organized-codebase/)
and [Features / Services / Libraries responsibilities](https://slack.engineering/stabilize-modularize-modernize-scaling-slacks-mobile-codebases-2/),
with an App directory for native application composition:

```text
Sources/
  App/          # Entry point, AppDelegate, windows and menus
  Features/     # Activity, Administration, Bundles, Diagnostics, Memory,
                # Projects, Reviews, ServerAccess, Settings, Workspace
  Services/     # Authentication, Daemon, Memory, Runtime, Server,
                # ServerAccess, Updates, Workspace
  Libraries/    # Concurrency, Configuration, Diagnostics, Diff,
                # Formatting, Models, Serialization, UI
Tests/
  App/          # Window lifecycle and layout
  Features/     # Feature behavior and presentation
  Services/     # Workspace and daemon contracts
  Libraries/    # Shared building blocks
  Integration/  # Opt-in live workspace tests
```

Keep feature state with its views. `AdministrationModel` owns page loading,
pagination and caches; `ActivityModel` owns activity selection and loading.
`WorkspaceStore` owns the shared account/project context and coordinates edits,
saving and project switches. `WorkspaceLoader` loads snapshots without owning
UI state. Authority changes invalidate administration requests synchronously.
`AppDelegate` creates the shared administration model and supplies it to both
the main and Settings windows; views observe that model directly.

The responsibility split is incomplete: `WorkspaceStore` still owns tab
navigation, document saving, synchronization and reconciliation, and Bundle and
Review operations. These remain coupled through shared mutable state. Moving
the store into Services does not resolve that coupling; further extraction
should move state and operations together into their owning feature or service,
while preserving save-before-switch and authority invalidation guarantees.

Services must not reference feature views or feature state. Libraries hold
shared values and building blocks without depending on Services or App startup.
Keep private components inside their feature until another feature uses them.
The UI uses **Activity** names; recall protocol types retain their wire semantics.

These are source boundaries within the existing `Clumsies` target. XcodeGen
includes `Sources` recursively; no packages, build tools or dependencies are
added by this organization. Keep Resources, Config and Scripts at the app root.
Run the normal hosted tests from the repository root:

```sh
bash apps/macos/Scripts/test.sh
```

Live tests require the existing explicit opt-in and are skipped by default.
