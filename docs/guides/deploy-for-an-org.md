# Deploy for an organization

This guide deploys one self-hosted Clumsies organization. The Rust Server image
contains only APIs and OIDC endpoints; setup and administration live in the
macOS App. Organization memory is the Memory section's organization scope in
Desktop and is not a deployed process.

## Runtime boundary

Production runs three containers:

- PostgreSQL stores every authority, draft, review, identity, audit, Blob,
  Tree, Commit, and Ref;
- Server runs migrations, the Public API, bearer Admin API, public health,
  memory export, and OIDC;
- Caddy terminates public HTTPS and proxies Server.

Only Caddy publishes host ports. Server and PostgreSQL stay on the Compose
network.

## Prerequisites

- Docker Engine and Docker Compose v2 or newer;
- a public HTTPS hostname;
- an OIDC confidential client with the callback below registered at the IdP;
- a published Clumsies Server image pinned by digest.

```text
https://memory.example.com/login/oauth2/code/oidc
```

Production must not use the legacy Python `docker-compose` command. The release
script requires Compose v2 or newer and rejects every older major version.

## Configure

Copy `.env.example` to `.env`, restrict it to the installation administrator,
and replace every placeholder. In particular:

```dotenv
CLUMSIES_SERVER_IMAGE=ghcr.io/lilhammerfun/clumsies-server@sha256:published-digest
CLUMSIES_PUBLIC_ORIGIN=https://memory.example.com
CLUMSIES_DB_PASSWORD=replace-with-a-random-password
CLUMSIES_SETUP_CODE=replace-with-at-least-32-random-characters
CLUMSIES_OIDC_ISSUER=https://identity.example.com
CLUMSIES_OIDC_CLIENT_ID=replace-with-oidc-client-id
CLUMSIES_OIDC_CLIENT_SECRET=replace-with-oidc-client-secret
CLUMSIES_CLIENT_REDIRECT_URIS=http://127.0.0.1/callback
```

`CLUMSIES_CLIENT_REDIRECT_URIS` is the post-provider allowlist for Clumsies
clients. The loopback template accepts Desktop's dynamic port only at the exact
callback path. Server derives the IdP callback from
`CLUMSIES_PUBLIC_ORIGIN`. The native App calls Server directly and no browser UI
is served, so the deployment needs no CORS origin configuration.

When the host requires an outbound proxy, configure Docker Engine and the
standard proxy variables in `.env`. Deployment-specific proxy or mirror
addresses are not embedded in the image.

## Start and initialize

```bash
docker compose --project-name clumsies -f compose.production.yml up -d --wait
curl --fail --silent https://memory.example.com/api/v1/admin/health
```

Open the macOS App and enter `https://memory.example.com` as the Server address.
The App accepts a remote address only as an HTTPS origin and detects that setup
is required. Enter the one-time Setup Code, organization name, default Project,
and optional email domains, then finish OIDC in the system browser. The App and
Server use state plus `S256` PKCE; Server creates the organization, first Owner,
default Project, external identity, and initial Refs in one transaction before
returning a client authorization code. The App exchanges it for bearer tokens,
installs them in daemon, and the installation permanently locks. Remove
`CLUMSIES_SETUP_CODE` from the active `.env` after setup.

Later organization configuration, membership, Projects, tokens, audit, and
health are managed in the App's **Administration** section. The Server keeps
the Admin API bearer-only; `/api/v1/admin/health` remains available for
deployment diagnostics and `/api/v1/admin/memory-export` remains the
authenticated migration export.

If daemon startup fails, choose **Administrator Recovery** in the App. It signs
in directly to the same trusted Server origin and keeps the temporary session
in App memory only so an administrator can inspect health, repair member
access, or revoke tokens before retrying normal startup.

## GitHub delivery

`CI` calls `.github/workflows/server-delivery.yml` after its `build` gate succeeds
on `main`, only when the change affects Server delivery. The reusable workflow
builds the exact validated commit for `linux/amd64` and `linux/arm64`, publishes
to GHCR with OCI source/revision labels and provenance, and deploys the manifest
digest. The same image architectures are build-checked without publishing on PRs.

Delivery is serialized and rejects a commit if newer Server changes already
exist on `main`; intervening unrelated docs changes do not discard the pending
Server update. If both site and Server delivery are selected, site delivery
first synchronizes their shared Compose/Caddy configuration. A manual dispatch
accepts only an existing immutable digest and its full commit for retry or rollback.

Site delivery builds the documentation and the official site, syncs them to
`/srv/docs` and `/srv/www`, copies the Compose file and `deploy/Caddyfile`,
and reloads Caddy so a synced configuration takes effect on the running
container.

