#!/bin/sh
set -eu

case "${CLUMSIES_BUILD_NUMBER:-}" in
  ''|*[!0-9]*) echo 'CLUMSIES_BUILD_NUMBER must be a positive integer.' >&2; exit 1 ;;
esac
test "$CLUMSIES_BUILD_NUMBER" -gt 0
test "$(uname -m)" = arm64 || { echo 'Ad-hoc builds require an Apple Silicon Mac.' >&2; exit 1; }

repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
derived_data="${CLUMSIES_MACOS_DERIVED_DATA:-$repo_root/build/macos-adhoc-derived}"
output_dir="${CLUMSIES_MACOS_OUTPUT_DIR:-$repo_root/dist/macos}"
app="$derived_data/Build/Products/Debug/Clumsies.app"

cd "$repo_root"
version=$(awk -F'"' '/MARKETING_VERSION:/ { print $2; exit }' apps/macos/project.yml)
case "${1:-preview}" in
  preview)
    image_name="Clumsies-$version-preview.$CLUMSIES_BUILD_NUMBER-macos-arm64.dmg"
    feed=preview-appcast.xml
    ;;
  release)
    image_name="Clumsies-$version-macos-arm64.dmg"
    feed=appcast.xml
    ;;
  *) echo 'Usage: archive-adhoc.sh [preview|release]' >&2; exit 1 ;;
esac
xcodegen generate --spec apps/macos/project.yml

# Both GitHub release channels use the existing ad-hoc Debug runtime contract.
# The Release runtime requires Developer ID signing for Agent installation.
CARGO_PROFILE_DEV_DEBUG=0 xcodebuild \
  -project apps/macos/Clumsies.xcodeproj \
  -scheme Clumsies \
  -configuration Debug \
  -destination "platform=macOS,arch=arm64" \
  -derivedDataPath "$derived_data" \
  ARCHS=arm64 \
  ONLY_ACTIVE_ARCH=YES \
  CODE_SIGN_STYLE=Manual \
  CODE_SIGN_IDENTITY=- \
  DEVELOPMENT_TEAM= \
  CURRENT_PROJECT_VERSION="$CLUMSIES_BUILD_NUMBER" \
  CLUMSIES_UPDATE_FEED_URL="https://github.com/lilhammerfun/clumsies/releases/download/macos-updates/$feed" \
  build

codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"
test "$(/usr/libexec/PlistBuddy -c 'Print :SUFeedURL' "$app/Contents/Info.plist")" = \
  "https://github.com/lilhammerfun/clumsies/releases/download/macos-updates/$feed"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" = "$version"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$app/Contents/Info.plist")" = "$CLUMSIES_BUILD_NUMBER"
lipo "$app/Contents/MacOS/Clumsies" -verify_arch arm64
lipo "$app/Contents/Resources/clumsiesd" -verify_arch arm64
lipo "$app/Contents/Resources/clumsies" -verify_arch arm64
sh apps/macos/Scripts/create-dmg.sh "$app" "$output_dir/$image_name"
sh apps/macos/Scripts/test-distribution-package.sh "$app" "$output_dir/$image_name"
(cd "$output_dir" && shasum -a 256 "$image_name" > "$image_name.sha256")
printf '%s\n' "$output_dir/$image_name"
