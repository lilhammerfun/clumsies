# Clumsies

<p align="center">
  <img src="docs/public/logo.png" width="72" height="72" alt="Clumsies Logo" />
</p>

[English](README.md) · [简体中文](README.zh-CN.md) · [Documentation](https://docs.clumsies.ai)

[![CI](https://github.com/lilhammerfun/clumsies/actions/workflows/ci.yml/badge.svg)](https://github.com/lilhammerfun/clumsies/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/github/license/lilhammerfun/clumsies?label=License)](LICENSE)

> **WIP — the native macOS App is under active development.** Public GitHub Releases contain the older CLI, not an installer for the current App. Follow the [installation guide](https://docs.clumsies.ai/quickstart/install) to build and install the App from source.

Clumsies helps teams maintain shared Markdown guidance for coding agents: architecture decisions, project constraints, and procedures. The organization keeps published **Memory**; each **Project** selects the Memory it uses and connects it to local repositories. Agents retrieve relevant guidance while they work and can propose changes when a user asks.

## How you use it

The [quickstart](https://docs.clumsies.ai/quickstart/) follows one workflow:

1. Create a Project and bind your working repository.
2. Select the Memory that Project needs from the organization.
3. Work in Codex with the Clumsies integration enabled; it retrieves relevant Memory as part of the task.
4. Explicitly ask Codex to update Memory when a rule or procedure needs to change.
5. Inspect the resulting Draft in Clumsies and submit a Review. An organization owner or administrator approves and publishes it.

Retrieval does not automatically rewrite Memory. A saved Draft is a proposal, and publication requires human review. Drafts can affect that Project's local Memory view before publication.

## Install and get started

The App runs on **macOS 14 or later**. Source installation requires a compatible full Xcode 26 or newer, stable Rust, Just, and XcodeGen; building has stricter macOS requirements than running the App. Check the [installation guide](https://docs.clumsies.ai/quickstart/install) for prerequisites, updates, and troubleshooting.

With those prerequisites ready:

```sh
git clone --branch main https://github.com/lilhammerfun/clumsies.git
cd clumsies
just install-macos
```

This builds the Debug configuration, installs the complete App and bundled daemon at **`~/Applications/Clumsies.app`**, and opens it. It uses the regular application's persistent accounts, Memory, and settings. The default Server address is **`https://app.clumsies.ai`**; signing in requires an account admitted by that organization. Use your team's Server address when connecting to another deployment.

Continue with [Connect to your organization](https://docs.clumsies.ai/quickstart/connect), then the [quickstart](https://docs.clumsies.ai/quickstart/).

### Ask an agent to install it

```text
Install Clumsies for everyday use on this Mac from the main branch of:
https://github.com/lilhammerfun/clumsies

Follow docs/quickstart/install.md in that checkout. Check prerequisites and
reuse working tools. Preserve repository changes, Clumsies accounts, Memory,
and settings. Run just install-macos to install ~/Applications/Clumsies.app.
Use the built-in Server address for a first installation unless I provide
another one. Do not create a Dev Instance or start a local Server.

If a command fails, report its error instead of changing installation methods.
Tell me when sign-in or macOS authorization requires my input. Confirm that
the installed App opens, then guide me to the documentation quickstart.
```

## Agent support and limits

Clumsies includes integrations for the macOS Codex App, Claude Code, opencode, DeepSeek Harness (`dsh`), and Google Antigravity. Install the agent host separately. Adapters are configured once per Mac user for all projects; Codex is selected by default during first-time setup. Repository bindings determine which Project's Memory an agent can use.

After changing the Codex integration, restart Codex and start a new task. Retrieval needs the local daemon and a ready index; first use downloads the retrieval models. Host-specific requirements are documented in [Agent integration](https://docs.clumsies.ai/guides/agent-runtime).

Teams can deploy the Rust Server and PostgreSQL with their own OIDC identity provider. See [organization deployment](https://docs.clumsies.ai/guides/deploy-for-an-org).

## Understand the project

[Overview](https://docs.clumsies.ai/overview) · [Architecture](https://docs.clumsies.ai/architecture) · [Data model](https://docs.clumsies.ai/data-model) · [Domain interfaces](https://docs.clumsies.ai/reference/domain-api) · [Development workflow](https://docs.clumsies.ai/guides/development-workflow)

## License

[MIT](LICENSE) © 2026 Clumsies Lab
