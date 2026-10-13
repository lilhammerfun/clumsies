# Clumsies Observability Installation and Operations

This guide serves operators installing and maintaining the optional monitoring stack for one Clumsies installation. It covers bootstrap, access, notification setup, configuration delivery and recovery. For measurement contracts and incident procedures, use the [design](../../docs/performance/observability-plan.md) and [operations runbook](../../docs/performance/observability-runbook.md).

## Services and network

The stack runs beside the production containers. Monitoring ports bind to host loopback; Caddy and Server metrics are reachable within the Compose network.

| Component | Role |
| --- | --- |
| `prometheus` | Scrapes/evaluates every 30 seconds by default; metrics retained 90 days |
| `alertmanager` | Groups firing alerts and delivers email |
| `grafana` | Overview, Business API, Entrance and Websites, Infrastructure and Operations; English and Chinese, eight dashboards |
| `node-exporter` | Host resources and collector textfile |
| `cadvisor` | Container CPU and memory |
| `postgres-exporter` | Database connections, transactions, size and reachability |
| `blackbox-exporter` | HTTPS probes for App, docs, official site and www |
| `loki` | Container logs retained 14 days |
| `alloy` | Ships Docker container logs to Loki |
| `server` (target) | Classified HTTP metrics, API request lifecycle and database pool |
| `clumsies-observability-metrics.timer` | Five-minute database, backup, restore and release snapshots |

The production `deploy/Caddyfile` enables per-host metrics on the internal admin endpoint:

```caddyfile
{
    admin :2019
    metrics {
        per_host
    }
}
```

`compose.production.yml` publishes ports 80 and 443. Server `/metrics` stays private to the Compose network; Caddy returns 404 for it on the public origin. Metric labels use registered route templates, bounded methods and response categories. IDs and paths stay in log fields.

## First installation

The production stack, Python 3, Docker Compose v2 and systemd must be available. Copy the monitoring configuration once, fill `.env` from the example, then install the collector and start services:

```bash
sudo install -d -m 0700 /opt/clumsies/observability /var/lib/clumsies-observability/textfile
sudo cp -r deploy/observability/. /opt/clumsies/observability/
cd /opt/clumsies/observability
sudo cp .env.example .env
# Set GRAFANA_ADMIN_PASSWORD and CLUMSIES_POSTGRES_DSN in .env.
sudo chmod 0600 .env
sudo install -m 0755 host/clumsies-observability-metrics /usr/local/sbin/clumsies-observability-metrics
sudo install -m 0644 host/clumsies-observability-metrics.service /etc/systemd/system/
sudo install -m 0644 host/clumsies-observability-metrics.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now clumsies-observability-metrics.timer
sudo chmod 0755 /var/lib/clumsies-observability /var/lib/clumsies-observability/textfile
sudo docker compose --file compose.observability.yml --env-file .env up -d --wait
```

The textfile collector runs as a host service; node-exporter reads its output unprivileged. Keep both output directories traversable and `.prom` files readable. Grafana administrator environment values initialize the first startup; an installed administrator account uses its existing credentials.

### Email notifications

On first installation, replace the Alertmanager template placeholders with the sender account, recipient and Google app password. Use the account's two-step-verification/app-password settings.

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

Restrict the installed credential file to Alertmanager's unprivileged user, validate and restart:

```bash
sudo chown 65534:65534 alertmanager/alertmanager.yml
sudo chmod 0400 alertmanager/alertmanager.yml
sudo docker compose --file compose.observability.yml --env-file .env exec alertmanager   amtool check-config /etc/alertmanager/alertmanager.yml
sudo docker compose --file compose.observability.yml --env-file .env restart alertmanager
```

Normal alerts wait 30 seconds initially, send group changes every five minutes, repeat every four hours and notify on recovery. Warning and critical share the receiver. Watchdog always fires, waits ten seconds initially and repeats every 24 hours; its receipt at the destination verifies email delivery.

## Access and navigation

Forward the loopback services through SSH:

```bash
ssh -N -L 3000:127.0.0.1:3000 -L 9090:127.0.0.1:9090 -L 9093:127.0.0.1:9093 <installation-host>
```

| Page | Address and use |
| --- | --- |
| Grafana | `http://localhost:3000/`, trends and request logs |
| Prometheus Alerts | `http://localhost:9090/alerts`, rule expressions and pending/firing |
| Alertmanager Alerts | `http://localhost:9093/#/alerts`, groups and silences |

