#!/bin/sh
# Register the installed App's embedded CLI; no separate macOS runtime or daemon.
set -eu
umask 077
script_dir=$(cd "$(dirname "$0")" && pwd)
app=${1:-}
if [ "$#" -gt 1 ]; then echo 'Usage: install-cli.sh [/path/to/Clumsies.app]' >&2; exit 64; fi
if [ -z "$app" ]; then
  case "$script_dir" in */Contents/Resources) app=$(cd "$script_dir/../.." && pwd) ;;
    *)
      for candidate in "$HOME/Applications/Clumsies.app" /Applications/Clumsies.app; do
        if [ -d "$candidate" ]; then
          [ -z "$app" ] || { echo 'Multiple installed Apps found; pass the intended Clumsies.app path.' >&2; exit 1; }
          app=$candidate
        fi
      done ;;
  esac
fi
[ -n "$app" ] && [ -d "$app" ] || { echo 'Install Clumsies.app into Applications first, then run this installer.' >&2; exit 1; }
app=$(cd "$app" && pwd -P)
case "$app" in /Volumes/*|*/AppTranslocation/*) echo 'Install the App on your Mac before installing its command entry.' >&2; exit 1 ;; esac
cli="$app/Contents/Resources/clumsies"
[ -x "$cli" ] || { echo "Bundled CLI is missing: $cli" >&2; exit 1; }
bin_dir=${CLUMSIES_BIN_DIR:-"$HOME/.local/bin"}
mkdir -p "$bin_dir"
bin_dir=$(cd "$bin_dir" && pwd -P)
entry="$bin_dir/clumsies"
if [ -e "$entry" ] || [ -L "$entry" ]; then
  [ -L "$entry" ] && [ "$(readlink "$entry")" = "$cli" ] || { echo "Refusing to replace unrelated command: $entry" >&2; exit 1; }
fi
# shellcheck source=crates/clumsiesd/packaging/shell-path.sh
. "$script_dir/shell-path.sh"
clumsies_add_shell_path "$bin_dir"
[ -L "$entry" ] || ln -s "$cli" "$entry"
"$entry" --version
printf 'Installed %s. Open a new terminal and run clumsies --version.\n' "$entry"
