# Clumsies

<p align="center">
  <img src="docs/public/logo.png" width="72" height="72" alt="Clumsies Logo" />
</p>

<p align="center">
  <b>The Collaborative Memory Platform for Agent Coding</b><br>
  <i>Share, review, and evolve organizational memory assets across engineering teams and coding agents.</i>
</p>

<p align="center">
  <a href="README.md">English</a> · <a href="README.zh-CN.md">简体中文</a>
</p>

<p align="center">
  <a href="https://github.com/lilhammerfun/clumsies/actions/workflows/ci.yml"><img src="https://github.com/lilhammerfun/clumsies/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/lilhammerfun/clumsies/blob/main/LICENSE"><img src="https://img.shields.io/github/license/lilhammerfun/clumsies?label=License" alt="License: MIT"></a>
  <a href="https://github.com/lilhammerfun/clumsies/releases"><img src="https://img.shields.io/github/v/release/lilhammerfun/clumsies?label=Release" alt="Release"></a>
</p>

---

## The Paradigm Shift

AI coding agents are changing the control plane of software development.

Engineering organizations used to manage only code in Git repositories. In the agentic era, teams must also manage the **architectural rules, domain constraints, and project context** that steer how AI agents write and refactor code.

Today, agent memory is trapped inside isolated model sessions or local markdown files. It cannot be peer-reviewed, shared across teammates, or synchronized across agent runs. When context limits hit, critical project guidelines are silently dropped.

**Clumsies is the collaborative memory platform for agent coding.** It treats agent memory as a first-class, versioned organizational asset — enabling human engineers and autonomous agents to build, review, and activate shared knowledge seamlessly.

---

## Key Features

- **Memory as a Team Asset (Git-Semantic Context)**: Rules, workflows, and project context live as first-class Markdown-backed Organization Memory. A Project selects the Organization resources it uses and carries private Draft overlays; reviewed changes merge atomically into immutable Organization Commit history.
- **Hybrid Retrieval & Precise Activation**: Combines SQLite FTS5 BM25 text search, local dense vector embeddings, Reciprocal Rank Fusion (RRF), and Cross-Encoder reranking. Agents retrieve task-relevant fragments on demand without exhausting token budgets.
- **MCP & Non-Blocking Lifecycle Integration**: Out-of-the-box integration for Google Antigravity, Claude Code, OpenAI Codex, opencode, and DeepSeek Harness (dsh) via a single signed Rust daemon (`clumsiesd`). Adapters are installed per user for all projects. Codex is selected by default and delivered as a user-level plugin; project-maintained skills remain in Memory Space and are loaded on demand. Restart Codex and start a new task after plugin changes, then complete the one-time `/hooks` review before its plugin Hook runs.
- **Self-Hosted Authority**: Run the Rust Server and PostgreSQL in your own infrastructure with organization OIDC, while the local resident daemon owns fast local state and XPC transport.

---

## Supported Agent Ecosystem

| Agent Host | Protocol Surface | Managed Files | Supported Lifecycle |
| :--- | :--- | :--- | :--- |
| **Google Antigravity** | MCP + Lifecycle Hook | `~/.gemini/config/mcp_config.json`, `~/.gemini/config/hooks.json` | `PreInvocation`; no root `Stop` |
| **Claude Code** | MCP + Lifecycle Hook | `~/.claude.json`, `~/.claude/settings.json` | Prompt, subagent, failure, and session events; no root `Stop` |
| **OpenAI Codex** | Plugin: MCP + Hook + bootstrap Skill | App-managed user plugin; no project files | Prompt, subagent, and session events after Hook trust; no root `Stop` |
| **opencode** | MCP + Plugin | `~/.config/opencode/opencode.json`, `~/.config/opencode/plugins/clumsies.ts` | Prompt, failure, and session events; no normal root `Stop` |
| **DeepSeek Harness (dsh)** | MCP + Hook Bridge | `~/.dsh/clumsies.json` | Prompt, failure, and session events; no normal root `Stop` |

