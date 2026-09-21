# macOS error feedback audit

Evidence date: September 21, 2026. Baseline: `940d5830`. Branch: `codex/macos-error-feedback`. This describes the workspace change, not a deployed release.

## Scope and finding

The audit covers all 127 Swift files under `apps/macos/Sources`: application lifecycle, Server/XPC boundaries, Inbox, Memory, Reviews, Bundles, Dashboard, Activity, projects, administration, settings, and diagnostics. Search matches count references, not defects.

Three defects compound each other: transport details become interface copy; multiple owners report one failure; load states conflate an initial failure, a failed refresh, and an unsuccessful user action. Removing every error would hide important outcomes. Rewording errors alone would leave duplicate presentation intact.

## Changes

| Area | Result |
|---|---|
| Server, XPC, authentication, bootstrap | Classify failures before presentation. Keep wire bodies, request IDs, decoding paths and process exit codes out of ordinary UI. Locally authored validation and recovery instructions remain useful. |
| Main and Settings windows | One dismissible service status per window. Ignore cancelled and superseded requests; clear account-scoped state on authority reset. |
| Workspace refresh | A temporary service failure retains an already usable window. Cold-start failures still present recovery; old-account failures cannot alter the new workspace. |
| Inbox reading | Welcome bodies open through native navigation. No reading sheet, duplicate receipt error, or list filters in the message destination. |
| Inbox receipts | Persist read/archive intents before updating the interface. Retry on the existing refresh cadence. Keep authority and notification-version boundaries, coalesce opposing intents, and roll back permanent failures. In-flight reads do not disable archiving or later unread intent. |
| Inbox loading | Failed first loads show recovery rather than an empty inbox. Refreshes retain notifications. A failed local status probe does not manufacture a business notification. |
| Reviews and reconciliation | Remove duplicate load feedback and nested error alerts. Keep failed-submission input and resolution choices in the current task. |
| Memory and collections | Background service failures do not also become operation-failed banners. Retain content; preserve necessary save, partial-completion, conflict and data-loss feedback. |
| Dashboard and Activity | Retain content after refresh failure. Keep initial-load and pagination recovery distinct. |
| Projects and administration | Retain repository bindings only within the same project and authority. Normalize storage errors, remove duplicate administration banners, and retain cached-data write guards and pagination recovery. |
| Retrieval history | Preserve loaded history and detail; distinguish failed initial load from no records. Put pagination recovery in the window corner and clear obsolete detail when selection changes. |
| Cancellation and exports | Recognize task, URL and Cocoa cancellation. Provide actionable disk-space and file-access copy. Keep export diagnostics in logs. |

## Feedback placement and semantics

The audit also covers Inbox receipts, Memory/Bundles/Reviews refresh feedback, workspace operation failures, Activity pagination, administration cache/action state, project forms and diagnostics. Presentation now distinguishes two cases:

- Window connection/cache state uses a low-emphasis bottom-right icon and text, without a card, background, shadow or auto-dismiss timer. It consumes no sidebar/content space. `pageFeedback` / `feedbackHost` show one non-form message per window and dismiss duplicate sources together. Explicit action failures use the error color.
- Save/submit errors belong inside the active form, using red text and an error icon. This covers project creation/details, organization/domain changes, organization/project members, Review submission, Draft reconciliation and sign-in setup. Input and native actions remain available. An unknown HTTP 400 does not identify a specific invalid field.

ServerClient writes still throw to their action owner, but no longer also create window service feedback. GET / HEAD reads and refreshes maintain connectivity state, avoiding a second copy of a failed Save behind its sheet.

Settings icons remain 20 × 20 pt during connectivity/authentication failures. Initial unavailable content, field validation, destructive decisions and historical diagnostic records retain their own semantics.

