# Adapter

Adapter is the daemon-owned integration layer that makes the Clumsies Agent
runtime usable inside Codex, Claude Code, opencode, dsh, and Antigravity. Every harness is configured once for the local macOS user.
Codex uses a Clumsies plugin; the other hosts use their user-level configuration files. Both delivery forms provide the
MCP registration and Memory guidance without creating a second
memory or runtime implementation.

## Runtime boundary

The macOS App bundle contains one signed Rust executable:

```text
Clumsies.app/Contents/Resources/clumsiesd
```

launchd runs that executable as the resident daemon. Adapter pins the same
absolute App-bundled path into every managed MCP entry and starts it in
one of two short-lived proxy modes:

```text
clumsiesd mcp serve
clumsiesd mcp serve --host codex --delivery host-plugin
```

The installer requires an executable whose canonical path ends in
`Contents/Resources/clumsiesd` and verifies its macOS code signature. It records
the path and SHA-256 in the adapter manifest. There is no checkout build,
`PATH`, environment-variable, or copied-helper fallback.

Each proxy verifies that its Agent runtime protocol revision and build identity
match the resident daemon before forwarding traffic over XPC. Replacing the App
therefore updates every newly started proxy, while a resident from an older
release is detected and must be restarted.

## Managed host surfaces

On first launch, the App offers a harness selection screen with Codex selected
by default. **Settings → Agents** manages the same choices later. These choices
belong to this Mac's user, independently of the signed-in account, Server, or
Project. The App reconciles saved choices on subsequent launches, including
while signed out. A disabled integration stays disabled across App updates.

| Host | MCP registration |
| --- | --- |
| Codex | Global `clumsies@clumsies-local` plugin with bootstrap Skill |
| Claude Code | `~/.claude.json` → `mcpServers.clumsies` |
| opencode | `~/.config/opencode/opencode.json` → `mcp.clumsies` |
| dsh | MCP entry in the user-managed profile |
| Antigravity | `~/.gemini/config/mcp_config.json` → `mcpServers.clumsies` |

These use the harnesses' documented user-level locations:
[Claude Code](https://code.claude.com/docs/en/settings),
[opencode](https://opencode.ai/docs/config/), and
[Antigravity](https://antigravity.google/docs/mcp).
No new repository configuration is written. A repository binding only identifies
which Project's Memory to use. Removing a binding does not uninstall any global
adapter. See [dsh integration](/guides/dsh-integration) for its profile setup.

Every host consumes the MCP tools directly. The Codex plugin carries one thin
`project-memory` Skill that directs agents to consult project Memory before
analysis, planning, or implementation, even when the user does not name Clumsies.
Clumsies complements host-native memory: agents follow applicable host memory
policies and also query Clumsies, even after consulting host memory. Clumsies
memory maintenance follows the bound Project's Memory Guidelines.
Project-maintained skills such as `coding` are ordinary
resources in Memory Space: the bootstrap loads them through `memory.load` when
relevant and never copies or installs them into a host skill directory.

The unrelated host-native `activate` / `ntmd` skills installed by older Codex
and Claude Code releases are retired. Historical Codex Adapter rows retain
enough ownership metadata to remove exact legacy `.codex/config.toml`,
`.codex/hooks.json`, and managed Hook fragments when that repository is
removed. Direct-file update paths likewise delete previously managed retired
skill files without touching user-owned content.

The Codex plugin executes the pinned binary as
`mcp serve --host codex --delivery host-plugin`. The marker identifies the global plugin delivery; it
does not select or authorize a Project. At startup and again before every
`tools/call`, daemon resolves the repository's canonical Project binding and
requires it to remain the same Project. A missing or changed binding therefore
fails closed without consulting a Codex project Adapter row. All MCP proxies require a canonical repository binding at startup and before
each tool call. Unbound directories never use the App’s selected Project.
See [Workspace binding](/guides/workspace-binding) for runtime routing.

## Safe install, update, and remove

Direct-file adapters merge the MCP entry into shared host configuration. Their
manifests retain ownership and installed hashes so updates can safely retire old
scripts, plugins, and registrations that are no longer part of the integration.

Codex uses a distinct `host_plugin` delivery. Once the user has saved their harness choices, the App
inspects the Codex host, App-owned local marketplace, installed/enabled state,
and expected plugin version. For selected harnesses, missing or stale managed state is reconciled
through the signed Codex CLI. Inspection is read-only; automatic reconciliation
and **Repair Selected Integrations** in **Settings → Agents** materialize the
marketplace and install or update the plugin. Neither operation writes a
`project_agent_adapters` row or repository file. Disabling Codex removes the
Clumsies plugin through the signed CLI; its saved preference prevents reinstallation.

Plugin updates require restarting Codex and opening a new task. Reconciliation removes retired owned lifecycle scripts and registrations, preserves foreign configuration, and reports modified-file conflicts.

The App refuses to bootstrap `clumsiesd` or persist an Agent runtime path while
macOS is running it from an App Translocation mount. Move the released App to
`/Applications` or `~/Applications` and reopen it first; this prevents a
temporary quarantine UUID from entering LaunchAgent and host configuration.

- Install refuses to replace an unrelated MCP entry or unmanaged file.
- Update uses the adapter record revision as an optimistic concurrency guard.
- A prior managed runtime path can be migrated to the current App-bundled path.
- Installations created directly by the archived Zig CLI are discovered
  read-only and left unchanged. Missing workspaces remain pending; reachable
  installs report an actionable review-and-reinstall warning. Inspection is
  best-effort, has a short App-side deadline, and never blocks reconciliation
  of daemon-owned integrations, including while signed out or offline. Their external
  manifests are not accepted as native ownership proof.
- Archived `repo`-scope generations are reported as unsupported. Remove their
  old Clumsies MCP/Hook entries; the App-owned global Codex Plugin replaces
  repository-local Codex integration. Other hosts also use user-level configuration.
- Reinstalling from the App is the explicit handoff: the native installer
  refuses foreign or drifted entries instead of silently adopting them.
- Remove deletes only exact managed entries and files; drift becomes a conflict
  instead of an overwrite.
- Filesystem and record changes are journaled so an interrupted install or
  migration is recovered deterministically on the next reconciliation pass.

User-level choices and manifests live in `host_agent_adapters`, keyed only by
harness. Direct-file changes use `host_adapter_fs_ops` with the existing checked
filesystem journal. Enabling or disabling a harness retires its daemon-owned
repository configurations across known Servers. Changed files report a conflict;
unreachable repositories remain recorded and are retried on subsequent launches.
Archived Zig manifests remain inspection-only and never establish ownership.

This keeps unrelated host configuration intact and prevents an old worktree or
helper copy from silently taking over the Agent runtime.

## Implementation map

| Concern | Active path |
| --- | --- |
| User-level choices, installation, and migration | `crates/clumsiesd/src/agent_adapter/global.rs` |
| Native installer, merge rules, and legacy discovery | `crates/clumsiesd/src/agent_adapter.rs` |
| Codex plugin materialization and CLI reconciliation | `crates/clumsiesd/src/agent_adapter/codex_plugin.rs` |
| Codex plugin source bundle | `packages/clumsies/` |
| MCP proxy mode | `crates/clumsiesd/src/main.rs` |
| Typed MCP contract | `crates/clumsiesd/src/agent_runtime/mcp_contract.rs` |

The retired Zig adapter implementation remains recoverable from Git commit
`4b18f7947a977dbc6b62f560b698dc992597f19d`; it is not present or executed as
an installation or compatibility path. The native daemon contains only a
bounded, read-only manifest discovery pass; it never runs retired code or
treats archived manifests as an ownership database.
