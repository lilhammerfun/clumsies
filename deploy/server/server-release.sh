#!/usr/bin/env bash

set -Eeuo pipefail
umask 0077

readonly CLUMSIES_ROOT="${CLUMSIES_ROOT:-/opt/clumsies}"
readonly COMPOSE_FILE="$CLUMSIES_ROOT/compose.production.yml"
readonly ENV_FILE="$CLUMSIES_ROOT/.env"
readonly BACKUP_DIR="$CLUMSIES_ROOT/backups"
readonly RELEASE_DIR="$CLUMSIES_ROOT/releases"
readonly IMAGE_PREFIX="ghcr.io/lilhammerfun/clumsies-server"
readonly LOCK_FILE="/run/lock/clumsies-server-release.lock"
readonly SERVER_STOP_TIMEOUT_SECONDS=10

log() {
  printf '[clumsies-release] %s\n' "$*" >&2
}

die() {
  log "error: $*"
  exit 1
}

require_root() {
  [[ "$(id -u)" -eq 0 ]] || die "this command must run as root"
}

acquire_lock() {
  exec 9>"$LOCK_FILE"
  flock --nonblock 9 || die "another release or backup operation is active"
}

compose() {
  (
    cd "$CLUMSIES_ROOT"
    docker compose --project-name clumsies --file "$COMPOSE_FILE" "$@"
  )
}

server_container_id() {
  docker ps --all \
    --filter label=com.docker.compose.project=clumsies \
    --filter label=com.docker.compose.service=server \
    --format '{{.ID}}' | sed -n '1p'
}

stop_server() {
  local container
  local exit_code

  container="$(server_container_id)" || {
    log "could not identify the Server container before stopping it"
    return 1
  }
  if [[ -z "$container" ]]; then
    log "no Server container exists to stop"
    return 1
  fi

  compose stop --timeout "$SERVER_STOP_TIMEOUT_SECONDS" server || return 1

  exit_code="$(docker inspect "$container" --format '{{.State.ExitCode}}' 2>/dev/null)" || {
    log "could not inspect the stopped Server container"
    return 1
  }
  if [[ ! "$exit_code" =~ ^[0-9]+$ ]]; then
    log "stopped Server container returned an invalid exit code: $exit_code"
    return 1
  fi
  if [[ "$exit_code" != 0 ]]; then
    log "Server container exited with code $exit_code while stopping"
    return 1
  fi
}

stop_isolated_server() {
  local container="$1"
  local exit_code

  if ! docker stop --time "$SERVER_STOP_TIMEOUT_SECONDS" "$container" >/dev/null; then
    log "isolated Server did not stop within ${SERVER_STOP_TIMEOUT_SECONDS} seconds"
    return 1
  fi

  exit_code="$(docker inspect "$container" --format '{{.State.ExitCode}}' 2>/dev/null)" || {
    log "could not inspect the stopped isolated Server"
    return 1
  }
  if [[ "$exit_code" != 0 ]]; then
    log "isolated Server exited with code $exit_code"
    return 1
  fi
}

read_env_value() {
  local key="$1"
  awk -v key="$key" '
    index($0, key "=") == 1 {
      sub(/^[^=]*=/, "")
      print
      exit
    }
  ' "$ENV_FILE"
}

current_container_image() {
  local container

  container="$(server_container_id)"
  [[ -n "$container" ]] || return 0
  docker inspect "$container" --format '{{.Config.Image}}' 2>/dev/null || true
}

current_image() {
  local image
  image="$(read_env_value CLUMSIES_SERVER_IMAGE)"
  if [[ -z "$image" ]]; then
    image="$(current_container_image)"
  fi
  [[ -n "$image" ]] || die "CLUMSIES_SERVER_IMAGE is unset and no Server container exists"
  printf '%s\n' "$image"
}

