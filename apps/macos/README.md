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

## App language

Clumsies supports English and Simplified Chinese. In **Clumsies → Settings → General
→ Language**, choose **Follow System**, **English**, or **简体中文**. The choice applies
only to Clumsies, so an English-language Mac can run Clumsies in Chinese.
**Restart and Apply** uses the normal save and unsaved-change checks, then waits
for the current process to exit before reopening the same App automatically.
Cancelling quit or failing to save cancels the restart. **Later** applies the
choice on the next launch.

The preference uses the app's `AppleLanguages` override and also recognizes a
language selected in macOS System Settings. **Follow System** removes that
override without changing system preferences. Unsupported languages fall back to
English.

`Resources/Localizable.xcstrings` is the translation source. Use literal localized
SwiftUI text, and `String(localized:)` for AppKit, computed labels and errors.
Build with Xcode to extract new keys, then provide both English and `zh-Hans`
translations in the catalog. Use catalog plural variations and whole sentences
with interpolation; keep protocol values, identifiers, paths and user content
separate from display labels. Number and date formatting follows the user's locale.
`bash apps/macos/Scripts/test.sh` runs the full suite in English, then
`LocalizationTests` in Simplified Chinese to check packaged resources, plural
forms and language-dependent display labels independently of the Mac's language.

## Dashboard preview

Dashboard uses native Swift Charts for inventory, retrieval activity, directory coverage,
frequently retrieved documents, published maintenance and retrieval recency. The period
picker uses local calendar days; coverage and rankings deduplicate document IDs.
The server's `/api/v1/org/memory-statistics` and
`/api/v1/projects/{project_id}/memory-statistics` endpoints calculate published inventory,
commit history, distinct changed documents and synced draft counts. Query parameters are
`days=7|30|90` and an IANA `time_zone`; PostgreSQL supplies DST-aware calendar boundaries.
Project inventory includes selected organization memories, excluding draft overlays.
Project selection changes count as changes to that published snapshot. Days before the
first retained commit remain unknown. Metadata lists now return the complete inventory.

The daemon's `dashboard_retrieval_statistics` RPC calculates local request outcomes,
deduplicated document retrievals, directory coverage, rankings and recency in one local
read transaction. Its scope and calendar boundaries come from the authorized server
response. The App requests these two summaries and renders them; it does not walk commits
or download individual retrieval traces. Local retention is 500 requests per project,
so these figures are retained observations, not complete organization-wide usage.
The server statistics API must be deployed before installing this App for daily use.

To preview all panels in an isolated local Dev Instance:

```sh
sh dev/dev-instance.sh up
python3 dev/seed-dashboard.py /absolute/path/to/this/instance/runtime.json
```

Restart that Dev App, then select **Dashboard** in its sidebar. The seed uses the local
fake OIDC provider and Draft/Review APIs, publishes several hundred demo documents,
creates 9 open and 5 submitted drafts, and installs an explicitly labelled 90-day fixture.
It accepts only this worktree's loopback instance, with an empty organization or its own
complete demo dataset. An **Empty project** exercises empty states. Fixture reads require
a Dev Instance identity and never apply to the stable app. `python3 dev/seed-dashboard.py --check`
checks deterministic data and inventory invariants without running a server.

## Source organization

