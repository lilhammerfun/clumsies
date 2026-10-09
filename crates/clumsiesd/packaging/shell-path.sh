#!/bin/sh
# Add the user command directory once without replacing existing shell configuration.
set -eu

clumsies_add_shell_path() {
  bin_dir=$1
  cr=$(printf '\r')
  case "$bin_dir" in *'
'*|*"$cr"*|*:*) echo 'Command directory must not contain line breaks or PATH separators' >&2; return 1 ;; esac
  case "${SHELL:-/bin/bash}" in
    */zsh) profile_dir=${ZDOTDIR:-$HOME}; profiles="$profile_dir/.zprofile
$profile_dir/.zshrc" ;;
    */bash|*/sh)
      profiles="$HOME/.profile
$HOME/.bashrc"
      if [ -f "$HOME/.bash_profile" ]; then profiles="$profiles
$HOME/.bash_profile"; fi
      ;;
    *) echo 'Automatic PATH setup supports bash and zsh; select one as your login shell.' >&2; return 1 ;;
  esac
  escaped=$(printf '%s' "$bin_dir" | sed "s/'/'\\\\''/g")
  path_line="case \":\$PATH:\" in *':$escaped:'*) ;; *) export PATH='$escaped':\"\$PATH\" ;; esac"
  printf '%s\n' "$profiles" | while IFS= read -r profile; do
    [ ! -e "$profile" ] || [ -f "$profile" ] || { echo "Not a shell configuration file: $profile" >&2; exit 1; }
    mkdir -p "$(dirname "$profile")"
    if [ ! -f "$profile" ] || ! grep -Fqx "$path_line" "$profile"; then
      printf '\n# Clumsies CLI command directory\n%s\n' "$path_line" >> "$profile"
    fi
  done
}
