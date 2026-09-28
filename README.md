# Clumsies

<p align="center">
  <img src="https://raw.githubusercontent.com/lilhammerfun/clumsies/main/docs/public/logo.png" width="72" height="72" alt="Clumsies Logo" />
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
- **Durable Project Experience**: Completed Agent runs become Server-authoritative Project Episodes with immutable source Evidence and versioned summaries. Episodes remain Project-scoped, survive the original host session, and can be inspected without being promoted to Organization authority.
- **Hybrid Joint Retrieval & Precise Activation**: Semantic Memory and current Episodic summaries share one Project index combining SQLite FTS5 BM25, local dense embeddings, Reciprocal Rank Fusion (RRF), and Cross-Encoder reranking. Agents retrieve task-relevant fragments on demand and explicitly page source Evidence only when a summary is insufficient.
- **Agent-Native Asynchronous Kanban**: Agents autonomously claim tasks (`begin_work`), build dependency DAGs, evaluate blocking predicates, and record structured verification steps. Human approval gates (`approve_closure`) ensure only verified work transitions to Done.
- **MCP & Non-Blocking Lifecycle Integration**: Out-of-the-box integration for Google Antigravity, Claude Code, OpenAI Codex, opencode, and DeepSeek Harness (dsh) via a single signed Rust daemon (`clumsiesd`). Codex is delivered as an automatically managed user-level plugin; project-maintained skills remain in Memory Space and are loaded on demand. Restart Codex and start a new task after plugin changes, then complete the one-time `/hooks` review before its plugin Hook runs. Managed adapters do not install normal root Stop hooks; Issue closure remains explicit in an opt-in skill or manually maintained workflow.
- **Self-Hosted Authority**: Run the Rust Server and PostgreSQL in your own infrastructure with organization OIDC, while the local resident daemon owns fast local state and XPC transport.

---

## Supported Agent Ecosystem

| Agent Host | Protocol Surface | Managed Files | Supported Lifecycle |
| :--- | :--- | :--- | :--- |
| **Google Antigravity** | MCP + Lifecycle Hook | `.mcp.json`, `.agents/hooks.json` | `PreInvocation`; no root `Stop` |
| **Claude Code** | MCP + Lifecycle Hook | `.mcp.json`, `.claude/settings.json` | Prompt, subagent, failure, and session events; no root `Stop` |
| **OpenAI Codex** | Plugin: MCP + Hook + bootstrap Skill | App-managed user plugin; no project files | Prompt, subagent, and session events after Hook trust; no root `Stop` |
| **opencode** | MCP + Plugin | `opencode.json`, `.opencode/plugins/clumsies.ts` | Prompt, failure, and session events; no normal root `Stop` |
| **DeepSeek Harness (dsh)** | MCP + Hook Bridge | `.dsh/clumsies.json` | Prompt, failure, and session events; no normal root `Stop` |

---

## Quick Start

### macOS App (Recommended)

1. Download the latest release from [Releases](https://github.com/lilhammerfun/clumsies/releases).
2. Move `Clumsies.app` to `/Applications` or `~/Applications`.
3. Open `Clumsies.app`. It automatically provisions the resident launchd daemon (`ai.clumsies.daemon`) and connects to your organization server.
4. Bind a repository to its Project. Codex uses the global Plugin automatically; configure optional repository-writing hosts in **Settings → Agent**.

### Development from Source

The complete Dev Instance requires Just, XcodeGen, Xcode, Rust, Bun, and Docker
Desktop.

```bash
# Clone repository
git clone https://github.com/lilhammerfun/clumsies.git
cd clumsies

# Launch a complete worktree-scoped Dev Instance
just dev-macos
```

For local server development:

```bash
# Start PostgreSQL, fake OIDC provider, and the Rust Server
bun run dev:server
```

---

## Documentation

Full documentation is available at [docs.clumsies.ai](https://docs.clumsies.ai).

---

## License

[MIT License](LICENSE) © 2026 Clumsies Lab
