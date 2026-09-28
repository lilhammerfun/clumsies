#!/usr/bin/env bash
# Upload only the committed observability tree; credentials remain on the host.
set -euo pipefail
[[ $# == 1 && $1 =~ ^[a-zA-Z0-9_.-]+$ && $1 != -* ]] || { echo 'Usage: deploy.sh SSH_ALIAS' >&2; exit 2; }
target=$1
commit=$(git rev-parse HEAD)
remote=$(ssh "$target" 'mktemp -d /tmp/clumsies-observability.XXXXXXXX')
[[ $remote =~ ^/tmp/clumsies-observability\.[a-zA-Z0-9]+$ ]] || exit 1
trap 'ssh "$target" "rm -rf -- $remote"' EXIT
# Both substitutions are validated local values, intentionally sent to SSH.
# shellcheck disable=SC2029
git archive "$commit" deploy/observability | ssh "$target" "tar -x -C $remote"
# shellcheck disable=SC2029
ssh "$target" "sudo -n python3 $remote/deploy/observability/delivery.py $commit"
