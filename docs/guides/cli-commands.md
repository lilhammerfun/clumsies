# Archived Zig CLI

The standalone `clumsies` CLI is not an active product or runtime surface. Its
last active source remains recoverable from Git commit
`4b18f7947a977dbc6b62f560b698dc992597f19d` and is not built, tested, packaged,
installed, or released.

Current human workflows use the macOS Desktop. Supported Agent hosts are wired
from **Settings → Agent** to the App-bundled Rust runtime:

```text
clumsiesd mcp serve
```

These are adapter-managed proxy modes, not a replacement general-purpose CLI.
See [Agent runtime](/guides/agent-runtime) and [Adapter](/adapter).