The GHCR package is linked to this repository through its OCI source label.
Make the package public once so self-hosted installations can pull it without a
personal token. GitHub documents both [anonymous pulls for public container
packages](https://docs.github.com/en/packages/learn-github-packages/configuring-a-packages-access-control-and-visibility)
and the recommended [`GITHUB_TOKEN` publishing
flow](https://docs.github.com/en/actions/tutorials/publish-packages/publish-docker-images).

Create a GitHub Environment named `production` with these secrets:

| Secret | Value |
|---|---|
| `DEPLOY_HOST` | SSH hostname or IP of the installation |
| `DEPLOY_USER` | `clumsies-deploy` |
| `DEPLOY_SSH_KEY` | Dedicated Ed25519 private key used only by Actions |
| `DEPLOY_KNOWN_HOSTS` | Pinned SSH host-key line for `DEPLOY_HOST` |

Set repository variable `SERVER_AUTO_DEPLOY_ENABLED=false` during bootstrap.
After the image package is public and the restricted deploy identity has been
tested, set it to `true`. Future green `main` commits then deploy automatically.

Do not upload a personal or root SSH key. Generate a dedicated key, copy the
public half to the host, install Compose v2, then run the installer from a
trusted release checkout:

```bash
sudo apt-get install --yes docker-compose-v2
sudo deploy/server/install.sh /path/to/github-deploy-key.pub
```

The installer creates `clumsies-deploy`. Its `authorized_keys` entry disables
PTY, forwarding, and user rc files and forces `clumsies-github-command`. The
command accepts only:

```text
deploy ghcr.io/lilhammerfun/clumsies-server@sha256:<64 hex> <40 hex commit>
```

The account can invoke only the validated release command through `sudo`; it
cannot obtain an interactive deployment shell.

## Release transaction

`clumsies-server-release deploy` performs the following operation under an
exclusive host lock:

1. validate Compose v2 or newer, the digest, commit, current configuration, and
   public origin;
2. pull the immutable image and render the Compose configuration;
3. create and validate an online PostgreSQL backup;
4. restore that backup into isolated PostgreSQL, start the target image against
   it, run its real SQLx migrations, and require Server health;
5. stop the current Server so no writes can occur during the cutover;
6. create a second, write-free PostgreSQL backup and verify it with `pg_restore`;
7. atomically persist the desired image digest, start only Server, and require
   both container and public HTTPS health;
8. record the commit, target and previous images, both backups, timestamp, and result.

Health is probed through the local edge by resolving the public origin to
loopback, so a stale resolver cache or an in-progress DNS move cannot roll back a
healthy release. When public DNS does not reach this host yet, the release still
succeeds and logs a warning.

If target container or public health fails after cutover, the script stops the
target Server, replaces the production database from the write-free backup,
then starts and verifies the previous image. Database and application rollback
are one operation. Released migrations therefore do not need to remain readable
by the previous Server image; destructive migrations still need migration tests,
but they do not require a compatibility implementation.

To retry a delivery, dispatch `Server Delivery` with its published digest and
original commit. Do not treat an older image as a standalone rollback after a
destructive migration: restoring such a release requires its recorded
pre-deploy database backup and previous image as one recovery operation.
Production never rebuilds source code.

Changing the canonical origin is a separate configuration transaction:

```bash
sudo clumsies-server-release reconfigure \
  https://memory.example.com \
  http://127.0.0.1/callback
```

The command backs up PostgreSQL and the active environment file, validates the
rendered Compose configuration, recreates Server and Caddy, waits for container
and public HTTPS health, and restores the previous environment and services if
the new origin fails.

## Backup and restore

The installer enables:

- `clumsies-backup.timer`: daily custom-format backup, verification, checksum,
  and 14-day local scheduled-backup retention;
- `clumsies-restore-drill.timer`: weekly restore into an isolated PostgreSQL and
  Server stack, followed by the real Server health check and automatic cleanup.

Run either operation explicitly:

```bash
sudo clumsies-server-release backup manual
sudo clumsies-server-release restore-drill
```

Backups and deployment records live under `/opt/clumsies/backups` and
`/opt/clumsies/releases`. Local retention is not disaster recovery. Configure
encrypted off-host storage appropriate to the installing organization and test
restoration from that copy; do not place database dumps in the source
repository or ordinary GitHub Actions artifacts.

## Observability

`deploy/observability` ships an optional Prometheus stack for one installation:
Prometheus, Alertmanager, Grafana, node-exporter, cadvisor, postgres-exporter, and
blackbox probes for the public endpoints. It reports host and container
resources, Server request rate and latency from Caddy, PostgreSQL connections and
size, backup and restore-drill freshness, and certificate expiry. Every port
binds to loopback, and alert delivery uses SMTP through Alertmanager.

The production `deploy/Caddyfile` opens a global block so Caddy exposes its HTTP
metrics on the admin endpoint of the Compose network, and answers 404 for the
Server's own `/metrics` route so the scrape endpoint stays internal. Install steps, the
required `.env`, the alert channel configuration, and the alert rules are
documented in [`deploy/observability/README.md`](https://github.com/lilhammerfun/clumsies/blob/main/deploy/observability/README.md).

## Service objectives

An installation is healthy when these hold. The numbers stay modest on purpose:
a single host without redundancy cannot honestly promise more nines.

| Objective | Target | Evidence |
|---|---|---|
| Public availability | external probes succeed 99.5 percent of the time over 30 days | `probe_success` from the blackbox exporter |
| Write freshness | 99 percent of client changes reach the Server within five minutes | `clumsies_commits_total` against client activity in the edge logs |
| Durability | recovery point at most 26 hours old, recovery time at most one hour | backup freshness metric and the weekly restore drill |
| Request latency | p95 below one second on the app origin | `caddy_http_request_duration_seconds` |
| Server errors | fewer than 1 percent of requests answer with a server error | `caddy_http_request_duration_seconds_count` by code |

Alert thresholds follow these targets instead of local judgement: the backup
rule fires at 26 hours, the error-rate rule at one percent. Queueing signals
(open drafts, reconciliation conflicts, unread notifications) are backlog
reporting until an objective exists for them.

Revisit the targets when the architecture changes. Availability above 99.5
percent needs redundancy that one host cannot provide, and the deployment has to
say so before anyone promises it.

## Documentation site

The docs site (`docs.clumsies.ai`) and the official site (`clumsies.ai`) are
static sites served by the same Caddy instance from `/srv/docs` and `/srv/www`
respectively (see `deploy/Caddyfile`). They live in this repository: the
VitePress sources under `docs/`, the official site under `site/` (plain
HTML/CSS, no build step).

### Deploy through CI/CD

`CI` calls `Site Delivery` (`.github/workflows/site-delivery.yml`) after the
selected checks pass on `main`. Changes to `docs/`, `site/`, Bun dependencies,
`deploy/site.sh`, Caddy/Compose configuration, and CI delivery policy select it.
README and screenshot changes alone do not deploy the site. The workflow builds
and deploys the exact tested commit, serializes site deliveries, and skips a
commit superseded by newer site changes. Manual dispatch remains available
from the Actions tab for an explicit retry or rollback.

The workflow needs these repository secrets in addition to the Server
Delivery secrets:

| Secret | Value |
|---|---|
| `SITE_DEPLOY_HOST` | SSH hostname or IP of the installation (same host as `DEPLOY_HOST`) |
| `SITE_DEPLOY_SSH_KEY` | dedicated private key whose public half is authorized on the host for site deploys |
| `SITE_DEPLOY_USER` | optional; defaults to `root` |
| `DEPLOY_KNOWN_HOSTS` | pinned SSH host-key line for the host (shared with Server Delivery) |

Add the public half of `SITE_DEPLOY_SSH_KEY` to the site-deploy user's
`authorized_keys` (for `root`, `/root/.ssh/authorized_keys`). Unlike the
Server Delivery key, this key is **not** restricted by a forced command:
`deploy/site.sh` needs an interactive-capable SSH session to run `mkdir`,
`rsync`, `scp`, and `docker compose` on the host. Keep the two keys separate so
the Server Delivery forced command cannot be widened by accident.

### Manual fallback

For a one-off deploy without waiting for CI, run the same script locally:

```bash
deploy/site.sh          # ssh target defaults to "aliyun"
deploy/site.sh my-host  # or pass an explicit ssh target
```

The script builds the VitePress site, syncs both static roots with `rsync`,
updates `deploy/Caddyfile` and `compose.production.yml` on the target, and
reloads Caddy so the synced configuration takes effect. DNS for
`docs.clumsies.ai` and `clumsies.ai` must point at the server; Caddy provisions
TLS certificates automatically.

## Operations

```bash
sudo clumsies-server-release preflight
docker compose --project-name clumsies -f compose.production.yml ps
docker compose --project-name clumsies -f compose.production.yml logs server
systemctl list-timers 'clumsies-*'
```

Do not use `docker compose down --volumes` for an installed organization. It
deletes the PostgreSQL volume.
