#!/bin/sh
# Assemble the Linux package: the client, the engine it starts, the launcher
# entry, the icons, the README and the installer, in a tarball with a checksum
# beside it.
#
#   assemble-linux.sh <version> <binary-dir> <out-dir>
#
# The binaries are stripped on the way in: symbols are a third of the two files
# and belong to a local build, not to a download.
set -eu

[ "$#" -eq 3 ] || {
  echo "usage: assemble-linux.sh <version> <binary-dir> <out-dir>" >&2
  exit 64
}

version="$1"
binaries="$2"
out="$3"
here=$(cd "$(dirname "$0")" && pwd)
package="Clumsies-${version}-linux-x86_64"

for program in clumsies-desktop clumsiesd; do
  test -x "${binaries}/${program}" || {
    echo "assemble-linux.sh: ${binaries}/${program} is missing" >&2
    exit 66
  }
done

rm -rf "${out:?}/${package}"
mkdir -p "${out}/${package}/icons"
strip --strip-all "${binaries}/clumsies-desktop" "${binaries}/clumsiesd"
install -m 755 "${binaries}/clumsies-desktop" "${binaries}/clumsiesd" "${out}/${package}/"
install -m 755 "${here}/install.sh" "${out}/${package}/install.sh"
install -m 644 "${here}/clumsies.desktop" "${out}/${package}/clumsies.desktop"
install -m 644 "${here}/README.txt" "${out}/${package}/README.txt"
cp "${here}/../assets/icons/"clumsies-*.png "${out}/${package}/icons/"

tar -C "${out}" -czf "${out}/${package}.tar.gz" "${package}"
(
  cd "${out}"
  sha256sum "${package}.tar.gz" > "${package}.tar.gz.sha256"
)
echo "assembled ${out}/${package}.tar.gz"
tar -tzf "${out}/${package}.tar.gz" | sort
