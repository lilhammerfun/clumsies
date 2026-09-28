# Observability

Optional Prometheus stack for one Clumsies installation. It runs beside the
three production containers and publishes every port on the host loopback
interface only.

| Component | What it reports |
|---|---|
| `prometheus` | Scrape, rule evaluation, and 90-day retention |
| `alertmanager` | Email delivery of firing alerts |
| `grafana` | Provisioned Prometheus and Loki datasources, and Overview, Business API, and Website Traffic dashboards in English and Chinese |
| `node-exporter` | Host CPU, memory, filesystem, and the textfile metrics below |
| `cadvisor` | Per-container CPU, memory, and restart activity |
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
grep -E 'GRAFANA_ADMIN_(USER|PASSWORD)' /opt/clumsies/observability/.env
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
database, backup and log-delivery health stay on Overview.

**Business API** uses Server route templates. Select a route to compare handler
p50/p95/p99, sample count, response codes and requests over one second. Health
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
   the other layer. An empty ID shows all logs in the selected time range.
4. If both entrance and handler are slow, investigate the backend. If only the
   entrance is slow, inspect proxy/body transfer. The two timers have different
   boundaries; their difference is not a measurement of network RTT.

Request IDs stay inside log lines; Loki stream labels remain `project`,
`service`, and `container`. `client_request_id` in Server logs can additionally
connect a client report to a Server request. Missing internal DB/external-call
spans cannot be reconstructed from an ID: add targeted timings when necessary.
A five-minute database snapshot can miss short query/connection waits.

Prometheus also scrapes Alloy and Loki. Overview shows send retries, dropped
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
above 2 GiB. Operations: no backup for 26 hours, no restore drill for 8 days, or
a failed backup unit. HTTP server errors above one percent for five minutes and
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
| `clumsies_{commits,drafts,reviews}_created_last_hour` | Retained rows whose `created_at` is in the last 60 minutes. Includes system/bootstrap Commits; deletion reduces the count. No `rate()` or `increase()` is applied. |
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

## Validate and update

From the repository root, run `python3 deploy/observability/test.py`. It uses
Docker Compose validation plus the shipped images' `promtool` and `amtool`, with
sample configuration and no production credentials. An ephemeral, isolated
PostgreSQL container applies the actual Server migrations and exercises the
installed collector with creation, deletion, zero counts, stale candidates and
SQL failure/recovery. The rule tests cover quiet workflow state and operational
failures firing and resolving, including failed or stopped collection. CI runs
the same check.

Monitoring configuration is deployed separately from Server and site delivery.
For an existing installation, back up the current configuration, then copy the
new `prometheus/rules.yml`, `compose.observability.yml`,
`host/clumsies-observability-metrics` and both `grafana/dashboards/*.json` files
into their matching paths under `/opt/clumsies/observability/`. Also back up the
installed `/usr/local/sbin/clumsies-observability-metrics` executable.
Preserve `.env`, data volumes, and the credential values in `alertmanager/alertmanager.yml`; merge only the Watchdog child route
from the template if it is missing.

The old `clumsies_{commits,drafts,reviews}_total` row-count counters,
`clumsies_reconciliation_candidates`, and `clumsies_inbox_unread` are retired.
Update custom queries to the metrics above. Historical series remain until
Prometheus retention expires; no history is rewritten or backfilled.
Install the collector and dashboards together, before loading the new health
rule. No application schema migration is needed for this monitoring update.

On the installation host:

```bash
cd /opt/clumsies/observability
sudo install -m 0755 host/clumsies-observability-metrics /usr/local/sbin/clumsies-observability-metrics
sudo systemctl start clumsies-observability-metrics.service
sudo systemctl is-active clumsies-observability-metrics.timer
sudo grep '^clumsies_database_collection_' /var/lib/clumsies-observability/textfile/clumsies.prom
sudo docker compose --file compose.observability.yml --env-file .env config -q
sudo docker compose --file compose.observability.yml --env-file .env exec prometheus \
  promtool check config /etc/prometheus/prometheus.yml
sudo docker compose --file compose.observability.yml --env-file .env exec alertmanager \
  amtool check-config /etc/alertmanager/alertmanager.yml
sudo docker compose --file compose.observability.yml --env-file .env up -d --no-deps --pull never prometheus alertmanager
```

Confirm `clumsies_database_collection_success` is 1 and the collection timestamp
is current before proceeding; the service also writes failure=0 on SQL errors.
Grafana's file provider reloads the dashboard JSON within 30 seconds.
Recreation is needed to apply the external URL flags. Confirm the updated rules are
loaded in Prometheus, the five workflow alerts are absent, the Watchdog route
repeats daily, and newly generated links work through the tunnel. An existing
workflow alert may send a final resolved email. To roll back, restore the
backed-up files and collector executable, run the collector once, and repeat
validation and recreation without deleting volumes. Restore both dashboards
with the collector so their metric names stay aligned.

## Deploying the request dashboards

These changes are configuration/code only; no database migration or new
service is needed. Validate with `python3 deploy/observability/test.py` and
`cargo test -p server --lib metrics::tests`. The request test uses an isolated
Docker network, synthetic backend and static site, real Caddy/Alloy/Loki, and
synthetic PromQL series. It verifies site isolation, probe exclusion, idle
windows, original paths, secret redaction and cross-layer request correlation.

Back up and deploy the following together:

- Production `deploy/Caddyfile`: validate with `caddy validate`, then use Caddy's
  config reload. Existing traffic need not be interrupted.
- Server release containing the extended histogram buckets. During the first
  five minutes after rollout, queries may mix old/new bucket layouts; wait a
  full window before interpreting the new tail estimate.
- `prometheus/prometheus.yml`, `rules.yml`: validate with `promtool` then reload
  Prometheus. This adds Alloy/Loki targets and changes errors to per-site scope.
- All six `grafana/dashboards/*.json` and
  `grafana/provisioning/datasources/loki.yml`: dashboards reload automatically;
  restart Grafana or reload datasource provisioning for request links.

Keep the existing Overview UIDs so saved links remain valid. The old global
latency panels are replaced, not relabeled as per-site history. Host-tagged
metrics and newly covered website logs begin only after deployment; historical
site separation cannot be backfilled. Preserve credentials and storage volumes.
Roll back the backed-up config and Server release together if validation fails.
Check all four hosts, an API request, its derived log link, and Alloy/Loki scrape
health after rollout. Never induce slow traffic or delivery failures in production
just to test these scenarios.

## Server metric implementation

The Server uses the `prometheus` Rust library for collectors, histogram
aggregation and text encoding. The existing `/metrics` endpoint, route/status
labels and latency bucket boundaries remain unchanged. Requests cancelled
before a response decrement the in-flight gauge without recording a completed
response. Handler latency still excludes response body transfer.

`clumsies_db_pool_size` reports current open connections, preserving its
historical values; `clumsies_db_pool_max_connections` reports the configured
limit. `clumsies_db_pool_connections{state="idle"|"used"}` samples current pool
usage at scrape time. These pool readings are approximate concurrent snapshots,
not an atomic transaction across all gauges.
