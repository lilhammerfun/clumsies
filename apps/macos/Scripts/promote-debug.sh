#!/bin/sh
set -eu

repo_root="$(cd "$(dirname "$0")/../../.." && pwd)"
derived_data="${CLUMSIES_MACOS_DERIVED_DATA:-/private/tmp/clumsies-macos-derived}"
install_dir="${CLUMSIES_MACOS_INSTALL_DIR:-$HOME/Applications}"
built_app="$derived_data/Build/Products/Debug/Clumsies.app"
installed_app="$install_dir/Clumsies.app"
staging_app="$install_dir/.Clumsies.$$.app"
previous_app="$install_dir/.Clumsies.previous.$$.app"
transaction_active=0
had_installed_app=0

cleanup() {
  trap - EXIT HUP INT TERM

  if [ "$transaction_active" -eq 1 ]; then
    if [ -e "$previous_app" ]; then
      if rm -rf -- "$installed_app" && mv "$previous_app" "$installed_app"; then
        if ! "$installed_app/Contents/Resources/clumsiesd" \
          --reconcile-launch-agent >/dev/null 2>&1; then
          echo "Warning: failed to reconcile the restored Clumsies daemon." >&2
        fi
      else
        echo "Failed to restore the previous Clumsies app." >&2
      fi
    elif [ "$had_installed_app" -eq 0 ]; then
      rm -rf -- "$installed_app"
    fi
  fi
  rm -rf -- "$staging_app"
}

trap cleanup EXIT
trap 'exit 1' HUP INT TERM

cd "$repo_root"
xcodegen generate --spec apps/macos/project.yml
xcodebuild \
  -quiet \
  -project apps/macos/Clumsies.xcodeproj \
  -scheme Clumsies \
  -configuration Debug \
  -derivedDataPath "$derived_data" \
  build

test -d "$built_app"
codesign --verify --deep --strict "$built_app"
mkdir -p "$install_dir"
ditto "$built_app" "$staging_app"

if pgrep -x Clumsies >/dev/null 2>&1; then
  osascript -e 'tell application id "ai.clumsies.desktop" to quit'

  attempts=0
  while pgrep -x Clumsies >/dev/null 2>&1; do
    attempts=$((attempts + 1))
    if [ "$attempts" -ge 100 ]; then
      echo "Clumsies did not quit within 10 seconds." >&2
      exit 1
    fi
    sleep 0.1
  done
fi

transaction_active=1
if [ -e "$installed_app" ]; then
  had_installed_app=1
  mv "$installed_app" "$previous_app"
fi

mv "$staging_app" "$installed_app"

if [ "$had_installed_app" -eq 1 ]; then
  "$installed_app/Contents/Resources/clumsiesd" --reconcile-launch-agent >/dev/null
fi
open -n "$installed_app"
transaction_active=0
trap - EXIT HUP INT TERM
rm -rf -- "$previous_app"
printf '%s\n' \
  "Clumsies.app installed at $installed_app." \
  'The App reconciles the global Plugin after launch; restart Codex and create a new task when it finishes.'
