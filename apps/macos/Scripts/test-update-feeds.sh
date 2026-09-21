#!/bin/sh
set -eu

repo_root=$(cd "$(dirname "$0")/../../.." && pwd)
test_root=$(mktemp -d "${TMPDIR:-/tmp}/clumsies-update-feeds-test.XXXXXX")
trap 'rm -rf "$test_root"' EXIT
trap 'exit 1' HUP INT TERM
mkdir -p "$test_root/bin" "$test_root/assets"

cat >"$test_root/bin/gh" <<'SH'
#!/bin/sh
set -eu
test "$1" = release
case "$2" in
  view)
    test -f "$CLUMSIES_FEED_TEST_ROOT/release"
    if [ "$#" -gt 3 ]; then
      for asset in "$CLUMSIES_FEED_TEST_ROOT/assets/"*; do
        if [ -f "$asset" ]; then basename "$asset"; fi
      done
    fi
    ;;
  create)
    test ! -e "$CLUMSIES_FEED_TEST_ROOT/release"
    touch "$CLUMSIES_FEED_TEST_ROOT/release"
    ;;
  upload)
    test "$#" = 4
    target="$CLUMSIES_FEED_TEST_ROOT/assets/$(basename "$4")"
    test ! -e "$target"
    cp "$4" "$target"
    ;;
  *) exit 1 ;;
esac
SH
cat >"$test_root/bin/curl" <<'SH'
#!/bin/sh
set -eu
for argument do url="$argument"; done
test "${CLUMSIES_FEED_TEST_HTTP_FAILURE:-0}" = 0
cat "$CLUMSIES_FEED_TEST_ROOT/assets/${url##*/}"
SH
chmod +x "$test_root/bin/gh" "$test_root/bin/curl"
export PATH="$test_root/bin:$PATH"
export CLUMSIES_FEED_TEST_ROOT="$test_root"
export GH_REPO=example/test GITHUB_SHA=test-source-commit

sh "$repo_root/apps/macos/Scripts/ensure-update-feeds.sh"
python3 - "$test_root/assets" <<'PY'
from pathlib import Path
import sys
import xml.etree.ElementTree as ET

for name in ('appcast.xml', 'preview-appcast.xml'):
    root = ET.parse(Path(sys.argv[1]) / name).getroot()
    assert root.tag == 'rss'
    assert root.find('channel') is not None
    assert not root.findall('./channel/item')
PY

# A later bootstrap must preserve every byte of a published update feed.
printf 'existing signed update feed\n' >"$test_root/assets/preview-appcast.xml"
cp "$test_root/assets/preview-appcast.xml" "$test_root/expected.xml"
rm "$test_root/assets/appcast.xml"
sh "$repo_root/apps/macos/Scripts/ensure-update-feeds.sh"
cmp "$test_root/expected.xml" "$test_root/assets/preview-appcast.xml"
test -s "$test_root/assets/appcast.xml"
sh "$repo_root/apps/macos/Scripts/ensure-update-feeds.sh"
cmp "$test_root/expected.xml" "$test_root/assets/preview-appcast.xml"

if CLUMSIES_FEED_TEST_HTTP_FAILURE=1 sh "$repo_root/apps/macos/Scripts/ensure-update-feeds.sh"; then
  echo 'Feed bootstrap accepted an unreachable public URL.' >&2
  exit 1
fi
printf '%s\n' 'Empty feeds, existing feed preservation, repeat runs, and HTTP failure checks passed.'