write_image_setting() {
  local image="$1"
  local temp

  [[ -n "$image" && "$image" != *$'\n'* && "$image" != *$'\r'* ]] ||
    die "invalid image setting"

  temp="$(mktemp "$CLUMSIES_ROOT/.env.image.XXXXXX")"
  awk -v image="$image" '
    BEGIN { replaced = 0 }
    /^CLUMSIES_SERVER_IMAGE=/ {
      if (!replaced) {
        print "CLUMSIES_SERVER_IMAGE=" image
        replaced = 1
      }
      next
    }
    { print }
    END {
      if (!replaced) {
        print "CLUMSIES_SERVER_IMAGE=" image
      }
    }
  ' "$ENV_FILE" >"$temp"
  chmod --reference="$ENV_FILE" "$temp"
  chown --reference="$ENV_FILE" "$temp"
  mv "$temp" "$ENV_FILE"
}

ensure_image_setting() {
  if [[ -z "$(read_env_value CLUMSIES_SERVER_IMAGE)" ]]; then
    write_image_setting "$(current_image)"
  fi
}

write_runtime_configuration() {
  local origin="$1"
  local redirects="$2"
  local temp

  validate_public_origin "$origin"
  [[ "$redirects" != *$'\n'* && "$redirects" != *$'\r'* ]] ||
    die "invalid client redirect URI setting"

  temp="$(mktemp "$CLUMSIES_ROOT/.env.runtime.XXXXXX")"
  awk -v origin="$origin" -v redirects="$redirects" '
    BEGIN { origin_replaced = 0; redirects_replaced = 0 }
    /^CLUMSIES_PUBLIC_ORIGIN=/ {
      if (!origin_replaced) {
        print "CLUMSIES_PUBLIC_ORIGIN=" origin
        origin_replaced = 1
      }
      next
    }
    /^CLUMSIES_CLIENT_REDIRECT_URIS=/ {
      if (!redirects_replaced) {
        print "CLUMSIES_CLIENT_REDIRECT_URIS=" redirects
        redirects_replaced = 1
      }
      next
    }
    { print }
    END {
      if (!origin_replaced) {
        print "CLUMSIES_PUBLIC_ORIGIN=" origin
      }
      if (!redirects_replaced) {
        print "CLUMSIES_CLIENT_REDIRECT_URIS=" redirects
      }
    }
  ' "$ENV_FILE" >"$temp"
  chmod --reference="$ENV_FILE" "$temp"
  chown --reference="$ENV_FILE" "$temp"
  mv "$temp" "$ENV_FILE"
}

require_environment() {
  local compose_version
  local compose_major

  [[ -f "$COMPOSE_FILE" ]] || die "missing $COMPOSE_FILE"
  [[ -f "$ENV_FILE" ]] || die "missing $ENV_FILE"
  compose_version="$(docker compose version --short 2>/dev/null || true)"
  compose_major="${compose_version%%.*}"
  [[ "$compose_major" =~ ^[0-9]+$ && "$compose_major" -ge 2 ]] ||
    die "Docker Compose v2 or newer is required; found ${compose_version:-none}"

  install -d -m 0700 "$BACKUP_DIR" "$RELEASE_DIR"
  ensure_image_setting
}

validate_target() {
  local image="$1"
  local commit="$2"
  local digest

  digest="${image#"$IMAGE_PREFIX@sha256:"}"
  [[ "$image" == "$IMAGE_PREFIX@sha256:$digest" && "$digest" =~ ^[a-f0-9]{64}$ ]] ||
    die "image must be an immutable ${IMAGE_PREFIX}@sha256 reference"
  [[ "$commit" =~ ^[a-f0-9]{40}$ ]] ||
    die "commit must contain 40 lowercase hexadecimal characters"
}

