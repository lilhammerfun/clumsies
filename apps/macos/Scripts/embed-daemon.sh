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
  cargo build -p clumsiesd --bins --release --target aarch64-apple-darwin
  cargo build -p clumsiesd --bins --release --target x86_64-apple-darwin
  for binary in clumsiesd clumsies; do
    lipo -create "$repo_root/target/aarch64-apple-darwin/release/$binary" "$repo_root/target/x86_64-apple-darwin/release/$binary" -output "$(dirname "$destination")/$binary"
  done
elif [ "$CONFIGURATION" = "Release" ]; then
  cargo build -p clumsiesd --bins --release
  for binary in clumsiesd clumsies; do cp "$repo_root/target/release/$binary" "$(dirname "$destination")/$binary"; done
else
  cargo build -p clumsiesd --bins
  for binary in clumsiesd clumsies; do cp "$repo_root/target/debug/$binary" "$(dirname "$destination")/$binary"; done
fi

printf '%s\n' "$CLUMSIES_AGENT_RUNTIME_BUILD_ID" > "$(dirname "$destination")/clumsiesd-build-id"
for binary in clumsiesd clumsies; do
  program="$(dirname "$destination")/$binary"
  identifier="$daemon_identifier"
  if [ "$binary" = clumsies ]; then identifier=ai.clumsies.cli; fi
  chmod 755 "$program"
  if [ -n "${EXPANDED_CODE_SIGN_IDENTITY:-}" ] && [ "$EXPANDED_CODE_SIGN_IDENTITY" != "-" ]; then
    codesign --force --sign "$EXPANDED_CODE_SIGN_IDENTITY" --identifier "$identifier" --options runtime "$program"
  else
    # Keep the file-keychain ACL stable across unsigned local rebuilds.
    codesign --force --sign - --identifier "$identifier" --requirements "=designated => identifier \"$identifier\"" "$program"
  fi
done