Compose sets external URLs to `http://localhost:9090` and `http://localhost:9093`, so email links use the local tunnel. Keep it open; forwarding different ports requires matching external-URL configuration. Existing delivered links retain their original address. Prometheus owns rules and thresholds; Alertmanager Status shows loaded notification routing.

| Dashboard | Work supported | Navigation |
| --- | --- | --- |
| Overview | Source confidence, API request throughput, successful handler latency, errors, API concurrency and site entrance | Business API, selected site, Infrastructure |
| Business API | Current method/route metrics, slow/busy routes, lifecycle, request correlation and collapsed product activity | App entrance to website W5; dependencies to Infrastructure |
| Entrance and Websites | One host selector for App/docs/site/www, entrance metrics and access logs | Request correlation to API A7 |
| Infrastructure and Operations | Expected targets, collection, database/resources, task results, success times, certificates, logs, notifications and actual Server/release information | Prometheus, Alertmanager, runbook |

Summary cards show a single metric, with the object in the title and the result in the body. Scraping status, snapshot status, collection age, success time and record validity use separate cards; comparable percentiles, sites and routes share time-series charts. Durations, recorded times, rates, CPU cores and days retain their own units.

Page links preserve time and applicable filters. For a log, expand **Related request logs** to carry its real ID and time into API A7. Enter complete IDs manually in the API Request ID field. That shared view matches Caddy and Server across site/route/method; empty IDs return no logs and prefixes do not match. Static requests have entrance records only.

Local server_request_id identifies the entrance/Server request directly. A client_request_id first identifies Server records; use their internal request_id for the entrance. One client identity may span attempts. This provides log correlation; full tracing requires a separate trace-context contract.

## Reading metrics and logs

### HTTP and dependencies

API request scope excludes health and metrics routes, retains unmatched requests and includes background polling and retries. Responses/s count returned handlers; logical client operations use their own completion events. A revision read followed by an update is two HTTP requests and one logical update in the load report.

| Metric group | Contract |
| --- | --- |
| Entrance | Caddy `handler="subroute"` selects one site-handling layer; includes synthetic probes and excludes automatic HTTP-to-HTTPS redirects |
| Response counter | `clumsies_http_requests_total{route,method,status}`; actual codes 100–599, otherwise `other`; daily API request queries require method and exclude health/metrics |
| Handler duration | `clumsies_http_request_duration_seconds{route,method,status_class}`; success 2xx/3xx, errors 4xx/5xx/other; ends at returned Response, with body transfer afterward |
| API request lifecycle | started, response/cancelled_or_aborted termination, in-flight; excludes `/metrics` and `/api/v1/admin/health`; cancellation balances in-flight without a response/duration sample |
| Review acquisition | `review_create` and `review_merge` outcomes, duration and concurrency; includes wait, validation and opening, with BEGIN/transaction execution separate |
| Pool | `clumsies_db_pool_size` is open connections; max_connections is configured limit; idle/used are approximate concurrent snapshots |

Request percentiles are rolling five-minute histogram estimates with matching sample counts. Server finite bounds are 5, 10, 25, 50, 100, 250, 500 ms and 1, 2.5, 5, 10, 30 seconds. Low-sample p99 needs its count; idle windows have no latency sample. Apply rate/increase to counters before aggregation; increase can be fractional. The one-second slow-log filter identifies investigation candidates. Entrance, handler and browser/client timings retain separate boundaries.

Caddy logs preserve site, original path, status, response size, duration in milliseconds and request_id. App records add upstream_duration_ms and upstream_latency_ms. Headers, URL query strings and bodies are excluded. Loki stream labels remain Compose project, service and container. The [runbook](../../docs/performance/observability-runbook.md) defines same-request investigation and local diagnostic procedures.

### Database and collection

The host timer samples every five minutes. Database metrics are gauges:

