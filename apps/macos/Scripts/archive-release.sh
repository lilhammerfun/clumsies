#!/bin/sh
set -eu

for name in APPLE_ID APPLE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_TEAM_ID CLUMSIES_VERSION CLUMSIES_BUILD_NUMBER; do
  eval "value=\${$name:-}"
  if [ -z "$value" ]; then
    echo "Missing release environment variable: $name" >&2
    exit 1
  fi
done

repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
derived_data="${CLUMSIES_MACOS_DERIVED_DATA:-$repo_root/build/macos-derived}"
archive_path="${CLUMSIES_MACOS_ARCHIVE_PATH:-$repo_root/build/Clumsies.xcarchive}"
output_dir="${CLUMSIES_MACOS_OUTPUT_DIR:-$repo_root/dist/macos}"
app_path="$archive_path/Products/Applications/Clumsies.app"
archive_name="Clumsies-$CLUMSIES_VERSION-macos-universal.zip"
update_archive="$output_dir/$archive_name"
disk_image="$output_dir/Clumsies-$CLUMSIES_VERSION-macos-universal.dmg"

cd "$repo_root"
mkdir -p "$output_dir" "$(dirname "$archive_path")"
xcodegen generate --spec apps/macos/project.yml

CLUMSIES_UNIVERSAL_BUILD=1 xcodebuild \
  -project apps/macos/Clumsies.xcodeproj \
  -scheme Clumsies \
  -configuration Release \
  -destination "generic/platform=macOS" \
  -derivedDataPath "$derived_data" \
  -archivePath "$archive_path" \
  ARCHS="arm64 x86_64" \
  ONLY_ACTIVE_ARCH=NO \
  CODE_SIGN_STYLE=Manual \
  CODE_SIGN_IDENTITY="$APPLE_SIGNING_IDENTITY" \
  DEVELOPMENT_TEAM="$APPLE_TEAM_ID" \
  MARKETING_VERSION="$CLUMSIES_VERSION" \
  CURRENT_PROJECT_VERSION="$CLUMSIES_BUILD_NUMBER" \
  archive

codesign --verify --deep --strict --verbose=2 "$app_path"
ditto -c -k --sequesterRsrc --keepParent "$app_path" "$update_archive"
xcrun notarytool submit "$update_archive" \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" \
  --wait
xcrun stapler staple "$app_path"
apps/macos/Scripts/verify-release-signature.sh "$app_path" "$APPLE_TEAM_ID"

rm "$update_archive"
ditto -c -k --sequesterRsrc --keepParent "$app_path" "$update_archive"

sh apps/macos/Scripts/create-dmg.sh "$app_path" "$disk_image"
codesign --sign "$APPLE_SIGNING_IDENTITY" --timestamp "$disk_image"
xcrun notarytool submit "$disk_image" \
  --apple-id "$APPLE_ID" \
  --password "$APPLE_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" \
  --wait
xcrun stapler staple "$disk_image"
xcrun stapler validate "$disk_image"
codesign --verify --strict --verbose=2 "$disk_image"
spctl --assess --type open --context context:primary-signature --verbose=2 "$disk_image"
printf '%s\n' "$update_archive" "$disk_image"
