#!/usr/bin/env bash
# Build the docs site and the official site, then sync them to the
# production server and reload Caddy.
#
# Usage:
#   deploy/site.sh [ssh-target]
#
# Default ssh-target is "aliyun". Requires:
#   - bun installed locally
#   - passwordless ssh to the target as root
#   - /srv/docs and /srv/www on the target (created by this script if missing)
set -euo pipefail

SSH_TARGET="${1:-aliyun}"
DOCS_DIST="docs/.vitepress/dist"
SITE_DIR="site"
DEPLOY_DIR="/opt/clumsies"

echo "==> Building docs site"
bun run build

echo "==> Ensuring static roots exist on ${SSH_TARGET}"
ssh "${SSH_TARGET}" "mkdir -p /srv/docs /srv/www"

echo "==> Syncing docs to ${SSH_TARGET}:/srv/docs"
rsync -az --delete "${DOCS_DIST}/" "${SSH_TARGET}:/srv/docs/"

echo "==> Syncing official site to ${SSH_TARGET}:/srv/www"
rsync -az --delete --exclude '.DS_Store' "${SITE_DIR}/" "${SSH_TARGET}:/srv/www/"

echo "==> Syncing Caddyfile and compose file"
scp deploy/Caddyfile "${SSH_TARGET}:${DEPLOY_DIR}/deploy/Caddyfile"
scp compose.production.yml "${SSH_TARGET}:${DEPLOY_DIR}/compose.production.yml"

echo "==> Applying the Caddy configuration"
# The Caddyfile is a mounted file: changing its contents does not make Docker
# recreate the container, and Caddy does not watch it by default. Start the
# container, then reload so a synced configuration takes effect.
# shellcheck disable=SC2029 # DEPLOY_DIR intentionally expands client-side
ssh "${SSH_TARGET}" "cd ${DEPLOY_DIR} && docker compose -f compose.production.yml up -d caddy"
# shellcheck disable=SC2029 # DEPLOY_DIR intentionally expands client-side
ssh "${SSH_TARGET}" "cd ${DEPLOY_DIR} && docker compose -f compose.production.yml exec -T caddy caddy reload --config /etc/caddy/Caddyfile --adapter caddyfile"

echo "==> Verifying locally reachable endpoints"
ssh "${SSH_TARGET}" <<'EOF'
set -euo pipefail
for host in docs.clumsies.ai clumsies.ai www.clumsies.ai app.clumsies.ai; do
  code=$(curl -s -o /dev/null -w "%{http_code}" --resolve "${host}:443:127.0.0.1" "https://${host}/" || true)
  echo "${host} -> ${code}"
done

# A missing docs path must answer 404. try_files must never rewrite to the 404
# page with status 200, or crawlers and uptime checks treat misses as content.
missing=$(curl -s -o /dev/null -w "%{http_code}" --resolve "docs.clumsies.ai:443:127.0.0.1" "https://docs.clumsies.ai/missing-page-check" || true)
if [[ "${missing}" != "404" ]]; then
  echo "docs.clumsies.ai missing path -> ${missing} (want 404)"
  exit 1
fi
echo "docs.clumsies.ai missing path -> 404"
EOF

echo "==> Done. Public DNS must point docs.clumsies.ai and clumsies.ai at the server (see issue notes)."
