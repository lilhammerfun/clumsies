# Project

A Project groups membership, selected knowledge, and proposed changes. A local repository directory can bind to it so Agents find the right Memory while working. Server issues the Project identity; a directory name cannot replace it.

This page retains the historical `/workspace` URL. Current APIs use `project_id`. See the [core data model](/data-model) for its relationship to Organization.

## What a Project manages

| State | Location | Purpose |
|---|---|---|
| Name, description, members | Server | Project identity and access |
| Org Selection | Server | Select published Organization Memory for this Project |
| Project Ref / Commit | Server, synchronized to daemon | Version the selection as an installable snapshot |
| Project-carried Drafts | Persisted locally, then synchronized to Server | Proposals targeting Organization publication |
| Local directory bindings | Daemon | Resolve a working directory to `project_id` |
| Installed generations and search index | Daemon-managed files / SQLite | Local Agent reads and retrieval |

Membership authorization and content selection are separate concerns. Selecting a Memory puts it in the baseline; a personal Bundle, directory binding, or Agent request does not change membership permissions.

## Create and configure in Memory

Use **New Project…** in the Memory project selector. Organization members can create projects; the creator becomes a project administrator and can edit its details, add or remove existing organization members, select Memory, and delete the project. These permissions do not grant organization administration or publication authority.

Open **Project Settings** beside the selector to configure the current project. **Repositories on This Mac** and cache settings apply locally; other members bind their own directories. The selector ends with **New Project…** for both members and organization administrators.

Organization administrators can open **Settings → Organization → Projects** to view all organization projects and manage their details and members, including projects they have not joined. Reading their Memory still requires project membership.

## From selection to readable content

```text
Current Organization Memory + Project Org Selection
  → Server generates Project Commit / Ref
  → daemon installs the snapshot
  + this Project's open/submitted Draft changes
  → Effective Memory
```

For example, Payments selects the deployment rollback checklist. All work directories bound to Payments refer to the same Server Project. An edit creates a Draft carried by Payments. After synchronization, another installation can present that Project's proposal too. It is not an independently published file in one directory, nor another Project's unpublished change.

Unpublished changes overlay only the carrying Project. After merge, Organization content changes and Projects selecting affected resources receive refreshed projections. Newly created Memory is also automatically selected for the originating Project.

`GET /api/v1/projects/{project_id}/memories` reads historical Project-authority data. It includes neither current selection projections nor local Draft overlays and cannot replace Effective Memory.

## Directory binding

A binding is:

```text
Normalized Server URL + canonical local directory → Server project_id
```

The daemon stores bindings in central SQLite `project_bindings`. From a subdirectory, it chooses the longest bound ancestor. An unbound Git worktree can also resolve through the main checkout's repository root. Moving or rebinding a directory changes local resolution, not Server Project identity.

Both managed host-plugins and manually started `mcp serve` require a working-directory binding. They validate it at startup and every `tools/call`, failing if the binding disappears or changes. Unbound directories cannot fall back to the Desktop-selected Project.

Two bound Agent processes can therefore serve different repositories concurrently. Switching the Desktop selection does not redirect managed processes. A tool request cannot choose an arbitrary `project_id` to bypass binding.

The runtime no longer reads or migrates `~/.clumsies/config.toml`, and legacy `ws_id` is not Project identity. See [Adapter](/adapter) and [Runtime](/runtime) for installation and entry-point details.

## Project Local Storage

An installation can choose where a Project stores generations and its search database. The setting is keyed by Server URL and `project_id`, applies only locally, and is not Server Project metadata.

The chosen directory is the parent of a daemon-managed subtree. Central Drafts, queued operations, credentials, cached snapshot objects, and shared models do not move. If a custom location is unavailable, the daemon reports it instead of silently creating another active cache. See [Runtime](/runtime#project-local-storage) for moves and recovery.

## Investigating missing Memory

Check the data flow in order: the directory's bound Project, its Org Selection, the installed Project Commit, any Draft overlay, and index readiness. A publication awaiting local synchronization, an unselected resource, and a Draft overriding the baseline are different problems.

Continue with [Organization Memory](/artifact), [Core data model](/data-model), and [Architecture](/architecture). Implementation sources: [binding resolution](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/state.rs), [MCP entry point](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/agent_runtime/mcp.rs), [Project projection](https://github.com/lilhammerfun/clumsies/blob/main/crates/server/src/app/memory/repository.rs), and [local storage](https://github.com/lilhammerfun/clumsies/blob/main/crates/daemon/src/project_storage.rs).
