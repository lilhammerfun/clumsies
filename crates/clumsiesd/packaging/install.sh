#!/usr/bin/env bash
# User installation, same-directory staging, and rollback without touching daemon data.
set -euo pipefail
umask 077
source_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
install_root=${CLUMSIES_INSTALL_ROOT:-"$HOME/.local/lib/clumsies"}
bin_root=${CLUMSIES_BIN_DIR:-"$HOME/.local/bin"}
mkdir -p -- "$install_root" "$bin_root"
exec 9>"$install_root/.install.lock"
flock -n 9 || { echo 'Another Clumsies installation is running' >&2; exit 1; }
runtime="$install_root/runtime"
[[ ! -L "$runtime" && ! -L "$install_root/.previous" ]] || { echo 'Refusing a symlinked installation directory' >&2; exit 1; }
if [[ -e "$runtime" ]]; then
  [[ -f "$runtime/.clumsies-cli" ]] || { echo "Refusing to replace an unrelated directory: $runtime" >&2; exit 1; }
fi
if [[ ${1:-} == --uninstall ]]; then
  if [[ -x "$runtime/clumsies" ]]; then "$runtime/clumsies" daemon stop; fi
  for name in clumsies clumsiesd; do
    if [[ -L "$bin_root/$name" && $(readlink -- "$bin_root/$name") == "$runtime/$name" ]]; then rm -- "$bin_root/$name"; fi
  done
  rm -rf -- "$runtime"
  echo 'Removed programs; credentials, bindings, cache, and local drafts were retained.'
  exit 0
fi
[[ $# == 0 ]] || { echo 'Usage: install.sh [--uninstall]' >&2; exit 64; }
(cd -- "$source_dir" && sha256sum --check --strict SHA256SUMS)
(cd -- "$source_dir" && diff -u SHA256SUMS <(sha256sum clumsies clumsiesd install.sh .clumsies-cli))
[[ -f "$source_dir/.clumsies-cli" ]] || { echo 'Missing CLI package marker' >&2; exit 1; }
for name in clumsies clumsiesd; do
  [[ -x "$source_dir/$name" ]] || { echo "Missing executable $name" >&2; exit 1; }
  if [[ -e "$bin_root/$name" || -L "$bin_root/$name" ]]; then
    [[ -L "$bin_root/$name" && $(readlink -- "$bin_root/$name") == "$runtime/$name" ]] || { echo "Refusing to replace $bin_root/$name; choose another CLUMSIES_BIN_DIR" >&2; exit 1; }
  fi
done
stage=$(mktemp -d "$install_root/.stage.XXXXXX")
backup="$install_root/.previous"
switched=false
cleanup() {
  result=$?
  if [[ $result != 0 && $switched == true ]]; then
    rm -rf -- "$runtime"
    if [[ -d "$backup" ]]; then mv -- "$backup" "$runtime"; fi
  fi
  rm -rf -- "$stage"
  exit "$result"
}
trap cleanup EXIT
for name in clumsies clumsiesd install.sh .clumsies-cli SHA256SUMS; do cp -a -- "$source_dir/$name" "$stage/"; done
"$stage/clumsies" --version
[[ ! -d "$backup" ]] || { echo "Previous recovery directory exists: $backup; restore or remove it before upgrading" >&2; exit 1; }
# Stop acknowledges through user-local IPC and waits for the resident lock.
if [[ -x "$runtime/clumsies" ]]; then "$runtime/clumsies" daemon stop; else "$stage/clumsies" daemon stop; fi
if [[ -d "$runtime" ]]; then mv -- "$runtime" "$backup"; fi
switched=true
mv -- "$stage" "$runtime"
for name in clumsies clumsiesd; do ln -sfn -- "$runtime/$name" "$bin_root/$name"; done
rm -rf -- "$backup"
switched=false
echo "Installed $runtime; add $bin_root to PATH. Agent integrations keep the same runtime path."
