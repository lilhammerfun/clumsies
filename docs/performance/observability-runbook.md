# Clumsies Observability Operations Runbook

For engineers investigating API, sync, index or retrieval faults, this runbook follows an abnormal operation through trustworthy evidence. The [detailed operations guide](/zh/performance/observability-runbook) includes tunnel addresses, installed-host checks and recovery criteria; metric definitions are in the [design](/performance/observability-plan).

## Establish the investigation window

Record the affected operation/site, absolute time range, client and Server versions, and available request or operation IDs. Reach the loopback monitoring ports through the installation's SSH tunnel. Start at Overview and confirm target health and snapshot freshness before interpreting throughput, successful latency, errors and concurrency. Current host/database ages must be between 0 and 900 seconds: -2 is invalid future time, -1 stale or missing. Preserve raw receipts while checking system clocks; do not clamp negative ages to zero. Keep the same time window across pages.

No firing alert does not prove health. Watchdog is an expected notification heartbeat; receipt at the destination verifies delivery. Zero traffic has no latency sample, and legacy mixed-result latency does not measure successful responses alone.

## Select the fault path

| Symptom | Investigation path |
| --- | --- |
| API or review creation is slow | API route/method → representative slow request → shared request correlation → Infrastructure pool/dependency evidence |
| A change has not synced or become searchable | Local original queued operation → actual Server ACK → local synced commit → Commit/Ref installation → current index publication |
| Retrieval is slow or times out | Local request_id → retrieval_run_started/run_id → complete activate duration → lock/model wait, queue, execution and history finish |
| Charts are empty or frozen | Expected targets → exporter/internal task distinction → real collection timestamp → log delivery and notification evidence |

The response server_request_id links Caddy and Server directly. A client request ID first matches Server client_request_id; use that record's internal request_id to find the entrance. A client identity may cover multiple attempts. Empty IDs and prefixes return no correlated records.

## Evaluate and close the fault

Pool acquisition metrics cover only review_create/review_merge and do not represent all database waiters. Caddy entrance, Server handler and local complete HTTP timing have different boundaries; subtracting their percentiles cannot isolate network time.

Select the affected project to inspect Commit last_checked_success_at; unscoped status and the Draft channel intentionally provide None. Compare that successful check with actual Ref installation, allowing old installation time during idle periods.

Local activate timing includes history finish; Run.total_us stops earlier, and the existing 60-second query budget applies only to the query segment. Health.performance is cumulative within one process; use two snapshots from the same process and collection epoch for an interval.

Before retrying a timed-out write, verify its actual result and use existing idempotency/version mechanisms. Re-test the same operation and representative workload, confirm correct results and reduced dominant wait/error, and document the evidence. Product Draft age or unread record counts do not justify clearing human work. Local summaries cannot yet establish the 99% five-minute write SLI.
