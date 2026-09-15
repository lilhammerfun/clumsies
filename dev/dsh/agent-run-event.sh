#!/bin/sh
# Forward a dsh lifecycle event (JSON on stdin) to the local Clumsies daemon.
# Best effort only: lifecycle observation must never block the dsh session.
#
# Usage:  printf '%s' "$PAYLOAD" | agent-run-event.sh
# Payload: {"hook_event_name":"UserPromptSubmit|StopFailure|SubagentStart|SubagentStop|SessionEnd",
#           "session_id":"...","turn_id":"...","agent_id":"...","agent_type":"...","cwd":"/path"}
#
# Uses the App-managed user-level adapter config; the event keeps its session cwd.
set -eu
CLUMSIES_BIN="$(node -e '
try {
  const fs = require("node:fs"), path = require("node:path"), os = require("node:os");
  const {runtime} = JSON.parse(fs.readFileSync(path.join(os.homedir(), ".dsh/clumsies.json"), "utf8"));
  if (typeof runtime === "string" && path.isAbsolute(runtime) && runtime.endsWith("/Contents/Resources/clumsiesd")) process.stdout.write(runtime);
} catch {}
' 2>/dev/null || true)"
[ -n "$CLUMSIES_BIN" ] && [ -x "$CLUMSIES_BIN" ] || exit 0
"$CLUMSIES_BIN" _agent agent-run-event --host dsh 2>/dev/null || true
