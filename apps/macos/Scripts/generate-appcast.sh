#!/bin/sh
set -eu

repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
output_dir="${CLUMSIES_MACOS_OUTPUT_DIR:-$repo_root/dist/macos}"

if [ -z "${SPARKLE_PRIVATE_KEY:-}" ]; then
  echo "Missing release environment variable: SPARKLE_PRIVATE_KEY" >&2
  exit 1
fi
tag="${1:?Usage: generate-appcast.sh RELEASE_TAG UPDATE.zip-or-dmg}"
archive="${2:?Usage: generate-appcast.sh RELEASE_TAG UPDATE.zip-or-dmg}"
test -f "$archive"
case "$tag" in
  macos-preview-*) feed_name=preview-appcast.xml ;;
  *) feed_name=appcast.xml ;;
esac

derived_data="${CLUMSIES_MACOS_DERIVED_DATA:-$repo_root/build/macos-derived}"
generate_appcast="$derived_data/SourcePackages/artifacts/sparkle/Sparkle/bin/generate_appcast"

if [ ! -x "$generate_appcast" ]; then
  echo "Sparkle generate_appcast was not resolved at $generate_appcast" >&2
  exit 1
fi

# Scan only the selected archive, so other formats cannot create duplicate updates.
archives="$(mktemp -d "${TMPDIR:-/tmp}/clumsies-appcast.XXXXXX")"
trap 'rm -rf "$archives"' EXIT
trap 'exit 1' HUP INT TERM
cp "$archive" "$archives/"

printf '%s' "$SPARKLE_PRIVATE_KEY" | "$generate_appcast" \
  --ed-key-file - \
  --download-url-prefix "https://github.com/lilhammerfun/clumsies/releases/download/$tag/" \
  --link "https://clumsies.ai" \
  --maximum-versions 1 \
  --maximum-deltas 0 \
  -o "$archives/appcast.xml" \
  "$archives"

# generate_appcast can emit an unsigned enclosure when the key does not match.
# Never replace the published feed with an update that clients cannot verify.
python3 - "$archives/appcast.xml" "$(basename "$archive")" <<'PY'
import sys
import xml.etree.ElementTree as ET

items = ET.parse(sys.argv[1]).findall("./channel/item")
if len(items) != 1:
    raise SystemExit("Expected one signed update")
enclosure = items[0].find("enclosure")
if enclosure is None or not enclosure.get("{http://www.andymatuschak.org/xml-namespaces/sparkle}edSignature"):
    raise SystemExit("Missing EdDSA signature; check SPARKLE_PRIVATE_KEY against SUPublicEDKey")
if not enclosure.get("url", "").endswith("/" + sys.argv[2]):
    raise SystemExit("Unexpected update archive URL")
PY
mkdir -p "$output_dir"
cp "$archives/appcast.xml" "$output_dir/$feed_name"
printf '%s\n' "$output_dir/$feed_name"