Error text and icons also convey the meaning without depending on red alone. [Apple: Differentiate Without Color Alone](https://developer.apple.com/help/app-store-connect/manage-app-accessibility/differentiate-without-color-alone-evaluation-criteria)

## Presentation policy

| Situation | Policy |
|---|---|
| Background connection/timeout/500/503/local-service failure | Keep usable content and show the window service state. No alert or Inbox notification. |
| First load without usable content | Unavailable content state with retry; never successful-empty copy. |
| Explicit save, submit or delete | Explain that the action did not complete and preserve the task/input. Do not automatically retry arbitrary writes. |
| 400/422 | Keep local field validation. Normalize unknown server rejection without displaying the response body. |
| 401 | Ask for sign-in without automatically signing out or discarding work. |
| 403 | Explain changed access and suggest refreshing permissions. |
| 404/410 | Explain that the item is unavailable; do not treat it as a connection failure or successful deletion. |
| 409/412 | Ask for the latest version and use existing reconciliation. |
| 429 | Ask the user to wait; do not add a rapid retry timer. |
| Cancelled/superseded request | No error presentation or overwrite of newer state. |
| Retryable notification receipt | Queue locally per server/organization/user. Acknowledging an old version does not read a newer event. |

Activity's explicitly expanded technical details, retrieval diagnostic records, and privacy-filtered logs remain diagnostic surfaces. Destructive confirmations and unsaved-input protection retain native prompts. Export and restart failures use corner feedback instead of modal error alerts.

## Design evidence

Apple recommends using alerts sparingly and cites Mail's nonintrusive connection indication and cached startup content as alternatives. [HIG: Alerts](https://developer.apple.com/design/human-interface-guidelines/alerts)

Reading a welcome message has no decision that requires a modal interruption. [HIG: Modality](https://developer.apple.com/design/human-interface-guidelines/modality)

Feedback severity, recovery advice and input preservation follow the error-message guidance; useful field errors can remain near their source. [NN/G: Error-Message Guidelines](https://www.nngroup.com/articles/error-message-guidelines/)

The specific window state and receipt queue are project decisions, not a universal Apple component prescription. They reuse native navigation, existing toolbar components, diagnostics and the 30-second data-refresh cadence, without a toast framework or a generic write retry layer.

## Verification and limits

Rust, lifecycle and live results below belong to the preceding error-policy audit; this placement follow-up did not change those service implementations. Its complete macOS rerun and native geometry evidence are recorded in the accompanying acceptance report.

| Check | Result |
|---|---|
| Placement follow-up: `just test-macos` | Full English and selected Simplified Chinese suites passed, exit 0; includes Settings icon regression |
| Full English macOS suite before placement follow-up | 422 passed, 0 failed, 2 skipped |
| Simplified Chinese Localization, AppLanguage and SettingsWindowLayout | 10 passed, 0 failed |
| `just test-macos-live` against the running Local Dev Instance | 3 passed, 0 failed, 0 skipped; workspace loading, draft and Bundle lifecycles |
| `just test-dev-macos` | Instance lifecycle contract passed |
| `just test-macos` and Debug promotion contract | Passed; the final Review access regression also passed a subsequent full English run |
| `cargo test --locked --workspace` | 433 passed, 0 failed, 2 model tests ignored by their existing configuration |
| `cargo fmt --all --check` | Passed |
| `cargo clippy --locked -p daemon -- -D warnings` | Passed |
| `cargo clippy --locked -p server --all-targets -- -D warnings` | Passed |
| `npm run build` and `git diff --check` | Passed |

Added and extended checks cover HTTP 400/401/403/404/409/422/429/500/503/504, URL/Cocoa cancellation, local-service timeout, disk and permission failures, late responses, authority switches, first-load versus refresh failure, failed-submission input retention, persisted/coalesced offline receipts, permanent-failure rollback, and native welcome-message navigation.

The two normally skipped macOS tests require `CLUMSIES_RUN_LIVE_TESTS=1`; both were subsequently exercised successfully through `just test-macos-live` on the Local Dev Instance. The two ignored Rust tests download large models and generate real embeddings. The first Rust run could not find the default Docker socket; the complete rerun passed with the actual OrbStack socket configured. Database tests create and clean up isolated containers.

Tests use an isolated test App, temporary preferences and injected failures. A hosted native window performs the welcome message button click and checks navigation title, absence of a sheet, read state and toolbar changes. Follow-up acceptance also runs the complete Local Dev Instance, introduces failures on its real HTTP port, drives native accessibility actions, captures application windows and checks receipts on the real Server.

Native acceptance also corrected layout and copy defects. Service feedback now uses a lightweight bottom-right label without taking space from the sidebar or content. Settings also shows one window-level indication, separate from its sidebar icons. Native geometry checks compare content positions before and during disconnection. A rejected notification receipt incorrectly asked users to check entered information; it now provides Inbox-specific recovery guidance.

This is not a production fault drill: live SSO reauthentication, prolonged outages and a real server-wide incident were not exercised against a production account. These changes reach installed releases only after merging and rebuilding the branch.

## Repeatable native fault checks

Start `just dev-macos` in this worktree and let Inbox load. Run one command at a time; each restores the Server after 60 seconds or Ctrl-C:

```sh
just dev-macos-fault offline
just dev-macos-fault 500
just dev-macos-fault 400
just dev-macos-fault 404
```

For `offline`, refresh, read the welcome body and mark/archive notifications; verify retained content, immediate local state and receipts after reconnection. For `500`, refresh and verify one service indication with retained content. The daemon's stale-cache fallback reports temporary connectivity failure rather than exposing HTTP details. For `400` and `404`, select a notification and explicitly mark it unread; verify contextual feedback and rollback, not just background refresh behavior.

The tool accepts only this worktree's loopback Local Instance, reuses ownership checks and the lifecycle lock, temporarily replaces its Server listener, then restores the original Server and verifies health. The App, daemon and database remain running. No in-product debug controls were added. A diagnostic marker in the artificial response body and its request ID must never appear in ordinary UI.

A separate PostgreSQL test verifies that a new user's first session creates one welcome with a body, subsequent sign-in does not resend it, and receipts survive. Native screenshots reuse the Dev user's existing welcome, restored and marked unread through the normal receipt API; they do not represent a second first login.