The app follows Slack's [feature organization](https://slack.engineering/happiness-is-a-freshly-organized-codebase/)
and [Features / Services / Libraries responsibilities](https://slack.engineering/stabilize-modularize-modernize-scaling-slacks-mobile-codebases-2/),
with an App directory for native application composition:

```text
Sources/
  App/          # Entry point, AppDelegate, windows and menus
  Features/     # Activity, Administration, Bundles, Dashboard, Diagnostics, Memory,
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

State and operations move together into their owner. The frontend-wide ownership
map includes each feature, not only the main workspace:

| Area | State and workflow owner | View / platform responsibility |
| --- | --- | --- |
| App | AppDelegate and window controllers compose shared services and models | App lifecycle, native menus, windows and termination |
| Workspace | WorkspaceCoordinator coordinates reload, project switching and save-before-exit; WorkspaceNavigation owns tabs, history and selection | Cross-feature layout, navigation and toolbars |
| Memory | MemoryModel owns collection actions; DocumentEditorModel owns editor/diff state; MemoryGuidelinesModel owns setup/adoption; MemoryFileOperationsModel owns batch operations | Render documents/tree, focus, selection and confirmation dialogs |
| Reviews | ReviewsModel owns collection/actions; ReviewDetailModel owns detail/files/comments; ReviewRequestModel owns preflight and batch submission | List/detail presentation and reconciliation sheets |
| Activity | ActivityModel owns filters/list/detail requests; ActivityFragmentModel owns full-fragment loading | Timeline, event detail and fragment presentation |
| Bundles | BundlesModel owns selection/page actions; BundleStore owns data and pending saves | Editor form and resource picker |
| Projects | ProjectCreationModel, ProjectMemberPickerModel, ProjectRepositoriesModel and ProjectStorageModel own their workflows; shared project writes use ProjectService and AdministrationModel | Project settings, form input, native folder pickers and confirmations |
| Administration | AdministrationModel owns permission-scoped snapshots, members, pagination and mutations | Organization forms, member/token controls and audit presentation |
| Settings | AgentsSettingsModel owns agent configuration/loading; SettingsNavigation owns navigation and unsaved-form transitions | Preferences, focus and form controls; app-scoped updates use SoftwareUpdateController |
| Diagnostics | RetrievalDiagnosticsModel owns list/detail pagination, evidence and evaluation operations | Diagnostic presentation and native export dialogs |
| ServerAccess | NativeServerAccessModel owns sign-in/setup; NativeAdministratorRecoveryState owns the temporary recovery session and requests | Server access forms and recovery controls |

Shared services own work that outlives a page:

| Service | Responsibility |
| --- | --- |
| WorkspaceContext and WorkspaceFeedback | Account, organization, permissions, current project, request generations and shared errors |
| WorkspaceLoader | Read snapshots without owning UI state |
| MemoryCatalog | Resource snapshots, content loading and freshness |
| DraftStore | Draft inventory, edit buffers, serialized writes and pending saves |
| DocumentSessions | Document synchronization tasks, locks and reconciliation state |
| MemorySyncService and DraftReconciliationService | Shared resource refresh, upload barriers and reconciliation |
| BundleStore | Bundle data, mutations and pending saves |
| ProjectService | Project creation, repositories and Memory selection |
| DaemonSyncService | Sync status and retry tasks |
| AgentIntegrationService | Agent adapter status and operations |
| Authentication, Server, ServerAccess, Updates | Network/platform capabilities used by their feature owners |

Libraries contain shared DTOs, configuration, serialization, concurrency helpers,
diff algorithms/rendering and reusable UI. Feature-specific state stays in the
owning feature; shared operation types live beside the service that uses them.

The coordinator composes concrete owners and connects completion events; services
never call back through a workspace facade. Leaf feature views receive their
specific page model and observe the shared objects they actually read. Only
cross-feature intents (reload, switch project, prepare index, reveal memory) use
`WorkspaceActions` in the SwiftUI environment; it forwards no state. Page models
are held with `StateObject`, so ordinary View reconstruction does not recreate
their requests or form state. AppKit subscribes to the same shared owners. Both
the main and Settings windows share the administration model.

Pending edits and synchronization outlive individual views. Project switches and
sign-out flush pending edits first; failures retain the edits and stop the
transition. Authority resets clear owner state and invalidate pending work;
completed writes from an old authority cannot repopulate the new workspace.
Page-owned requests also reject stale results after project, query or session
changes. Bulk file operations stop when their original project/authority changes.
An authoritative rename can update an editor path/title without losing dirty text.

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

The regression suite covers failed-save transitions, owner observation, stale
storage/repository/search/review/Activity/Diagnostics responses, recovery-session
reset, editor rename preservation and project changes during bulk operations.
Live tests require the existing explicit opt-in and are skipped by default.
