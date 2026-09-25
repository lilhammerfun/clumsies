#!/bin/sh
# The Linux counterpart of dev-instance.sh.
#
# One instance per worktree, addressed by the hash of its path, with its own
# ports, database, Server, daemon root and secrets. The Setup Code is generated
# here and read back from the instance's compose.env, so nobody types it: it is
# a deployment secret, not something a developer is expected to know.
#
# Why this is a second file rather than a branch of dev-instance.sh: that script
# is macOS mechanics end to end (launchd, Keychain, a signed .app, Xcode). The
# two share the layout, the conventions and dev/dev-server.sh, and differ only
# in what supervises the processes -- which is exactly the part that cannot be
# shared.
#
#   dev/dev-instance-linux.sh up       start containers, Server, daemon; sign in
#   dev/dev-instance-linux.sh status   what is running and where it lives
#   dev/dev-instance-linux.sh logs     tail the instance's logs
#   dev/dev-instance-linux.sh down     stop the Server and the daemon
#   dev/dev-instance-linux.sh reset    stop everything and delete the instance
#
# Containers may need sudo (Omarchy keeps users out of the docker group on
# purpose). When that is the case this script prints the one command to run and
# waits for the ports instead of failing.

set -eu

umask 077
unset CDPATH

die() {
  printf 'clumsies dev: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat >&2 <<'EOF'
usage: dev/dev-instance-linux.sh up|status|logs|down|reset
EOF
  exit 64
}

command_name=${1:-}
[ -n "$command_name" ] || usage
shift

python=${PYTHON:-python3}
command -v "$python" >/dev/null 2>&1 || die "python3 is required"

script_dir=$(cd -- "$(dirname -- "$0")" && pwd -P)
repo_root=$(git -C "$script_dir/.." rev-parse --show-toplevel 2>/dev/null) \
  || die "the runner must be inside a git worktree"
repo_root=$(cd -- "$repo_root" && pwd -P)
instance_id=$(printf '%s' "$repo_root" | sha256sum | awk '{print substr($1, 1, 12)}')