validate_public_origin() {
  local origin="$1"

  [[ "$origin" =~ ^https://[A-Za-z0-9.-]+(:[0-9]{1,5})?$ ]] ||
    die "CLUMSIES_PUBLIC_ORIGIN must be an HTTPS origin without a path"
}

public_origin() {
  local origin

  origin="$(read_env_value CLUMSIES_PUBLIC_ORIGIN)"
  validate_public_origin "$origin"
  printf '%s\n' "$origin"
}

# Reach the public origin through the local edge instead of this host's
# resolver. A stale resolver cache (for example while an installation moves to
# another host) would otherwise fail the probe against a healthy Server and
# roll back a good release.
local_edge_healthy() {
  local origin="$1"
  local authority="${origin#https://}"
  local host="${authority%%:*}"
  local port="${authority#"$host"}"

  port="${port#:}"
  curl --fail --silent --show-error --max-time 8 \
    --resolve "$host:${port:-443}:127.0.0.1" \
    "$origin/api/v1/admin/health" >/dev/null 2>&1
}

public_dns_reaches_host() {
  local origin="$1"

  curl --fail --silent --show-error --max-time 8 \
    "$origin/api/v1/admin/health" >/dev/null 2>&1
}

wait_public_health() {
  local attempts="${1:-30}"
  local origin="${2:-}"
  local attempt
  local warned=0

  if [[ -z "$origin" ]]; then
    origin="$(public_origin)"
  else
    validate_public_origin "$origin"
  fi
  for ((attempt = 1; attempt <= attempts; attempt += 1)); do
    if local_edge_healthy "$origin"; then
      if [[ "$warned" -eq 0 ]] && ! public_dns_reaches_host "$origin"; then
        warned=1
        log "warning: $origin does not reach this host through public DNS yet"
      fi
      return 0
    fi
    sleep 2
  done
  return 1
}

prune_scheduled_backups() {
  local retention_days="${CLUMSIES_BACKUP_RETENTION_DAYS:-14}"

  [[ "$retention_days" =~ ^[0-9]+$ ]] || die "invalid backup retention"
  find "$BACKUP_DIR" -maxdepth 1 -type f \
    \( -name 'scheduled-*.dump' -o -name 'scheduled-*.dump.sha256' \) \
    -mtime "+$retention_days" -delete
}

backup_database() {
  local reason="$1"
  local timestamp
  local backup
  local temp
  local size

  [[ "$reason" =~ ^[a-z0-9-]+$ ]] || die "invalid backup reason"
  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  backup="$BACKUP_DIR/${reason}-${timestamp}.dump"
  temp="$BACKUP_DIR/.${reason}-${timestamp}.dump.tmp"

  log "creating PostgreSQL backup: $backup"
  # POSTGRES_USER and POSTGRES_DB are expanded inside the Postgres container.
  # shellcheck disable=SC2016
  if ! compose exec -T postgres sh -c \
    'pg_dump -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Fc' >"$temp"; then
    rm -f "$temp"
    die "pg_dump failed"
  fi

  size="$(stat -c %s "$temp")"
  if ((size < 1024)); then
    rm -f "$temp"
    die "database backup is unexpectedly small"
  fi

  if ! compose exec -T postgres sh -c 'pg_restore --list >/dev/null' <"$temp"; then
    rm -f "$temp"
    die "pg_restore could not read the new backup"
  fi

  mv "$temp" "$backup"
  sha256sum "$backup" >"$backup.sha256"
  if [[ "$reason" == scheduled ]]; then
    prune_scheduled_backups
  fi
  printf '%s\n' "$backup"
}

record_release() {
  local status="$1"
  local timestamp="$2"
  local commit="$3"
  local image="$4"
  local previous_image="$5"
  local backup="$6"
  local preflight_backup="${7:-}"
  local record="$RELEASE_DIR/deploy-${timestamp}-${commit:0:12}.env"
  local temp

  temp="$(mktemp "$RELEASE_DIR/.deploy-record.XXXXXX")"
  {
    printf 'status=%s\n' "$status"
    printf 'timestamp=%s\n' "$timestamp"
    printf 'commit=%s\n' "$commit"
    printf 'image=%s\n' "$image"
    printf 'previous_image=%s\n' "$previous_image"
    printf 'backup=%s\n' "$backup"
    printf 'preflight_backup=%s\n' "$preflight_backup"
  } >"$temp"
  mv "$temp" "$record"
  log "release record: $record"
}

validate_backup_archive() {
  local backup="$1"

  [[ -f "$backup" ]] || {
    log "backup does not exist: $backup"
    return 1
  }
  if [[ -f "$backup.sha256" ]] && ! sha256sum --check "$backup.sha256" >/dev/null; then
    log "backup checksum failed: $backup"
    return 1
  fi
  if ! compose exec -T postgres sh -c 'pg_restore --list >/dev/null' <"$backup"; then
    log "pg_restore could not read backup: $backup"
    return 1
  fi
}

restore_database() {
  local backup="$1"

  validate_backup_archive "$backup" || return 1
  log "restoring PostgreSQL from $backup"
  # POSTGRES_USER and POSTGRES_DB are expanded inside the Postgres container.
  # shellcheck disable=SC2016
  if ! compose exec -T postgres sh -ceu '
    case "$POSTGRES_DB" in
      ""|postgres|template0|template1)
        printf "refusing to replace protected database: %s\n" "$POSTGRES_DB" >&2
        exit 64
        ;;
    esac
    dropdb -U "$POSTGRES_USER" --if-exists --force "$POSTGRES_DB"
    createdb -U "$POSTGRES_USER" --owner "$POSTGRES_USER" "$POSTGRES_DB"
    pg_restore \
      -U "$POSTGRES_USER" \
      -d "$POSTGRES_DB" \
      --exit-on-error \
      --no-owner \
      --no-privileges
    psql -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" \
      -c "SELECT count(*) FROM _sqlx_migrations" >/dev/null
  ' <"$backup"; then
    log "database restore failed: $backup"
    return 1
  fi
  log "database restore completed: $backup"
}

resume_release() {
  local image="$1"

  write_image_setting "$image"
  if ! compose up --detach --no-deps --force-recreate --pull never \
    --wait --wait-timeout 180 server; then
    return 1
  fi
  wait_public_health 30
}

# Persist the failed Server container's log before rollback's
# --force-recreate destroys the only copy of it.
capture_server_logs() {
  local container
  local logfile

  logfile="$RELEASE_DIR/failed-container-$(date -u +%Y%m%dT%H%M%SZ).log"
  container="$(server_container_id || true)"
  if [[ -z "$container" ]]; then
    log "no Server container found to capture logs from"
    return 0
  fi
  docker logs --tail 400 "$container" >"$logfile" 2>&1 || true
  log "captured failed Server container logs: $logfile"
  sed 's/^/    /' "$logfile" >&2 || true
}

rollback_release() {
  local previous_image="$1"
  local backup="$2"

  log "rolling back database and Server to $previous_image"
  if ! stop_server; then
    log "could not stop the failed Server before rollback"
    return 1
  fi
  write_image_setting "$previous_image"
  restore_database "$backup" || return 1
  if ! compose up --detach --no-deps --force-recreate --pull never \
    --wait --wait-timeout 180 server; then
    log "rollback container recreation failed"
    return 1
  fi
  if ! wait_public_health 30; then
    log "rollback did not restore public health"
    return 1
  fi
  log "database and Server rollback completed"
}

deploy_release() {
  local image="$1"
  local commit="$2"
  local previous_image
  local timestamp
  local preflight_backup
  local backup

  validate_target "$image" "$commit"
  previous_image="$(current_image)"

  if [[ "$previous_image" == "$image" ]] && wait_public_health 1; then
    log "image is already healthy: $image"
    return 0
  fi

  log "pulling $image"
  docker pull "$image"
  (
    export CLUMSIES_SERVER_IMAGE="$image"
    compose config --quiet
  )

  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  preflight_backup="$(backup_database "preflight-${commit:0:12}")"
  if ! verify_backup_with_image "$preflight_backup" "$image" release-candidate; then
    record_release preflight-failed "$timestamp" "$commit" "$image" \
      "$previous_image" "$preflight_backup" "$preflight_backup"
    die "target image failed against an isolated copy of production; production was not changed"
  fi

  log "stopping the current Server for a write-free database cutover"
  if ! stop_server; then
    resume_release "$previous_image" || true
    record_release cutover-stop-failed "$timestamp" "$commit" "$image" \
      "$previous_image" "$preflight_backup" "$preflight_backup"
    die "current Server could not be stopped cleanly"
  fi

  if ! backup="$(backup_database "pre-deploy-${commit:0:12}")"; then
    if ! resume_release "$previous_image"; then
      record_release recovery-failed "$timestamp" "$commit" "$image" \
        "$previous_image" "$preflight_backup" "$preflight_backup"
      die "cutover backup failed and the previous Server did not recover"
    fi
    record_release cutover-backup-failed "$timestamp" "$commit" "$image" \
      "$previous_image" "$preflight_backup" "$preflight_backup"
    die "cutover backup failed; previous Server resumed without changing the database"
  fi

  write_image_setting "$image"

  if ! compose up --detach --no-deps --pull never \
    --wait --wait-timeout 180 server; then
    capture_server_logs
    if ! rollback_release "$previous_image" "$backup"; then
      record_release rollback-failed "$timestamp" "$commit" "$image" \
        "$previous_image" "$backup" "$preflight_backup"
      die "new Server failed and automatic database rollback failed; manual recovery is required"
    fi
    record_release rolled-back "$timestamp" "$commit" "$image" \
      "$previous_image" "$backup" "$preflight_backup"
    die "new Server container failed; database and previous image restored"
  fi

  if ! wait_public_health 30; then
    capture_server_logs
    if ! rollback_release "$previous_image" "$backup"; then
      record_release rollback-failed "$timestamp" "$commit" "$image" \
        "$previous_image" "$backup" "$preflight_backup"
      die "public health failed and automatic database rollback failed; manual recovery is required"
    fi
    record_release rolled-back "$timestamp" "$commit" "$image" \
      "$previous_image" "$backup" "$preflight_backup"
    die "public health check failed; database and previous image restored"
  fi

  record_release success "$timestamp" "$commit" "$image" \
    "$previous_image" "$backup" "$preflight_backup"
  log "deployment healthy: $image"
}

record_reconfiguration() {
  local status="$1"
  local timestamp="$2"
  local previous_origin="$3"
  local origin="$4"
  local backup="$5"
  local env_backup="$6"
  local record="$RELEASE_DIR/reconfigure-${timestamp}.env"
  local temp

  temp="$(mktemp "$RELEASE_DIR/.reconfigure-record.XXXXXX")"
  {
    printf 'status=%s\n' "$status"
    printf 'timestamp=%s\n' "$timestamp"
    printf 'previous_origin=%s\n' "$previous_origin"
    printf 'origin=%s\n' "$origin"
    printf 'backup=%s\n' "$backup"
    printf 'env_backup=%s\n' "$env_backup"
  } >"$temp"
  mv "$temp" "$record"
  log "reconfiguration record: $record"
}

rollback_reconfiguration() {
  local env_backup="$1"
  local previous_origin="$2"

  log "rolling back runtime configuration to $previous_origin"
  cp --preserve=mode,ownership,timestamps "$env_backup" "$ENV_FILE"
  compose config --quiet
  if ! compose up --detach --no-deps --force-recreate --pull never \
    --wait --wait-timeout 240 server caddy; then
    die "runtime configuration rollback failed"
  fi
  wait_public_health 30 "$previous_origin" ||
    die "runtime configuration rollback did not restore public health"
}

reconfigure_runtime() {
  local origin="$1"
  local redirects="${2:-}"
  local previous_origin
  local previous_redirects
  local timestamp
  local env_backup
  local backup

  validate_public_origin "$origin"
  previous_origin="$(public_origin)"
  previous_redirects="$(read_env_value CLUMSIES_CLIENT_REDIRECT_URIS)"
  if [[ "$#" -lt 2 ]]; then
    redirects="$previous_redirects"
  fi

  if [[ "$previous_origin" == "$origin" && "$previous_redirects" == "$redirects" ]] &&
    wait_public_health 1 "$origin"; then
    log "runtime configuration is already healthy: $origin"
    return 0
  fi

  timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
  env_backup="$RELEASE_DIR/reconfigure-${timestamp}.env.backup"
  cp --preserve=mode,ownership,timestamps "$ENV_FILE" "$env_backup"
  write_runtime_configuration "$origin" "$redirects"

  if ! compose config --quiet; then
    cp --preserve=mode,ownership,timestamps "$env_backup" "$ENV_FILE"
    die "new runtime configuration is invalid; previous configuration restored"
  fi

  if ! backup="$(backup_database pre-reconfigure)"; then
    cp --preserve=mode,ownership,timestamps "$env_backup" "$ENV_FILE"
    die "database backup failed; previous configuration restored"
  fi
  if ! compose up --detach --no-deps --force-recreate --pull never \
    --wait --wait-timeout 240 server caddy; then
    rollback_reconfiguration "$env_backup" "$previous_origin"
    record_reconfiguration rolled-back "$timestamp" "$previous_origin" \
      "$origin" "$backup" "$env_backup"
    die "runtime reconfiguration failed; previous configuration restored"
  fi

  if ! wait_public_health 60 "$origin"; then
    rollback_reconfiguration "$env_backup" "$previous_origin"
    record_reconfiguration rolled-back "$timestamp" "$previous_origin" \
      "$origin" "$backup" "$env_backup"
    die "new public origin failed health checks; previous configuration restored"
  fi

  record_reconfiguration success "$timestamp" "$previous_origin" \
    "$origin" "$backup" "$env_backup"
  log "runtime configuration healthy: $origin"
}

latest_backup() {
  local -a candidates

  mapfile -t candidates < <(
    find "$BACKUP_DIR" -maxdepth 1 -type f -name '*.dump' -printf '%T@ %p\n' |
      sort -nr
  )
  ((${#candidates[@]} > 0)) || die "no PostgreSQL backup is available"
  printf '%s\n' "${candidates[0]#* }"
}

verify_backup_with_image() (
  set -Eeuo pipefail

  local backup="$1"
  local image="$2"
  local purpose="$3"
  local suffix
  local network
  local postgres_container
  local server_container
  local password
  local attempt
  local table_count
  local migration_count

  [[ "$purpose" =~ ^[a-z0-9-]+$ ]] || die "invalid verification purpose"
  validate_backup_archive "$backup" || die "backup archive validation failed"
  docker image inspect "$image" >/dev/null
  suffix="$(date -u +%Y%m%d%H%M%S)-$$"
  network="clumsies-$purpose-$suffix"
  postgres_container="clumsies-$purpose-postgres-$suffix"
  server_container="clumsies-$purpose-server-$suffix"
  password="$(openssl rand -hex 24)"

  # Invoked by the EXIT trap for this isolated subshell.
  # shellcheck disable=SC2317,SC2329
  cleanup() {
    docker stop --time "$SERVER_STOP_TIMEOUT_SECONDS" \
      "$server_container" >/dev/null 2>&1 || true
    docker rm --force "$server_container" "$postgres_container" >/dev/null 2>&1 || true
    docker network rm "$network" >/dev/null 2>&1 || true
  }
  trap cleanup EXIT

  docker network create "$network" >/dev/null
  docker run --detach --name "$postgres_container" --network "$network" \
    --env POSTGRES_PASSWORD="$password" \
    --env POSTGRES_DB=clumsies_restore \
    postgres:16-alpine >/dev/null

  for ((attempt = 1; attempt <= 60; attempt += 1)); do
    # The official image briefly exposes an initialization server and then
    # restarts PostgreSQL. Wait until PID 1 is the final postgres process so a
    # successful pg_isready cannot race that shutdown.
    if [[ "$(docker exec "$postgres_container" cat /proc/1/comm 2>/dev/null || true)" == postgres ]] &&
      docker exec "$postgres_container" \
      pg_isready -U postgres -d clumsies_restore >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
  ((attempt <= 60)) || die "restore PostgreSQL did not become ready"

  docker exec -i "$postgres_container" \
    pg_restore -U postgres -d clumsies_restore --exit-on-error \
    --no-owner --no-privileges <"$backup"

  docker run --detach --name "$server_container" --network "$network" \
    --env-file "$ENV_FILE" \
    --env "DATABASE_URL=postgres://postgres:$password@$postgres_container:5432/clumsies_restore" \
    --env CLUMSIES_SERVER_ADDR=0.0.0.0:8080 \
    "$image" >/dev/null

  for ((attempt = 1; attempt <= 90; attempt += 1)); do
    if docker exec "$server_container" curl --fail --silent \
      http://127.0.0.1:8080/api/v1/admin/health >/dev/null 2>&1; then
      break
    fi
    if [[ "$(docker inspect "$server_container" --format '{{.State.Status}}')" == exited ]]; then
      docker logs "$server_container" >&2
      die "restored Server exited"
    fi
    sleep 1
  done
  ((attempt <= 90)) || die "restored Server did not become healthy"

  table_count="$(docker exec "$postgres_container" psql -At -U postgres \
    -d clumsies_restore -c \
    "SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname = 'public';")"
  migration_count="$(docker exec "$postgres_container" psql -At -U postgres \
    -d clumsies_restore -c 'SELECT count(*) FROM _sqlx_migrations;')"
  stop_isolated_server "$server_container" ||
    die "$purpose Server did not stop cleanly"
  log "$purpose healthy: backup=$backup tables=$table_count migrations=$migration_count image=$image"
)

restore_drill() {
  local backup="${1:-}"
  local image

  if [[ -z "$backup" ]]; then
    backup="$(latest_backup)"
  elif [[ "$backup" != /* ]]; then
    backup="$BACKUP_DIR/$backup"
  fi
  image="$(current_image)"
  verify_backup_with_image "$backup" "$image" restore-drill
}

preflight() {
  local image
  local origin

  image="$(current_image)"
  origin="$(public_origin)"
  compose config --quiet
  compose ps
  log "preflight ok: compose=$(docker compose version --short) origin=$origin image=$image"
}

usage() {
  cat >&2 <<'EOF'
Usage:
  clumsies-server-release preflight
  clumsies-server-release deploy IMAGE@sha256:DIGEST COMMIT
  clumsies-server-release reconfigure PUBLIC_ORIGIN [CLIENT_REDIRECT_URIS]
  clumsies-server-release backup [reason]
  clumsies-server-release restore-drill [backup-file]
EOF
  exit 2
}

main() {
  local command="${1:-}"

  require_root
  acquire_lock
  require_environment

  case "$command" in
    preflight)
      [[ "$#" -eq 1 ]] || usage
      preflight
      ;;
    deploy)
      [[ "$#" -eq 3 ]] || usage
      deploy_release "$2" "$3"
      ;;
    reconfigure)
      [[ "$#" -ge 2 && "$#" -le 3 ]] || usage
      if [[ "$#" -eq 2 ]]; then
        reconfigure_runtime "$2"
      else
        reconfigure_runtime "$2" "$3"
      fi
      ;;
    backup)
      [[ "$#" -le 2 ]] || usage
      backup_database "${2:-manual}"
      ;;
    restore-drill)
      [[ "$#" -le 2 ]] || usage
      restore_drill "${2:-}"
      ;;
    *)
      usage
      ;;
  esac
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
