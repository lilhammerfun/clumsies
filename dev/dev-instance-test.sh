#!/bin/sh
set -eu

unset CDPATH
repo_root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
runner=$repo_root/dev/dev-instance.sh
test_root=$(mktemp -d "${TMPDIR:-/tmp}/clumsies-dev-instance-test.XXXXXX")
fake_bin=$test_root/bin
fake_log=$test_root/commands.log
fake_reject_log=$test_root/rejected-commands.log
fake_docker_state=$test_root/docker.running
fake_app_state=$test_root/app.running
fake_daemon_state=$test_root/daemon.running
fake_server_registry=$test_root/server.registry
compose_descriptor_marker=$test_root/compose-descriptor.checked
test_home=$test_root/home
dev_root=$test_root/dev-root
mkdir -p "$fake_bin" "$test_home/Applications/Clumsies.app" \
  "$test_home/Library/Application Support/ai.clumsies" "$test_home/.codex"
printf 'stable-app\n' > "$test_home/Applications/Clumsies.app/sentinel"
printf 'stable-daemon\n' > "$test_home/Library/Application Support/ai.clumsies/sentinel"
printf 'stable-codex\n' > "$test_home/.codex/sentinel"

cleanup() {
  if [ -n "${reset_pid:-}" ]; then
    : > "$reset_rm_continue"
    wait "$reset_pid" 2>/dev/null || true
  fi
  if [ -n "${victim_pid:-}" ]; then
    kill "$victim_pid" 2>/dev/null || true
    wait "$victim_pid" 2>/dev/null || true
  fi
  if [ -f "$fake_server_registry" ]; then
    cleanup_pid=$(sed -n 's/|.*//p' "$fake_server_registry")
    case "$cleanup_pid" in
      ''|*[!0-9]*) ;;
      *) kill "$cleanup_pid" 2>/dev/null || true ;;
    esac
  fi
  if [ -d "$dev_root" ]; then
    run down >/dev/null 2>&1 || true
  fi
  rm -rf -- "$test_root"
}
trap cleanup EXIT INT TERM

cat > "$fake_bin/docker" <<'EOF'
#!/bin/sh
set -eu
printf 'docker %s\n' "$*" >> "$FAKE_COMMAND_LOG"
reject() {
  printf 'docker %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}
[ "$#" -ge 9 ] || reject "$@"
[ "$1" = compose ] || reject "$@"
shift
[ "$1" = --env-file ] || reject "$@"
case "$2" in
  "$FAKE_COMPOSE_ENV"|"$FAKE_RECOVERY_COMPOSE_ENV") ;;
  *) reject "$@" ;;
esac
shift 2
[ "$1" = -f ] && [ "$2" = "$FAKE_COMPOSE_FILE" ] || reject "$@"
shift 2
[ "$1" = -p ] && [ "$2" = "$FAKE_COMPOSE_PROJECT" ] || reject "$@"
shift 2
case "${1:-}" in
  up)
    [ "$#" -eq 5 ] && [ "$2" = -d ] && [ "$3" = --wait ] \
      && [ "$4" = postgres ] && [ "$5" = fake-oidc ] || reject "$@"
    grep -Fx 'CLUMSIES_HOST_BIND_ADDRESS=127.0.0.1' "$FAKE_COMPOSE_ENV" >/dev/null \
      || reject "$@"
    /usr/bin/python3 - "$FAKE_RUNTIME_FILE" "$FAKE_INSTANCE_ROOT" \
      "$FAKE_COMPOSE_PROJECT" "$FAKE_EXPECTED_SERVER_BIN" <<'PY'
import json, sys

path, root, project, binary = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    value = json.load(handle)
assert value["state"] == "starting"
assert value["paths"]["instance_root"] == root
assert value["paths"]["server_binary"] == binary
assert value["compose_project"] == project
assert value["processes"] == {"server_pid": None, "server_start_identity": None}
PY
    : > "$FAKE_COMPOSE_DESCRIPTOR_MARKER"
    : > "$FAKE_DOCKER_STATE"
    ;;
  port)
    [ -f "$FAKE_DOCKER_STATE" ] || reject "$@"
    if [ "$#" -eq 3 ] && [ "$2" = postgres ] && [ "$3" = 5432 ]; then
      printf '127.0.0.1:55432\n'
    elif [ "$#" -eq 3 ] && [ "$2" = fake-oidc ] && [ "$3" = 8080 ]; then
      printf '127.0.0.1:18091\n'
    else
      reject "$@"
    fi
    ;;
  ps)
    [ "$#" -eq 4 ] && [ "$2" = --status ] && [ "$3" = running ] \
      && [ "$4" = --services ] || reject "$@"
    [ -f "$FAKE_DOCKER_STATE" ] || exit 1
    printf 'postgres\nfake-oidc\n'
    ;;
  down)
    if [ "$#" -eq 2 ] && [ "$2" = --remove-orphans ]; then
      :
    elif [ "$#" -eq 3 ] && [ "$2" = -v ] && [ "$3" = --remove-orphans ]; then
      :
    else
      reject "$@"
    fi
    rm -f "$FAKE_DOCKER_STATE"
    ;;
  logs)
    [ "$#" -eq 5 ] && [ "$2" = --tail ] && [ "$3" = 200 ] \
      && [ "$4" = postgres ] && [ "$5" = fake-oidc ] || reject "$@"
    ;;
  *) reject "$@" ;;
esac
EOF

cat > "$fake_bin/curl" <<'EOF'
#!/bin/sh
set -eu
printf 'curl %s\n' "$*" >> "$FAKE_COMMAND_LOG"
reject() {
  printf 'curl %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}

for argument do url=$argument; done
case "${url:-}" in http://*|https://*) ;; *) reject "$@" ;; esac
[ "${FAKE_CURL_FAIL:-0}" = 0 ] || exit 22

case "$url" in
  http://127.0.0.1:*/api/v1/admin/health|https://pr-*.example.test/api/v1/admin/health)
    printf '{"status":"ok"}\n'
    ;;
  *) reject ;;
esac
EOF

