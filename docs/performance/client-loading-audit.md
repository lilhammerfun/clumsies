# Client loading and request audit

Evidence date: September 17, 2026. Baseline: `f3917816`. “Fixed” below describes this workspace change, not an installed or deployed release. The audit covers startup, project switching, Memory, Guidelines, Reviews, Bundles, Activity, retrieval diagnostics, administration, and the App → XPC → daemon → HTTP path. Server capacity upgrades are outside its scope.

## Findings

Both presentation state and request scheduling have defects. Existing Reviews, Bundles, and administration views already distinguish loading and failure; Workspace has concurrency limits, revision checks, and refresh throttling. The gaps are inconsistent placeholders, document and Activity failures that do not terminate loading correctly, and work performed before the user needs it.

Guidelines preview reads bundled Markdown without HTTP. Its previous independent presentation Boolean and optional document allowed an empty sheet. The new `.sheet(item:)` requires the document before presentation. This removes a demonstrable state gap; without a recording of the reported click, it does not establish the cause of every millisecond in that screenshot.

## Fixed in this change

| Area | Behavior |
|---|---|
| Startup | Loading uses a 360 pt window; forms retain their previous dimensions and transitions preserve the window center. |
| Guidelines | One **Set Up Guidelines** primary action; preview remains a link and includes every starter document. |
| Starter | One local transaction creates `CLUMSIES.md`, `knowledge/README.md`, `procedures/README.md`, and `lessons/README.md`. Existing guidance and folders are preserved; failures roll back the batch. |
| Guidelines requests | Presentation reuses loaded metadata. Adoption revalidates shared authority once and passes that revision into the batch. |
| First loads | Memory, Reviews, Bundles, Activity, and project preparation use a shared list/document skeleton. Refreshes retain existing content. |
| Document failures | Inline errors and retry replace endless loading; readers of the same revision share the request and its failure. Authority changes cancel old tasks. |
| Activity | Failure, loading, and successful empty results are distinct. Duplicate loads are suppressed, stale results ignored, and refresh failures retain content. |
| Diagnostics | Successful server response logs now include App elapsed time, covering limiter wait, XPC, and response delivery. |

Starter READMEs explain their folders without inventing project knowledge. Documents enter the existing Draft and review process, not direct publication. See [Memory Guidelines](../guides/memory-guidelines.md).

## Remaining request design work

These are confirmed code findings, not implementations included in this change.

| Priority | Evidence | Next change |
|---|---|---|
| High | `AppDelegate.startAfterNativeSetupCheck`; `WorkspaceLoader.loadAuthenticatedWorkspaceIdentity` and `reconcileManagedAgentAdapters` | Startup awaits `/setup`, then configured adapter maintenance, before identity and workspace loading. Adapter XPC has a 130 s budget. Give maintenance independent status and remove it from first-ready while preserving maintenance during sign-out and offline operation. Coordinate setup detection with offline startup. |
| High | `crates/daemon/src/recall.rs:list_recalls`, `enrich_session` | The async handler synchronously parses local files; DSH enriches every session before the final limit. Codex limits candidates earlier but still loads content and enriches details for the list. Gather bounded summaries on the blocking pool and load selected details on demand, retaining corrupt-session isolation. |
| High | `RetrievalDiagnosticsModel.loadMore` | Pagination lacks project/load-generation checks. Separate list and detail generations, invalidate old pages, and retain content after a same-project refresh failure. |
| Medium | `WorkspaceLoader.load` | With one selected project and one page per memory scope, first-ready needs approximately nine GETs: identity, three initial state/selection requests, two lists, and three verification requests. Some are parallel, but final organization and project checks remain sequential. Parallelize independent checks, then assess a versioned aggregate snapshot without dropping consistency checks. |
| Medium | `WorkspaceLoader.loadBundles`; `BundleNavigator` | List loading fetches every bundle detail (N+1), and navigation prepares the workspace index. Fetch display summaries first and defer detail/selector data until needed. |
| Medium | `DaemonXPCClient.call`; `state.rs:HTTP_REQUEST_TIMEOUT` and `server_request` | Both ordinary XPC and a single HTTP request have 30 s budgets. Cache fallback follows network failure; refresh/retry can outlast XPC. Share an operation deadline and cancellation policy, reserving fallback time and distinguishing read-only display from mutation authority. |
| Medium | `ServerRequestLimiter` | Concurrency is capped at 12, but queued cancellation is only checked after admission. Add cancelable admission; cross-view GET coalescing must respect identity and revision, not URL alone. |

Do not remove authorization or revision checks for speed. Existing Workspace status/data refreshes use 2 s/30 s cadence and in-flight guards. This audit did not establish an unlimited request storm.

## Local evidence and limits

Read-only inspection of installed client and daemon logs, September 17, China Standard Time; no credentials, document bodies, or project identifiers exported:

| Time | Observation | Interpretation |
|---|---|---|
| 13:26:38 | `/setup`: 200, 169 ms | This request was not a multi-second startup delay; other starts may differ. |
| 13:37:48 / 13:38:48 | Review file: 5,710 / 5,873 ms; subsequent content hits: 0–2 ms | Initial content loading was slow. This does not separate server execution, transport, and queue time. |
| 13:38:07 | `list_recalls`: XPC timeout after 31,282 ms | A local session-loading operation also timed out; this path is not a server download. |
| 13:37–13:38 | Several daemon server requests: approximately 3.2–5.8 s | Real request latency occurred, without enough boundary evidence to attribute it to bandwidth. |

These historical samples are neither percentiles nor a before/after benchmark. They do not establish that every blank view is network-caused.

## Loading contract and verification

Use the existing state models and native SwiftUI: stable skeleton for first load; empty state only after successful empty data; inline retry for first-load failure; retained content and progress/error for refresh; invalidated results after a context switch. Decorative skeleton shapes are hidden from accessibility while the loading title remains readable.

Validation includes the complete macOS suite, daemon unit/lifecycle suites, Rust Clippy, and documentation build. New checks cover starter assets and preservation, the Swift/daemon batch payload, rollback, shared document failures, Activity stale/duplicate results and retry, and startup dimensions.

Follow-up request changes need isolated slow-response, offline, credential-refresh, canceled-queue, and large-session measurements. Per-page visual acceptance and installed-build performance recordings remain outstanding; this change does not claim to resolve every latency problem.
