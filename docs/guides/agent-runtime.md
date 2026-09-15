# Agent runtime

This page describes the host runtime path, not the human member workflow.

## Entrypoint

Direct-file adapters register the App-bundled runtime as:

```bash
/path/to/Clumsies.app/Contents/Resources/clumsiesd mcp serve
```

Codex receives the same runtime through the App-managed global Clumsies plugin:

```bash
/path/to/Clumsies.app/Contents/Resources/clumsiesd \
  mcp serve --host codex --delivery host-plugin
```

These are adapter-managed commands, not a general-purpose CLI. The repository
must already have a daemon-owned Project binding, the resident macOS daemon must
be running, and the matching Project Commit generation must be ready. A
direct-file Hook invokes the private lifecycle bridge as:

```bash
/path/to/Clumsies.app/Contents/Resources/clumsiesd \
  _agent agent-run-event --host codex|claude-code|opencode
```

The Codex plugin Hook invokes the host-plugin form:

```bash
/path/to/Clumsies.app/Contents/Resources/clumsiesd \
  _agent agent-run-event --host codex --delivery host-plugin
```

Both modes are short-lived proxies. They are parsed before daemon
initialization and never open local databases or start models and workers.

## Runtime path

```text
agent host
  -> App-bundled Rust proxy (`clumsiesd mcp serve`)
  -> resident Rust clumsiesd over macOS XPC
  -> Effective Memory (installed Commit + current local Draft overlay)
  -> SQLite FTS/vector index and local ONNX models
```

The proxy first compares its runtime protocol revision and build identity with
the resident daemon, then asks daemon to resolve the current directory to a
canonical Project. Every later Agent-scoped XPC request carries that identity;
the resident rejects a missing or mismatched marker before decoding the
operation. This is a release-compatibility fence, not an authorization
boundary. Unmarked `health` remains available for Desktop startup discovery,
and the few Desktop operations that share domain handlers use reserved
`desktop_*` method names. MCP does not crawl the repository or read a second
cache. The resident daemon is the single owner of the current local read and
write model.

## Model preparation

The daemon starts model preparation in the background before the first MCP
request. It downloads a pinned int8
[`multilingual-e5-small`](https://huggingface.co/intfloat/multilingual-e5-small)
embedding model and a pinned int8
[`bge-reranker-base`](https://huggingface.co/Xenova/bge-reranker-base)
conversion. The complete payload is 431,831,479 bytes.

Every repository revision and artifact SHA-256 is fixed in the daemon. Files
are downloaded into the model cache with resume support, verified before ONNX
loading, and reused offline afterward. Search index status reports `preparing`
with downloaded and total bytes while this work is in progress. Activation
returns `search_model_preparing` immediately during that state; it never blocks
an MCP request on an unreported download and never falls back to a weaker
retrieval path.

## Tool loop

The current MCP surface contains exactly one tool:

- `memory`: dispatches three Memory operations:
  - `activate`: send a natural-language task cue and receive ranked fragments
  ready for the current reasoning context.
  - `load`: read complete resources by known stable ID or exact path.
  - `store`: persist an explicit user-requested Memory Draft.

Typical use is:

```text
memory.activate(query, optional state)
  -> use returned fragments
  -> optionally memory.load known complete resources
  -> memory.store only for explicit memory maintenance
```

For an update, `memory.load` is mandatory: use the returned complete-resource hash and
exact source text in one or more atomic `memory.store.update` replacements. The agent
does not send a reconstructed complete document.

There is no setup call, host-session binding, or `META_PROMPT.md` bootstrap.
Protocol guidance comes from the MCP initialization instructions and tool
descriptions. The `state` returned by `memory.activate` is only a bounded fragment
delta token; pass it again only while earlier fragments remain in model
context.

## Adapter boundary

Adapters make a concrete host launch the MCP server. Codex receives one thin
bootstrap Skill inside the plugin; it coordinates `memory.activate`, loads
relevant project-maintained skills from Memory Space with `memory.load`. The project skills themselves remain
ordinary Memory resources and are never installed into a host skill directory.
Other hosts consume the MCP tools directly. Host-specific Hooks observe
lifecycle outside the MCP memory contract and must not reimplement retrieval or
inject a second bootstrap protocol.

Codex plugin proxies identify both `host=codex` and `delivery=host-plugin`.
The Plugin is installed and enabled globally, but it can operate only inside a
repository with a canonical Project binding. The resident daemon resolves that
binding at startup and before every `tools/call`; removing or changing the
binding makes an already running plugin proxy fail its next call closed. Every harness has one user-level choice in Settings → Agents, independent of
Project bindings. Unbound directories are rejected by every MCP entry point;
no proxy falls back to the Project selected in the App. Plugin installation does not grant
Hook trust: the user must review the current
Clumsies Hook in `/hooks` before AgentRun observation becomes available.
Installation also does not hot-load the plugin into an
existing Codex task; restart Codex after install or update, then start a new
task with the new plugin snapshot.

The native daemon installer writes the exact code-signed App-bundled
`clumsiesd` path into host configuration and its managed resolver. It does not
copy a helper into `~/.clumsies`, consult a checkout's build output, or fall back
to `PATH`. Updating the App therefore updates the executable used by every new
MCP or Hook proxy; the release-identity check detects a resident that still
needs to be restarted.

Members configure and select Projects through product clients. Agents consume
the selected Project through MCP. Keeping those roles separate prevents host
integration details from becoming a second memory system.
