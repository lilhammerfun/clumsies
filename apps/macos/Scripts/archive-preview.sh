#!/bin/sh
set -eu

case "${CLUMSIES_BUILD_NUMBER:-}" in
  ''|*[!0-9]*) echo 'CLUMSIES_BUILD_NUMBER must be a positive integer.' >&2; exit 1 ;;
esac
test "$CLUMSIES_BUILD_NUMBER" -gt 0

repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
derived_data="${CLUMSIES_MACOS_DERIVED_DATA:-$repo_root/build/macos-preview-derived}"
output_dir="${CLUMSIES_MACOS_OUTPUT_DIR:-$repo_root/dist/macos-preview}"
app="$derived_data/Build/Products/Debug/Clumsies.app"

cd "$repo_root"
version=$(awk -F'"' '/MARKETING_VERSION:/ { print $2; exit }' apps/macos/project.yml)
image_name="Clumsies-$version-preview.$CLUMSIES_BUILD_NUMBER-macos-universal.dmg"
xcodegen generate --spec apps/macos/project.yml

# Debug is the existing ad-hoc runtime contract used by install-macos.
# Release continues to require a Developer ID; previews use the regular App identity.
CLUMSIES_UNIVERSAL_BUILD=1 CARGO_PROFILE_DEV_DEBUG=0 xcodebuild \
  -project apps/macos/Clumsies.xcodeproj \
  -scheme Clumsies \
  -configuration Debug \
  -destination "generic/platform=macOS" \
  -derivedDataPath "$derived_data" \
  ARCHS="arm64 x86_64" \
  ONLY_ACTIVE_ARCH=NO \
  CODE_SIGN_STYLE=Manual \
  CODE_SIGN_IDENTITY=- \
  DEVELOPMENT_TEAM= \
  CURRENT_PROJECT_VERSION="$CLUMSIES_BUILD_NUMBER" \
  build

codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"
lipo -verify_arch arm64 x86_64 "$app/Contents/MacOS/Clumsies"
lipo -verify_arch arm64 x86_64 "$app/Contents/Resources/clumsiesd"
sh apps/macos/Scripts/create-dmg.sh "$app" "$output_dir/$image_name"
sh apps/macos/Scripts/test-distribution-package.sh "$app" "$output_dir/$image_name"
(cd "$output_dir" && shasum -a 256 "$image_name" > "$image_name.sha256")
printf '%s\n' "$output_dir/$image_name"
