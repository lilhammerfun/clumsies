# Packaging

## Arch / Omarchy

`PKGBUILD` builds `clumsies-bin`. On Omarchy, everything is installed through
pacman or the AUR, so that is the shape this takes:

```sh
# Once the package is on the AUR:
omarchy pkg aur add clumsies-bin

# Or from a package built locally:
sudo pacman -U clumsies-bin-0.1.0-1-x86_64.pkg.tar.zst
```

The package installs:

| Path | What |
| --- | --- |
| `/usr/bin/clumsies-desktop` | the client |
| `/usr/share/applications/ai.clumsies.desktop` | launcher entry |
| `/usr/share/icons/hicolor/256x256/apps/ai.clumsies.desktop.png` | icon |

The window sets the app id `ai.clumsies.desktop` and the entry sets the same
`StartupWMClass`, which is what keeps the launcher and the compositor from
showing two entries for one window.

## Not packaged yet

- `clumsiesd`, the local engine. It attends the client over a local socket on
  Linux, and that transport is not implemented yet; a binary that exits on
  start would only mislead. It arrives with a systemd user unit.
- Windows: an installer and a signing story.