---

## Quick Start

Install from `main` for everyday use on your Mac. These steps build the Debug
configuration, install **`~/Applications/Clumsies.app`**, and open it. Debug is
the build configuration; this is the regular app installation with persistent
accounts, Memory, and Agent integrations.

The app includes the default Server address, `https://app.clumsies.ai`.
Sign-in automatically loads the organization configured on that Server.

### Ask an agent to install it

Copy this prompt to your coding agent:

```text
Install Clumsies for everyday use on this Mac from:
https://github.com/lilhammerfun/clumsies

Follow the README's source installation steps on the main branch. Check
and install missing prerequisites: full Xcode 26 or newer compatible with
this Mac, stable Rust, Just, and XcodeGen. Reuse working installations and
preserve existing repository changes, Clumsies accounts, Memory, and configuration.
For a first installation, use the app's built-in Server and sign-in defaults.

Run just install-macos from the repository root to build and install
the complete Debug app, including its daemon, at ~/Applications/Clumsies.app.
Open that installed app. Do not create a Dev Instance or start a local Server.
If a step fails, report the failing command and cause instead of switching
installation methods. Tell me when login or macOS authorization needs my input.

Confirm that the installed app opens, then guide me through organization
sign-in, binding my working repository to a Project, and connecting my agent.
```

### Install from source

1. Install **full Xcode 26 or newer**, choosing a version compatible with your
   macOS from [Apple's system requirements](https://developer.apple.com/xcode/system-requirements/).
   Open Xcode once, accept its license, and install the requested components.
   Select it as the active toolchain (adjust the path if installed elsewhere):

   ```sh
   sudo xcode-select --switch /Applications/Xcode.app/Contents/Developer
   sudo xcodebuild -runFirstLaunch
   xcodebuild -version
   ```

2. Install [Homebrew](https://brew.sh/) if needed, then install
   [Just](https://formulae.brew.sh/formula/just),
   [XcodeGen](https://formulae.brew.sh/formula/xcodegen), and
   [Rust](https://formulae.brew.sh/formula/rust):

   ```sh
   brew install just xcodegen rust
   ```

   If you already have a working stable Rust toolchain, omit `rust` from the
   command. Both `cargo` and `rustc` must be available in your current shell.

3. Build and install the app:

   ```sh
   git clone --branch main https://github.com/lilhammerfun/clumsies.git
   cd clumsies
   just install-macos
   ```

   The first build downloads dependencies and can take several minutes. The
   command builds the app and its bundled daemon, verifies signing, installs
   `~/Applications/Clumsies.app`, and opens it. Replacing an existing app keeps
   your accounts, Memory, and configuration.

### Sign in and connect your agent

1. Open the app, keep the prefilled **Server address**, and click
   **Continue in Browser** to sign in. The app loads your organization automatically.
2. Choose which agents to connect on this Mac; Codex is selected by default.
   These adapters are installed once for all projects. Change them later in
   **Settings → Agents**. dsh also requires [profile setup](https://docs.clumsies.ai/guides/dsh-integration).
3. Select a Project and add your working repository through
   **Repositories → Add Repositories…**. For Codex, restart it, start a new task
   in that repository, and review the Clumsies Hook in `/hooks`.
4. Allow the initial model download and Memory indexing to finish, then ask:

   ```text
   Use Clumsies to read this repository's Project Memory and summarize the
   guidelines that apply to my current task.
   ```

See the [usage guide](https://docs.clumsies.ai/guides/how-to-use-clumsies) for
the full workflow. If sign-in or Project access is denied, contact your
organization administrator. Change the Server address only when connecting
to another deployment.

To update later, run these commands in your `main` checkout:

```sh
git pull --ff-only
just install-macos
```

---

## Documentation

Full documentation is available at [docs.clumsies.ai](https://docs.clumsies.ai).

---

## License

[MIT License](LICENSE) © 2026 Clumsies Lab
