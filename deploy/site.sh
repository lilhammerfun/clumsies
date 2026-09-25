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

echo "==> Recreating Caddy container to pick up new mounts"
# shellcheck disable=SC2029 # DEPLOY_DIR intentionally expands client-side
ssh "${SSH_TARGET}" "cd ${DEPLOY_DIR} && docker compose -f compose.production.yml up -d caddy"

echo "==> Verifying locally reachable endpoints"
ssh "${SSH_TARGET}" <<'EOF'
for host in docs.clumsies.ai clumsies.ai www.clumsies.ai app.clumsies.ai; do
  code=$(curl -s -o /dev/null -w "%{http_code}" --resolve "${host}:443:127.0.0.1" "https://${host}/" || true)
  echo "${host} -> ${code}"
done
EOF

echo "==> Done. Public DNS must point docs.clumsies.ai and clumsies.ai at the server (see issue notes)."
