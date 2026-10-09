---
title: Clumsies CLI
description: Install the Windows/Linux CLI, connect an Agent, and review and publish Memory without a graphical client.
---
# Clumsies CLI

`clumsies` is the human client. `clumsiesd` owns credentials, local drafts, caches, directory bindings, synchronization, and the existing `clumsiesd mcp serve` Agent entry. Both executables come from one Rust package and must be upgraded together. Windows/Linux GUI development is paused and its implementation and packaging have been removed from the active tree; the native macOS App remains supported.

## Install

The initial CLI packages target **Linux x86_64 (Ubuntu 24.04 or a compatible glibc system)** and **Windows x64**. The CLI workflow produces tested archives and a Windows user installer; subsequent tagged releases include these assets. Until that first release, download the artifacts from the repository's **CLI** workflow. Verify the archive or installer against its adjacent SHA-256 file before running it.

On Linux, extract `clumsies-cli-VERSION-linux-x86_64.tar.gz`, enter its directory, and run:

```sh
sha256sum --check SHA256SUMS
./install.sh
export PATH="$HOME/.local/bin:$PATH"
clumsies daemon start
```

The default program location is `~/.local/lib/clumsies/runtime`; entry links go in `~/.local/bin`. `CLUMSIES_INSTALL_ROOT` and `CLUMSIES_BIN_DIR` select other user-owned locations. `install.sh --uninstall` removes the programs and managed links while retaining daemon data. The archive includes no desktop or system service dependency. Linux needs its system C/C++ runtime libraries and `flock` (util-linux); CI checks both binaries for missing shared libraries.

On Windows, run `clumsies-cli-VERSION-windows-x86_64-Setup.exe`. It installs for the current user under `%LOCALAPPDATA%\Programs\ClumsiesCLI`, adds its `runtime` directory to the user PATH, and needs no administrator account. Reopen terminals after installation. The package includes the required MSVC runtime DLLs. The installer is currently unsigned; only run an artifact you verified from this repository.

For a portable installation, extract the ZIP to a writable directory and invoke `clumsies.exe` there, or install that extracted package with:

```powershell
.\install.ps1 -AddToPath
clumsies daemon start
```

The portable ZIP has its files at the archive root. `install.ps1 -Uninstall` removes an installed runtime and its PATH entry while retaining daemon data. Windows Settings → Apps can uninstall the user installer.

On macOS, the App embeds `Contents/Resources/clumsies`. Run that executable, or add its resource directory to PATH. It reuses the App's launch agent, Keychain, configuration, and cache. Standalone `daemon stop/restart` is unavailable on macOS; manage that runtime through the App. A source CLI build also discovers `/Applications/Clumsies.app` or `~/Applications/Clumsies.app`.

## List pages and select projects

`project list`, `review list PROJECT`, `review comments REVIEW_ID`, and `draft list` share `--limit` (1–200, default 100), `--cursor`, and `--all`. By default each command returns one page. Pass the returned opaque cursor unchanged to continue; `--all` starts at the beginning and combines all pages. It cannot be combined with `--cursor`. Any failed page or repeated cursor fails the command without printing a partial collection. JSON remains the output format.

```sh
clumsies project list --limit 20
clumsies project list --limit 20 --cursor 'CURSOR_FROM_PAGE_INFO'
clumsies project list --all
clumsies review list AgentOS --all
clumsies review comments REVIEW_ID --all
clumsies draft list --status open --limit 20
clumsies draft list --cursor 'CURSOR_FROM_NEXT_CURSOR'
```

Server lists return `page_info.next_cursor` and `page_info.has_more`; local Drafts return top-level `next_cursor`. A null cursor marks the end. Project, Review, and discussion paging requires a Server containing this change; older Servers ignore these parameters and can truncate results at 200. Server pages use offsets with stable ID tie-breaking; concurrent mutations can move records between pages, so restart the listing after changes rather than treating it as a snapshot. `--all` holds the combined result in memory.

Project `show`, `join`, `bind`, `bindings`, and `review list` accept a Project ID or unique case-insensitive name. Names are resolved across all accessible pages; unknown or ambiguous names fail without changing state. IDs are preferred for automation. `join` (also available as `select`) only selects a project; it does not bind a directory.

