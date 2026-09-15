# DeepSeek Harness (dsh) Integration

The Clumsies daemon treats the DeepSeek Harness web app as a first-class
Agent host ("dsh"). This guide covers both halves of the integration:

1. **MCP access** — the dsh web profile connects to the Clumsies MCP server,
   which gives the model `mcp__clumsies__memory`; `activate` / `load` /
   `store` are operations of that tool.
2. **AgentRun lifecycle** — a dsh-side client plugin forwards non-blocking
   session/turn events to the daemon's hook proxy, which issues `dsh`
   AgentRuns and records failure or session boundaries.

## MCP access (read + mutate content)

Register the Clumsies MCP server in the dsh profile patch layer
(`~/.dsh/profiles/<profile>/cordis.patch.yml`):

```yaml
- insert:
    - id: mcp-clumsies
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: clumsies
        transport: stdio
        command: /Users/weiwang/Applications/Clumsies.app/Contents/Resources/clumsiesd
        args: [mcp, serve]
        cwd: /path/to/workspace
```

`cwd` pins the proxy to the Project whose Effective Memory the session should
see. The tool appears as `mcp__clumsies__memory` and does not require an
AgentRun.

## Agent adapter on this Mac

Select **DeepSeek Harness (dsh)** during first launch or in **Settings → Agents**.
The App installs `~/.dsh/clumsies.json` with the signed runtime path. There is
no Project ID, Server URL, or repository-local marker in this configuration.

Copy `dev/dsh/clumsies-hook.mjs` to `~/.dsh/clumsies-hook.mjs` (replace an older
copy when migrating from repository adapters). Register that bridge once in the dsh profile patch layer, using your
home directory's absolute path:

```yaml
- insert:
    - id: clumsies-hook
      name: /Users/your-name/.dsh/clumsies-hook.mjs
```

Keep the MCP registration from the previous section in the same profile. Its
`cwd` must identify a bound repository. Profile registration is a separate dsh
step; the App does not overwrite your profile configuration.

The bridge reads the user-level runtime path and preserves each session's cwd.
The daemon resolves that directory to its Project. Disabling dsh removes the
managed runtime config; the separately registered bridge becomes inactive. It
never removes a Project binding or changes the dsh profile.

Old daemon-owned repository markers are removed when the integration is
configured. Unreachable directories stay recorded for later cleanup. Changed
files produce a conflict and are left intact.

## AgentRun lifecycle hook

The daemon accepts hook events from the `dsh` host:

```sh
printf '%s' "$PAYLOAD" | clumsiesd _agent agent-run-event --host dsh
```

`$PAYLOAD` is a JSON object with the shared hook vocabulary:

| Field | Required | Meaning |
|---|---|---|
| `hook_event_name` | yes | New integrations send `UserPromptSubmit`, `StopFailure`, `SubagentStart`, `SubagentStop`, or `SessionEnd`; `Stop` is accepted only for legacy/manual compatibility |
| `session_id` | yes | dsh session id (e.g. `session-…`); deduplicates events |
| `turn_id` | root events | one id per user prompt, reused by a matching `StopFailure` or legacy/manual `Stop` |
| `agent_id` | subagent events | subagent id (`subagent:…`) |
| `agent_type` | subagent events | display label for the subagent run |
| `cwd` | no | workspace path; resolves the project binding when present |
| `error` | `StopFailure` | ignored (never stored) |

Example turn lifecycle:

```json
{"hook_event_name":"UserPromptSubmit","session_id":"session-abc","turn_id":"turn-001","cwd":"/work/repo"}
{"hook_event_name":"SubagentStart","session_id":"session-abc","turn_id":"turn-001","agent_id":"sub-1","agent_type":"reviewer"}
{"hook_event_name":"SubagentStop","session_id":"session-abc","turn_id":"turn-001","agent_id":"sub-1","agent_type":"reviewer"}
{"hook_event_name":"SessionEnd","session_id":"session-abc"}
```

The hook proxy is fail-open: a missing daemon or a malformed payload never
blocks the dsh session.

### Wiring the dsh side

A small client plugin in the dsh web profile subscribes to session lifecycle
events and forwards them. Skeleton (cordis plugin in the dsh profile):

```ts
// forward session/turn lifecycle events to the Clumsies daemon
import { Context } from 'cordis'
export const name = 'clumsies-hook'
export function apply(ctx: Context) {
  const forward = (payload: object) => {
    try { execFileSync('clumsiesd', ['_agent', 'agent-run-event', '--host', 'dsh'], { input: JSON.stringify(payload), stdio: 'pipe' }) } catch { /* fail-open */ }
  }
  // ctx.on('session/…', …) — turn start/failure, session end, subagent start/stop
}
```

The shipped plugin (`dev/dsh/clumsies-hook.mjs`) reads the
user-level `~/.dsh/clumsies.json` described above; a shell wrapper
(`dev/dsh/agent-run-event.sh`) drives the same contract from any event
source. A normal successful turn does not emit root `Stop`. The invariant to
preserve is **one `turn_id` per user prompt, reused for a failure event**, and
**no event ever blocks the session**. If an older or manually maintained
integration still sends `Stop`, the bridge records telemetry only and returns
no stop-blocking decision.

## Daemon-side changes (landed)

- `AgentRunHost::Dsh` + `HookHost::Dsh` (`--host dsh`) accepted by
  `clumsiesd _agent agent-run-event`.
- `agent_runs.host` CHECK constraint extended with `'dsh'` via schema
  migration 35 → 36 (table rebuild, idempotent).
- `StopFailure` supported for dsh (root run ends with outcome `failed`).