# Same layout as dev-instance.sh, with the XDG data directory standing in for
# ~/Library/Application Support.
dev_root=${CLUMSIES_DEV_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/ai.clumsies.dev}
case "$dev_root" in /*) ;; *) die "CLUMSIES_DEV_ROOT must be absolute" ;; esac
instances_root=$dev_root/instances
instance_root=$instances_root/$instance_id
runtime_file=$instance_root/runtime.json
compose_env=$instance_root/compose.env
ready_file=$instance_root/server-ready.json
logs_dir=$instance_root/logs
daemon_root=$instance_root/daemon
daemon_cache=$instance_root/cache
server_bin_dir=$instance_root/bin
server_binary=$server_bin_dir/clumsies-server
server_pid_file=$instance_root/server.pid
daemon_pid_file=$instance_root/daemon.pid
compose_project=clumsies-dev-$instance_id

random_hex() {
  "$python" -c 'import secrets, sys; print(secrets.token_hex(int(sys.argv[1])))' "$1"
}

free_port() {
  "$python" - <<'PY'
import socket
with socket.socket() as probe:
    probe.bind(("127.0.0.1", 0))
    print(probe.getsockname()[1])
PY
}

env_value() {
  # Absent on the first run, which is not an error: it means nothing is chosen yet.
  [ -f "$compose_env" ] || return 0
  awk -F= -v key="$1" '$1 == key { print substr($0, index($0, "=") + 1) }' "$compose_env"
}

json_value() {
  "$python" -c 'import json,sys; d=json.load(open(sys.argv[1]));
keys=sys.argv[2].split(".");
for key in keys: d=d[key]
print(d)' "$runtime_file" "$1" 2>/dev/null || true
}

write_compose_env() {
  temporary=$compose_env.$$.tmp
  {
    printf 'CLUMSIES_HOST_BIND_ADDRESS=127.0.0.1\n'
    printf 'CLUMSIES_DB_NAME=clumsies\n'
    printf 'CLUMSIES_DB_USER=clumsies\n'
    printf 'CLUMSIES_DB_PASSWORD=%s\n' "$database_password"
    printf 'CLUMSIES_DB_PORT=%s\n' "$database_port"
    printf 'CLUMSIES_OIDC_PORT=%s\n' "$oidc_port"
    printf 'CLUMSIES_SETUP_CODE=%s\n' "$setup_code"
  } > "$temporary"
  chmod 600 "$temporary"
  mv -f "$temporary" "$compose_env"
}

instance_ports() {
  database_port=$(env_value CLUMSIES_DB_PORT)
  oidc_port=$(env_value CLUMSIES_OIDC_PORT)
  server_port=$(json_value ports.server)
}

# Containers are the one part that may need another user's privileges.
ensure_containers() {
  if docker info >/dev/null 2>&1; then
    docker compose --env-file "$compose_env" -p "$compose_project" \
      up -d --wait postgres fake-oidc
  else
    printf 'clumsies dev: Docker needs elevated rights here. Run this, then re-run "up":\n\n'
    printf '  sudo docker compose --env-file %s -p %s up -d --wait postgres fake-oidc\n\n' \
      "$compose_env" "$compose_project"
    printf 'waiting for port %s and %s' "$database_port" "$oidc_port"
    for _ in $(seq 1 90); do
      if "$python" -c 'import socket,sys
for port in sys.argv[1:]:
    with socket.socket() as probe:
        probe.settimeout(1)
        if probe.connect_ex(("127.0.0.1", int(port))) != 0: raise SystemExit(1)' \
        "$database_port" "$oidc_port"; then
        printf '\n'
        return 0
      fi
      printf '.'
      sleep 2
    done
    printf '\n'
    die "the containers never came up"
  fi
}

build_server() {
  mkdir -p "$server_bin_dir"
  ( cd "$repo_root" && cargo build -p server --bin clumsies-server ) \
    || die "the Server did not build"
  cp "$repo_root/target/debug/clumsies-server" "$server_binary"
  chmod 755 "$server_binary"
}

start_server() {
  instance_ports
  if [ -f "$server_pid_file" ] && kill -0 "$(cat "$server_pid_file")" 2>/dev/null; then
    return 0
  fi
  rm -f "$ready_file"
  # dev/dev-server.sh is the same launcher the macOS instance uses: it reads
  # the ports, the database password and the Setup Code from compose.env.
  nohup "$repo_root/dev/dev-server.sh" "$compose_env" "$server_binary" \
    "127.0.0.1:$server_port" "$ready_file" > "$logs_dir/server.log" 2>&1 &
  echo $! > "$server_pid_file"
  printf 'waiting for the Server'
  for _ in $(seq 1 90); do
    [ -f "$ready_file" ] && { printf '\n'; return 0; }
    printf '.'
    sleep 1
  done
  printf '\n'
  die "the Server never became ready; see $logs_dir/server.log"
}

start_daemon() {
  if [ -f "$daemon_pid_file" ] && kill -0 "$(cat "$daemon_pid_file")" 2>/dev/null; then
    return 0
  fi
  mkdir -p "$daemon_root" "$daemon_cache"
  ( cd "$repo_root" && cargo build -p clumsiesd --bin clumsiesd ) || die "the daemon did not build"
  CLUMSIES_DAEMON_ROOT="$daemon_root" CLUMSIES_DAEMON_CACHE_DIR="$daemon_cache" \
    nohup "$repo_root/target/debug/clumsiesd" > "$logs_dir/daemon.log" 2>&1 &
  echo $! > "$daemon_pid_file"
  printf 'waiting for the daemon'
  for _ in $(seq 1 60); do
    [ -S "$daemon_root/daemon.sock" ] && { printf '\n'; return 0; }
    printf '.'
    sleep 1
  done
  printf '\n'
  die "the daemon never opened its socket; see $logs_dir/daemon.log"
}

write_runtime() {
  instance_ports
  "$python" - "$runtime_file" "$instance_id" "$repo_root" "$server_port" \
    "$database_port" "$oidc_port" "$daemon_root" <<'PY'
import json, sys
path, instance_id, worktree, server, database, oidc, daemon_root = sys.argv[1:]
json.dump(
    {
        "instance_id": instance_id,
        "worktree_path": worktree,
        "server_url": f"http://127.0.0.1:{server}",
        "ports": {"server": int(server), "postgres": int(database), "oidc": int(oidc)},
        "daemon_root": daemon_root,
    },
    open(path, "w"),
    indent=2,
)
PY
  chmod 600 "$runtime_file"
}

sign_in() {
  instance_ports
  # The Setup Code comes from the instance, not from the developer.
  CLUMSIES_DAEMON_ROOT="$daemon_root" "$python" "$repo_root/dev/dev-login.py" \
    --server-url "http://127.0.0.1:$server_port" \
    --setup-code "$(env_value CLUMSIES_SETUP_CODE)" \
    --org-name "Clumsies Dev $instance_id" \
    --project-name clumsies
  CLUMSIES_DAEMON_ROOT="$daemon_root" "$python" "$repo_root/dev/seed-memory.py" \
    --project-id "$(project_id)" || true
}

project_id() {
  CLUMSIES_DAEMON_ROOT="$daemon_root" "$python" - <<'PY'
import json, os, socket, struct

root = os.environ["CLUMSIES_DAEMON_ROOT"]


def call(method, payload):
    body = json.dumps({"method": method, "payload": payload}).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(60)
        connection.connect(os.path.join(root, "daemon.sock"))
        connection.sendall(struct.pack(">I", len(body)) + body)
        length = struct.unpack(">I", connection.recv(4))[0]
        data = b""
        while len(data) < length:
            data += connection.recv(length - len(data))
    reply = json.loads(data)
    if not reply.get("ok"):
        raise SystemExit(1)
    return reply["payload"]


page = call("server_request", {"method": "GET", "path": "/api/v1/projects", "headers": {}, "body": None})
items = json.loads(page["body"]).get("items") or []
print(items[0]["project_id"] if items else "")
PY
}

run_up() {
  mkdir -p "$instance_root" "$logs_dir" "$daemon_root" "$daemon_cache"

  database_port=$(env_value CLUMSIES_DB_PORT || true)
  oidc_port=$(env_value CLUMSIES_OIDC_PORT || true)
  [ -n "$database_port" ] || database_port=$(free_port)
  [ -n "$oidc_port" ] || oidc_port=$(free_port)
  database_password=$(env_value CLUMSIES_DB_PASSWORD || true)
  [ -n "$database_password" ] || database_password=$(random_hex 24)
  setup_code=$(env_value CLUMSIES_SETUP_CODE || true)
  [ -n "$setup_code" ] || setup_code=$(random_hex 24)
  server_port=$(json_value ports.server || true)
  [ -n "$server_port" ] || server_port=$(free_port)

  write_compose_env
  "$python" - "$runtime_file" "$instance_id" "$repo_root" "$server_port" \
    "$database_port" "$oidc_port" "$daemon_root" <<'PY'
import json, sys
path, instance_id, worktree, server, database, oidc, daemon_root = sys.argv[1:]
json.dump(
    {
        "instance_id": instance_id,
        "worktree_path": worktree,
        "server_url": f"http://127.0.0.1:{server}",
        "ports": {"server": int(server), "postgres": int(database), "oidc": int(oidc)},
        "daemon_root": daemon_root,
    },
    open(path, "w"),
    indent=2,
)
PY
  chmod 600 "$runtime_file"

  ensure_containers
  build_server
  start_server
  write_runtime
  start_daemon
  sign_in

  printf '\ninstance %s is up\n' "$instance_id"
  printf '  server:  http://127.0.0.1:%s\n' "$server_port"
  printf '  daemon:  %s/daemon.sock\n' "$daemon_root"
  printf '  client:  CLUMSIES_DAEMON_ROOT=%s cargo run -p desktop\n' "$daemon_root"
}

run_status() {
  [ -f "$runtime_file" ] || die "no instance here yet"
  printf 'instance %s\n' "$instance_id"
  cat "$runtime_file"
  if [ -f "$server_pid_file" ] && kill -0 "$(cat "$server_pid_file")" 2>/dev/null; then
    printf 'server: running (pid %s)\n' "$(cat "$server_pid_file")"
  else
    printf 'server: stopped\n'
  fi
  if [ -f "$daemon_pid_file" ] && kill -0 "$(cat "$daemon_pid_file")" 2>/dev/null; then
    printf 'daemon: running (pid %s)\n' "$(cat "$daemon_pid_file")"
  else
    printf 'daemon: stopped\n'
  fi
}

run_logs() {
  tail -n 40 "$logs_dir/server.log" "$logs_dir/daemon.log" 2>/dev/null || die "no logs yet"
}

run_down() {
  for pid_file in "$daemon_pid_file" "$server_pid_file"; do
    [ -f "$pid_file" ] || continue
    pid=$(cat "$pid_file")
    kill "$pid" 2>/dev/null || true
    rm -f "$pid_file"
  done
  if docker info >/dev/null 2>&1; then
    docker compose --env-file "$compose_env" -p "$compose_project" down >/dev/null 2>&1 || true
  fi
  printf 'instance %s stopped\n' "$instance_id"
}

run_reset() {
  run_down
  rm -rf "$instance_root"
  printf 'instance %s deleted\n' "$instance_id"
}

case "$command_name" in
  up) run_up "$@" ;;
  status) run_status ;;
  logs) run_logs ;;
  down) run_down ;;
  reset) run_reset ;;
  *) usage ;;
esac
