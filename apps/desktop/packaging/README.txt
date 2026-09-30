Clumsies — the desktop client
=============================

What is in this package
-----------------------

  clumsies-desktop   the client: the window, the screens, the session
  clumsiesd          the engine: it holds the session, the Project's checkout
                     and the local cache, and it is what the client talks to
  clumsies.desktop   the Linux launcher entry
  icons/             the application icons, in the sizes a launcher asks for
  install.sh         Linux only: puts the above where the desktop finds them

The client starts the engine itself: `clumsiesd` is expected beside
`clumsies-desktop`, and remains running for Agent sessions after the client
closes. Nothing else has to be installed.

Running it
----------

Linux:

  ./clumsies-desktop

  To have it in the launcher instead:

  ./install.sh

  which copies the client and engine to ~/.local/bin, the launcher entry to
  ~/.local/share/applications, and the icons to
  ~/.local/share/icons/hicolor. `install.sh --prefix DIR` installs under DIR
  (for example /usr/local) instead.

Windows:

  clumsies-desktop.exe

  The package is not signed, so Windows may show a SmartScreen warning the
  first time: "More info" → "Run anyway".

Where your data lives
---------------------

Linux:   ~/.local/share/ai.clumsies/       (engine state, cache, logs)
Windows: %LOCALAPPDATA%\ai.clumsies\       (engine state, cache, logs)

Signing in
----------

The client asks for the address of your Clumsies Server and continues in your
browser, or with a local password when the Server's deployment has passwords
enabled. The session is handed to the engine, which is the only part of the
installation that keeps it.

Verifying the download
----------------------

Each package ships with a `.sha256` file:

  sha256sum -c Clumsies-<version>-linux-x86_64.tar.gz.sha256

  certutil -hashfile Clumsies-<version>-windows-x86_64.zip SHA256
