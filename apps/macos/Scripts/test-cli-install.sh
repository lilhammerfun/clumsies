#!/bin/sh
set -eu
repo_root=$(cd "$(dirname "$0")/../../.." && pwd)
root=$(mktemp -d)
root=$(cd "$root" && pwd -P)
trap 'rm -rf "$root"' EXIT
export HOME="$root/home with spaces"
export SHELL=/bin/zsh
unset ZDOTDIR
app="$HOME/Applications/Clumsies.app"
resources="$app/Contents/Resources"
mkdir -p "$resources" "$HOME/.local/bin"
printf '# existing user settings\nexport KEEP_ME=yes\n' > "$HOME/.zshrc"
cp "$repo_root/apps/macos/Scripts/install-cli.sh" "$resources/"
cp "$repo_root/crates/clumsiesd/packaging/shell-path.sh" "$resources/"
printf '#!/bin/sh\nprintf "clumsies test-version\\n"\n' > "$resources/clumsies"
chmod 755 "$resources/clumsies"
sh "$resources/install-cli.sh"
sh "$resources/install-cli.sh"
test "$(grep -c '^# Clumsies CLI command directory$' "$HOME/.zshrc")" = 1
grep -q 'export KEEP_ME=yes' "$HOME/.zshrc"
test "$(env PATH=/usr/bin:/bin zsh -lic 'command -v clumsies; clumsies --version')" = "$HOME/.local/bin/clumsies
clumsies test-version"
# Variables are evaluated by the isolated child shell.
# shellcheck disable=SC2016
test "$(env PATH=/usr/bin:/bin zsh -ic 'echo $KEEP_ME; clumsies --version')" = "yes
clumsies test-version"
# Stable command entry follows replacement of the App's embedded runtime.
printf '#!/bin/sh\nprintf "clumsies upgraded\\n"\n' > "$resources/clumsies"
test "$(env PATH=/usr/bin:/bin zsh -lic 'clumsies --version')" = 'clumsies upgraded'
rm "$HOME/.local/bin/clumsies"
printf 'unrelated command\n' > "$HOME/.local/bin/clumsies"
if sh "$resources/install-cli.sh"; then echo 'Replaced an unrelated command' >&2; exit 1; fi
test "$(cat "$HOME/.local/bin/clumsies")" = 'unrelated command'
# Shared Linux PATH helper also supports bash login and non-login terminals.
export HOME="$root/bash-home"
export SHELL=/bin/bash
mkdir -p "$HOME"
printf 'export KEEP_ME=yes\n' > "$HOME/.bash_login"
. "$repo_root/crates/clumsiesd/packaging/shell-path.sh"
clumsies_add_shell_path "$root/command dir's"
clumsies_add_shell_path "$root/command dir's"
test "$(grep -c '^# Clumsies CLI command directory$' "$HOME/.bash_login")" = 1
# shellcheck disable=SC2016
env PATH=/usr/bin:/bin bash -lc 'case "$PATH" in *"command dir"*) exit 0 ;; *) exit 1 ;; esac'
printf 'export KEEP_ME=yes\n' > "$HOME/.bash_profile"
clumsies_add_shell_path "$root/command dir's"
clumsies_add_shell_path "$root/command dir's"
test "$(grep -c '^# Clumsies CLI command directory$' "$HOME/.bash_profile")" = 1
# shellcheck disable=SC2016
env PATH=/usr/bin:/bin bash -lc 'case "$PATH" in *"command dir"*) exit 0 ;; *) exit 1 ;; esac'
# shellcheck disable=SC2016
env PATH=/usr/bin:/bin bash --noprofile --rcfile "$HOME/.bashrc" -ic 'case "$PATH" in *"command dir"*) exit 0 ;; *) exit 1 ;; esac'
echo 'CLI shell installation, idempotence, upgrade and command protection passed.'
