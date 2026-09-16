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

if [ "$CONFIGURATION" = "Release" ] && [ "${CLUMSIES_UNIVERSAL_BUILD:-0}" = "1" ]; then
  cargo build -p daemon --bin clumsiesd --release --target aarch64-apple-darwin
  cargo build -p daemon --bin clumsiesd --release --target x86_64-apple-darwin
  lipo -create \
    "$repo_root/target/aarch64-apple-darwin/release/clumsiesd" \
    "$repo_root/target/x86_64-apple-darwin/release/clumsiesd" \
    -output "$destination"
elif [ "$CONFIGURATION" = "Release" ]; then
  cargo build -p daemon --bin clumsiesd --release
  cp "$repo_root/target/release/clumsiesd" "$destination"
else
  cargo build -p daemon --bin clumsiesd
  cp "$repo_root/target/debug/clumsiesd" "$destination"
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