| Metric | Meaning |
| --- | --- |
| `clumsies_{commits,drafts,reviews}_created_last_hour` | Retained rows with `now()-60min ≤ created_at ≤ now()`, including bootstrap/system Commits; deletion reduces counts |
| `clumsies_drafts`, `clumsies_reviews` | Current counts by lifecycle state, including zero for empty states |
| `clumsies_drafts_with_reconciliation_conflicts` | Open/submitted Drafts with a valid non-invalidated conflicting candidate matching version, base and current main ref; unevaluated work is outside this count |
| `clumsies_inbox_unread_records` | Stored unread receipts, including archived/inaccessible notifications |
| `clumsies_oldest_open_draft_age_seconds` | Age since creation; zero when no open Draft exists |
| `clumsies_db_active_backends`, `clumsies_db_longest_query_seconds` | Active sessions/queries in this database, excluding the collector and idle transactions |
| `clumsies_database_collection_success`, `clumsies_database_collection_timestamp_seconds` | Latest SQL attempt result/time; failure emits success=0 and removes database samples |

Open Draft net change is a six-hour fitted inventory slope multiplied by 3600, displayed as Drafts/hour. Retained-row gauges describe activity and state; event throughput and enqueue-to-ACK latency need event measurements. Five-minute query snapshots describe sampled state, including their coverage gaps for short waits.

Usable current snapshots require SQL success and both host/database ages in 0–900 seconds. Future operational event times are invalid with raw values preserved. Snapshot display distinguishes valid, failed, stale/missing and invalid time. Certificates use future expiry as a deadline.

Backup/restore task states distinguish success, failed, never, missing and running. Latest result is independent of last-success history. Backup/restore age, original success time and confidence are separate fields; release time and confidence follow the same separation. A new valid success can repair an invalid future cache; a still-future source receipt/archive remains visible for investigation.

### Targets and alerts

Discovery catalogs `prometheus/targets.json` and `prometheus/probes.json` define thirteen targets: nine service targets and four public probes. Expected-target recording rules share this catalog contract; target health distinguishes up, scrape failure and missing discovery. App readiness requires HTTP 200 with JSON status=ok; static sites use the HTTPS module.

| Alert group | Trigger and duration |
| --- | --- |
| Availability | Probe failure 2 minutes; failed/missing scrape 5 minutes; certificate below 14 days for 1 hour |
| Resource risks | Disk available below 15%, host memory available below 10%, PostgreSQL connections above 80%, or Clumsies working set above 2 GiB, each for 10 minutes |
| Database | pg_up=0 for 2 minutes; query above 60 seconds in valid snapshots for 5 minutes |
| HTTP errors | Per-site rolling five-minute 5xx above 1% for 5 minutes |
| Collection | Missing/failed/stale/invalid host or database snapshot for 5 minutes |
| Operational time | OperationalTimestampInvalid: a reachable node reports host/DB/backup/restore/release/unit-exit time later than Prometheus time for 5 minutes |
| Logs and notifications | Send retries for 5 minutes; dropped entries or failed notifications within the last 5 minutes |
| Backup and restore | Missing/invalid/older-than-26-hour backup for 30 minutes; missing/invalid/older-than-8-day drill for 1 hour; failed/unavailable units for 5 minutes |

Read error counts with ratios at low traffic. Product inventories remain informational; alert ownership and an actionable objective are prerequisites for adding product alerts. The 2 GiB memory rule is an absolute reminder, with configured limits evaluated separately. Same-host monitoring and the single notification channel require an external observer for whole-host or channel loss.

## Validate configuration

From the repository root:

```bash
python3 deploy/observability/test.py
python3 deploy/observability/test-delivery.py
python3 -m unittest discover -s dev -p test_ci_impact.py
```

The isolated checks use disposable databases/networks, configured image versions, actual PromQL/LogQL and Grafana provisioning. They cover idle traffic, cancellation, method/result isolation, missing targets, SQL failure, invalid/stale time, JSON readiness, exact IDs, redaction, configuration rollback and change selection. Production acceptance uses readiness and observed traffic; failure injection remains in the isolated fixtures.

## Configuration delivery

### Prerequisites and managed files

`Observability Delivery` applies relevant `deploy/observability/` changes after main CI succeeds. Documentation/test-only changes leave the installed configuration unchanged. When Site or Server changes in the same push, delivery waits for the corresponding successful production deployment; image publication alone leaves the dependency unsatisfied. Runs serialize, reject stale configuration commits and record the applied commit.

The installed stack uses `SITE_DEPLOY_HOST`, `SITE_DEPLOY_USER`, `SITE_DEPLOY_SSH_KEY`, `DEPLOY_KNOWN_HOSTS` and existing `production` environment protections. The SSH account needs noninteractive sudo; the host needs Python 3, Compose v2 and systemd.

