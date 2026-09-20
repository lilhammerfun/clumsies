#!/bin/sh
set -eu

repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
app="${1:?Usage: test-distribution-package.sh /path/to/Clumsies.app [image.dmg]}"
test_root="$(mktemp -d "${TMPDIR:-/tmp}/clumsies-distribution-test.XXXXXX")"
mountpoint="$test_root/mounted"
mounted=0

cleanup() {
  if [ "$mounted" = 1 ]; then
    hdiutil detach -quiet "$mountpoint"
  fi
  rm -rf "$test_root"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

output="$test_root/output with spaces"
mkdir -p "$output"
image="${2:-$output/Clumsies-0.1.0-macos-universal.dmg}"
if [ "$#" -lt 2 ]; then
  sh "$repo_root/apps/macos/Scripts/create-dmg.sh" "$app" "$image"
fi
mkdir -p "$mountpoint"
hdiutil attach -quiet -readonly -nobrowse -mountpoint "$mountpoint" "$image"
mounted=1
test "$(readlink "$mountpoint/Applications")" = /Applications
codesign --verify --deep --strict "$mountpoint/Clumsies.app"
codesign --verify --strict "$mountpoint/Clumsies.app/Contents/Resources/clumsiesd"
test "$(codesign -dvv "$mountpoint/Clumsies.app/Contents/Resources/clumsiesd" 2>&1 | sed -n 's/^Identifier=//p')" = ai.clumsies.daemon
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$mountpoint/Clumsies.app/Contents/Info.plist")" = ai.clumsies.desktop
test -z "$(/usr/libexec/PlistBuddy -c 'Print :CLUMSIES_DEV_INSTANCE_ID' "$mountpoint/Clumsies.app/Contents/Info.plist")"
cmp "$app/Contents/MacOS/Clumsies" "$mountpoint/Clumsies.app/Contents/MacOS/Clumsies"
cmp "$app/Contents/Resources/clumsiesd" "$mountpoint/Clumsies.app/Contents/Resources/clumsiesd"
hdiutil detach -quiet "$mountpoint"
mounted=0

# A failed input must not leave a downloadable image behind.
if sh "$repo_root/apps/macos/Scripts/create-dmg.sh" "$test_root/missing.app" "$output/invalid.dmg"; then
  echo "DMG creation accepted a missing app." >&2
  exit 1
fi
test ! -e "$output/invalid.dmg"

# Exercise archive selection without accessing a signing keychain or network.
derived_data="$test_root/derived"
tool_dir="$derived_data/SourcePackages/artifacts/sparkle/Sparkle/bin"
mkdir -p "$tool_dir"
cat >"$tool_dir/generate_appcast" <<'EOF'
#!/bin/sh
set -eu
cat >/dev/null
while [ "$#" -gt 1 ]; do
  if [ "$1" = -o ]; then output="$2"; fi
  if [ "$1" = --download-url-prefix ]; then prefix="$2"; fi
  shift
done
test "$(ls -A "$1" | wc -l | tr -d ' ')" = 1
set -- "$1"/*
test -f "$1"
signature='sparkle:edSignature="test-signature"'
if [ "${TEST_UNSIGNED_APPCAST:-0}" = 1 ]; then signature=; fi
printf '<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item><enclosure url="%s%s" %s /></item></channel></rss>\n' \
  "$prefix" "$(basename "$1")" "$signature" >"$output"
EOF
chmod 755 "$tool_dir/generate_appcast"
printf 'test archive\n' >"$output/Clumsies-0.1.0-macos-universal.zip"
for release_tag in v0.1.0 macos-preview-16; do
  archive_name=Clumsies-0.1.0-macos-universal.zip
  feed_name=appcast.xml
  if [ "$release_tag" = macos-preview-16 ]; then
    archive_name=Clumsies-0.1.0-preview.16-macos-arm64.dmg
    feed_name=preview-appcast.xml
  fi
  printf 'test archive\n' >"$output/$archive_name"
  SPARKLE_PRIVATE_KEY=test-key \
    CLUMSIES_MACOS_DERIVED_DATA="$derived_data" \
    CLUMSIES_MACOS_OUTPUT_DIR="$output" \
    sh "$repo_root/apps/macos/Scripts/generate-appcast.sh" "$release_tag" "$output/$archive_name"
  grep -F "https://github.com/lilhammerfun/clumsies/releases/download/$release_tag/$archive_name" "$output/$feed_name" >/dev/null
done
python3 - "$output/preview-appcast.xml" <<'PYTEST'
import sys
import xml.etree.ElementTree as ET
item = ET.parse(sys.argv[1]).find("./channel/item")
ns = {"sparkle": "http://www.andymatuschak.org/xml-namespaces/sparkle"}
assert item.find("sparkle:informationalUpdate", ns) is None
assert item.find("enclosure") is not None, "Preview must offer an installable update"
assert item.find("enclosure").get("url").endswith(".dmg")
PYTEST
cp "$output/$feed_name" "$test_root/valid-appcast.xml"
if SPARKLE_PRIVATE_KEY='' CLUMSIES_MACOS_OUTPUT_DIR="$output" \
    sh "$repo_root/apps/macos/Scripts/generate-appcast.sh" macos-preview-16 "$output/$archive_name"; then
  echo 'Update generation accepted a missing signing key.' >&2
  exit 1
fi
cmp "$output/$feed_name" "$test_root/valid-appcast.xml"
if TEST_UNSIGNED_APPCAST=1 SPARKLE_PRIVATE_KEY=wrong-key \
    CLUMSIES_MACOS_DERIVED_DATA="$derived_data" \
    CLUMSIES_MACOS_OUTPUT_DIR="$output" \
    sh "$repo_root/apps/macos/Scripts/generate-appcast.sh" macos-preview-16 "$output/$archive_name"; then
  echo 'Unsigned appcast was accepted.' >&2
  exit 1
fi
cmp "$output/$feed_name" "$test_root/valid-appcast.xml"
test -s "$output/$feed_name"
test -s "$image"
printf '%s\n' 'DMG contents, signatures, signed DMG/ZIP selection, and unsigned install rejection passed.'
