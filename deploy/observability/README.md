# Observability

Optional Prometheus stack for one Clumsies installation. It runs beside the
three production containers and publishes every port on the host loopback
interface only.

| Component | What it reports |
|---|---|
| `prometheus` | Scrape, rule evaluation, and 90-day retention |
| `alertmanager` | Email delivery of firing alerts |
| `grafana` | Overview, Business API, Website Traffic, and Infrastructure dashboards in English and Chinese, with Prometheus and Loki datasources |
| `node-exporter` | Host CPU, memory, filesystem, and the textfile metrics below |
| `cadvisor` | Per-container CPU and memory usage |
| `postgres-exporter` | Connections, transactions, and database size |
| `blackbox-exporter` | Public HTTPS probes for the app, docs, official site, and the `www` alias |
| `loki` | Container logs, 14-day retention |
| `alloy` | Ships container logs from the Docker socket into Loki |
| `server` (scrape target) | Request rate, latency histogram, in-flight gauge, and database pool gauges from the Server itself |
| `clumsies-observability-metrics.timer` | Backup, restore-drill, and release freshness |

Caddy exposes its HTTP metrics on the admin API, so the production
`deploy/Caddyfile` starts with a global block that binds the admin endpoint to
the Compose network and enables metrics:

```caddyfile
{
	admin :2019
	metrics {
		per_host
	}
}
```

The admin endpoint stays reachable only from the Compose network because
`compose.production.yml` publishes nothing but ports 80 and 443.

The Server exposes Prometheus metrics on `/metrics` for the same network. Its
route labels are registered route templates and its status labels are bounded response codes,
so cardinality cannot grow with the number of organizations or resources.
Caddy answers 404 for that path on the public origin, so the endpoint is never
reachable from the internet.

## Install

```bash
sudo install -d -m 0700 /opt/clumsies/observability /var/lib/clumsies-observability/textfile
sudo cp -r deploy/observability/. /opt/clumsies/observability/
cd /opt/clumsies/observability
sudo cp .env.example .env        # fill GRAFANA_ADMIN_PASSWORD and CLUMSIES_POSTGRES_DSN
sudo chmod 600 .env
sudo install -m 0755 host/clumsies-observability-metrics /usr/local/sbin/clumsies-observability-metrics
sudo install -m 0644 host/clumsies-observability-metrics.service /etc/systemd/system/
sudo install -m 0644 host/clumsies-observability-metrics.timer /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now clumsies-observability-metrics.timer
sudo chmod 0755 /var/lib/clumsies-observability /var/lib/clumsies-observability/textfile
sudo docker compose --file compose.observability.yml --env-file .env up -d --wait
```

The freshness script writes a textfile metric file that `node-exporter` reads.
That collector runs unprivileged, so both directories must stay traversable and
the metric file readable.

## Access

Every published port binds to `127.0.0.1`. Reach Grafana through a tunnel:

```bash
ssh -L 3000:127.0.0.1:3000 -N <installation-host>
# open http://localhost:3000 and sign in with the configured administrator
```

Prometheus listens on `127.0.0.1:9090` and Alertmanager on `127.0.0.1:9093`, so
the same tunnel can forward all three:

```bash
ssh -N -L 3000:127.0.0.1:3000 -L 9090:127.0.0.1:9090 -L 9093:127.0.0.1:9093 <installation-host>
```

The Compose commands set `--web.external-url` to `http://localhost:9090` and
`http://localhost:9093`. Email and Source links therefore use these tunnel
addresses instead of Docker hostnames. Keep the tunnel open when following a
link; if you forward different local ports, adjust the corresponding flags.
This does not expose the monitoring services on the public interface. Links
in emails already delivered cannot be changed.