```sh
cd /absolute/path/to/repository
clumsies project join AgentOS
clumsies project bind AgentOS
clumsies project current
```

`bind` defaults its directory to `.`. If the directory belongs to a different project, the error identifies that project and its binding revision. Inspect it with `project bindings OLD_PROJECT_ID`, then deliberately replace it with `project bind AgentOS . --revision INSPECTED_REVISION`. Never guess a revision. Local lookup by ID still works after the old project was deleted. Draft status filters (`open`, `submitted`, `discarded`, `merged`) apply before pagination.

## Connect an account and repository

Use your configured Server origin. **An administrator must invite new users first; the CLI does not offer open registration.** Choose the path matching your invitation. Remote origins require HTTPS; loopback HTTP is allowed for local development. Tokens and passwords are never command-line arguments or CLI output.

### Invited through your Google email

```sh
clumsies login --server https://app.clumsies.ai
```

In the browser, choose **the Google account whose email was invited** (other deployments use their configured identity provider). The Server checks membership and completes first-time activation and sign-in. An uninvited email does not automatically join the organization. Use `--no-browser` when automatic browser opening is unavailable.

### No Google account: accept an invitation code

After an administrator gives you a one-time invitation code, run this for first-time activation:

```sh
clumsies redeem --server https://app.clumsies.ai --username myname
```

Replace `myname` with your chosen username. Enter **the invitation code and a new password** at the hidden prompts. Success activates the account and **also signs the CLI in; do not run `login` again**.

For subsequent sign-ins with an existing password account, use:

```sh
clumsies login --server https://app.clumsies.ai --username myname
```

Enter your password at the prompt; automation may use `--password-stdin`. Password login does not register an account with an unredeemed invitation.

### After either path succeeds: select a Project and bind its directory

```sh
clumsies project list
clumsies project join prj_example
clumsies project bind prj_example /absolute/path/to/repository
clumsies project current
clumsies agent enable claude-code
# Codex must expose its plugin-capable executable; --host-binary overrides discovery.
clumsies agent enable codex --host-binary /absolute/path/to/codex
```

On Windows, `agent enable codex` discovers the registered Codex App before PATH; `--host-binary` still overrides discovery. Its MCP executable and adjacent DLLs are staged under `%USERPROFILE%\.clumsies\agent-runtimes\codex\<bundle-hash>` so Store-hosted Codex can access them outside virtualized AppData. After upgrading, run `agent enable codex` again and reconnect Codex. Older staged bundles remain available for active sessions and consume disk space.

`project join` selects a Project **after the Server confirms membership**. It does not grant membership: an administrator must first admit the account to that Project. `project create NAME` uses existing Server permissions. Directory bindings are separate from selection; Agent requests resolve their working directory and never fall back to an unrelated selected Project. Worktrees inherit the repository binding according to the existing daemon rules.

For password resets, use `clumsies redeem --server ORIGIN --reset-password`. It prompts for the one-time token and new password. Invitation acceptance and password reset also support `--stdin` with a JSON object containing `token` and `password`; keep that input private.

Browser login uses the existing loopback PKCE callback and a five-minute callback wait. On a remote/headless host, use password login when the deployment enables it, or `--no-browser` with the printed callback port forwarded to the machine running the browser. There is no new device-code authentication endpoint. The Server must already be configured.

Supported adapter names are `codex`, `claude-code`, `opencode`, `dsh`, and `antigravity`; install the Agent host separately. `agent list` inspects settings, and `agent disable HOST` removes daemon-managed configuration while preserving user modifications. Reconnect the integration or start a new Agent task after binding or adapter changes.

Inspect `project bindings PROJECT_ID` before replacing or removing a binding. `project bind ... --revision N` replaces the exact inspected revision; `project unbind PATH --revision N` removes it without deleting local drafts or repository files.

## Propose, review, and publish

The Agent uses the existing MCP `memory` tool with `activate`, `load`, and `store`. `store` saves a durable local Draft, not a publication. Human Review commands call the existing Server APIs through the daemon's authenticated proxy.