cat > "$fake_bin/xcodegen" <<'EOF'
#!/bin/sh
set -eu
printf 'xcodegen %s\n' "$*" >> "$FAKE_COMMAND_LOG"
[ "$#" -eq 3 ] && [ "$1" = generate ] && [ "$2" = --spec ] \
  && [ "$3" = "$FAKE_REPO_ROOT/apps/macos/project.yml" ] || {
  printf 'xcodegen %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}
EOF

cat > "$fake_bin/xcodebuild" <<'EOF'
#!/bin/sh
set -eu
printf 'xcodebuild %s\n' "$*" >> "$FAKE_COMMAND_LOG"
derived=
project=
scheme=
configuration=
product=
display_name=
bundle=
instance_id=
daemon_root=
daemon_cache=
daemon_logs=
launch_agents=
server_url=
codex_home=
build_id=
skip_daemon_build=
only_testing=
operation=
while [ $# -gt 0 ]; do
  case "$1" in
    -project|-scheme|-configuration)
      option=$1
      shift
      case "$option" in
        -project) project=$1 ;;
        -scheme) scheme=$1 ;;
        -configuration) configuration=$1 ;;
      esac
      ;;
    -derivedDataPath)
      shift
      derived=$1
      ;;
    -only-testing:*) only_testing=$1 ;;
    build|test) operation=$1 ;;
    CLUMSIES_APP_PRODUCT_NAME=*) product=${1#CLUMSIES_APP_PRODUCT_NAME=} ;;
    CLUMSIES_APP_DISPLAY_NAME=*) display_name=${1#CLUMSIES_APP_DISPLAY_NAME=} ;;
    CLUMSIES_APP_BUNDLE_IDENTIFIER=*) bundle=${1#CLUMSIES_APP_BUNDLE_IDENTIFIER=} ;;
    CLUMSIES_DEV_INSTANCE_ID=*) instance_id=${1#CLUMSIES_DEV_INSTANCE_ID=} ;;
    CLUMSIES_DAEMON_ROOT=*) daemon_root=${1#CLUMSIES_DAEMON_ROOT=} ;;
    CLUMSIES_DAEMON_CACHE_DIR=*) daemon_cache=${1#CLUMSIES_DAEMON_CACHE_DIR=} ;;
    CLUMSIES_DAEMON_LOG_DIR=*) daemon_logs=${1#CLUMSIES_DAEMON_LOG_DIR=} ;;
    CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR=*) launch_agents=${1#CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR=} ;;
    CLUMSIES_SERVER_URL=*) server_url=${1#CLUMSIES_SERVER_URL=} ;;
    CLUMSIES_CODEX_HOME=*) codex_home=${1#CLUMSIES_CODEX_HOME=} ;;
    CLUMSIES_AGENT_RUNTIME_BUILD_ID=*) build_id=${1#CLUMSIES_AGENT_RUNTIME_BUILD_ID=} ;;
    CLUMSIES_SKIP_DAEMON_BUILD=*) skip_daemon_build=${1#CLUMSIES_SKIP_DAEMON_BUILD=} ;;
  esac
  shift
done
[ "$derived" = "$FAKE_DERIVED_DATA" ] \
  && [ "$project" = "$FAKE_REPO_ROOT/apps/macos/Clumsies.xcodeproj" ] \
  && [ "$configuration" = Debug ] \
  && [ "$product" = "$FAKE_PRODUCT_NAME" ] \
  && [ "$display_name" = "$FAKE_DISPLAY_NAME" ] \
  && [ "$bundle" = "$FAKE_BUNDLE_ID" ] \
  && [ "$instance_id" = "$FAKE_INSTANCE_ID" ] \
  && [ "$daemon_root" = "$FAKE_DAEMON_ROOT" ] \
  && [ "$daemon_cache" = "$FAKE_DAEMON_CACHE" ] \
  && [ "$daemon_logs" = "$FAKE_DAEMON_LOGS" ] \
  && [ "$launch_agents" = "$FAKE_LAUNCH_AGENTS" ] \
  && [ "$codex_home" = "$FAKE_CODEX_HOME" ] \
  && [ "$build_id" = "$FAKE_BUILD_ID" ] \
  && [ "$server_url" = "$FAKE_OPEN_SERVER_URL" ] || {
  printf 'xcodebuild identity mismatch\n' >> "$FAKE_REJECT_LOG"
  exit 97
}
case "$operation" in
  build)
    [ "$scheme" = Clumsies ] && [ -z "$skip_daemon_build" ] || exit 97
    ;;
  test)
    [ "$scheme" = ClumsiesLiveTests ] \
      && [ "$skip_daemon_build" = 1 ] \
      && [ "$only_testing" = -only-testing:ClumsiesTests/LiveWorkspaceIntegrationTests ] \
      && [ "${CLUMSIES_RUN_LIVE_TESTS:-}" = 1 ] \
      && [ "${CLUMSIES_SKIP_DAEMON_BUILD:-}" = 1 ] \
      && [ -f "$FAKE_INSTANCE_LOCK/pid" ] || exit 97
    ;;
  *) exit 97 ;;
esac
[ "${FAKE_XCODEBUILD_FAIL:-0}" = 0 ] || exit 73
app=$derived/Build/Products/Debug/$product.app
mkdir -p "$app/Contents/Resources" "$app/Contents/MacOS"
printf '#!/bin/sh\nexit 0\n' > "$app/Contents/Resources/clumsiesd"
printf '#!/bin/sh\nexit 0\n' > "$app/Contents/MacOS/$product"
chmod +x "$app/Contents/Resources/clumsiesd" "$app/Contents/MacOS/$product"
EOF

