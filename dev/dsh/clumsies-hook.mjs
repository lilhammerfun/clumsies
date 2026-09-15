// DSH → Clumsies hook bridge.
//
// Forwards web-session lifecycle events to the local Clumsies daemon so it
// can issue `dsh` AgentRuns and end them on failure or session disposal.
// Fail-open: a missing daemon or a failed forward never blocks the session.
//
// Install: copy to ~/.dsh/profiles/web/clumsies-hook.mjs and add to
// cordis.patch.yml:
//   - insert:
//       - id: clumsy-hook
//         name: /Users/weiwang/.dsh/profiles/web/clumsies-hook.mjs
//
// The App installs one user-level ~/.dsh/clumsies.json. Each session keeps
// its own cwd; the daemon resolves the Project binding from that directory.
// Disabling the adapter removes the config and stops lifecycle forwarding.
//
// Root sessions only: subagent/child sessions (origin 'subagent' or
// delegationDepth > 0) are skipped so one user prompt maps to one root run.
// turn_id embeds the session id so a fresh session never collides with an
// older session's run key.
//
// Notes:
// - The payload must reach the hook proxy via stdin, so we use spawn() and
//   write the JSON ourselves — async execFile() has no input option and the
//   child would block on stdin forever.
// - Diagnostics are opt-in via CLUMSIES_HOOK_LOG (default: off).

import { spawn } from 'node:child_process'
import { appendFileSync, readFileSync } from 'node:fs'
import { isAbsolute, join, normalize } from 'node:path'
import { homedir } from 'node:os'

const FALLBACK_CWD = process.env.CLUMSIES_HOOK_CWD || process.cwd()
const LOG = process.env.CLUMSIES_HOOK_LOG || ''
export function resolveWorkspace(start, home = homedir()) {
  try {
    const config = JSON.parse(readFileSync(join(home, '.dsh/clumsies.json'), 'utf8'))
    if (typeof config.runtime !== 'string' || !isAbsolute(config.runtime)
        || !config.runtime.endsWith('/Contents/Resources/clumsiesd')) return null
    const workspace = normalize(isAbsolute(start) ? start : join(FALLBACK_CWD, start))
    return { workspace, runtime: config.runtime }
  } catch {
    return null
  }
}

function log(line) {
  if (!LOG) return
  try { appendFileSync(LOG, `[${new Date().toISOString()}] ${line}\n`) } catch { /* never block */ }
}

function forward(payload, runtime) {
  return new Promise((resolve) => {
    const bin = runtime
    const input = JSON.stringify(payload)
    log('forward ' + input.slice(0, 140))
    let child
    try {
      child = spawn(bin, ['_agent', 'agent-run-event', '--host', 'dsh'], {
        stdio: ['pipe', 'ignore', 'pipe'],
        windowsHide: true,
      })
    } catch (err) {
      log('forward spawn error: ' + (err.message || err).slice(0, 200))
      resolve()
      return
    }
    let stderr = ''
    let settled = false
    let watchdog
    const settle = () => {
      if (settled) return
      settled = true
      if (watchdog) clearTimeout(watchdog)
      resolve()
    }
    child.stderr?.on('data', (chunk) => { stderr += chunk })
    child.once('error', (err) => {
      log('forward error: ' + (err.message || err).slice(0, 200))
      settle()
    })
    child.once('close', (code, signal) => {
      log('forward close code=' + code + ' signal=' + signal + (stderr ? ' stderr=' + stderr.slice(0, 120) : ''))
      settle()
    })
    child.stdin.on('error', () => { /* EPIPE when the child exits early */ })
    child.stdin.end(input)
    // Fail-open watchdog: release the per-session queue even if a child does
    // not report its SIGKILL promptly.
    watchdog = setTimeout(() => {
      child.kill('SIGKILL')
      settle()
    }, 5000)
  })
}

export default {
  name: 'clumsies-hook',
  apply(ctx) {
    log('plugin loaded')

    // Preserve lifecycle order within one dsh session. Each forward remains
    // fail-open, while unrelated sessions can still make progress in parallel.
    const forwarding = new Map()
    const enqueue = (sessionId, payload, runtime) => {
      const previous = forwarding.get(sessionId) ?? Promise.resolve()
      const current = previous
        .catch((err) => log('forward queue error: ' + (err.message || err).slice(0, 200)))
        .then(() => forward(payload, runtime))
        .catch((err) => log('forward queue error: ' + (err.message || err).slice(0, 200)))
      forwarding.set(sessionId, current)
      void current.then(() => {
        if (forwarding.get(sessionId) === current) forwarding.delete(sessionId)
      })
    }

    const emit = (session, event) => {
      const header = session?.header ?? session
      log('session/event type=' + (event?.type ?? '?') + ' session=' + (header?.id ?? '?'))
      if (!header || header.origin === 'subagent' || (header.delegationDepth ?? 0) > 0) {
        return
      }
      const sessionId = header.id ?? 'unknown'
      const resolved = resolveWorkspace(header.cwd ?? FALLBACK_CWD)
      if (!resolved) return
      const { workspace: cwd, runtime } = resolved
      switch (event?.type) {
        case 'turn/start': {
          const turn = event.data?.turn ?? '?'
          const turnId = sessionId + ':t' + turn
          enqueue(sessionId, {
            hook_event_name: 'UserPromptSubmit',
            session_id: sessionId,
            turn_id: turnId,
            cwd,
          }, runtime)
          break
        }
        case 'turn/end': {
          const turn = event.data?.turn ?? '?'
          const turnId = sessionId + ':t' + turn
          const failed = event.data?.reason?.kind === 'error'
          if (failed) {
            enqueue(sessionId, {
              hook_event_name: 'StopFailure',
              session_id: sessionId,
              turn_id: turnId,
              cwd,
              error: 'turn-failed',
            }, runtime)
          }
          break
        }
      }
    }
    ctx.on('session/event', (session, event) => emit(session, event))
    ctx.on('session/disposed', (session) => {
      const header = session?.header ?? session
      log('session/disposed session=' + (header?.id ?? '?'))
      if (!header || header.origin === 'subagent' || (header.delegationDepth ?? 0) > 0) {
        return
      }
      const sessionId = header.id ?? 'unknown'
      const resolved = resolveWorkspace(header.cwd ?? FALLBACK_CWD)
      if (!resolved) return
      const { workspace: cwd, runtime } = resolved
      enqueue(sessionId, {
        hook_event_name: 'SessionEnd',
        session_id: sessionId,
        cwd,
      }, runtime)
    })
    log('plugin handlers attached')
  },
}