```sh
clumsies draft list
clumsies draft show LOCAL_DRAFT_ID
clumsies draft sync LOCAL_DRAFT_ID
clumsies review create LOCAL_DRAFT_ID --title 'Clarify the release procedure'
# Multiple local draft IDs create one ordered Review.
clumsies review list prj_example
clumsies review show REVIEW_ID
clumsies review diff REVIEW_ID
clumsies review comment REVIEW_ID --version 1 'Checked the procedure'
clumsies review approve REVIEW_ID --version 1 --note 'Ready to publish'
# Inspect the new version and coordination reference after approval.
clumsies review show REVIEW_ID
clumsies review merge REVIEW_ID --version 2 --reference CURRENT_COMMIT_ID
# An empty reference is spelled ref-none.
# Or reject an open Review at its inspected version:
clumsies review reject REVIEW_ID --version 1 --note 'Please revise this proposal'
```

Versions above are examples, not defaults. Use the version and reference you actually inspected. Server policy may publish during approval; if the response is already `merged`, no separate merge is needed. Permissions, expected versions, reference validators, approval fingerprints, and lifecycle rules remain Server-owned. A stale mutation fails without being retried against unseen content. Rejected drafts may be edited and submitted in a new Review.

`review show` includes full ordered operations and comments. `review diff` uses each draft's immutable base snapshot, not the latest local cache. A Server read failure stops the command; it does not substitute an empty ancestor.

### Reconcile upstream changes

Before initial submission, `draft plan LOCAL_DRAFT_ID` returns a candidate containing the ancestor, upstream, proposal, conflicts, and optional merge preview. To apply it separately:

```sh
clumsies draft rebase LOCAL_DRAFT_ID --candidate CANDIDATE_ID --version DRAFT_VERSION --reference CURRENT_COMMIT_ID --resolved resolved-state.json
```

`resolved-state.json` contains the complete `ReconciliationResourceState` (`exists`, `resource`, and `content`) after your edits. Omit `--resolved` for a clean candidate. Alternatively, `review create ... --reconciliations choices.json` accepts an array of the inspected `ReviewDraftRequest` values (`draft_id`, `expected_draft_version`, `candidate_id`, `resolved_state`). Choose all drafts from one Project and scope.

For an existing Review, keep all proposals and one consistent revision together:

```sh
clumsies review plan REVIEW_ID --version INSPECTED_VERSION > plan.json
# Inspect plan.candidates and all plan.detail proposals.
# Copy the top-level request object into update.json.
# For conflicts, edit each candidate's merge_preview.state and assign it as resolved_state.
clumsies review update REVIEW_ID --file update.json --reference CURRENT_COMMIT_ID
clumsies review diff REVIEW_ID
```

The template deliberately leaves conflict resolutions null. Inspect and edit them before applying; it never silently chooses the ancestor, upstream, or proposal. The Server rejects stale candidates, missing proposals, stale Review versions, and moved references. Approval may be invalidated after updates; inspect and review again.

## Diagnose and upgrade

`status` does not start a daemon. `daemon start` reuses a compatible resident; ordinary commands and MCP cold starts start it on demand on Windows/Linux. `status --project PROJECT_ID` includes retrieval model download bytes, readiness, index progress, and errors. Initial use prepares the pinned models (about 412 MiB); a preparing model is actionable status, not a successful empty search.

`draft sync` waits up to a minute for upload without dropping local work. `draft retry PROJECT_ID` retries failed operations; inspect `draft show` for the error first. Expired access tokens are refreshed by the daemon. If refresh is revoked, run `login` again; local drafts and bindings remain. `logout` attempts Server revocation and clears local credentials even if revocation fails.

Re-run the verified Linux script, Windows script, or Windows installer to upgrade. Installation coordinates startup with a program-directory lock, stops the resident, stages the whole binary pair, and rolls back a failed switch. The executable paths stay stable so adapters keep working. Close active CLI/MCP processes when Windows reports a file in use. Reconnect Agent hosts after upgrading.

Program uninstall retains credentials, directory bindings, local drafts, and caches. Use `logout` first when you want to remove credentials. Do not roll back to a daemon that cannot read a newer local database schema; stop the runtime and back up the complete daemon data root before a downgrade. The installers do not promise automatic schema downgrades or overwrite unrelated program directories.

Build from source with `cargo build --locked --release -p clumsiesd --bins`. The CLI has no separate credentials file, cache, or business-rule implementation. The historical Zig CLI remains archived in Git commit `4b18f7947a977dbc6b62f560b698dc992597f19d`; this Rust CLI does not restore it.
