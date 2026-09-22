# Troubleshooting

Use this page to locate the failing layer before choosing a recovery action. Start with the [three outcomes](/flows): saved locally, synchronized to the Server, and published through authorization.

## Start with the symptom

| Symptom | Check first | Next action |
| --- | --- | --- |
| An agent cannot resolve its Project | Task directory, directory binding, and selected Server | Confirm the binding in Desktop and start a new agent task; see [Agent integration](/guides/agent-runtime) |
| An agent stops connecting after an update | Whether the proxy and resident daemon belong to the same App build | Finish the update, restart the relevant processes, and open a new task; see [Host adapters](/adapter) |
| A saved Draft never synchronizes | Draft/Project sync state, network, Server address, and sign-in | Repair connectivity or sign in again, then use Retry; saved operations remain queued locally |
| A Draft is behind | The upstream version advanced while the Draft Base stayed fixed | Compare Base / Current / Draft Result and confirm reconciliation; do not keep submitting a stale candidate |
| Review submission or merge reports a version error | Review/Draft versions, candidate, and target Ref freshness | Refresh state, inspect the differences again, and confirm a new operation |
| Published content still looks old to an agent | Project selection, local Commit/index readiness, and any unmerged Draft overlay | Check selection, synchronization, index, and drafts in that order; publication is not readiness on every device |
| `load` finds a resource but `activate` omits it | Query, ranks, budget, and incremental state | Inspect the Retrieval Run; absence from a search response does not prove the resource is missing |
| Retrieval fails after a custom disk disconnects | Accessibility of Project Local Storage | Restore access and inspect storage state; do not delete the central database as a recovery step |
| Server health succeeds but a page is slow | Actual request timing, response size, request count, and local rendering | Collect the stages below; health does not prove business-request or page readiness |

## Locate a slow page

For a Review page, distinguish at least these measurements:

1. Start and completion of Review submission.
2. Start and completion of the Review detail request.
3. Commit snapshot download count, repeated requests for the same Commit, and total bytes.
4. Local decoding, diff calculation, and page display completion.

If submission already succeeded, read the created Review's state before creating another proposal because its page has not appeared. Many short serial requests can accumulate into a long wait; examining only the slowest request misses repeated downloads and sequential dependencies. See [Latency and diagnosis](/performance/latency-model).

## Collect useful evidence

Record the operation time and timezone, App/daemon versions, Project, action, exact error code, and the last stage known to have succeeded. Preserve request IDs for matching client and Server logs. Do not paste tokens or complete private Memory content into a report.

Stable installations put client diagnostics under:

```text
~/Library/Logs/ai.clumsies/
```

See [Local runtime](/runtime) for filenames, rotation, and request correlation. Isolated development instances have their own directories; use `just dev-macos-logs` rather than reading stable-install logs. macOS crash reports live under `~/Library/Logs/DiagnosticReports/`; they diagnose process exits rather than business requests.

## Keep cache recovery separate from saved edits

Project generations and search indexes are rebuildable. Central SQLite also holds Drafts and operations that may not have been uploaded. Use the product's Project cache management after confirming that the failure belongs to that layer. Deleting `local.db` can lose unsynchronized edits.

Administrators can continue with [Deployment](/guides/deploy-for-an-org) and [Authentication](/reference/auth) for OIDC, database, and service health. Developers can use the [Codebase map](/repos) to find the implementation for the affected layer.

## Project sync paused after project removal

An older client may report “Commit state response is missing ETag” when a
removed or inaccessible Project actually returns HTTP 404. Update the macOS App
and its embedded daemon together. No Server migration is required for this fix.

The updated client pauses unavailable Projects after checking current membership;
other Projects and Organization memory continue syncing. Open **Inbox → Project
sync paused → Manage Unavailable Projects** to remove an obsolete directory binding or
export retained local Drafts as JSON. Removing a binding preserves the repository
and Drafts. Managed agent integrations must be removed successfully before the
binding is removed; if their directory is unavailable, restore its location first.
Use **Check Again** after access is restored. A network or sign-in failure is
reported separately and does not remove local bindings.
