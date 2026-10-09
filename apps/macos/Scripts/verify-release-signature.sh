#!/bin/sh
set -eu

fail() {
  echo "$1" >&2
  exit 1
}

if [ "$#" -ne 2 ]; then
  echo "Usage: $0 /path/to/Clumsies.app APPLE_TEAM_ID" >&2
  exit 2
fi

app="$1"
expected_team="$2"
runtime="$app/Contents/Resources/clumsiesd"
cli="$app/Contents/Resources/clumsies"

[ -d "$app" ] || fail "Not an app bundle: $app"
[ -x "$runtime" ] || fail "Missing bundled Agent runtime: $runtime"
[ -x "$cli" ] || fail "Missing bundled human CLI: $cli"
[ -n "$expected_team" ] || fail "APPLE_TEAM_ID must not be empty."

codesign --verify --deep --strict --verbose=2 "$app"
codesign --verify --strict --verbose=2 "$runtime"
codesign --verify --strict --verbose=2 "$cli"

app_signature=$(codesign --display --verbose=4 "$app" 2>&1)
runtime_signature=$(codesign --display --verbose=4 "$runtime" 2>&1)
cli_signature=$(codesign --display --verbose=4 "$cli" 2>&1)
app_identifier=$(printf '%s\n' "$app_signature" | sed -n 's/^Identifier=//p')
app_team=$(printf '%s\n' "$app_signature" | sed -n 's/^TeamIdentifier=//p')
runtime_team=$(printf '%s\n' "$runtime_signature" | sed -n 's/^TeamIdentifier=//p')
runtime_identifier=$(printf '%s\n' "$runtime_signature" | sed -n 's/^Identifier=//p')

[ "$app_identifier" = ai.clumsies.desktop ] || fail "Clumsies.app has an unexpected signing identifier."
[ "$app_team" = "$expected_team" ] || fail "Clumsies.app is not signed by the expected Apple team."
[ "$runtime_team" = "$expected_team" ] || fail "clumsiesd is not signed by the expected Apple team."
[ "$runtime_identifier" = ai.clumsies.daemon ] || fail "clumsiesd has an unexpected signing identifier."
[ "$(printf '%s\n' "$cli_signature" | sed -n 's/^TeamIdentifier=//p')" = "$expected_team" ] || fail "clumsies is not signed by the expected Apple team."
[ "$(printf '%s\n' "$cli_signature" | sed -n 's/^Identifier=//p')" = ai.clumsies.cli ] || fail "clumsies has an unexpected signing identifier."
printf '%s\n' "$cli_signature" | grep -Eq '^CodeDirectory .*flags=.*runtime' || fail "clumsies is missing the hardened-runtime signing flag."

printf '%s\n' "$app_signature" \
  | grep -Eq '^CodeDirectory .*flags=.*runtime' \
  || fail "Clumsies.app is missing the hardened-runtime signing flag."
printf '%s\n' "$runtime_signature" \
  | grep -Eq '^CodeDirectory .*flags=.*runtime' \
  || fail "clumsiesd is missing the hardened-runtime signing flag."

xcrun stapler validate "$app"
printf '%s\n' "Verified signed and notarized release app: $app"