| Page | Purpose |
|---|---|
| [Prometheus Alerts](http://localhost:9090/alerts) | All alert rules, their expressions, pending duration, and current state |
| [Alertmanager Alerts](http://localhost:9093/#/alerts) | Firing alerts, notification groups, and temporary silences |
| [Grafana](http://localhost:3000/) | Metric trends and logs |

An inactive Prometheus rule is enabled but not currently firing. Alertmanager
does not list inactive rules or edit Prometheus thresholds. Its Status page
shows the loaded notification routing configuration.

## Request investigation

Open **Overview** to compare availability, request rate, 5xx rate and entrance
p95 by site. The four configured hosts remain separate. Click a site series to
open **Website Traffic** with that host and the current time range. Shared host,
database, backup, collection and log-delivery details are in **Infrastructure**.

**Business API** uses Server route templates. Select a route and method to compare
successful handler p50/p95/p99, matching sample counts, error latency, response
codes and requests over one second. Health
checks (`/api/v1/admin/health`) and `/metrics` are excluded in these queries;
the underlying series remain available for operational investigations. Handler
latency ends when a Response is returned, not after the body has been sent.
The separate app entrance slow-log panel covers all app paths regardless of the
selected backend route, including slow transmission and proxy failures.

**Website Traffic** reuses one dashboard with a host selector for the docs,
official site, www redirect and app entrance. Caddy queries select only
`handler="subroute"`, measuring one site-handling layer instead of adding
nested middleware observations. This includes synthetic availability requests,
but excludes automatic HTTP-to-HTTPS redirects. Neither this duration nor the
Server duration measures browser page-load time.

Every percentile is a **rolling five-minute histogram estimate**, not a maximum
or the duration of a particular request. Small samples make p99 unstable;
read the count beside it. `increase()` extrapolates at window edges, so counts
can be fractional. Zero traffic has no latency sample, and must not become a
zero-latency success. One second is a slow-log investigation filter, not an SLO.
Server buckets add 2.5, 10 and 30 seconds to improve tail visibility; quantiles
remain estimates, and durations beyond the last finite bucket remain bounded
by histogram limitations.

## Logs

All four sites share the same redacted access-log schema: `site`, original
`path`, `status`, response `size`, `duration` in **milliseconds**, and
`request_id`. App entrance records also include `upstream_duration_ms` and
`upstream_latency_ms`. Headers, URL query strings and bodies are not recorded.
Paths remain log fields, not metric or Loki stream labels. Website rewrites
must not replace the original requested path in logs.

1. Select an abnormal time interval in the dashboard and inspect its slow-log
   panel. Website logs filter by site; backend logs filter by route.
2. Expand a line and click **Related request logs** on the derived Request field.
   The request panel opens a Loki query for that ID across both `caddy` and
   `server`, preserving the time range. Static sites naturally have no Server
   counterpart. This is log correlation, not a distributed trace.
3. Alternatively paste an `x-request-id` response header into **Request ID**.
   The dedicated correlation panel ignores site/route filters so it cannot hide
   the other layer. Empty IDs return no logs; IDs match the exact JSON field.
4. If both entrance and handler are slow, investigate the backend. If only the
   entrance is slow, inspect proxy/body transfer. The two timers have different
   boundaries; their difference is not a measurement of network RTT.

Request IDs stay inside log lines; Loki stream labels remain `project`,
`service`, and `container`. `client_request_id` in Server logs can additionally
connect a client report to a Server request. Missing internal DB/external-call
spans cannot be reconstructed from an ID: add targeted timings when necessary.
A five-minute database snapshot can miss short query/connection waits.

Prometheus also scrapes Alloy and Loki. Infrastructure shows send retries, dropped
entries and delivered entries; sustained retries and any drops alert. A failed
scrape uses `ScrapeTargetDown`. Zero delivered logs alone does not prove a
failure (the source may be idle), and successful delivery does not prove that
every source was configured correctly. The isolated request test covers that
configuration boundary. Logs retain 14 days, versus 90 days of metrics; older
metric anomalies may no longer have request-level evidence.

## Alert delivery

`alertmanager/alertmanager.yml` ships with placeholders. Replace them with a
Google account and an app password (Google Account, Security, two-step
verification, app passwords):

```yaml
global:
  smtp_from: you@gmail.com
  smtp_auth_username: you@gmail.com
  smtp_auth_password: the-16-character-app-password
receivers:
  - name: gmail
    email_configs:
      - to: alerts@example.com
```

The file holds a credential, so keep it readable only by the unprivileged user
Alertmanager runs as:

```bash
sudo chown 65534:65534 alertmanager/alertmanager.yml
sudo chmod 0400 alertmanager/alertmanager.yml
sudo docker compose --file compose.observability.yml --env-file .env exec alertmanager \
  amtool check-config /etc/alertmanager/alertmanager.yml
sudo docker compose --file compose.observability.yml --env-file .env restart alertmanager
```

The `Watchdog` rule always fires and has a separate route that emails once per
24 hours, after an initial 10-second wait. This checks notification delivery,
not application health. Other alerts wait 30 seconds initially, notify group
changes on a five-minute interval, and repeat every four hours until resolved.
Recovery also sends an email. Warning and critical alerts use the same receiver.

## Alert rules

Availability: a public endpoint down for two minutes, a certificate expiring
within 14 days, or a scrape target down. Capacity: a filesystem below 15 percent
free or host memory below 10 percent available. Application: PostgreSQL
unreachable, more than 80 percent of connections in use, or a Clumsies container
above 2 GiB. Operations: no successful backup for 26 hours, no successful restore
drill for 8 days, failed backup/restore units, missing expected targets, stale
host collection, invalid future operational timestamps lasting five minutes,
and failed Alertmanager notifications. HTTP server errors above one percent for five minutes and
database queries above one minute for five minutes also alert. A failed, missing,
or more than 15-minute-old database snapshot alerts after five minutes while
node-exporter is reachable; exporter outages use `ScrapeTargetDown`.

Draft age/count, recent Commit records, current reconciliation conflicts, and
stored unread receipts remain in the collector and Grafana dashboards. They do
not page the operator: an open Draft can still be used and updated in its Project, reading
does not require publication, and human work has no installation-wide response
deadline. Add an alert only when a measured failure or an agreed service
objective gives its recipient a concrete action. Commit counts cannot measure
client synchronization latency.

## Database metric semantics

The host timer samples every five minutes. All database metrics are gauges,
not counters; deleting records can reduce counts. Both dashboard languages use
the same queries and explain these limits in panel descriptions.

| Metric | Meaning |
| --- | --- |
| `clumsies_{commits,drafts,reviews}_created_last_hour` | Retained rows whose `created_at` falls between now minus 60 minutes and now. Future rows are excluded. Includes system/bootstrap Commits; deletion reduces the count. No `rate()` or `increase()` is applied. |
| `clumsies_drafts`, `clumsies_reviews` | Current row counts by lifecycle state, including zero for empty states. Merged/discarded Drafts are history, not backlog. |
| `clumsies_drafts_with_reconciliation_conflicts` | Open/submitted Drafts with a non-invalidated conflicting candidate matching their version, base commit and current main ref, following the Draft repository's validity checks. Unevaluated Drafts are excluded; zero is not proof that every Draft is conflict-free. |
| `clumsies_inbox_unread_records` | Stored unread receipts, including archived or inaccessible notifications. Not the visible inbox badge or a delivery failure. |
| `clumsies_oldest_open_draft_age_seconds` | Age since creation, not last edit, processing time or synchronization latency; zero if no open Draft exists. |
| `clumsies_db_active_backends`, `clumsies_db_longest_query_seconds` | Active sessions/queries in this database only, excluding the collector and idle transactions. |
| `clumsies_database_collection_success`, `clumsies_database_collection_timestamp_seconds` | Latest attempt's outcome and time. A failed query emits success=0 and omits database samples, preserving independent backup/release metrics. |

Open Draft net change is the six-hour fitted slope multiplied by 3600 to show
Drafts/hour. It includes state transitions and deletion, not just creation.
Recent-row counts are approximate activity snapshots, not durable event totals.
True event throughput or client enqueue-to-acknowledgment latency requires
instrumentation at those events; it cannot be reconstructed from table sizes.

Current host/database snapshots require an age between zero and 15 minutes.
Future event times remain visible as invalid, with their raw values retained.
`OperationalTimestampInvalid` warns after five minutes when a reachable node
reports a future collection, backup, restore, release, or unit-exit timestamp.
This does not apply to certificate expiry, which is a future deadline. A new
valid success can replace an invalid cached snapshot; existing future receipts
or archives still require investigation.

## Validate and update

Run `python3 deploy/observability/test.py` and
`python3 deploy/observability/test-delivery.py` from the repository root. The
checks use disposable databases and networks, the deployed image versions,
actual PromQL/LogQL, and real Grafana provisioning. They cover idle traffic,
old/new Server schema, cancelled requests, missing targets, SQL failure and
stale snapshots, JSON readiness, exact request IDs, secret redaction, and
configuration rollback. No production data or credentials are required.

Use the automated configuration delivery below for installed stacks. Deploy a
matching Server release before the dashboards so method/result classifications
and business lifecycle metrics become available. Update the installed
`/usr/local/sbin/clumsies-server-release` from the tested release helper to
require JSON readiness and persist successful restore receipts; retain the old
helper for recovery. Do not run the bootstrap installer again merely to replace
this executable, because it also configures deployment identities.

The managed configuration includes the two file-discovery catalogs,
Prometheus rules/configuration, Blackbox JSON-ready module, host collector,
Grafana provisioning and all eight dashboards. Existing Overview/API/Website
UIDs remain valid; panel links moved to Infrastructure must be updated.
No Server database migration or monitoring image upgrade is required.

Keep `.env`, notification credentials/routing, data volumes and unrelated files.
The deployment verifies all expected targets, four public probes, fresh
successful collection, and actual stored dashboard definitions. Never induce
slow traffic, collector failures or notification errors on production for these
checks. New classifications begin at rollout; historical results cannot be
backfilled. Wait a complete five-minute request window before reading new
percentiles. On failure, restore the managed configuration and collector from
the retained snapshot, then verify recovery without deleting volumes.

## Local performance diagnostics and load tests

Daemon Health optionally exposes a process-scoped performance snapshot with
fixed operation/result categories, started/in-flight counts, cumulative outcome
histograms, process identity and collection times. macOS diagnostics export
includes `performance.json` when available; older daemons remain compatible.
The snapshot contains no query text, resource IDs or credentials and is not
uploaded to Grafana. Process restart resets these counters.

Sync status separates successful Commit checks from Ref installation, and
reports the original oldest pending/failed operation times. Index status
provides a trustworthy current-backlog start and actual publication time;
legacy backlog without that start remains unknown. Attempt metrics and Server
ACK timings do not implement the durable, deduplicated write-freshness SLI.

`python3 dev/load-http.py --self-check` validates the bounded load scheduler.
The [local load instructions](../../dev/load-http.README.md) describe release
builds, authenticated read/read-write profiles, complete planned-operation
accounting and owned fixture cleanup. The tool accepts no external target.
Read the [design](../../docs/zh/performance/observability-plan.md) and
[runbook](../../docs/zh/performance/observability-runbook.md) for measurement
contracts and incident procedures.

## Server metric implementation

The Server uses the `prometheus` Rust library for collectors, histogram
aggregation and text encoding. `/metrics` remains private to the Compose
network. Response counters use registered route templates, bounded HTTP
methods, and actual status codes from 100 to 599 (other values become `other`).
The existing histogram bucket boundaries remain unchanged; histogram labels
now include method and status_class. Dashboards label legacy route-only
histograms as mixed outcomes, rather than treating them as successful latency.
Wait one complete five-minute query window after rollout before interpreting
the new distributions. Handler latency still excludes response body transfer.

Business request lifecycle metrics exclude health and `/metrics`: started,
in-flight, and terminal outcomes (`response` or `cancelled_or_aborted`). The
legacy global in-flight gauge includes those operational requests. A cancelled
future balances both gauges without recording a completed response or latency.

Connection acquisition metrics cover only `review_create` and `review_merge`:
attempt outcomes, acquisition duration, and in-flight attempts. Duration includes
waiting, validation and opening connections; it excludes BEGIN and transaction
execution. These series do not measure every database operation or pure queue
time.

`clumsies_db_pool_size` reports current open connections, preserving its
historical values; `clumsies_db_pool_max_connections` reports the configured
limit. `clumsies_db_pool_connections{state="idle"|"used"}` samples current pool
usage at scrape time. These pool readings are approximate concurrent snapshots,
not an atomic transaction across all gauges.

## Automated configuration delivery

`Observability Delivery` deploys changes under `deploy/observability/` after
main CI passes. Documentation and test-only edits do not deploy. It waits for
Site/Server delivery when those components change in the same push; a Server
image publication without a completed production deployment blocks dependent
observability delivery. The workflow serializes deployments, rejects outdated
configuration commits and records the successfully applied commit on the host.

The existing stack must first be installed using the bootstrap instructions
above. Delivery reuses `SITE_DEPLOY_HOST`, `SITE_DEPLOY_USER`,
`SITE_DEPLOY_SSH_KEY` and `DEPLOY_KNOWN_HOSTS`, with the existing `production`
environment protections. The SSH account needs noninteractive sudo to run the
configuration deployment, as it already manages production configuration.
Python 3, Docker Compose v2 and systemd must be available on the host. This is
not a new public endpoint, monitoring administrator credential, or SSH tunnel.

The workflow uploads the exact commit's files, validates them with the images
already running on the host, snapshots managed configuration, then applies it.
Only managed configuration directories and the Compose file are synchronized;
`.env`, data volumes, the existing Alertmanager notification configuration and
unrelated files are preserved. The Alertmanager template is for bootstrap;
editing it does not replace installed SMTP credentials or routing. Prometheus
receives SIGHUP; other changed services restart. Compose changes
use the running immutable image IDs with no pulls or builds. Adding/removing
services or upgrading images remains a separate infrastructure operation.

Delivery checks readiness, a healthy fresh scrape for every expected target in
`prometheus/targets.json` and `probes.json`, actual public probe success, fresh
successful host/database collection, Prometheus reload status, and the stored
content of all eight Grafana dashboards, including Grafana 13 unified storage.
Prometheus/Loki smoke queries accept empty request results. These are bounded checks, not
proof that every application path or external website is healthy. The existing
integration tests cover slow traffic and failure injection without production
data. A pre-existing monitoring outage can block delivery and require repair.

On application/verification failure, the script restores configuration and the
installed host collector, reloads affected services, verifies recovery, and
still fails the workflow. If recovery also fails, it reports that explicitly.
The latest pre-deployment snapshot is retained at
`/opt/clumsies/observability/.delivery/previous`; the successful commit is in
`.delivery/current`. No history volumes are removed. Hard host loss or SIGKILL
cannot execute rollback; use the retained snapshot for manual recovery. The
snapshot includes the old Compose configuration and running image IDs in
`images.json`, but deliberately excludes `.env`.

Use **Actions → Observability Delivery → Run workflow → main** to retry the
current configuration after its prerequisites have deployed. Manual runs also
validate configuration/tests. To roll back a successful rollout, revert the
configuration commit through a PR so the repository remains authoritative;
the resulting main push deploys the reverted files. For urgent recovery,
restore the snapshot's managed files into the existing directories (preserve
mounted directory inodes), restore `installed/` host files to their original
locations, and reload the affected services before reconciling Git.

Run `python3 deploy/observability/test-delivery.py` and
`python3 -m unittest discover -s dev -p test_ci_impact.py` for isolated delivery,
rollback, readiness, and change-selection tests. They never connect to the
production host. `bash deploy/observability/deploy.sh SSH_ALIAS` deploys the
current **committed** configuration to an explicitly chosen installed host.
