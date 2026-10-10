---
title: Clumsies CLI
description: Install the Windows/Linux CLI, connect an Agent, and review and publish Memory without a graphical client.
---
# Clumsies CLI

`clumsies` is the human client. `clumsiesd` owns credentials, local drafts, caches, directory bindings, synchronization, and the existing `clumsiesd mcp serve` Agent entry. Both executables come from one Rust package and must be upgraded together. Windows/Linux GUI development is paused and its implementation and packaging have been removed from the active tree; the native macOS App remains supported.

## Install

The initial CLI packages target **Linux x86_64 (Ubuntu 24.04 or a compatible glibc system)** and **Windows x64**. Download released archives and the Windows user installer from [GitHub Releases](https://github.com/lilhammerfun/clumsies/releases). For changes not yet released, the repository's **CLI** workflow provides tested build artifacts. Verify the archive or installer against its adjacent SHA-256 file before running it.

On Linux, extract `clumsies-cli-VERSION-linux-x86_64.tar.gz`, enter its directory, and run:

```sh
sha256sum --check SHA256SUMS
./install.sh
```

After installation, open a new terminal and run:

```sh
clumsies --version
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

After installing the macOS App, double-click **Install CLI.command** in the DMG. With more than one installed App, specify which App to use:

```sh
sh /Volumes/Clumsies/Install\ CLI.command "$HOME/Applications/Clumsies.app"
# The installed App also carries the installer:
sh "$HOME/Applications/Clumsies.app/Contents/Resources/install-cli.sh"
```

The installer creates `~/.local/bin/clumsies` and appends PATH setup to bash/zsh user startup files without replacing their contents. Open a new terminal and run `clumsies --version`; no manual export is needed. Linux `install.sh` also sets up bash/zsh PATH. Other shells currently report an explicit setup error. Unrelated commands are never replaced. The macOS link uses the embedded App runtime and survives App upgrades at the same location. Moving the App requires updating the command entry. Credentials, bindings and Drafts are retained; the App continues to own the macOS daemon lifecycle.

## Output and terminal reading

Commands default to readable text. Lists show names, IDs, lifecycle and sync information; details retain Review versions, references, authority scopes and semantic changes. `status` summarizes readiness and required attention; `--verbose` includes diagnostic fields. Long output automatically uses a terminal pager; short output displays directly. Configure `CLUMSIES_PAGER` or `PAGER`, or pass `--no-pager`. The default detects `less`, then falls back to available `more` (the system reader on Windows), or direct output if no reader exists. Reader key bindings depend on the installed pager. JSON, pipes and redirected output never launch a pager or emit animations or colors.

```sh
clumsies project list
clumsies draft list --status open
clumsies review diff REVIEW_ID
clumsies --no-pager review show REVIEW_ID
```

**Script migration: JSON is no longer the default. Add `--json` explicitly.** JSON keeps existing response envelopes without human messages. Export editable reconciliation plans in JSON:

```sh
clumsies --json status
clumsies project list --json
clumsies review plan REVIEW_ID --version INSPECTED_VERSION --json > plan.json
```

## Automatic pagination and project selection

Text lists automatically follow Server/local cursors and display each batch without collecting every page first. No manual cursor copying is needed. Closing the reader stops subsequent requests, with reader-dependent prefetch. An interrupted fetch reports incomplete results and exits nonzero; already displayed rows do not imply a complete collection. Concurrent updates can move records between pages, so traversal is not a fixed snapshot.

JSON retains `--limit` (request size 1–200, default 100), `--cursor` and `--all`. By default it returns one page; `--all` starts from the first page and conflicts with `--cursor`. JSON all-pages output is emitted only after every request succeeds, and retains the collection in memory. Failures and cursor loops never produce a partial JSON document. In text mode, `--limit` remains a request batch size rather than a total result limit, and `--cursor` selects the traversal's starting point.

```sh
clumsies project list --json --limit 20
clumsies project list --json --cursor 'returned cursor'
clumsies review list AgentOS --json --all
```

Older Servers may ignore pagination and return at most 200 records; the CLI cannot recover records the Server does not expose. Server lists use `page_info.next_cursor`, local Drafts use top-level `next_cursor`.

Project commands and Review lists accept an ID or unique case-insensitive name. Ambiguous names fail; scripts should use IDs. `project join` (alias `select`) selects a project without binding the directory or granting membership. `project bind PROJECT` defaults to the current directory. Inspect conflicting bindings before passing `--revision`; do not guess revisions. Draft `--status` filtering occurs before pagination and accepts `open`, `submitted`, `discarded`, or `merged`.

## Directory context and Draft filters

Text `draft list` and `review list` (without a project argument) use the current directory binding, never the globally selected project. Unbound or inaccessible directories fail explicitly: bind the directory or specify a project. `draft list --global` inspects retained Drafts from all local projects, including unavailable projects. Explicit project IDs work without a successful Server project-name lookup.

```sh
cd /absolute/path/to/repository
clumsies draft list --status open
clumsies review list
clumsies draft list --project AgentOS --scope project --status open
clumsies draft list --global
```

Draft project, scope and status filters run in the daemon before pagination. `--scope` accepts `project` or `org`; `--project` and `--global` are mutually exclusive. JSON `draft list` keeps its existing global default for scripts; add `--project` explicitly when needed. Upgrade CLI and daemon together. The CLI rejects Drafts outside requested filters instead of showing an unfiltered collection from an older resident.

## Multiline input and editors

Comments accept inline text, `--file PATH`, `--file -` (stdin), or `--editor`. Review creation accepts `--description-file`; approve/reject accept `--note-file`. Their `--editor` option composes the description or decision note. Input sources are mutually exclusive. Files/stdin must be UTF-8 and at most 4 MiB. Empty comments are rejected before submission.

```sh
clumsies review comment REVIEW_ID --version INSPECTED_VERSION --file comment.md
printf 'First line\nSecond line\n' | clumsies review comment REVIEW_ID --version INSPECTED_VERSION --file -
clumsies review create DRAFT_ID --title 'Proposal' --description-file description.md
clumsies review approve REVIEW_ID --version INSPECTED_VERSION --note-file decision.md
clumsies review comment REVIEW_ID --version INSPECTED_VERSION --editor
```

Editors use `VISUAL`, then `EDITOR` (for example `code --wait` or `vim`; Windows can use `notepad`). Configure an editor that waits until editing ends. Editor mode requires interactive stdin and stdout; use files/stdin for automation or redirected JSON receipts. A successful editor exit submits the input. Cancel by exiting the editor unsuccessfully; empty comments also prevent submission. Editor, validation or Server failures retain the input in a private temporary directory and report its path on stderr; successful submissions remove it. Retained buffers may contain private content: remove them after recovery.

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

Run `draft sync LOCAL_DRAFT_ID` before `draft diff LOCAL_DRAFT_ID` to compare the complete synchronized proposal with its immutable ancestor. Diff rejects pending or failed uploads rather than showing incomplete content.

`draft plan` and `review plan` show conflict paths, ancestor/upstream/proposal differences, and the exact candidate, version, and reference guards. Computing a plan applies no changes.

Resolve ordinary text conflicts without editing JSON:

```sh
clumsies draft plan LOCAL_DRAFT_ID
clumsies draft rebase LOCAL_DRAFT_ID --candidate CANDIDATE_ID --version DRAFT_VERSION --reference CURRENT_COMMIT_ID --edit

clumsies review show REVIEW_ID
clumsies review plan REVIEW_ID --version INSPECTED_VERSION
clumsies review update REVIEW_ID --resolve --version INSPECTED_VERSION --reference CURRENT_COMMIT_ID
clumsies review diff REVIEW_ID
```

The editor contains only the partial merge's text, including automatically merged surrounding content. Remove all generated conflict markers, leave the final text, and save/exit. Resource identity, authority scope, and content metadata remain intact; Review updates retain every proposal and its revision. No decision or publication is automatic.

Path and deletion choices must be explicit:

| Conflict | Single Draft rebase option | Review update --resolve option |
| --- | --- | --- |
| Path conflict or occupied path | `--path FINAL_PATH` | `--path SERVER_DRAFT_ID=FINAL_PATH` |
| Deletion conflict: keep the surviving resource | `--keep` | `--keep SERVER_DRAFT_ID` |
| Explicitly delete the resource | `--delete` (omit `--edit`) | `--delete SERVER_DRAFT_ID` |

Review options may repeat and use the Server Draft IDs shown by the plan; the positional `draft rebase` ID remains local. Deletion cannot also rename or supply content. Path-only conflicts and explicit keep/delete choices do not require an editor. Unsupported conflict dimensions use the full JSON workflow below.

Supply final text files for noninteractive use or to recover an interrupted edit:

```sh
clumsies draft rebase LOCAL_DRAFT_ID --candidate CANDIDATE_ID --version DRAFT_VERSION --reference CURRENT_COMMIT_ID --content resolved.txt
clumsies review update REVIEW_ID --resolve --version INSPECTED_VERSION --reference CURRENT_COMMIT_ID --content SERVER_DRAFT_ID=resolved.txt
# For stdin use --content - on Drafts or --content SERVER_DRAFT_ID=- on Reviews.
```

Multiple proposals are edited in candidate order. The CLI submits once, after all choices and edits validate. Cancellation, remaining markers, or Server rejection retains every editor file and reports its location (`input-1.txt`, `input-2.txt`, etc. for a batch). Recover with the matching `--content` file options; quote the entire argument when its path contains spaces. If the version/reference changed, inspect show/diff/plan again and reconcile saved text with the new evidence before reusing it. Do not blindly replace version numbers. Updates can invalidate approval and require another review.

The following JSON inputs remain available for scripts and full-state editing.


Before initial submission, `draft plan LOCAL_DRAFT_ID` returns a candidate containing the ancestor, upstream, proposal, conflicts, and optional merge preview. To apply it separately:

```sh
clumsies draft rebase LOCAL_DRAFT_ID --candidate CANDIDATE_ID --version DRAFT_VERSION --reference CURRENT_COMMIT_ID --resolved resolved-state.json
```

`resolved-state.json` contains the complete `ReconciliationResourceState` (`exists`, `resource`, and `content`) after your edits. Omit `--resolved` for a clean candidate. Alternatively, `review create ... --reconciliations choices.json` accepts an array of the inspected `ReviewDraftRequest` values (`draft_id`, `expected_draft_version`, `candidate_id`, `resolved_state`). Choose all drafts from one Project and scope.

For an existing Review, keep all proposals and one consistent revision together:

```sh
clumsies review plan REVIEW_ID --version INSPECTED_VERSION --json > plan.json
# Inspect plan.candidates and all plan.detail proposals.
# Copy the top-level request object into update.json.
# For conflicts, edit each candidate's merge_preview.state and assign it as resolved_state.
clumsies review update REVIEW_ID --file update.json --reference CURRENT_COMMIT_ID
clumsies review diff REVIEW_ID
```

You can also update directly in an editor:

```sh
clumsies review update REVIEW_ID --edit --version INSPECTED_VERSION --reference CURRENT_COMMIT_ID
```

The editor shows `request` and the complete `plan`. Inspect candidates and all proposals, then place each author-confirmed complete state in `request.drafts[].resolved_state`; conflicts still default to null. Only `request` is submitted; editing `plan` cannot replace Server evidence. The CLI rejects changes to the inspected Review version. Candidates, Draft versions, complete proposal sets and upstream references retain Server validation. The JSON template retains path, directory and authority-scope metadata so editing a text body cannot lose other changes. `--file update.json` and `--file -` continue accepting the original request object.

The template deliberately leaves conflict resolutions null. Inspect and edit them before applying; it never silently chooses the ancestor, upstream, or proposal. The Server rejects stale candidates, missing proposals, stale Review versions, and moved references. Approval may be invalidated after updates; inspect and review again.

## Diagnose and upgrade

`status` does not start a daemon. `daemon start` reuses a compatible resident; ordinary commands and MCP cold starts start it on demand on Windows/Linux. `status --project PROJECT_ID` includes retrieval model download bytes, readiness, index progress, and errors. Initial use prepares the pinned models (about 412 MiB); a preparing model is actionable status, not a successful empty search.

`draft sync` waits up to a minute for upload without dropping local work. `draft retry PROJECT_ID` retries failed operations; inspect `draft show` for the error first. Expired access tokens are refreshed by the daemon. If refresh is revoked, run `login` again; local drafts and bindings remain. `logout` attempts Server revocation and clears local credentials even if revocation fails.

Re-run the verified Linux script, Windows script, or Windows installer to upgrade. Installation coordinates startup with a program-directory lock, stops the resident, stages the whole binary pair, and rolls back a failed switch. The executable paths stay stable so adapters keep working. Close active CLI/MCP processes when Windows reports a file in use. Reconnect Agent hosts after upgrading.

Program uninstall retains credentials, directory bindings, local drafts, and caches. Use `logout` first when you want to remove credentials. Do not roll back to a daemon that cannot read a newer local database schema; stop the runtime and back up the complete daemon data root before a downgrade. The installers do not promise automatic schema downgrades or overwrite unrelated program directories.

Build from source with `cargo build --locked --release -p clumsiesd --bins`. The CLI has no separate credentials file, cache, or business-rule implementation. The historical Zig CLI remains archived in Git commit `4b18f7947a977dbc6b62f560b698dc992597f19d`; this Rust CLI does not restore it.
