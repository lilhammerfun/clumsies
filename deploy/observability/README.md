# Observability

Optional Prometheus stack for one Clumsies installation. It runs beside the
three production containers and publishes every port on the host loopback
interface only.

| Component | What it reports |
|---|---|
| `prometheus` | Scrape, rule evaluation, and 90-day retention |
| `alertmanager` | Email delivery of firing alerts |
| `grafana` | Provisioned Prometheus datasource and the *Clumsies Overview* dashboard |
| `node-exporter` | Host CPU, memory, filesystem, and the textfile metrics below |
| `cadvisor` | Per-container CPU, memory, and restart activity |
| `postgres-exporter` | Connections, transactions, and database size |
| `blackbox-exporter` | Public HTTPS probes for the app, docs, official site, and the `www` alias |
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

The `Watchdog` rule always fires, so a working pipeline produces a periodic
"everything is fine" email. Remove the rule if that is unwanted noise.

## Alert rules

Availability: a public endpoint down for two minutes, a certificate expiring
within 14 days, or a scrape target down. Capacity: a filesystem below 15 percent
free or host memory below 10 percent available. Application: PostgreSQL
unreachable, more than 80 percent of connections in use, or a Clumsies container
above 2 GiB. Operations: no backup for 26 hours, no restore drill for 8 days, or
a failed backup unit.