cat > "$fake_bin/open" <<'EOF'
#!/bin/sh
set -eu
printf 'open %s\n' "$*" >> "$FAKE_COMMAND_LOG"
[ "$#" -eq 20 ] && [ "$1" = -n ] \
  && [ "$2" = --stdout ] && [ "$3" = "$FAKE_APP_STDOUT" ] \
  && [ "$4" = --stderr ] && [ "$5" = "$FAKE_APP_STDERR" ] \
  && [ "$6" = --env ] && [ "$7" = "CLUMSIES_DEV_INSTANCE_ID=$FAKE_INSTANCE_ID" ] \
  && [ "$8" = --env ] && [ "$9" = "CLUMSIES_DAEMON_ROOT=$FAKE_DAEMON_ROOT" ] \
  && [ "${10}" = --env ] && [ "${11}" = "CLUMSIES_DAEMON_CACHE_DIR=$FAKE_DAEMON_CACHE" ] \
  && [ "${12}" = --env ] && [ "${13}" = "CLUMSIES_DAEMON_LOG_DIR=$FAKE_DAEMON_LOGS" ] \
  && [ "${14}" = --env ] && [ "${15}" = "CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR=$FAKE_LAUNCH_AGENTS" ] \
  && [ "${16}" = --env ] && [ "${17}" = "CLUMSIES_SERVER_URL=$FAKE_OPEN_SERVER_URL" ] \
  && [ "${18}" = --env ] && [ "${19}" = "CLUMSIES_CODEX_HOME=$FAKE_CODEX_HOME" ] \
  && [ "${20}" = "$FAKE_APP_PATH" ] || {
  printf 'open %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}
case "$FAKE_OPEN_SERVER_URL" in
  http://127.0.0.1:*) [ -f "$FAKE_INSTANCE_ROOT/signed-in" ] || exit 97 ;;
  https://*) ;;
  *) exit 97 ;;
esac
: > "$FAKE_APP_STATE"
: > "$FAKE_DAEMON_STATE"
EOF

cat > "$fake_bin/python" <<'EOF'
#!/bin/sh
set -eu
if [ "${1:-}" = "$FAKE_REPO_ROOT/dev/seed-review-playground.py" ]; then
  [ "$#" -eq 2 ] && [ "$2" = --login-only ] || exit 97
  printf 'dev-login\n' >> "$FAKE_COMMAND_LOG"
  [ "${FAKE_LOGIN_FAIL:-0}" = 0 ] || exit 74
  [ -x "$FAKE_APP_PATH/Contents/Resources/clumsiesd" ] || exit 97
  : > "$FAKE_INSTANCE_ROOT/signed-in"
  : > "$FAKE_DAEMON_STATE"
  exit 0
fi
exec /usr/bin/python3 "$@"
EOF

cat > "$fake_bin/launchctl" <<'EOF'
#!/bin/sh
set -eu
printf 'launchctl %s\n' "$*" >> "$FAKE_COMMAND_LOG"
reject() {
  printf 'launchctl %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}

server_pid() {
  [ -f "$FAKE_SERVER_REGISTRY" ] || return 1
  pid=$(sed -n 's/|.*//p' "$FAKE_SERVER_REGISTRY")
  case "$pid" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "$pid" -gt 1 ] && kill -0 "$pid" 2>/dev/null || return 1
  printf '%s\n' "$pid"
}

case "${1:-}" in
  bootstrap)
    [ "$#" -eq 3 ] \
      && [ "$2" = "gui/$FAKE_UID" ] \
      && [ "$3" = "$FAKE_SERVER_PLIST" ] \
      && [ ! -f "$FAKE_SERVER_REGISTRY" ] || reject "$@"
    server_address=$(/usr/bin/python3 - \
      "$FAKE_SERVER_PLIST" "$FAKE_SERVER_LABEL" "$FAKE_SERVER_LAUNCHER" \
      "$FAKE_COMPOSE_ENV" "$FAKE_EXPECTED_SERVER_BIN" \
      "$FAKE_INSTANCE_ROOT/server-ready.json" \
      "$FAKE_INSTANCE_ROOT/logs/server.log" "$FAKE_SERVER_PORT" <<'PY'
import os, plistlib, stat, sys

(
    path, label, launcher, compose_env, server_binary, ready_file,
    server_log, server_port,
) = sys.argv[1:]
with open(path, "rb") as handle:
    value = plistlib.load(handle)
arguments = value.get("ProgramArguments")
assert isinstance(arguments, list) and len(arguments) == 5
assert arguments[:3] == [launcher, compose_env, server_binary]
assert arguments[3] in {"127.0.0.1:0", f"127.0.0.1:{server_port}"}
assert arguments[4] == ready_file
assert value == {
    "Label": label,
    "ProgramArguments": arguments,
    "RunAtLoad": True,
    "KeepAlive": True,
    "StandardOutPath": server_log,
    "StandardErrorPath": server_log,
}
assert stat.S_IMODE(os.stat(path).st_mode) == 0o600
print(arguments[3])
PY
    ) || reject "$@"
    "$FAKE_SERVER_LAUNCHER" "$FAKE_COMPOSE_ENV" "$FAKE_EXPECTED_SERVER_BIN" \
      "$server_address" "$FAKE_INSTANCE_ROOT/server-ready.json" \
      >> "$FAKE_INSTANCE_ROOT/logs/server.log" 2>&1 &
    ;;
  print|bootout)
    [ "$#" -eq 2 ] || reject "$@"
    case "$2" in
      "gui/$FAKE_UID/$FAKE_DAEMON_LABEL")
        if [ "$1" = print ]; then
          [ -f "$FAKE_DAEMON_STATE" ]
        else
          rm -f "$FAKE_DAEMON_STATE"
        fi
        ;;
      "gui/$FAKE_UID/$FAKE_SERVER_LABEL")
        pid=$(server_pid) || exit 1
        if [ "$1" = print ]; then
          printf '    pid = %s\n' "$pid"
        else
          kill "$pid"
          attempts=0
          while kill -0 "$pid" 2>/dev/null && [ "$attempts" -lt 25 ]; do
            /bin/sleep 0.02
            attempts=$((attempts + 1))
          done
          if kill -0 "$pid" 2>/dev/null; then
            kill -KILL "$pid"
          fi
          attempts=0
          while kill -0 "$pid" 2>/dev/null && [ "$attempts" -lt 50 ]; do
            /bin/sleep 0.02
            attempts=$((attempts + 1))
          done
          ! kill -0 "$pid" 2>/dev/null || reject "$@"
          rm -f "$FAKE_SERVER_REGISTRY"
        fi
        ;;
      *) reject "$@" ;;
    esac
    ;;
  *) reject "$@" ;;