| Managed by delivery | Preserved installation state |
| --- | --- |
| Compose file, Prometheus catalogs/rules/config, Blackbox, Loki, Alloy, Grafana provisioning/eight dashboards, host collector/service/timer | `.env`, existing Alertmanager SMTP credentials/routing, data volumes and unrelated files |

The Alertmanager source template initializes bootstrap configuration. Installed notification configuration remains under operator control. Delivery uses running immutable image IDs without pulls/builds; service additions/removals and image upgrades are separate infrastructure operations.

### Apply and accept

1. Upload the exact committed configuration and validate it using the running image versions.
2. Snapshot managed files and installed host files; apply changes while preserving mounted directory inodes.
3. Reload Prometheus with SIGHUP and restart other changed services; pin Compose to existing image IDs.
4. Verify fresh healthy scrapes for all expected targets, four successful probes, valid host/database collection, Prometheus reload and all eight stored Grafana definitions/mounted files, including Grafana 13 unified storage.
5. Run Prometheus/Loki smoke queries and record `.delivery/current` after success. Empty request results are valid for idle traffic.

Stored Grafana definition checks use the existing installation state. Administrator credential mismatch is investigated through the installed account; configuration validation preserves credentials. Acceptance verifies the monitoring contract, with application-path and capacity coverage supplied by their own tests. A pre-existing source outage may need repair before delivery passes.

Retry with **Actions → Observability Delivery → Run workflow → main** after prerequisites are ready. Manual runs also validate configuration/tests. An explicit installed host can receive the current committed configuration through:

```bash
bash deploy/observability/deploy.sh SSH_ALIAS
```

### Failure and rollback

Application or verification failure restores managed configuration and installed host files, reloads services, verifies recovery and fails the workflow. Recovery failure is reported separately. The latest pre-deployment snapshot is `/opt/clumsies/observability/.delivery/previous`; successful commit identity is `.delivery/current`. The snapshot contains old Compose configuration and running image IDs in `images.json`; `.env` and history volumes remain in place.

For rollback after a successful delivery, revert the configuration commit through a PR and let the resulting main push apply it. For urgent recovery, restore snapshot files into existing directories, restore `installed/` host files to original locations, reload affected services and reconcile Git afterward. Host loss or SIGKILL may interrupt automatic recovery; the retained snapshot supports manual repair.

## Local diagnostics and load measurements

Daemon Health exposes optional process-scoped performance counts, in-flight work and outcome histograms with process/collection identity. macOS diagnostics exports local `performance.json` when available, with fixed categories and no query text, resource IDs or credentials. Two same-process snapshots define an interval; restart resets counters.

Activate measures daemon time through call return, including history acknowledgment waiting. Query and normal acknowledgment wait share a 60-second deadline established after parameter validation; accepted history persistence can continue after the call returns. Inspect the Run terminal state and history_lock_wait/history_persistence separately. Run.total_us ends at query completion; initial Run creation, timeout terminalization and failure recording retain their own waits.

Project Commit checks, Ref installation, original pending/failed enqueue times, index backlog start and atomic publication have separate fields. The complete durable, deduplicated five-minute write-freshness SLI remains to implement.

```bash
python3 dev/load-http.py --self-check
```

The [load tool instructions](../../dev/load-http.README.md) cover release builds, authenticated read/read-write profiles, all-planned-operation accounting and owned loopback cleanup. The tool accepts only its own local target. The [load report](../../docs/performance/observability-load-report.md) preserves measured conditions and raw evidence.

## Appendix: Compatibility and upgrade records

Daily views use current route/method/status_class and API request lifecycle metrics. Historical route-only histograms contain mixed outcomes; global in-flight includes operational requests. Historical observations retain their original interpretation. Allow one complete five-minute window after a Server upgrade before reading classified latency. Older daemons without performance or durable backlog starts report unavailable/unknown age through compatible decoding.

For an older installation, update the installed `/usr/local/sbin/clumsies-server-release` from the tested helper to obtain JSON readiness checks and atomic successful restore receipts. Retain the prior executable for recovery. This helper has its own installation path, outside observability configuration delivery; the bootstrap installer also configures deployment identities and is reserved for installation. Existing success records, notification credentials, `.env` and data volumes remain preserved through upgrades.
