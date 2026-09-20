#!/bin/sh
set -eu

umask 077
unset CDPATH

die() {
  printf 'clumsies dev: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat >&2 <<'EOF'
usage: dev/dev-instance.sh up [--review-playground | --preview DESCRIPTOR.json]
       dev/dev-instance.sh status
       dev/dev-instance.sh logs
       dev/dev-instance.sh test-live
       dev/dev-instance.sh down
       dev/dev-instance.sh reset
EOF
  exit 64
}

command_name=${1:-}
[ -n "$command_name" ] || usage
shift

case "$command_name" in
  up|status|logs|test-live|down|reset) ;;
  *) usage ;;
esac

python=${PYTHON:-/usr/bin/python3}
[ -x "$python" ] || die "python3 is required"

script_dir=$(cd -- "$(dirname -- "$0")" && pwd -P)
repo_root=$(git -C "$script_dir/.." rev-parse --show-toplevel 2>/dev/null) \
  || die "the runner must be inside a git worktree"
repo_root=$(cd -- "$repo_root" && pwd -P)
instance_id=$(printf '%s' "$repo_root" | shasum -a 256 | awk '{print substr($1, 1, 12)}')
case "$instance_id" in
  [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
  *) die "could not derive a valid instance id" ;;
esac

dev_root=${CLUMSIES_DEV_ROOT:-$HOME/Library/Application Support/ai.clumsies.dev}
case "$dev_root" in
  /*) ;;
  *) die "CLUMSIES_DEV_ROOT must be absolute" ;;
esac
instances_root=$dev_root/instances
instance_root=$instances_root/$instance_id
runtime_file=$instance_root/runtime.json
instance_compose_env=$instance_root/compose.env
compose_env=$instance_compose_env
ready_file=$instance_root/server-ready.json
locks_root=$instances_root/.locks
lock_dir=$locks_root/$instance_id
logs_dir=$instance_root/logs
daemon_root=$instance_root/daemon
daemon_cache=$instance_root/cache
daemon_logs=$logs_dir/daemon
launch_agents=$instance_root/LaunchAgents
codex_home=$instance_root/codex-home
server_bin_dir=$instance_root/bin
owned_server_binary=$server_bin_dir/clumsies-server
server_launcher=$server_bin_dir/dev-server
server_launcher_source=$repo_root/dev/dev-server.sh
derived_data=$instance_root/macos-derived
compose_project=clumsies-dev-$instance_id
bundle_id=ai.clumsies.desktop.dev.$instance_id
daemon_label=ai.clumsies.daemon.dev.$instance_id
server_label=ai.clumsies.server.dev.$instance_id
server_launch_agent_plist=$launch_agents/$server_label.plist
keychain_service=ai.clumsies.dev.$instance_id
product_name=ClumsiesDev-$instance_id
display_name="Clumsies Dev $instance_id"
app_path=$derived_data/Build/Products/Debug/$product_name.app
app_executable=$app_path/Contents/MacOS/$product_name
app_process_pattern=$(printf '%s' "$app_executable" | sed 's/[][\\.^$*+?(){}|]/\\&/g')
server_log=$logs_dir/server.log
app_stdout=$logs_dir/app.out.log
app_stderr=$logs_dir/app.err.log

compose() {
  docker compose \
    --env-file "$compose_env" \
    -f "$repo_root/docker-compose.yml" \
    -p "$compose_project" \
    "$@"
}

json_get() {
  "$python" - "$1" "$2" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    value = json.load(handle)
for component in sys.argv[2].split("."):
    if not isinstance(value, dict) or component not in value:
        sys.exit(2)
    value = value[component]
if value is None:
    print("")
elif isinstance(value, bool):
    print("true" if value else "false")
elif isinstance(value, (str, int)):
    rendered = str(value)
    if any(character in rendered for character in "\r\n\t"):
        sys.exit(2)
    print(rendered)
else:
    sys.exit(2)
PY
}

validate_runtime() {
  [ -f "$runtime_file" ] || die "no runtime descriptor for $instance_id"
  "$python" - \
    "$runtime_file" "$instance_id" "$repo_root" "$instance_root" \
    "$bundle_id" "$daemon_label" "$server_label" "$keychain_service" "$compose_project" \
    "$app_path" "$owned_server_binary" "$server_launcher" "$server_launch_agent_plist" \
    "$daemon_root" "$daemon_cache" "$daemon_logs" "$launch_agents" "$codex_home" <<'PY'
import json, sys

(
    path, instance_id, worktree, root, bundle, label, server_label, keychain, compose,
    app, owned_server_binary, server_launcher, server_launch_agent_plist,
    daemon_root, cache, logs, launch_agents, codex_home,
) = sys.argv[1:]
try:
    with open(path, encoding="utf-8") as handle:
        value = json.load(handle)
except (OSError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid runtime descriptor: {error}")

if not isinstance(value, dict) or value.get("schema_version") != 1:
    raise SystemExit("invalid runtime descriptor schema")
expected = {
    "instance_id": instance_id,
    "worktree_path": worktree,
}
for key, wanted in expected.items():
    if value.get(key) != wanted:
        raise SystemExit(f"runtime descriptor {key} does not match this worktree")
identities = value.get("identities")
paths = value.get("paths")
if not isinstance(identities, dict) or not isinstance(paths, dict):
    raise SystemExit("runtime descriptor ownership is incomplete")
for key, wanted in {
    "bundle_id": bundle,
    "launch_agent_label": label,
    "mach_service": label,
    "server_launch_agent_label": server_label,
    "keychain_service": keychain,
}.items():
    if identities.get(key) != wanted:
        raise SystemExit(f"runtime descriptor identity {key} is not owned by this instance")
for key, wanted in {
    "instance_root": root,
    "app": app,
    "server_launcher": server_launcher,
    "server_launch_agent_plist": server_launch_agent_plist,
    "daemon_root": daemon_root,
    "cache": cache,
    "logs": logs,
    "launch_agents": launch_agents,
    "codex_home": codex_home,
}.items():
    if paths.get(key) != wanted:
        raise SystemExit(f"runtime descriptor path {key} is not owned by this instance")
mode = value.get("mode")
if mode not in {"local", "preview"}:
    raise SystemExit("runtime descriptor mode is invalid")
processes = value.get("processes")
if not isinstance(processes, dict):
    raise SystemExit("runtime descriptor process ownership is incomplete")
server_pid = processes.get("server_pid")
server_start_identity = processes.get("server_start_identity")
if server_pid is not None and (isinstance(server_pid, bool) or not isinstance(server_pid, int) or server_pid <= 1):
    raise SystemExit("runtime descriptor Server PID is invalid")
if server_start_identity is not None and (
    not isinstance(server_start_identity, str)
    or not server_start_identity
    or any(character in server_start_identity for character in "\r\n\t")
):
    raise SystemExit("runtime descriptor Server start identity is invalid")
if (server_pid is None) != (server_start_identity is None):
    raise SystemExit("runtime descriptor Server process identity is incomplete")
if mode == "local":
    if value.get("compose_project") != compose:
        raise SystemExit("runtime descriptor Compose project is not owned by this instance")
    if paths.get("server_binary") != owned_server_binary:
        raise SystemExit("runtime descriptor path server_binary is not owned by this instance")
else:
    if value.get("compose_project") is not None or paths.get("server_binary") is not None:
        raise SystemExit("Preview runtime descriptor contains local Server ownership")
    if server_pid is not None:
        raise SystemExit("Preview runtime descriptor contains a local Server process")
    if not isinstance(value.get("preview_environment_id"), str) or not value["preview_environment_id"]:
        raise SystemExit("Preview runtime descriptor environment identity is invalid")
    if not isinstance(value.get("preview_expires_at"), str) or not value["preview_expires_at"]:
        raise SystemExit("Preview runtime descriptor expiry is invalid")
if value.get("state") not in {"starting", "running", "stopped"}:
    raise SystemExit("runtime descriptor state is invalid")
server_url = value.get("server_url")
if not isinstance(server_url, str) or not server_url:
    raise SystemExit("runtime descriptor Server URL is invalid")
if any(character in server_url for character in "\r\n\t"):
    raise SystemExit("runtime descriptor contains control characters")
PY
}

normalize_preview() {
  preview_source=$1
  preview_target=$2
  [ -f "$preview_source" ] || die "Preview descriptor does not exist: $preview_source"
  "$python" - "$preview_source" "$preview_target" <<'PY'
import datetime, json, os, re, sys, urllib.parse

source, target = sys.argv[1:]
try:
    with open(source, encoding="utf-8") as handle:
        value = json.load(handle)
except (OSError, json.JSONDecodeError) as error:
    raise SystemExit(f"invalid Preview descriptor: {error}")

allowed = {"schema_version", "mode", "environment_id", "server_url", "oidc_issuer", "expires_at"}
if not isinstance(value, dict) or set(value) - allowed:
    raise SystemExit("Preview descriptor contains unsupported fields")
if value.get("schema_version") != 1 or value.get("mode", "preview") != "preview":
    raise SystemExit("Preview descriptor schema or mode is invalid")
environment_id = value.get("environment_id")
if not isinstance(environment_id, str) or not re.fullmatch(r"[A-Za-z0-9._-]{1,128}", environment_id):
    raise SystemExit("Preview environment_id is invalid")

def remote_origin(name, required):
    raw = value.get(name)
    if raw is None and not required:
        return None
    if not isinstance(raw, str):
        raise SystemExit(f"Preview {name} is required")
    parsed = urllib.parse.urlsplit(raw)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password:
        raise SystemExit(f"Preview {name} must be an HTTPS origin without credentials")
    if parsed.hostname.rstrip(".").lower() == "app.clumsies.ai":
        raise SystemExit(f"Preview {name} must not target the stable production Server")
    if parsed.path not in {"", "/"} or parsed.query or parsed.fragment:
        raise SystemExit(f"Preview {name} must contain only an origin")
    return raw.rstrip("/")

normalized = {
    "schema_version": 1,
    "mode": "preview",
    "environment_id": environment_id,
    "server_url": remote_origin("server_url", True),
    "oidc_issuer": remote_origin("oidc_issuer", False),
    "expires_at": value.get("expires_at"),
}
expires_at = normalized["expires_at"]
if not isinstance(expires_at, str) or not re.fullmatch(
    r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})",
    expires_at,
):
    raise SystemExit("Preview expires_at must be an RFC3339 timestamp")
try:
    expires = datetime.datetime.fromisoformat(expires_at.replace("Z", "+00:00"))
except ValueError as error:
    raise SystemExit(f"Preview expires_at must be an RFC3339 timestamp: {error}")
if expires <= datetime.datetime.now(datetime.timezone.utc):
    raise SystemExit("Preview descriptor has expired")
temporary = f"{target}.{os.getpid()}.tmp"
with open(temporary, "x", encoding="utf-8") as handle:
    json.dump(normalized, handle, separators=(",", ":"), sort_keys=True)
    handle.write("\n")
    handle.flush()
    os.fsync(handle.fileno())
os.chmod(temporary, 0o600)
os.replace(temporary, target)
PY
}

write_runtime() {
  "$python" - \
    "$runtime_file" "$runtime_mode" "$runtime_state" "$instance_id" "$repo_root" \
    "$commit_id" "$build_id" "$server_url" "$oidc_issuer" "$preview_environment_id" "$preview_expires_at" \
    "$compose_project_value" "$server_port" "$database_port" "$oidc_port" \
    "$server_pid" "$server_start_identity" "$bundle_id" "$daemon_label" "$server_label" "$keychain_service" \
    "$instance_root" "$app_path" "$server_binary" "$server_launcher" "$server_launch_agent_plist" \
    "$daemon_root" "$daemon_cache" \
    "$daemon_logs" "$launch_agents" "$codex_home" <<'PY'
import datetime, json, os, sys

(
    path, mode, state, instance_id, worktree, commit, build_id, server_url, oidc_issuer,
    environment_id, preview_expires_at, compose_project, server_port, database_port, oidc_port,
    server_pid, server_start_identity, bundle_id, daemon_label, server_label, keychain_service, instance_root,
    app_path, server_binary, server_launcher, server_launch_agent_plist,
    daemon_root, daemon_cache, daemon_logs,
    launch_agents, codex_home,
) = sys.argv[1:]

def number(value):
    return int(value) if value else None

value = {
    "schema_version": 1,
    "mode": mode,
    "state": state,
    "instance_id": instance_id,
    "worktree_path": worktree,
    "commit": commit,
    "build_id": build_id,
    "server_url": server_url,
    "oidc_issuer": oidc_issuer or None,
    "preview_environment_id": environment_id or None,
    "preview_expires_at": preview_expires_at or None,
    "compose_project": compose_project or None,
    "ports": {
        "server": number(server_port),
        "postgres": number(database_port),
        "oidc": number(oidc_port),
    },
    "processes": {
        "server_pid": number(server_pid),
        "server_start_identity": server_start_identity or None,
    },
    "identities": {
        "bundle_id": bundle_id,
        "launch_agent_label": daemon_label,
        "mach_service": daemon_label,
        "server_launch_agent_label": server_label,
        "keychain_service": keychain_service,
    },
    "paths": {
        "instance_root": instance_root,
        "app": app_path,
        "server_binary": server_binary or None,
        "server_launcher": server_launcher,
        "server_launch_agent_plist": server_launch_agent_plist,
        "daemon_root": daemon_root,
        "cache": daemon_cache,
        "logs": daemon_logs,
        "launch_agents": launch_agents,
        "codex_home": codex_home,
    },
    "updated_at": datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z"),
}
temporary = f"{path}.{os.getpid()}.tmp"
with open(temporary, "x", encoding="utf-8") as handle:
    json.dump(value, handle, indent=2, sort_keys=True)
    handle.write("\n")
    handle.flush()
    os.fsync(handle.fileno())
os.chmod(temporary, 0o600)
os.replace(temporary, path)
PY
}

set_runtime_state() {
  "$python" - "$runtime_file" "$1" <<'PY'
import datetime, json, os, sys

path, state = sys.argv[1:]
with open(path, encoding="utf-8") as handle:
    value = json.load(handle)
value["state"] = state
value["updated_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z")
temporary = f"{path}.{os.getpid()}.tmp"
with open(temporary, "x", encoding="utf-8") as handle:
    json.dump(value, handle, indent=2, sort_keys=True)
    handle.write("\n")
    handle.flush()
    os.fsync(handle.fileno())
os.chmod(temporary, 0o600)
os.replace(temporary, path)
PY
}

acquire_lock() {
  mkdir -p "$instances_root"
  [ ! -L "$instances_root" ] || die "instances root must not be a symlink"
  canonical_dev_root=$(cd -- "$dev_root" && pwd -P)
  canonical_instances_root=$(cd -- "$instances_root" && pwd -P)
  [ "$canonical_instances_root" = "$canonical_dev_root/instances" ] \
    || die "instances root escapes CLUMSIES_DEV_ROOT"
  [ ! -L "$locks_root" ] || die "lifecycle locks root must not be a symlink"
  mkdir -p "$locks_root"
  canonical_locks_root=$(cd -- "$locks_root" && pwd -P)
  [ "$canonical_locks_root" = "$canonical_instances_root/.locks" ] \
    || die "lifecycle locks root escapes instances root"
  chmod 700 "$locks_root"
  [ ! -L "$instance_root" ] || die "instance root must not be a symlink"
  mkdir -p "$instance_root"
  canonical_instance_root=$(cd -- "$instance_root" && pwd -P)
  [ "$canonical_instance_root" = "$canonical_instances_root/$instance_id" ] \
    || die "instance root is not owned by this worktree"
  chmod 700 "$instance_root"
  if ! mkdir "$lock_dir" 2>/dev/null; then
    [ ! -L "$lock_dir" ] || die "lifecycle lock must not be a symlink"
    lock_pid=$(sed -n '1p' "$lock_dir/pid" 2>/dev/null || true)
    case "$lock_pid" in
      ''|*[!0-9]*) die "lifecycle lock requires manual review: $lock_dir" ;;
    esac
    if ! [ "$lock_pid" -gt 1 ] 2>/dev/null; then
      die "lifecycle lock requires manual review: $lock_dir"
    fi
    if kill -0 "$lock_pid" 2>/dev/null; then
      die "another lifecycle command is active for $instance_id"
    fi
    rm -f "$lock_dir/pid" "$lock_dir/compose.env" "$lock_dir/compose.env.$lock_pid.tmp"
    rmdir "$lock_dir" 2>/dev/null || die "stale lifecycle lock requires manual review: $lock_dir"
    mkdir "$lock_dir"
  fi
  printf '%s\n' "$$" > "$lock_dir/pid"
  lock_held=1
}

release_lock() {
  if [ "${lock_held:-0}" = 1 ]; then
    rm -f "$lock_dir/pid"
    rmdir "$lock_dir" 2>/dev/null || true
    lock_held=0
  fi
}

cleanup_recovery_compose_env() {
  if [ -n "${recovery_compose_env:-}" ]; then
    rm -f "$recovery_compose_env"
    recovery_compose_env=
    compose_env=$instance_compose_env
  fi
}

write_compose_env() {
  db_port_value=$1
  oidc_port_value=$2
  temporary=$compose_env.$$.tmp
  {
    printf 'CLUMSIES_HOST_BIND_ADDRESS=127.0.0.1\n'
    printf 'CLUMSIES_DB_NAME=clumsies\n'
    printf 'CLUMSIES_DB_USER=clumsies\n'
    printf 'CLUMSIES_DB_PASSWORD=%s\n' "$database_password"
    printf 'CLUMSIES_DB_PORT=%s\n' "$db_port_value"
    printf 'CLUMSIES_OIDC_PORT=%s\n' "$oidc_port_value"
    printf 'CLUMSIES_SETUP_CODE=%s\n' "$setup_code"
  } > "$temporary"
  chmod 600 "$temporary"
  mv -f "$temporary" "$compose_env"
}

write_server_launch_agent() {
  "$python" - \
    "$server_launch_agent_plist" "$server_label" "$server_launcher" "$compose_env" \
    "$server_binary" "$server_address" "$ready_file" "$server_log" <<'PY'
import os, plistlib, sys

(
    path, label, launcher, compose_env, server_binary, server_address,
    ready_file, server_log,
) = sys.argv[1:]
value = {
    "Label": label,
    "ProgramArguments": [
        launcher,
        compose_env,
        server_binary,
        server_address,
        ready_file,
    ],
    "RunAtLoad": True,
    "KeepAlive": True,
    "StandardOutPath": server_log,
    "StandardErrorPath": server_log,
}
temporary = f"{path}.{os.getpid()}.tmp"
with open(temporary, "xb") as handle:
    plistlib.dump(value, handle)
    handle.flush()
    os.fsync(handle.fileno())
os.chmod(temporary, 0o600)
os.replace(temporary, path)
PY
}

load_or_create_secrets() {
  database_password=
  setup_code=
  if [ -f "$compose_env" ]; then
    database_password=$(awk -F= '$1 == "CLUMSIES_DB_PASSWORD" { print substr($0, index($0, "=") + 1) }' "$compose_env")
    setup_code=$(awk -F= '$1 == "CLUMSIES_SETUP_CODE" { print substr($0, index($0, "=") + 1) }' "$compose_env")
  fi
  case "$database_password" in
    ''|*[!0-9a-f]*) database_password=$("$python" -c 'import secrets; print(secrets.token_hex(24))') ;;
    *) ;;
  esac
  case "$setup_code" in
    ''|*[!0-9a-f]*) setup_code=$("$python" -c 'import secrets; print(secrets.token_hex(24))') ;;
    *) ;;
  esac
  [ "${#database_password}" -eq 48 ] || die "invalid instance database password"
  [ "${#setup_code}" -eq 48 ] || die "invalid instance setup code"
}

published_port() {
  address=$(compose port "$1" "$2") || return 1
  case "$address" in
    *:*) port=${address##*:} ;;
    *) return 1 ;;
  esac
  case "$port" in
    ''|*[!0-9]*) return 1 ;;
  esac
  [ "$port" -ge 1 ] && [ "$port" -le 65535 ] || return 1
  printf '%s\n' "$port"
}

server_is_healthy() {
  health=$(curl --fail --silent --max-time 3 "$server_url/api/v1/admin/health" 2>/dev/null) || return 1
  printf '%s' "$health" | "$python" -c \
    'import json,sys; raise SystemExit(0 if json.load(sys.stdin).get("status") == "ok" else 1)' \
    >/dev/null 2>&1
}

daemon_is_running() {
  uid=$(id -u)
  launchctl print "gui/$uid/$daemon_label" >/dev/null 2>&1
}

app_is_running() {
  pgrep -f -x "$app_process_pattern" >/dev/null 2>&1
}

server_process_command() {
  ps -p "$1" -o command= 2>/dev/null | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'
}

server_process_start_identity() {
  ps -p "$1" -o lstart= 2>/dev/null | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'
}

server_job_pid() {
  uid=$(id -u)
  launchctl print "gui/$uid/$server_label" 2>/dev/null \
    | awk '$1 == "pid" && $2 == "=" { print $3; exit }'
}

server_process_is_owned() {
  [ -n "${server_pid:-}" ] && [ -n "${server_start_identity:-}" ] || return 1
  case "$server_pid" in
    *[!0-9]*) return 1 ;;
  esac
  [ "$server_pid" -gt 1 ] || return 1
  [ "$server_binary" = "$owned_server_binary" ] || return 1
  kill -0 "$server_pid" 2>/dev/null || return 1
  [ "$(server_process_command "$server_pid")" = "$owned_server_binary" ] || return 1
  [ "$(server_process_start_identity "$server_pid")" = "$server_start_identity" ] || return 1
}

compose_is_running() {
  [ -f "$compose_env" ] || return 1
  services=$(compose ps --status running --services 2>/dev/null) || return 1
  printf '%s\n' "$services" | grep -qx postgres \
    && printf '%s\n' "$services" | grep -qx fake-oidc
}

print_status() {
  validate_runtime
  mode=$(json_get "$runtime_file" mode)
  server_url=$(json_get "$runtime_file" server_url)
  server_pid=$(json_get "$runtime_file" processes.server_pid)
  server_start_identity=$(json_get "$runtime_file" processes.server_start_identity)
  server_binary=$(json_get "$runtime_file" paths.server_binary)
  server_running=false
  compose_running=false
  daemon_running=false
  app_running=false
  healthy=false
  if [ "$mode" = preview ] \
    || { [ "$(server_job_pid || true)" = "$server_pid" ] && server_process_is_owned; }; then
    server_running=true
  fi
  if [ "$mode" = preview ] || compose_is_running; then compose_running=true; fi
  if daemon_is_running; then daemon_running=true; fi
  if app_is_running; then app_running=true; fi
  if server_is_healthy; then healthy=true; fi
  "$python" - "$runtime_file" \
    "$server_running" "$compose_running" "$daemon_running" "$app_running" "$healthy" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    descriptor = json.load(handle)
names = ["server", "compose", "daemon", "app", "health"]
observed = {name: value == "true" for name, value in zip(names, sys.argv[2:])}
print(json.dumps({"descriptor": descriptor, "observed": observed}, indent=2, sort_keys=True))
raise SystemExit(0 if all(observed.values()) else 1)
PY
}

stop_server_job() {
  if [ -n "$server_pid" ] && kill -0 "$server_pid" 2>/dev/null; then
    server_process_is_owned || return 1
  fi
  uid=$(id -u)
  if launchctl print "gui/$uid/$server_label" >/dev/null 2>&1; then
    launchctl bootout "gui/$uid/$server_label" >/dev/null 2>&1 || return 1
  fi
  attempts=0
  while [ -n "$server_pid" ] && kill -0 "$server_pid" 2>/dev/null && [ "$attempts" -lt 50 ]; do
    sleep 0.1
    attempts=$((attempts + 1))
  done
  if [ -n "$server_pid" ] && kill -0 "$server_pid" 2>/dev/null; then
    server_process_is_owned || return 1
    kill -KILL "$server_pid" 2>/dev/null || true
  fi
}

ensure_recovery_compose_env() {
  if [ -f "$instance_compose_env" ]; then
    compose_env=$instance_compose_env
    return 0
  fi
  recovery_compose_env=$lock_dir/compose.env
  compose_env=$recovery_compose_env
  database_password=$("$python" -c 'print("0" * 48)')
  setup_code=$("$python" -c 'print("0" * 48)')
  database_port=$(json_get "$runtime_file" ports.postgres)
  oidc_port=$(json_get "$runtime_file" ports.oidc)
  write_compose_env "${database_port:-0}" "${oidc_port:-0}"
}

stop_instance() {
  validate_runtime
  mode=$(json_get "$runtime_file" mode)
  server_binary=$(json_get "$runtime_file" paths.server_binary)
  server_pid=$(json_get "$runtime_file" processes.server_pid)
  server_start_identity=$(json_get "$runtime_file" processes.server_start_identity)

  stop_server_job || die "PID $server_pid no longer belongs to this instance Server"
  osascript -e "tell application id \"$bundle_id\" to quit" >/dev/null 2>&1 || true
  pkill -TERM -f -x "$app_process_pattern" >/dev/null 2>&1 || true
  uid=$(id -u)
  launchctl bootout "gui/$uid/$daemon_label" >/dev/null 2>&1 || true
  rm -f "$ready_file"
  if [ "$mode" = local ]; then
    ensure_recovery_compose_env
    compose down "$@" --remove-orphans
    cleanup_recovery_compose_env
  fi
  set_runtime_state stopped
}

cleanup_failed_up() {
  rm -f "${preview_candidate:-}"
  [ "${up_complete:-0}" = 1 ] && return 0
  [ "${up_mutated:-0}" = 1 ] || return 0
  osascript -e "tell application id \"$bundle_id\" to quit" >/dev/null 2>&1 || true
  pkill -TERM -f -x "$app_process_pattern" >/dev/null 2>&1 || true
  uid=$(id -u 2>/dev/null || true)
  if [ -n "$uid" ]; then
    launchctl bootout "gui/$uid/$daemon_label" >/dev/null 2>&1 || true
  fi
  if [ "${server_submitted:-0}" = 1 ]; then
    uid=$(id -u 2>/dev/null || true)
    [ -z "$uid" ] \
      || launchctl bootout "gui/$uid/$server_label" >/dev/null 2>&1 \
      || true
  fi
  if [ "${compose_started:-0}" = 1 ]; then
    compose down --remove-orphans >/dev/null 2>&1 || true
  fi
  if [ -f "$runtime_file" ]; then
    set_runtime_state stopped 2>/dev/null || true
  fi
}

on_exit() {
  status=$?
  trap - 0
  if [ "$command_name" = up ] && [ "$status" -ne 0 ]; then
    cleanup_failed_up
  fi
  cleanup_recovery_compose_env
  release_lock
  exit "$status"
}

install_executable() {
  source_executable=$1
  target_executable=$2
  temporary=$target_executable.$$.tmp
  cp "$source_executable" "$temporary"
  chmod 700 "$temporary"
  mv -f "$temporary" "$target_executable"
}

run_up() {
  preview_source=
  review_playground=0
  case $# in
    0) ;;
    1)
      [ "$1" = --review-playground ] || usage
      review_playground=1
      ;;
    2)
      [ "$1" = --preview ] || usage
      preview_source=$2
      ;;
    *) usage ;;
  esac

  acquire_lock
  trap on_exit 0
  up_mutated=0
  preview_file=$instance_root/preview.json
  preview_candidate=
  mkdir -p "$logs_dir" "$daemon_root" "$daemon_cache" "$daemon_logs" "$launch_agents" "$codex_home" "$server_bin_dir"
  chmod 700 "$logs_dir" "$daemon_root" "$daemon_cache" "$daemon_logs" "$launch_agents" "$codex_home" "$server_bin_dir"

  requested_mode=local
  requested_server_url=
  requested_oidc_issuer=
  requested_preview_environment_id=
  requested_preview_expires_at=
  if [ -n "$preview_source" ]; then
    requested_mode=preview
    preview_candidate=$instance_root/preview.candidate.$$.json
    normalize_preview "$preview_source" "$preview_candidate"
    requested_server_url=$(json_get "$preview_candidate" server_url)
    requested_oidc_issuer=$(json_get "$preview_candidate" oidc_issuer)
    requested_preview_environment_id=$(json_get "$preview_candidate" environment_id)
    requested_preview_expires_at=$(json_get "$preview_candidate" expires_at)
    server_url=$requested_server_url
    server_is_healthy || die "Preview Server is not healthy: $requested_server_url"
  fi

  old_server_url=
  old_server_port=
  old_database_port=
  old_oidc_port=
  if [ -f "$runtime_file" ]; then
    validate_runtime
    old_mode=$(json_get "$runtime_file" mode)
    if [ "$old_mode" = local ] && [ ! -f "$instance_compose_env" ]; then
      die "instance secrets are missing; run reset before starting it again"
    fi
    old_server_url=$(json_get "$runtime_file" server_url)
    [ "$old_mode" = "$requested_mode" ] \
      || die "instance mode changed; run reset before switching from $old_mode to $requested_mode"
    if [ "$requested_mode" = preview ]; then
      old_oidc_issuer=$(json_get "$runtime_file" oidc_issuer)
      old_preview_environment_id=$(json_get "$runtime_file" preview_environment_id)
      old_preview_expires_at=$(json_get "$runtime_file" preview_expires_at)
      if [ "$old_server_url" != "$requested_server_url" ] \
        || [ "$old_oidc_issuer" != "$requested_oidc_issuer" ] \
        || [ "$old_preview_environment_id" != "$requested_preview_environment_id" ] \
        || [ "$old_preview_expires_at" != "$requested_preview_expires_at" ]; then
        die "Preview environment changed; run reset before selecting another Preview environment"
      fi
    fi
    up_mutated=1
    stop_instance
    old_server_port=$(json_get "$runtime_file" ports.server)
    old_database_port=$(json_get "$runtime_file" ports.postgres)
    old_oidc_port=$(json_get "$runtime_file" ports.oidc)
    if [ "$old_server_url" = http://127.0.0.1:0 ]; then
      old_server_url=
      old_server_port=
    fi
  fi

  commit_id=$(git -C "$repo_root" rev-parse HEAD)
  commit_short=$(printf '%.12s' "$commit_id")
  build_id=dev-$instance_id-$commit_short
  [ -z "$(git -C "$repo_root" status --porcelain --untracked-files=normal)" ] \
    || build_id=$build_id-dirty
  runtime_mode=$requested_mode
  runtime_state=starting
  server_url=$requested_server_url
  server_port=
  database_port=
  oidc_port=
  server_pid=
  server_start_identity=
  server_binary=
  oidc_issuer=$requested_oidc_issuer
  preview_environment_id=$requested_preview_environment_id
  preview_expires_at=$requested_preview_expires_at
  compose_project_value=
  compose_started=0
  server_submitted=0
  up_complete=0
  up_mutated=1

  if [ "$runtime_mode" = local ]; then
    compose_project_value=$compose_project
    [ -x "$server_launcher_source" ] || die "Server launcher is not executable: $server_launcher_source"

    if [ -n "${CLUMSIES_DEV_SERVER_BIN:-}" ]; then
      server_source_binary=$CLUMSIES_DEV_SERVER_BIN
      if [ ! -f "$server_source_binary" ] || [ ! -x "$server_source_binary" ]; then
        die "CLUMSIES_DEV_SERVER_BIN is not executable"
      fi
    else
      cargo build --locked -p server --bin clumsies-server
      server_source_binary=$repo_root/target/debug/clumsies-server
    fi
    case "$server_source_binary" in
      /*) ;;
      *) die "the Server binary path must be absolute" ;;
    esac
    install_executable "$server_source_binary" "$owned_server_binary"
    install_executable "$server_launcher_source" "$server_launcher"
    server_binary=$owned_server_binary
    server_port=${old_server_port:-0}
    server_url=${old_server_url:-http://127.0.0.1:$server_port}
    database_port=${old_database_port:-0}
    oidc_port=${old_oidc_port:-0}
    load_or_create_secrets
    write_compose_env "$database_port" "$oidc_port"

    # Publish deterministic ownership before the first Compose mutation. A
    # later command can now reclaim even a launch interrupted inside `up`.
    write_runtime
    compose_started=1
    compose up -d --wait postgres fake-oidc
    database_port=$(published_port postgres 5432) \
      || die "could not resolve the PostgreSQL host port"
    oidc_port=$(published_port fake-oidc 8080) \
      || die "could not resolve the fake OIDC host port"
    write_compose_env "$database_port" "$oidc_port"
    write_runtime

    rm -f "$ready_file"
    server_address=127.0.0.1:${old_server_port:-0}
    oidc_issuer=http://127.0.0.1:$oidc_port/clumsies
    uid=$(id -u)
    launchctl print "gui/$uid/$server_label" >/dev/null 2>&1 \
      && die "Server launch job already exists: $server_label"
    write_server_launch_agent
    launchctl bootstrap "gui/$uid" "$server_launch_agent_plist"
    server_submitted=1
    attempts=0
    server_pid=
    server_start_identity=
    while [ -z "$server_start_identity" ] && [ "$attempts" -lt 250 ]; do
      candidate_pid=$(server_job_pid || true)
      if [ -n "$candidate_pid" ] && kill -0 "$candidate_pid" 2>/dev/null \
        && [ "$(server_process_command "$candidate_pid")" = "$owned_server_binary" ]; then
        candidate_start_identity=$(server_process_start_identity "$candidate_pid" || true)
        if [ -n "$candidate_start_identity" ]; then
          server_pid=$candidate_pid
          server_start_identity=$candidate_start_identity
          break
        fi
      fi
      sleep 0.02
      attempts=$((attempts + 1))
    done
    [ -n "$server_start_identity" ] \
      || die "Server launch job did not assume its owned binary; see $server_log"
    server_url=http://127.0.0.1:0
    server_port=0
    write_runtime

    attempts=0
    while [ ! -f "$ready_file" ] && [ "$attempts" -lt "${CLUMSIES_DEV_READY_ATTEMPTS:-600}" ]; do
      kill -0 "$server_pid" 2>/dev/null || die "Server exited before becoming ready; see $server_log"
      sleep 0.1
      attempts=$((attempts + 1))
    done
    [ -f "$ready_file" ] || die "Server readiness timed out; see $server_log"
    server_url=$(json_get "$ready_file" public_origin)
    listen_addr=$(json_get "$ready_file" listen_addr)
    case "$server_url" in
      http://127.0.0.1:*) ;;
      *) die "Server returned a non-loopback development origin" ;;
    esac
    server_port=${listen_addr##*:}
    case "$server_port" in
      ''|*[!0-9]*) die "Server returned an invalid bound port" ;;
    esac
    if [ -n "$old_server_url" ] && [ "$server_url" != "$old_server_url" ]; then
      die "local Server URL changed; run reset before allocating another port"
    fi
    attempts=0
    while ! server_is_healthy && [ "$attempts" -lt 100 ]; do
      kill -0 "$server_pid" 2>/dev/null || die "Server exited during its health check; see $server_log"
      sleep 0.1
      attempts=$((attempts + 1))
    done
    server_is_healthy || die "Server health did not become ready; see $server_log"
  else
    mv -f "$preview_candidate" "$preview_file"
    preview_candidate=
  fi

  # Publish ownership before the comparatively long Xcode build. A later up
  # can then reclaim an interrupted Server/Compose launch without guessing.
  write_runtime

  host_arch=$(uname -m)
  case "$host_arch" in
    arm64|x86_64) ;;
    *) die "unsupported macOS architecture: $host_arch" ;;
  esac
  xcodegen generate --spec "$repo_root/apps/macos/project.yml" >> "$logs_dir/build.log" 2>&1
  xcodebuild \
    -project "$repo_root/apps/macos/Clumsies.xcodeproj" \
    -scheme Clumsies \
    -configuration Debug \
    -destination "platform=macOS,arch=$host_arch" \
    -derivedDataPath "$derived_data" \
    ARCHS="$host_arch" \
    ONLY_ACTIVE_ARCH=YES \
    CLUMSIES_APP_BUNDLE_IDENTIFIER="$bundle_id" \
    CLUMSIES_APP_PRODUCT_NAME="$product_name" \
    CLUMSIES_APP_DISPLAY_NAME="$display_name" \
    CLUMSIES_DEV_INSTANCE_ID="$instance_id" \
    CLUMSIES_DAEMON_ROOT="$daemon_root" \
    CLUMSIES_DAEMON_CACHE_DIR="$daemon_cache" \
    CLUMSIES_DAEMON_LOG_DIR="$daemon_logs" \
    CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR="$launch_agents" \
    CLUMSIES_SERVER_URL="$server_url" \
    CLUMSIES_CODEX_HOME="$codex_home" \
    CLUMSIES_AGENT_RUNTIME_BUILD_ID="$build_id" \
    build >> "$logs_dir/build.log" 2>&1
  [ -d "$app_path" ] || die "Xcode did not produce $app_path"
  [ -x "$app_path/Contents/Resources/clumsiesd" ] || die "the Dev App does not contain an executable daemon"

  if [ "$review_playground" = 1 ]; then
    "$python" "$repo_root/dev/seed-review-playground.py" --prepare-app
  fi

  open -n \
    --stdout "$app_stdout" \
    --stderr "$app_stderr" \
    --env "CLUMSIES_DEV_INSTANCE_ID=$instance_id" \
    --env "CLUMSIES_DAEMON_ROOT=$daemon_root" \
    --env "CLUMSIES_DAEMON_CACHE_DIR=$daemon_cache" \
    --env "CLUMSIES_DAEMON_LOG_DIR=$daemon_logs" \
    --env "CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR=$launch_agents" \
    --env "CLUMSIES_SERVER_URL=$server_url" \
    --env "CLUMSIES_CODEX_HOME=$codex_home" \
    "$app_path"

  attempts=0
  while ! daemon_is_running && [ "$attempts" -lt 200 ]; do
    sleep 0.1
    attempts=$((attempts + 1))
  done
  daemon_is_running || die "Dev daemon did not become ready; see $app_stderr and $daemon_logs"
  server_is_healthy || die "Server became unhealthy while the Dev App was building"
  runtime_state=running
  write_runtime
  up_complete=1
  printf '%s\n' "$runtime_file"
}

run_status() {
  [ $# -eq 0 ] || usage
  print_status
}

run_logs() {
  [ $# -eq 0 ] || usage
  acquire_lock
  trap on_exit 0
  validate_runtime
  mode=$(json_get "$runtime_file" mode)
  if [ "$mode" = local ]; then
    ensure_recovery_compose_env
    compose logs --tail 200 postgres fake-oidc || true
  fi
  for log in "$server_log" "$app_stdout" "$app_stderr" "$daemon_logs/daemon.log"; do
    if [ -f "$log" ]; then
      printf '\n==> %s <==\n' "$log"
      tail -n 200 "$log"
    fi
  done
}

run_test_live() {
  [ $# -eq 0 ] || usage
  acquire_lock
  trap on_exit 0
  validate_runtime
  [ "$(json_get "$runtime_file" state)" = running ] \
    || die "Dev Instance is not running; run up before test-live"
  mode=$(json_get "$runtime_file" mode)
  server_url=$(json_get "$runtime_file" server_url)
  build_id=$(json_get "$runtime_file" build_id 2>/dev/null) \
    || die "runtime descriptor has no build identity; run up before test-live"
  case "$build_id" in
    "dev-$instance_id-"*) ;;
    *) die "runtime descriptor build identity is not owned by this instance" ;;
  esac
  if [ ! -d "$app_path" ] || [ ! -x "$app_path/Contents/Resources/clumsiesd" ]; then
    die "Dev App is missing; run up before test-live"
  fi
  daemon_is_running || die "Dev daemon is not running; run up before test-live"
  server_is_healthy || die "Dev Server is not healthy; run up before test-live"
  if [ "$mode" = local ]; then
    server_binary=$(json_get "$runtime_file" paths.server_binary)
    server_pid=$(json_get "$runtime_file" processes.server_pid)
    server_start_identity=$(json_get "$runtime_file" processes.server_start_identity)
    if [ "$(server_job_pid || true)" != "$server_pid" ] || ! server_process_is_owned; then
      die "Dev Server process is not owned by this instance"
    fi
    compose_is_running || die "Dev Compose services are not running"
  fi

  host_arch=$(uname -m)
  case "$host_arch" in
    arm64|x86_64) ;;
    *) die "unsupported macOS architecture: $host_arch" ;;
  esac
  xcodegen generate --spec "$repo_root/apps/macos/project.yml"
  env CLUMSIES_RUN_LIVE_TESTS=1 CLUMSIES_SKIP_DAEMON_BUILD=1 \
    xcodebuild -quiet \
      -project "$repo_root/apps/macos/Clumsies.xcodeproj" \
      -scheme ClumsiesLiveTests \
      -configuration Debug \
      -destination "platform=macOS,arch=$host_arch" \
      -derivedDataPath "$derived_data" \
      ARCHS="$host_arch" \
      ONLY_ACTIVE_ARCH=YES \
      CLUMSIES_SKIP_DAEMON_BUILD=1 \
      CLUMSIES_APP_BUNDLE_IDENTIFIER="$bundle_id" \
      CLUMSIES_APP_PRODUCT_NAME="$product_name" \
      CLUMSIES_APP_DISPLAY_NAME="$display_name" \
      CLUMSIES_DEV_INSTANCE_ID="$instance_id" \
      CLUMSIES_DAEMON_ROOT="$daemon_root" \
      CLUMSIES_DAEMON_CACHE_DIR="$daemon_cache" \
      CLUMSIES_DAEMON_LOG_DIR="$daemon_logs" \
      CLUMSIES_DAEMON_LAUNCH_AGENTS_DIR="$launch_agents" \
      CLUMSIES_SERVER_URL="$server_url" \
      CLUMSIES_CODEX_HOME="$codex_home" \
      CLUMSIES_AGENT_RUNTIME_BUILD_ID="$build_id" \
      -only-testing:ClumsiesTests/LiveWorkspaceIntegrationTests \
      test
}

run_down() {
  [ $# -eq 0 ] || usage
  acquire_lock
  trap on_exit 0
  stop_instance
}

run_reset() {
  [ $# -eq 0 ] || usage
  acquire_lock
  trap on_exit 0
  validate_runtime
  stop_instance -v
  security delete-generic-password -s "$keychain_service" -a server-session >/dev/null 2>&1 || true
  case "$instance_root" in
    "$instances_root/$instance_id") ;;
    *) die "refusing to remove an unowned instance root" ;;
  esac
  rm -rf -- "$instance_root"
}

case "$command_name" in
  up) run_up "$@" ;;
  status) run_status "$@" ;;
  logs) run_logs "$@" ;;
  test-live) run_test_live "$@" ;;
  down) run_down "$@" ;;
  reset) run_reset "$@" ;;
esac
