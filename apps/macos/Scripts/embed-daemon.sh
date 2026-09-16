#!/bin/sh
set -eu

if [ "${CLUMSIES_SKIP_DAEMON_BUILD:-0}" = "1" ]; then
  exit 0
fi

repo_root="$(cd "$SRCROOT/../.." && pwd)"
cd "$repo_root"
unset http_proxy https_proxy HTTP_PROXY HTTPS_PROXY ALL_PROXY all_proxy
export CLUMSIES_AGENT_RUNTIME_BUILD_ID="${CLUMSIES_AGENT_RUNTIME_BUILD_ID:-app-$(date +%s)-$$}"
destination="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/clumsiesd"
daemon_identifier="ai.clumsies.daemon"
mkdir -p "$(dirname "$destination")"

profile=debug
set --
if [ "$CONFIGURATION" = "Release" ]; then
  profile=release
  set -- --release
fi

if [ "${CLUMSIES_UNIVERSAL_BUILD:-0}" = "1" ]; then
  cargo build --locked -p daemon --bin clumsiesd "$@" --target aarch64-apple-darwin
  cargo build --locked -p daemon --bin clumsiesd "$@" --target x86_64-apple-darwin
  lipo -create \
    "$repo_root/target/aarch64-apple-darwin/$profile/clumsiesd" \
    "$repo_root/target/x86_64-apple-darwin/$profile/clumsiesd" \
    -output "$destination"
else
  cargo build --locked -p daemon --bin clumsiesd "$@"
  cp "$repo_root/target/$profile/clumsiesd" "$destination"
fi

chmod 755 "$destination"

if [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ] && [ "$EXPANDED_CODE_SIGN_IDENTITY" != "-" ]; then
  codesign \
    --force \
    --sign "$EXPANDED_CODE_SIGN_IDENTITY" \
    --identifier "$daemon_identifier" \
    --options runtime \
    "$destination"
else
  # Keep the file-keychain ACL stable across unsigned local rebuilds.
  codesign \
    --force \
    --sign - \
    --identifier "$daemon_identifier" \
    --requirements "=designated => identifier \"$daemon_identifier\"" \
    "$destination"
fi