esac
EOF

cat > "$fake_bin/pgrep" <<'EOF'
#!/bin/sh
set -eu
printf 'pgrep %s\n' "$*" >> "$FAKE_COMMAND_LOG"
[ "$#" -eq 3 ] && [ "$1" = -f ] && [ "$2" = -x ] \
  && [ "$3" = "$FAKE_APP_PATTERN" ] || {
  printf 'pgrep %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}
[ -f "$FAKE_APP_STATE" ]
EOF

cat > "$fake_bin/pkill" <<'EOF'
#!/bin/sh
set -eu
printf 'pkill %s\n' "$*" >> "$FAKE_COMMAND_LOG"
[ "$#" -eq 4 ] && [ "$1" = -TERM ] && [ "$2" = -f ] && [ "$3" = -x ] \
  && [ "$4" = "$FAKE_APP_PATTERN" ] || {
  printf 'pkill %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}
rm -f "$FAKE_APP_STATE"
EOF

cat > "$fake_bin/osascript" <<'EOF'
#!/bin/sh
set -eu
printf 'osascript %s\n' "$*" >> "$FAKE_COMMAND_LOG"
[ "$#" -eq 2 ] && [ "$1" = -e ] \
  && [ "$2" = "tell application id \"$FAKE_BUNDLE_ID\" to quit" ] || {
  printf 'osascript %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}
rm -f "$FAKE_APP_STATE"
EOF

cat > "$fake_bin/security" <<'EOF'
#!/bin/sh
set -eu
printf 'security %s\n' "$*" >> "$FAKE_COMMAND_LOG"
[ "$#" -eq 5 ] && [ "$1" = delete-generic-password ] \
  && [ "$2" = -s ] && [ "$3" = "$FAKE_KEYCHAIN_SERVICE" ] \
  && [ "$4" = -a ] && [ "$5" = server-session ] || {
  printf 'security %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}
EOF

cat > "$fake_bin/rm" <<'EOF'
#!/bin/sh
set -eu
if [ "${FAKE_PAUSE_RESET_RM:-0}" = 1 ] \
  && [ "$#" -eq 3 ] && [ "$1" = -rf ] && [ "$2" = -- ] \
  && [ "$3" = "$FAKE_INSTANCE_ROOT" ]; then
  : > "$FAKE_RESET_RM_STARTED"
  attempts=0
  while [ ! -f "$FAKE_RESET_RM_CONTINUE" ] && [ "$attempts" -lt 200 ]; do
    /bin/sleep 0.05
    attempts=$((attempts + 1))
  done
  [ -f "$FAKE_RESET_RM_CONTINUE" ] || exit 98
fi
exec /bin/rm "$@"
EOF

cat > "$fake_bin/ps" <<'EOF'
#!/bin/sh
set -eu
printf 'ps %s\n' "$*" >> "$FAKE_COMMAND_LOG"
[ "$#" -eq 4 ] && [ "$1" = -p ] && [ "$3" = -o ] || {
  printf 'ps %s\n' "$*" >> "$FAKE_REJECT_LOG"
  exit 97
}
case "$2" in
  ''|*[!0-9]*)
    printf 'ps %s\n' "$*" >> "$FAKE_REJECT_LOG"
    exit 97
    ;;
esac
[ -f "$FAKE_SERVER_REGISTRY" ] || exit 1
IFS='|' read -r registered_pid registered_start < "$FAKE_SERVER_REGISTRY"
[ "$2" = "$registered_pid" ] || exit 1
kill -0 "$registered_pid" 2>/dev/null || exit 1
case "$4" in
  command=) printf '%s\n' "$FAKE_EXPECTED_SERVER_BIN" ;;
  lstart=) printf '%s\n' "$registered_start" ;;
  *)
    printf 'ps %s\n' "$*" >> "$FAKE_REJECT_LOG"
    exit 97
    ;;
esac
EOF

cat > "$test_root/fake-server" <<'EOF'
#!/bin/sh
set -eu
printf 'server started\n' >> "$FAKE_COMMAND_LOG"
port=${FAKE_SERVER_PORT:-49152}
delay_attempts=${FAKE_SERVER_READY_DELAY_ATTEMPTS:-0}
start_identity=fake-start-$$
temporary_registry=$FAKE_SERVER_REGISTRY.$$.tmp
printf '%s|%s\n' "$$" "$start_identity" > "$temporary_registry"
chmod 600 "$temporary_registry"
mv "$temporary_registry" "$FAKE_SERVER_REGISTRY"
cleanup_server() {
  if [ -f "$FAKE_SERVER_REGISTRY" ] \
    && [ "$(sed -n 's/|.*//p' "$FAKE_SERVER_REGISTRY")" = "$$" ]; then
    rm -f "$FAKE_SERVER_REGISTRY"
  fi
  rm -f "$CLUMSIES_SERVER_READY_FILE"
}
trap cleanup_server 0
trap 'exit 0' TERM INT
while [ "$delay_attempts" -gt 0 ]; do
  /bin/sleep 0.1
  delay_attempts=$((delay_attempts - 1))
done
temporary=$CLUMSIES_SERVER_READY_FILE.$$.tmp
printf '{"listen_addr":"127.0.0.1:%s","public_origin":"http://127.0.0.1:%s"}\n' \
  "$port" "$port" > "$temporary"
chmod 600 "$temporary"
mv "$temporary" "$CLUMSIES_SERVER_READY_FILE"
while :; do /bin/sleep 1; done
EOF

