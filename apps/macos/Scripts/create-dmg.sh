#!/bin/sh
set -eu

if [ "$#" -ne 2 ]; then
  echo "Usage: $0 /path/to/Clumsies.app /path/to/Clumsies.dmg" >&2
  exit 2
fi

app="$1"
image="$2"
if [ ! -x "$app/Contents/MacOS/Clumsies" ] || [ ! -x "$app/Contents/Resources/clumsiesd" ]; then
  echo "Expected a complete Clumsies.app with its bundled daemon: $app" >&2
  exit 1
fi

staging="$(mktemp -d "${TMPDIR:-/tmp}/clumsies-dmg.XXXXXX")"
trap 'rm -rf "$staging"' EXIT
trap 'exit 1' HUP INT TERM

ditto "$app" "$staging/Clumsies.app"
ln -s /Applications "$staging/Applications"
mkdir -p "$(dirname "$image")"
hdiutil create -quiet -volname Clumsies -srcfolder "$staging" \
  -fs HFS+ -format UDZO "$image"
hdiutil verify -quiet "$image"
printf '%s\n' "$image"
