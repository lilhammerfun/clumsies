#!/usr/bin/env bash
# Exercise the assembled package in an independent user home and persistent daemon root.
set -euo pipefail
# Performance observations vary; installation identity and readiness must stay equal.
compare_health_identity() {
  python3 - "$1" "$2" "$3" <<'PYTHON'
import json,sys
from pathlib import Path
first=json.loads(Path(sys.argv[1]).read_text())
second=json.loads(Path(sys.argv[2]).read_text())
first_performance=first.pop("performance",None)
second_performance=second.pop("performance",None)
assert first==second, "Daemon identity, version, database or readiness changed"
if sys.argv[3]=="reuse" and first_performance is not None and second_performance is not None:
    assert first_performance["process_id"]==second_performance["process_id"], "Resident was not reused"
PYTHON
}
[[ $# == 1 ]] || { echo 'Usage: test-linux-package.sh PACKAGE.tar.gz' >&2; exit 64; }
test_root=$(mktemp -d)
export HOME="$test_root/home"
export CLUMSIES_DAEMON_ROOT="$test_root/data"
export CLUMSIES_DAEMON_CACHE_DIR="$test_root/cache"
export CLUMSIES_DAEMON_LOG_DIR="$test_root/logs"
export CLUMSIES_SYNC_ENABLED=false
export CLUMSIES_INSTALL_ROOT="$test_root/programs"
export CLUMSIES_BIN_DIR="$test_root/bin"
export SHELL=/bin/bash
unset CLUMSIES_DEV_INSTANCE_ID CLUMSIES_DAEMON_SOCKET CLUMSIES_AGENT_RUNTIME_TEST_MACH_SERVICE
mkdir -p "$HOME" "$CLUMSIES_DAEMON_ROOT" "$test_root/package"
cli="$CLUMSIES_INSTALL_ROOT/runtime/clumsies"
cleanup() { if [[ -x "$cli" ]]; then "$cli" daemon stop || true; fi; rm -rf -- "$test_root"; }
trap cleanup EXIT
tar -xzf "$1" -C "$test_root/package"
package=$(find "$test_root/package" -mindepth 1 -maxdepth 1 -type d)
"$package/install.sh"
bash --noprofile --rcfile "$HOME/.bashrc" -ic 'command -v clumsies && clumsies --version'
"$cli" --json daemon start > "$test_root/started.json"
"$cli" --json daemon start > "$test_root/reused.json"
compare_health_identity "$test_root/started.json" "$test_root/reused.json" reuse
printf 'retained draft and binding proof\n' > "$CLUMSIES_DAEMON_ROOT/retained-work"
"$package/install.sh"
[[ -s "$CLUMSIES_DAEMON_ROOT/local.db" ]]
[[ $(cat "$CLUMSIES_DAEMON_ROOT/retained-work") == 'retained draft and binding proof' ]]
"$cli" --json daemon start > "$test_root/upgraded.json"
compare_health_identity "$test_root/started.json" "$test_root/upgraded.json" upgrade
"$cli" daemon stop
# A corrupt package must leave both installed executables intact.
before=$(sha256sum "$cli" "$CLUMSIES_INSTALL_ROOT/runtime/clumsiesd")
printf 'corrupt\n' >> "$package/clumsiesd"
if "$package/install.sh"; then echo 'Corrupt upgrade unexpectedly succeeded' >&2; exit 1; fi
[[ $(sha256sum "$cli" "$CLUMSIES_INSTALL_ROOT/runtime/clumsiesd") == "$before" ]]
# Startup must fail while the installer owns its lock, without starting another resident.
(
  exec 8>"$CLUMSIES_INSTALL_ROOT/.install.lock"
  flock -n 8
  if "$cli" daemon start; then echo 'Startup raced installation' >&2; exit 1; fi
)
"$CLUMSIES_INSTALL_ROOT/runtime/install.sh" --uninstall
[[ ! -e "$cli" && ! -L "$CLUMSIES_BIN_DIR/clumsies" ]]
[[ -s "$CLUMSIES_DAEMON_ROOT/local.db" && -s "$CLUMSIES_DAEMON_ROOT/retained-work" ]]
echo 'Linux install/reuse/upgrade/corruption/startup-lock/uninstall checks passed.'
