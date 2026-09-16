# Workspace binding and Memory routing

The MCP proxy resolves Memory from its working directory. The host must launch
`clumsiesd mcp serve` in the task's workspace; a global plugin does not select a Project.

```text
MCP process cwd → daemon project_bindings → Project → Effective Memory
```

The daemon canonicalizes the directory and selects the most specific bound ancestor
on the configured Server, with main-checkout resolution for Git worktrees. An unbound
directory fails with `project_binding_not_found`; the App's selected Project is not a fallback.

The proxy stores the resolved Project and fills it into `activate`, `load`, and `store`
requests. Agents do not supply a Project ID. Each tool call revalidates the original
startup directory. Rebinding it to another Project returns `project_binding_changed`;
start a new task to obtain a new MCP connection. A shell `cd` does not retarget that connection.

The Codex bootstrap Skill and MCP instructions explain when to call Memory. No lifecycle
Hook or AgentRun is required. Activity reads host session logs and Retrieval Runs instead.

## Upgrade from lifecycle integrations

Adapter reconciliation removes owned lifecycle scripts and registrations while preserving
foreign hooks and reporting conflicts for modified managed files. Codex plugin updates remove
the old hook registry and script; restart Codex and open a new task to load the new snapshot.

The retired AgentRun IPC endpoint and command are removed. Schema 42 preserves older records
in `retired_agent_runs` and `retired_agent_run_events`; no runtime reads or writes those tables.
New installations do not create them. Retrieval history is unchanged.
