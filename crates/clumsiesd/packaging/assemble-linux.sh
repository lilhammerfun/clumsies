#!/usr/bin/env bash
# Build an offline-verifiable Linux package containing a single matching binary pair.
set -euo pipefail
[[ $# == 3 ]] || { echo 'Usage: assemble-linux.sh VERSION BIN_DIR OUT_DIR' >&2; exit 64; }
version=$1
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || exit 64
binaries=$(cd -- "$2" && pwd)
mkdir -p -- "$3"
out=$(cd -- "$3" && pwd)
scripts=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
package="clumsies-cli-$version-linux-x86_64"
stage=$(mktemp -d)
trap 'rm -rf -- "$stage"' EXIT
mkdir "$stage/$package"
for name in clumsies clumsiesd; do cp -- "$binaries/$name" "$stage/$package/$name"; done
cp -- "$scripts/install.sh" "$stage/$package/install.sh"
chmod 755 "$stage/$package/"{clumsies,clumsiesd,install.sh}
echo "$version" > "$stage/$package/.clumsies-cli"
[[ $("$stage/$package/clumsies" --version) == "clumsies $version" ]] || { echo 'CLI and package version differ' >&2; exit 1; }
[[ $("$stage/$package/clumsiesd" --version) == "clumsiesd $version" ]] || { echo 'Daemon and package version differ' >&2; exit 1; }
(cd "$stage/$package" && sha256sum clumsies clumsiesd install.sh .clumsies-cli > SHA256SUMS)
tar -czf "$out/$package.tar.gz" -C "$stage" "$package"
(cd "$out" && sha256sum "$package.tar.gz" > "$package.tar.gz.sha256")
