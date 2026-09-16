#!/bin/sh
set -eu

if [ -z "${SPARKLE_PRIVATE_KEY:-}" ]; then
  echo "Missing release environment variable: SPARKLE_PRIVATE_KEY" >&2
  exit 1
fi
if [ -z "${GITHUB_REF_NAME:-}" ]; then
  echo "Missing release environment variable: GITHUB_REF_NAME" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
derived_data="${CLUMSIES_MACOS_DERIVED_DATA:-$repo_root/build/macos-derived}"
output_dir="${CLUMSIES_MACOS_OUTPUT_DIR:-$repo_root/dist/macos}"
generate_appcast="$derived_data/SourcePackages/artifacts/sparkle/Sparkle/bin/generate_appcast"

if [ ! -x "$generate_appcast" ]; then
  echo "Sparkle generate_appcast was not resolved at $generate_appcast" >&2
  exit 1
fi

# Keep the download DMG out of Sparkle's archive discovery for the same version.
archives="$(mktemp -d "${TMPDIR:-/tmp}/clumsies-appcast.XXXXXX")"
trap 'rm -rf "$archives"' EXIT
trap 'exit 1' HUP INT TERM
cp "$output_dir/Clumsies-${GITHUB_REF_NAME#v}-macos-universal.zip" "$archives/"

printf '%s' "$SPARKLE_PRIVATE_KEY" | "$generate_appcast" \
  --ed-key-file - \
  --download-url-prefix "https://github.com/lilhammerfun/clumsies/releases/download/$GITHUB_REF_NAME/" \
  --link "https://clumsies.ai" \
  --maximum-versions 1 \
  --maximum-deltas 0 \
  -o "$output_dir/appcast.xml" \
  "$archives"

test -s "$output_dir/appcast.xml"
printf '%s\n' "$output_dir/appcast.xml"
