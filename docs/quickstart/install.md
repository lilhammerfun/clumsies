---
title: Install Clumsies
description: Download the macOS DMG, open Clumsies, and connect to your organization, or install from source.
prev:
  text: Quickstart
  link: /quickstart/
next:
  text: Connect to your organization
  link: /quickstart/connect
---
# Install Clumsies

## Download and install

Supports **macOS 14 or later on Apple Silicon Macs (M1 or newer)**, without build tools.

1. Open [GitHub Releases](https://github.com/lilhammerfun/clumsies/releases) and download `Clumsies-*-macos-arm64.dmg` from the newest **Clumsies macOS Preview**.
2. Open the DMG, drag `Clumsies.app` into `Applications`, and eject the disk image.
3. Open the App. The preview is not notarized by Apple. If macOS blocks it, verify that you trust the download, then use **System Settings → Privacy & Security → Open Anyway**. [Apple's instructions](https://support.apple.com/102445)
4. Continue to [Connect to your organization](/quickstart/connect). The default Server is `https://app.clumsies.ai`; sign-in requires an admitted account. First use downloads the retrieval models.

::: warning Preview
The DMG is ad-hoc signed and not notarized by Apple. Managed Macs may restrict first-open exceptions. It uses the regular App's accounts, Memory, and settings; it is not a stable release.
:::

To update, quit the App, download a newer DMG, and replace the existing application. Your data is retained. If your source installation is at `~/Applications/Clumsies.app`, replace it there to avoid keeping two copies. Preview updates use a new DMG download.

## Install from source

The following tools are only needed to compile the App. DMG users can skip this section.

### Prepare your Mac

The App targets **macOS 14 or later**. Building it requires a newer macOS version compatible with the Xcode you select; check [Apple's Xcode system requirements](https://developer.apple.com/xcode/system-requirements/).

| Tool | Requirement |
| --- | --- |
| Xcode | Full Xcode 26 or newer, compatible with your Mac; Command Line Tools alone are insufficient |
| Rust | A stable toolchain with `cargo` and `rustc` available in your shell |
| Just | The `just` command, which runs repository tasks |
| XcodeGen | Version 2.46.0 or newer, available as `xcodegen` |

Open Xcode once, accept its license, and install the requested components. Select it as the active toolchain, adjusting the path if necessary:

```sh
sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
sudo xcodebuild -runFirstLaunch
xcodebuild -version
```

With [Homebrew](https://brew.sh/) installed, you can install the remaining tools:

```sh
brew install just xcodegen rust
```

If you already have a working stable Rust toolchain, omit `rust`. Keep existing tools that meet the requirements.

### Build and install

For a new checkout:

```sh
git clone --branch main https://github.com/lilhammerfun/clumsies.git
cd clumsies
just install-macos
```

The first build downloads Swift and Rust dependencies and can take several minutes. The command builds the App and its bundled daemon, verifies their signing, installs the App, and opens it. When replacing an existing installation, it quits the running App and replaces the application bundle; accounts, Memory, and settings are retained.

The App includes the default Server address **`https://app.clumsies.ai`**. You still need an account admitted by that organization. If your team uses another deployment, obtain its Server address and access details from an administrator.

This installation connects to an existing Server. It does not require a local Docker environment. Contributors who need an isolated App and local services can follow [Development workflow](/guides/development-workflow).

### Update a source installation

In your `main` checkout, preserve any local changes before updating:

```sh
git pull --ff-only
just install-macos
```

If Git reports local changes or a divergent branch, resolve that before reinstalling; do not discard changes to force an update.

When the App finishes reconciling its Codex plugin, restart Codex and start a new task. Detailed agent setup follows in the quickstart.

## If installation stops

| Symptom | Next action |
| --- | --- |
| First launch reports an unidentified developer | Verify the download source, then use System Settings → Privacy & Security → Open Anyway; managed devices may restrict this. |
| `xcodebuild` cannot find full Xcode or requests first-launch components | Recheck the active Xcode path and complete its setup. |
| `just`, `xcodegen`, `cargo`, or `rustc` is unavailable | Install the missing tool or correct the current shell's `PATH`, then rerun the command. |
| Dependency download or build fails | Keep the failing command and error output; use [Troubleshooting](/guides/troubleshooting) or report the failure. |
| The App opens but sign-in is denied | Installation succeeded. Ask your organization's administrator to check account access. |

Once the installed **Clumsies.app** opens, continue to [Connect to your organization](/quickstart/connect).
