#!/bin/sh
set -eu

: "${GH_REPO:?Set GH_REPO to the GitHub repository}"
: "${GITHUB_SHA:?Set GITHUB_SHA to the source commit}"
feed_tag=macos-updates
temporary_dir=$(mktemp -d "${TMPDIR:-/tmp}/clumsies-update-feeds.XXXXXX")
trap 'rm -rf "$temporary_dir"' EXIT
trap 'exit 1' HUP INT TERM

if ! gh release view "$feed_tag" >/dev/null 2>&1; then
  gh release create "$feed_tag" --target "$GITHUB_SHA" --prerelease --latest=false \
    --title "macOS update feeds" \
    --notes "Stable URLs for Sparkle update feeds. Download installers from the versioned releases."
fi

assets=$(gh release view "$feed_tag" --json assets --jq '.assets[].name')
for feed in appcast.xml preview-appcast.xml; do
  if ! printf '%s\n' "$assets" | grep -Fx "$feed" >/dev/null; then
    # No eligible release is a valid empty feed, not a missing endpoint.
    cat >"$temporary_dir/$feed" <<'XML'
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>Clumsies macOS Updates</title>
    <link>https://clumsies.ai</link>
    <description>Available Clumsies macOS updates.</description>
  </channel>
</rss>
XML
    # Never clobber an existing feed, including one published concurrently.
    gh release upload "$feed_tag" "$temporary_dir/$feed"
  fi
  curl --fail --silent --show-error --location --retry 3 --retry-all-errors --max-time 15 \
    "https://github.com/$GH_REPO/releases/download/$feed_tag/$feed" >/dev/null
done
