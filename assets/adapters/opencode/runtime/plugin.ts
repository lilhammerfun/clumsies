import type { Plugin, PluginInput } from "@opencode-ai/plugin"

/**
 * Clumsies opencode integration plugin.
 *
 * opencode has no config-file-level hook surface; its lifecycle observation
 * surface is the plugin event bus. This plugin subscribes to session and
 * message events, normalizes the non-intrusive lifecycle signals that the
 * codex / claude-code shell hooks produce, and forwards each event to
 * `clumsiesd _agent agent-run-event --host opencode` over stdio.
 *
 * Only events opencode actually emits are forwarded; nothing is synthesized.
 * Every forwarding failure is swallowed so Clumsies never affects the agent.
 */

const HOST = "opencode"

type MessageRole = "user" | "assistant"

type SessionState = {
  roles: Map<string, MessageRole>
  parents: Map<string, string>
  systemContext: string | null
}

const sessions = new Map<string, SessionState>()

function sessionState(sessionID: string): SessionState {
  let state = sessions.get(sessionID)
  if (!state) {
    state = { roles: new Map(), parents: new Map(), systemContext: null }
    sessions.set(sessionID, state)
  }
  return state
}

function clumsiesRuntime(): string {
  return "__CLUMSIES_RUNTIME_BINARY__"
}

/**
 * Forwards a lifecycle payload to the daemon bridge and returns its stdout.
 * The bridge prints a `hookSpecificOutput.additionalContext` JSON blob after
 * a successful start event; we keep that text for system-prompt injection.
 * Fail-open: any error yields null.
 */
async function forward(
  ctx: PluginInput["$"],
  cwd: string,
  payload: Record<string, unknown>,
): Promise<string | null> {
  const input = JSON.stringify({ ...payload, cwd })
  try {
    const result =
      await ctx`printf '%s' ${input} | ${clumsiesRuntime()} _agent agent-run-event --host ${HOST}`.quiet()
    return result.stdout.toString()
  } catch {
    return null
  }
}

function extractAdditionalContext(stdout: string): string | null {
  try {
    const line = stdout.trim().split("\n").pop()
    if (!line) return null
    const parsed = JSON.parse(line)
    const context = parsed?.hookSpecificOutput?.additionalContext
    return typeof context === "string" && context.length > 0 ? context : null
  } catch {
    return null
  }
}

/**
 * Walks assistant.parentID up to the user message that started this turn.
 * Returns null when the chain bottoms out or loops; the caller then skips
 * the event rather than fabricating a run identity.
 */
function rootUserMessageID(state: SessionState, startID: string): string | null {
  let current = startID
  let hops = 0
  const seen = new Set<string>()
  while (current && hops++ < 16 && !seen.has(current)) {
    seen.add(current)
    const role = state.roles.get(current)
    if (role === "user") return current
    const parent = state.parents.get(current)
    if (!parent) return null
    current = parent
  }
  return null
}

export const ClumsiesOpencode: Plugin = async (input) => {
  const cwd = input.directory
  return {
    /**
     * Fires synchronously when a new user message is received, before the
     * LLM call. This is our UserPromptSubmit equivalent: it both records the
     * root run start and captures the run context for injection.
     */
    "chat.message": async (chatInput, output) => {
      const messageID = chatInput.messageID ?? output.message?.id
      if (!messageID) return
      const state = sessionState(chatInput.sessionID)
      state.roles.set(messageID, "user")
      const stdout = await forward(input.$, cwd, {
        hook_event_name: "UserPromptSubmit",
        session_id: chatInput.sessionID,
        message_id: messageID,
      })
      state.systemContext = extractAdditionalContext(stdout ?? "")
    },
    event: async ({ event: ev }) => {
      const properties = ev.properties as Record<string, unknown> | undefined
      if (!properties || typeof properties !== "object") return
      const sessionID = properties.sessionID as string | undefined
      if (!sessionID) return
      const state = sessionState(sessionID)

      if (ev.type === "message.updated") {
        const info = properties.info as
          | { role?: string; id?: string; parentID?: string; error?: unknown }
          | undefined
        if (!info || typeof info !== "object" || !info.id) return
        if (info.role === "assistant") {
          state.roles.set(info.id, "assistant")
          if (info.parentID) state.parents.set(info.id, info.parentID)
          if (!info.error) return
          const time = (info as unknown as { time?: { completed?: number } }).time
          const isFinished = typeof time?.completed === "number"
          if (!isFinished) return
          const parent = info.parentID ?? null
          const rootMessageID = parent ? rootUserMessageID(state, parent) : null
          if (!rootMessageID) return
          await forward(input.$, cwd, {
            hook_event_name: "StopFailure",
            session_id: sessionID,
            message_id: rootMessageID,
          })
        }
        return
      }

      if (ev.type === "session.deleted") {
        await forward(input.$, cwd, {
          hook_event_name: "SessionEnd",
          session_id: sessionID,
        })
        sessions.delete(sessionID)
      }
    },
    /**
     * Injects the current run context (run_id, revision, semantic
     * instructions) into the system prompt before each LLM call. The context
     * is captured from the bridge output of the latest chat.message.
     */
    "experimental.chat.system.transform": async ({ sessionID }, output) => {
      const state = sessionID ? sessions.get(sessionID) : undefined
      if (!state?.systemContext) return
      output.system.push(`[clumsies] ${state.systemContext}`)
    },
  }
}

export default ClumsiesOpencode