chmod +x "$fake_bin"/* "$test_root/fake-server"

instance_id=$(printf '%s' "$repo_root" | shasum -a 256 | awk '{print substr($1, 1, 12)}')
instance_root=$dev_root/instances/$instance_id
instance_lock=$dev_root/instances/.locks/$instance_id
runtime=$instance_root/runtime.json
compose_project=clumsies-dev-$instance_id
owned_server_binary=$instance_root/bin/clumsies-server
owned_server_launcher=$instance_root/bin/dev-server
bundle_id=ai.clumsies.desktop.dev.$instance_id
daemon_label=ai.clumsies.daemon.dev.$instance_id
server_label=ai.clumsies.server.dev.$instance_id
keychain_service=ai.clumsies.dev.$instance_id
product_name=ClumsiesDev-$instance_id
display_name="Clumsies Dev $instance_id"
derived_data=$instance_root/macos-derived
app_path=$derived_data/Build/Products/Debug/$product_name.app
app_executable=$app_path/Contents/MacOS/$product_name
app_process_pattern=$(printf '%s' "$app_executable" | sed 's/[][\\.^$*+?(){}|]/\\&/g')
daemon_root=$instance_root/daemon
daemon_cache=$instance_root/cache
daemon_logs=$instance_root/logs/daemon
launch_agents=$instance_root/LaunchAgents
server_launch_agent_plist=$launch_agents/$server_label.plist
codex_home=$instance_root/codex-home
compose_env=$instance_root/compose.env
recovery_compose_env=$instance_lock/compose.env
reset_rm_started=$test_root/reset-rm.started
reset_rm_continue=$test_root/reset-rm.continue
commit_id=$(git -C "$repo_root" rev-parse HEAD)
commit_short=$(printf '%.12s' "$commit_id")
expected_build_id=dev-$instance_id-$commit_short
[ -z "$(git -C "$repo_root" status --porcelain --untracked-files=normal)" ] \
  || expected_build_id=$expected_build_id-dirty

runner_environment() {
  env \
    HOME="$test_home" \
    CLUMSIES_DEV_ROOT="$dev_root" \
    CLUMSIES_DEV_SERVER_BIN="$test_root/fake-server" \
    PYTHON="$fake_bin/python" \
    FAKE_COMMAND_LOG="$fake_log" \
    FAKE_REJECT_LOG="$fake_reject_log" \
    FAKE_DOCKER_STATE="$fake_docker_state" \
    FAKE_APP_STATE="$fake_app_state" \
    FAKE_DAEMON_STATE="$fake_daemon_state" \
    FAKE_SERVER_REGISTRY="$fake_server_registry" \
    FAKE_COMPOSE_DESCRIPTOR_MARKER="$compose_descriptor_marker" \
    FAKE_RUNTIME_FILE="$runtime" \
    FAKE_INSTANCE_ROOT="$instance_root" \
    FAKE_COMPOSE_PROJECT="$compose_project" \
    FAKE_COMPOSE_ENV="$compose_env" \
    FAKE_RECOVERY_COMPOSE_ENV="$recovery_compose_env" \
    FAKE_COMPOSE_FILE="$repo_root/docker-compose.yml" \
    FAKE_EXPECTED_SERVER_BIN="$owned_server_binary" \
    FAKE_REPO_ROOT="$repo_root" \
    FAKE_INSTANCE_ID="$instance_id" \
    FAKE_BUNDLE_ID="$bundle_id" \
    FAKE_DISPLAY_NAME="$display_name" \
    FAKE_DAEMON_LABEL="$daemon_label" \
    FAKE_SERVER_LABEL="$server_label" \
    FAKE_SERVER_PLIST="$server_launch_agent_plist" \
    FAKE_SERVER_LAUNCHER="$owned_server_launcher" \
    FAKE_KEYCHAIN_SERVICE="$keychain_service" \
    FAKE_PRODUCT_NAME="$product_name" \
    FAKE_DERIVED_DATA="$derived_data" \
    FAKE_INSTANCE_LOCK="$instance_lock" \
    FAKE_APP_PATH="$app_path" \
    FAKE_APP_PATTERN="$app_process_pattern" \
    FAKE_APP_STDOUT="$instance_root/logs/app.out.log" \
    FAKE_APP_STDERR="$instance_root/logs/app.err.log" \
    FAKE_DAEMON_ROOT="$daemon_root" \
    FAKE_DAEMON_CACHE="$daemon_cache" \
    FAKE_DAEMON_LOGS="$daemon_logs" \
    FAKE_LAUNCH_AGENTS="$launch_agents" \
    FAKE_CODEX_HOME="$codex_home" \
    FAKE_BUILD_ID="$expected_build_id" \
    FAKE_RESET_RM_STARTED="$reset_rm_started" \
    FAKE_RESET_RM_CONTINUE="$reset_rm_continue" \
    FAKE_UID="$(id -u)" \
    FAKE_OPEN_SERVER_URL="${FAKE_OPEN_SERVER_URL:-http://127.0.0.1:${FAKE_SERVER_PORT:-49152}}" \
    FAKE_SERVER_PORT="${FAKE_SERVER_PORT:-49152}" \
    FAKE_SERVER_READY_DELAY_ATTEMPTS="${FAKE_SERVER_READY_DELAY_ATTEMPTS:-0}" \
    FAKE_XCODEBUILD_FAIL="${FAKE_XCODEBUILD_FAIL:-0}" \
    FAKE_LOGIN_FAIL="${FAKE_LOGIN_FAIL:-0}" \
    FAKE_CURL_FAIL="${FAKE_CURL_FAIL:-0}" \
    FAKE_PAUSE_RESET_RM="${FAKE_PAUSE_RESET_RM:-0}" \
    PATH="$fake_bin:/usr/bin:/bin" \
    "$@"
}

run() {
  runner_environment "$runner" "$@"
}

if run test-live > "$test_root/no-runtime-live.out" 2> "$test_root/no-runtime-live.err"; then
  echo "expected test-live without a runtime descriptor to fail" >&2
  exit 1
fi
grep -F 'no runtime descriptor' "$test_root/no-runtime-live.err" >/dev/null
if grep -q '^xcodebuild ' "$fake_log" 2>/dev/null; then
  echo "test-live must not invoke xcodebuild without an owned runtime" >&2
  exit 1
fi

FAKE_SERVER_PORT=49152 FAKE_SERVER_READY_DELAY_ATTEMPTS=300 \
  runner_environment "$runner" up \
  > "$test_root/interrupted-up.out" 2> "$test_root/interrupted-up.err" &
launcher_pid=$!
attempts=0
while [ "$attempts" -lt 100 ]; do
  if [ -f "$runtime" ] && /usr/bin/python3 -c \
    'import json,sys; value=json.load(open(sys.argv[1])); raise SystemExit(0 if value["processes"]["server_pid"] and value["server_url"] == "http://127.0.0.1:0" else 1)' \
    "$runtime"; then
    break
  fi
  /bin/sleep 0.05
  attempts=$((attempts + 1))
done
[ "$attempts" -lt 100 ]
[ -f "$runtime" ]
/usr/bin/python3 - "$runtime" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    value = json.load(handle)
assert value["state"] == "starting"
assert value["server_url"] == "http://127.0.0.1:0"
assert value["ports"]["server"] == 0
assert value["processes"]["server_pid"] > 1
assert value["processes"]["server_start_identity"].startswith("fake-start-")
PY
runner_pid=$(sed -n '1p' "$instance_lock/pid")
case "$runner_pid" in
  ''|*[!0-9]*)
    echo "expected the runner lock to contain a PID" >&2
    exit 1
    ;;
esac
kill -KILL "$runner_pid"
wait "$launcher_pid" 2>/dev/null || true
run up > "$test_root/recovered-up.out"
[ "$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["state"])' "$runtime")" = running ]
[ -f "$compose_descriptor_marker" ]
run reset

FAKE_XCODEBUILD_FAIL=1
export FAKE_XCODEBUILD_FAIL
if run up > "$test_root/failed-up.out" 2> "$test_root/failed-up.err"; then
  echo "expected the interrupted build to fail" >&2
  exit 1
fi
unset FAKE_XCODEBUILD_FAIL
[ -f "$runtime" ]
[ "$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["state"])' "$runtime")" = stopped ]
[ ! -f "$fake_server_registry" ]
grep -F -- "--env-file $instance_root/compose.env" "$fake_log" >/dev/null
grep -F -- "-p $compose_project" "$fake_log" >/dev/null
grep -F -- "server started" "$fake_log" >/dev/null
grep -F -- "CLUMSIES_APP_BUNDLE_IDENTIFIER=ai.clumsies.desktop.dev.$instance_id" "$fake_log" >/dev/null
grep -F -- "CLUMSIES_APP_PRODUCT_NAME=ClumsiesDev-$instance_id" "$fake_log" >/dev/null
if grep -E '(^| )PRODUCT_(BUNDLE_IDENTIFIER|NAME)=' "$fake_log" >/dev/null; then
  echo "expected App identity overrides to use scoped build settings" >&2
  exit 1
fi
grep -F -- "CLUMSIES_DB_PORT=55432" "$instance_root/compose.env" >/dev/null
grep -F -- "CLUMSIES_OIDC_PORT=18091" "$instance_root/compose.env" >/dev/null
grep -F -- "CLUMSIES_HOST_BIND_ADDRESS=127.0.0.1" "$instance_root/compose.env" >/dev/null
grep -F -- "\${CLUMSIES_HOST_BIND_ADDRESS:-0.0.0.0}:\${CLUMSIES_DB_PORT:-5432}:5432" "$repo_root/docker-compose.yml" >/dev/null
grep -F -- "\${CLUMSIES_HOST_BIND_ADDRESS:-0.0.0.0}:\${CLUMSIES_OIDC_PORT:-18081}:8080" "$repo_root/docker-compose.yml" >/dev/null
grep -F -- "\${CLUMSIES_HOST_BIND_ADDRESS:-0.0.0.0}:\${CLUMSIES_SERVER_PORT:-18080}:8080" "$repo_root/docker-compose.yml" >/dev/null

open_before=$(grep -c '^open ' "$fake_log" || true)
if FAKE_LOGIN_FAIL=1 run up > "$test_root/failed-login.out" 2> "$test_root/failed-login.err"; then
  echo "expected login failure to stop before opening the App" >&2
  exit 1
fi
unset FAKE_LOGIN_FAIL
[ "$(grep -c '^open ' "$fake_log" || true)" -eq "$open_before" ]
[ ! -f "$fake_server_registry" ]
run up > "$test_root/up.out"
[ "$(stat -f '%Lp' "$runtime")" = 600 ]
/usr/bin/python3 - "$runtime" "$repo_root" "$instance_root" "$instance_id" "$expected_build_id" <<'PY'
import json, sys

path, worktree, root, instance_id, build_id = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    value = json.load(handle)
assert value["mode"] == "local"
assert value["state"] == "running"
assert value["instance_id"] == instance_id
assert value["worktree_path"] == worktree
assert value["build_id"] == build_id
assert value["server_url"] == "http://127.0.0.1:49152"
assert value["ports"] == {"server": 49152, "postgres": 55432, "oidc": 18091}
assert value["compose_project"] == f"clumsies-dev-{instance_id}"
assert value["identities"]["launch_agent_label"] == f"ai.clumsies.daemon.dev.{instance_id}"
assert value["identities"]["server_launch_agent_label"] == f"ai.clumsies.server.dev.{instance_id}"
assert value["identities"]["keychain_service"] == f"ai.clumsies.dev.{instance_id}"
assert value["paths"]["instance_root"] == root
assert value["paths"]["codex_home"] == f"{root}/codex-home"
assert value["paths"]["server_binary"] == f"{root}/bin/clumsies-server"
assert value["paths"]["server_launcher"] == f"{root}/bin/dev-server"
assert value["paths"]["server_launch_agent_plist"] == (
    f"{root}/LaunchAgents/ai.clumsies.server.dev.{instance_id}.plist"
)
assert value["processes"]["server_pid"] > 1
assert value["processes"]["server_start_identity"].startswith("fake-start-")
PY

run status > "$test_root/status.json"
grep -F '"health": true' "$test_root/status.json" >/dev/null
grep -F -- "pgrep -f -x" "$fake_log" >/dev/null

xcodebuild_before=$(grep -c '^xcodebuild ' "$fake_log" || true)
run test-live > "$test_root/live-test.out"
xcodebuild_after=$(grep -c '^xcodebuild ' "$fake_log" || true)
[ "$xcodebuild_after" -eq $((xcodebuild_before + 1)) ]
[ ! -e "$instance_lock" ]

xcodebuild_before=$(grep -c '^xcodebuild ' "$fake_log" || true)
login_before=$(grep -c '^dev-login$' "$fake_log" || true)
run up > "$test_root/repeated-up.out"
[ "$(grep -c '^dev-login$' "$fake_log" || true)" -eq $((login_before + 1)) ]
xcodebuild_after=$(grep -c '^xcodebuild ' "$fake_log" || true)
[ "$xcodebuild_after" -eq $((xcodebuild_before + 1)) ]

cp "$runtime" "$test_root/runtime.saved"
/usr/bin/python3 - "$runtime" <<'PY'
import json, os, sys

path = sys.argv[1]
with open(path, encoding="utf-8") as handle:
    value = json.load(handle)
value["paths"]["cache"] = os.path.expanduser("~/Library/Caches/ai.clumsies")
with open(path, "w", encoding="utf-8") as handle:
    json.dump(value, handle)
PY
if run reset > "$test_root/tampered.out" 2> "$test_root/tampered.err"; then
  echo "expected tampered ownership to be rejected" >&2
  exit 1
fi
grep -F 'path cache is not owned' "$test_root/tampered.err" >/dev/null
cp "$test_root/runtime.saved" "$runtime"
chmod 600 "$runtime"

cp "$runtime" "$test_root/runtime-process.saved"
/bin/sleep 30 &
victim_pid=$!
/usr/bin/python3 - "$runtime" "$victim_pid" <<'PY'
import json, os, sys

path, victim_pid = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    value = json.load(handle)
value["processes"]["server_pid"] = int(victim_pid)
value["processes"]["server_start_identity"] = "forged-start-identity"
temporary = f"{path}.{os.getpid()}.tmp"
with open(temporary, "x", encoding="utf-8") as handle:
    json.dump(value, handle)
os.chmod(temporary, 0o600)
os.replace(temporary, path)
PY
if run down > "$test_root/tampered-pid.out" 2> "$test_root/tampered-pid.err"; then
  echo "expected an unregistered PID to be rejected" >&2
  exit 1
fi
grep -F 'no longer belongs to this instance Server' "$test_root/tampered-pid.err" >/dev/null
kill -0 "$victim_pid"
cp "$test_root/runtime-process.saved" "$runtime"
chmod 600 "$runtime"
kill "$victim_pid"
wait "$victim_pid" 2>/dev/null || true
victim_pid=

run down
[ "$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["state"])' "$runtime")" = stopped ]
[ -f "$server_launch_agent_plist" ]
stopped_xcodebuild_before=$(grep -c '^xcodebuild ' "$fake_log" || true)
if run test-live > "$test_root/stopped-live.out" 2> "$test_root/stopped-live.err"; then
  echo "expected test-live to reject a stopped Dev Instance" >&2
  exit 1
fi
grep -F 'Dev Instance is not running' "$test_root/stopped-live.err" >/dev/null
stopped_xcodebuild_after=$(grep -c '^xcodebuild ' "$fake_log" || true)
[ "$stopped_xcodebuild_before" -eq "$stopped_xcodebuild_after" ]
grep -F -- "pkill -TERM -f -x" "$fake_log" >/dev/null
grep -F -- "launchctl bootstrap gui/$(id -u) $server_launch_agent_plist" "$fake_log" >/dev/null
grep -F -- "launchctl print gui/$(id -u)/$server_label" "$fake_log" >/dev/null
grep -F -- "launchctl bootout gui/$(id -u)/$server_label" "$fake_log" >/dev/null

rm "$compose_env"
run logs >/dev/null
[ ! -e "$compose_env" ]
[ ! -e "$recovery_compose_env" ]
run down
[ ! -e "$compose_env" ]
[ ! -e "$recovery_compose_env" ]
if run up > "$test_root/missing-secrets.out" 2> "$test_root/missing-secrets.err"; then
  echo "expected missing instance secrets to require reset" >&2
  exit 1
fi
grep -F 'instance secrets are missing; run reset' "$test_root/missing-secrets.err" >/dev/null
[ ! -e "$compose_env" ]

FAKE_PAUSE_RESET_RM=1 runner_environment "$runner" reset \
  > "$test_root/paused-reset.out" 2> "$test_root/paused-reset.err" &
reset_pid=$!
attempts=0
while [ ! -f "$reset_rm_started" ] && [ "$attempts" -lt 100 ]; do
  /bin/sleep 0.05
  attempts=$((attempts + 1))
done
[ -f "$reset_rm_started" ]
reset_lock_pid=$(sed -n '1p' "$instance_lock/pid")
case "$reset_lock_pid" in
  ''|*[!0-9]*) exit 1 ;;
esac
kill -0 "$reset_lock_pid"
if run up > "$test_root/concurrent-up.out" 2> "$test_root/concurrent-up.err"; then
  echo "expected reset to hold the lifecycle lock through instance deletion" >&2
  exit 1
fi
grep -F 'another lifecycle command is active' "$test_root/concurrent-up.err" >/dev/null
: > "$reset_rm_continue"
wait "$reset_pid"
reset_pid=
[ ! -e "$instance_root" ]
[ ! -e "$instance_lock" ]
grep -F -- "down -v --remove-orphans" "$fake_log" >/dev/null
grep -F -- "security delete-generic-password -s ai.clumsies.dev.$instance_id -a server-session" "$fake_log" >/dev/null

mkdir "$instance_lock"
if run up > "$test_root/missing-lock-pid.out" 2> "$test_root/missing-lock-pid.err"; then
  echo "expected a pidless lifecycle lock to require manual review" >&2
  exit 1
fi
grep -F 'lifecycle lock requires manual review' "$test_root/missing-lock-pid.err" >/dev/null
[ -d "$instance_lock" ]
rmdir "$instance_lock"
mkdir "$instance_lock"
printf 'not-a-pid\n' > "$instance_lock/pid"
if run up > "$test_root/invalid-lock-pid.out" 2> "$test_root/invalid-lock-pid.err"; then
  echo "expected an invalid lifecycle lock to require manual review" >&2
  exit 1
fi
grep -F 'lifecycle lock requires manual review' "$test_root/invalid-lock-pid.err" >/dev/null
grep -Fx 'not-a-pid' "$instance_lock/pid" >/dev/null
rm "$instance_lock/pid"
rmdir "$instance_lock"
rm -rf -- "$instance_root"

symlink_target=$test_root/stable-symlink-target
mkdir -p "$symlink_target" "$dev_root/instances"
printf 'stable-target\n' > "$symlink_target/sentinel"
ln -s "$symlink_target" "$instance_root"
docker_before_symlink=$(grep -c '^docker ' "$fake_log" || true)
if run up > "$test_root/symlink-root.out" 2> "$test_root/symlink-root.err"; then
  echo "expected a symlinked instance root to be rejected" >&2
  exit 1
fi
grep -F 'instance root must not be a symlink' "$test_root/symlink-root.err" >/dev/null
[ "$(grep -c '^docker ' "$fake_log" || true)" -eq "$docker_before_symlink" ]
grep -F 'stable-target' "$symlink_target/sentinel" >/dev/null
rm "$instance_root"
rm -rf -- "$symlink_target"

bad_preview=$test_root/bad-preview.json
printf '%s\n' '{"schema_version":1,"environment_id":"pr-73","server_url":"http://preview.example.test","expires_at":"2099-01-01T00:00:00Z"}' > "$bad_preview"
if run up --preview "$bad_preview" > "$test_root/bad-preview.out" 2> "$test_root/bad-preview.err"; then
  echo "expected insecure Preview URL to be rejected" >&2
  exit 1
fi
grep -F 'must be an HTTPS origin' "$test_root/bad-preview.err" >/dev/null

stable_preview=$test_root/stable-preview.json
printf '%s\n' '{"schema_version":1,"environment_id":"production","server_url":"https://app.clumsies.ai.","expires_at":"2099-01-01T00:00:00Z"}' > "$stable_preview"
if run up --preview "$stable_preview" > "$test_root/stable-preview.out" 2> "$test_root/stable-preview.err"; then
  echo "expected the stable production Server to be rejected as a Preview" >&2
  exit 1
fi
grep -F 'must not target the stable production Server' "$test_root/stable-preview.err" >/dev/null

expired_preview=$test_root/expired-preview.json
printf '%s\n' '{"schema_version":1,"environment_id":"pr-73","server_url":"https://pr-73.example.test","expires_at":"2020-01-01T00:00:00Z"}' > "$expired_preview"
if run up --preview "$expired_preview" > "$test_root/expired-preview.out" 2> "$test_root/expired-preview.err"; then
  echo "expected an expired Preview descriptor to be rejected" >&2
  exit 1
fi
grep -F 'Preview descriptor has expired' "$test_root/expired-preview.err" >/dev/null

preview_a=$test_root/preview-a.json
preview_b=$test_root/preview-b.json
printf '%s\n' '{"schema_version":1,"mode":"preview","environment_id":"pr-73","server_url":"https://pr-73.example.test","oidc_issuer":"https://oidc.example.test","expires_at":"2099-01-01T00:00:00Z"}' > "$preview_a"
printf '%s\n' '{"schema_version":1,"mode":"preview","environment_id":"pr-74","server_url":"https://pr-73.example.test","oidc_issuer":"https://oidc.example.test","expires_at":"2099-01-01T00:00:00Z"}' > "$preview_b"
docker_before=$(grep -c '^docker ' "$fake_log" || true)
FAKE_CURL_FAIL=1
export FAKE_CURL_FAIL
if run up --preview "$preview_a" > "$test_root/unhealthy-preview.out" 2> "$test_root/unhealthy-preview.err"; then
  echo "expected an unhealthy Preview Server to be rejected" >&2
  exit 1
fi
unset FAKE_CURL_FAIL
grep -F 'Preview Server is not healthy' "$test_root/unhealthy-preview.err" >/dev/null
[ ! -f "$runtime" ]
[ ! -f "$instance_root/preview.json" ]
[ ! -f "$fake_app_state" ]
[ ! -f "$fake_daemon_state" ]

FAKE_OPEN_SERVER_URL=https://pr-73.example.test
export FAKE_OPEN_SERVER_URL
preview_curl_before=$(grep -c '^curl ' "$fake_log" || true)
run up --preview "$preview_a" >/dev/null
preview_curl_after=$(grep -c '^curl ' "$fake_log" || true)
[ "$preview_curl_after" -ge $((preview_curl_before + 2)) ]
if run up --preview "$preview_b" > "$test_root/changed-preview.out" 2> "$test_root/changed-preview.err"; then
  echo "expected a changed Preview identity to require reset" >&2
  exit 1
fi
grep -F 'run reset' "$test_root/changed-preview.err" >/dev/null
[ "$(/usr/bin/python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["state"])' "$runtime")" = running ]
/usr/bin/python3 - "$instance_root/preview.json" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    value = json.load(handle)
assert value["environment_id"] == "pr-73"
assert value["server_url"] == "https://pr-73.example.test"
assert value["expires_at"] == "2099-01-01T00:00:00Z"
PY
run down
run reset
unset FAKE_OPEN_SERVER_URL
docker_after=$(grep -c '^docker ' "$fake_log" || true)
[ "$docker_before" -eq "$docker_after" ]

grep -F 'stable-app' "$test_home/Applications/Clumsies.app/sentinel" >/dev/null
grep -F 'stable-daemon' "$test_home/Library/Application Support/ai.clumsies/sentinel" >/dev/null
grep -F 'stable-codex' "$test_home/.codex/sentinel" >/dev/null
[ ! -s "$fake_reject_log" ]

printf 'dev instance contract: ok\n'
