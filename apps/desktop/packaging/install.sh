#!/bin/sh
# Put Clumsies where the desktop finds it: the two programs in PATH, the
# launcher entry and the icons in the user's own share directories.
#
#   ./install.sh                 ~/.local (no root needed)
#   ./install.sh --prefix /usr/local
set -eu

prefix="${HOME}/.local"
if [ "${1:-}" = "--prefix" ] && [ -n "${2:-}" ]; then
  prefix="$2"
elif [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
  sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'
  exit 0
fi

here=$(cd "$(dirname "$0")" && pwd)
bindir="${prefix}/bin"
applications="${prefix}/share/applications"
icons="${prefix}/share/icons/hicolor"

for program in clumsies-desktop clumsiesd; do
  test -x "${here}/${program}" || {
    echo "install.sh: ${program} is missing from this package" >&2
    exit 1
  }
done

mkdir -p "${bindir}" "${applications}" "${icons}"
install -m 755 "${here}/clumsies-desktop" "${bindir}/clumsies-desktop"
install -m 755 "${here}/clumsiesd" "${bindir}/clumsiesd"
install -m 644 "${here}/clumsies.desktop" "${applications}/clumsies.desktop"

for size in 16 32 48 64 128 256 512; do
  source="${here}/icons/clumsies-${size}.png"
  target="${icons}/${size}x${size}/apps"
  [ -f "${source}" ] || continue
  mkdir -p "${target}"
  install -m 644 "${source}" "${target}/clumsies.png"
done

# The launcher reads the entry as it is written, but its icon lookup and the
# window's class are cached by the running desktop.
command -v update-desktop-database >/dev/null 2>&1 &&
  update-desktop-database "${applications}" >/dev/null 2>&1 || true
command -v gtk-update-icon-cache >/dev/null 2>&1 &&
  gtk-update-icon-cache -qtf "${icons}" >/dev/null 2>&1 || true

echo "installed Clumsies into ${prefix}"
echo "  ${bindir}/clumsies-desktop"
case ":${PATH}:" in
  *":${bindir}:"*) ;;
  *) echo "  note: ${bindir} is not in PATH" ;;
esac
