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
  Services/     # Authentication, Bundles, Daemon, Memory, Projects,
                # Runtime, Server, ServerAccess, Updates, Workspace
  Libraries/    # Concurrency, Configuration, Diagnostics, Diff,
                # Formatting, Models, Serialization, UI
Tests/
  App/          # Window lifecycle and layout
  Features/     # Feature behavior and presentation
  Services/     # Workspace and daemon contracts
  Libraries/    # Shared building blocks
  Integration/  # Opt-in live workspace tests
```

State and operations move together into their owner:

| Owner | Responsibility |
| --- | --- |
| `Features/Workspace/WorkspaceCoordinator` | Shared component lifetime, reload, project switching, save-before-exit and refresh ordering |
| `Features/Workspace/WorkspaceNavigation` | Tabs, history, selection and toolbar commands |
| `Features/Memory/MemoryModel` | Memory presentation, creation, export and document Sync interaction |
| `Features/Reviews/ReviewsModel` | Review loading, selection and actions |
| `Features/Bundles/BundlesModel` | Bundle selection and page actions |
| `Features/Administration/AdministrationModel` | Administration loading, pagination and caches |
| `Features/Activity/ActivityModel` | Activity selection and loading |
| `Services/Workspace/WorkspaceContext` | Account, organization, permissions, projects and request generations |
| `Services/Memory/MemoryCatalog` | Resource snapshots, content loading and freshness |
| `Services/Memory/DraftStore` | Draft inventory, edit buffers, serialized writes and pending saves |
| `Services/Memory/DocumentSessions` | Document synchronization tasks, locks and reconciliation state |
| `Services/Memory/MemorySyncService` and `DraftReconciliationService` | Shared resource refresh, draft upload barriers and reconciliation |
| `Services/Bundles/BundleStore` | Bundle data, mutations and pending saves |
| `Services/Projects/ProjectService` | Project creation, repositories and Memory selection |
| `Services/Daemon/DaemonSyncService` | Sync status and retry tasks |
| `Services/Runtime/AgentIntegrationService` | Agent adapter status and operations |

The coordinator composes concrete owners and connects completion events; services
never call back through a workspace facade. `WorkspaceLoader` reads snapshots
without owning UI state. SwiftUI observes the actual state owners through
`workspaceEnvironment`, including owners used by derived feature properties.
AppKit subscribes to those same owners. Both the main and Settings windows share
the administration model.

Pending edits and synchronization outlive individual views. Project switches and
sign-out flush pending edits first; failures retain the edits and stop the
transition. Authority resets clear owner state and invalidate pending work;
completed writes from an old authority cannot repopulate the new workspace.

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
