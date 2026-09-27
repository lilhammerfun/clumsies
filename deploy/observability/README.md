# Observability

Optional Prometheus stack for one Clumsies installation. It runs beside the
three production containers and publishes every port on the host loopback
interface only.

| Component | What it reports |
|---|---|
| `prometheus` | Scrape, rule evaluation, and 90-day retention |
| `alertmanager` | Email delivery of firing alerts |
| `grafana` | Provisioned Prometheus and Loki datasources, and the *Clumsies Overview* dashboards in English and Chinese |
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
	metrics
}
```

The admin endpoint stays reachable only from the Compose network because
`compose.production.yml` publishes nothing but ports 80 and 443.

The Server exposes Prometheus metrics on `/metrics` for the same network. Its
route labels are registered route templates and its status labels are classes,
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

## Logs

Both dashboards carry a *Logs* row with a `request id` text box. Every response
from the Server returns the edge request id in the `x-request-id` header, so one
value follows a request through the whole installation:

1. paste the header value into `request id`; the edge panel and the Server panel
   then show every line for that request, from both containers;
2. a client that sends `x-clumsies-request-id` appears as `client_request_id` in
   the Server log, which links a user report to the same request.

Labels stay low-cardinality (`project`, `service`, `container`). Request ids live
inside the log lines and are matched with a line filter, never used as a label.
The existing logging invariant still holds: no bodies and no credentials are
recorded, and Caddy already drops request headers and query strings.

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
database queries above one minute for five minutes also alert.

Draft age/count, Commit throughput, reconciliation conflicts, and unread inbox
items remain in the collector and Grafana dashboards. They do not page the
operator: an open Draft can still be used and updated in its Project, reading
does not require publication, and human work has no installation-wide response
deadline. Add an alert only when a measured failure or an agreed service
objective gives its recipient a concrete action. Commit counts cannot measure
client synchronization latency.

## Validate and update

From the repository root, run `python3 deploy/observability/test.py`. It uses
Docker Compose validation plus the shipped images' `promtool` and `amtool`, with
sample configuration and no production credentials or running stack. The rule
tests cover long-lived workflow state without emails and database failures
firing and resolving. CI runs the same check.

Monitoring configuration is deployed separately from Server and site delivery.
For an existing installation, back up the current configuration, then copy the
new `prometheus/rules.yml` and `compose.observability.yml` into
`/opt/clumsies/observability/`. Preserve `.env`, data volumes, and the credential
values in `alertmanager/alertmanager.yml`; merge only the Watchdog child route
from the template if it is missing.

On the installation host:

```bash
cd /opt/clumsies/observability
sudo docker compose --file compose.observability.yml --env-file .env config -q
sudo docker compose --file compose.observability.yml --env-file .env exec prometheus \
  promtool check config /etc/prometheus/prometheus.yml
sudo docker compose --file compose.observability.yml --env-file .env exec alertmanager \
  amtool check-config /etc/alertmanager/alertmanager.yml
sudo docker compose --file compose.observability.yml --env-file .env up -d --no-deps --pull never prometheus alertmanager
```

Recreation is needed to apply the external URL flags. Confirm 14 rules are
loaded in Prometheus, the five workflow alerts are absent, the Watchdog route
repeats daily, and newly generated links work through the tunnel. An existing
workflow alert may send a final resolved email. To roll back, restore the
backed-up files and repeat validation and recreation without deleting volumes.
