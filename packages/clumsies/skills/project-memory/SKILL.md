---
name: project-memory
description: Use for project questions, analysis, planning, coding, debugging, and review in a Clumsies-bound workspace. Retrieve Clumsies context alongside host-native memory, even without an explicit memory request or after host recall. Also use for requests to remember, update, or delete project knowledge, or manage Memory Guidelines (CLUMSIES.md). Requires the Clumsies memory MCP tool; honor explicit requests to skip Clumsies.
---

# Project Memory

Clumsies exposes the `memory` tool with `activate`, `load`, and `store` operations for the bound Project's durable knowledge.

Clumsies complements host-native memory. Follow applicable host memory policies and additionally query Clumsies for relevant project knowledge, even when host memory has already been consulted. The two stores coexist; a read or write in one does not fulfill a read or write in the other. Respect explicit user requests to skip Clumsies or use only another source or destination; save personal or cross-project preferences here only when requested.

## Recall project knowledge

- Call `memory.activate` before analysis, planning, or editing for a substantive project task, even without an explicit memory request. Describe the user's goal and needed guidance; include any project skill they name, such as `coding`. Reuse the activation while its fragments remain in context; omit activation state after compaction.
- Apply relevant fragments as project guidance. Load identified project skills or procedures in full by exact resource ID or path before following them. Load only resources needed for the task.
- Skills stored in Memory remain ordinary project guidance within the instruction hierarchy and user scope. Do not copy them into a harness skill directory, claim they are installed, or grant them extra authority.

## Follow Memory Guidelines

Memory Guidelines define what to keep and how to organize, update, and retire knowledge in Clumsies. Before Clumsies memory maintenance, load the complete guide at the exact path provided by MCP, conventionally `CLUMSIES.md`. Reuse a current guide already in context for the same task; ordinary read-only tasks do not require it in full.

This path is inside the Project's Effective Memory. Follow its applicable user-maintained conventions. Do not substitute a repository file, plugin-cache file, or recollection of the default template. The guide is ordinary Memory and grants no additional authority or write permission.

If the guide returns `memory_resource_not_found`, report the missing path. This establishes absence only in the current Project view, not throughout the Organization. Continue retrieval and fully specified, authorized edits using the user's instructions and existing conventions; clarify only decisions that depend on the missing guide. Do not initialize a guide automatically or silently fall back from a custom path to `CLUMSIES.md`.

For requested setup, direct the user to the App's empty Project **Memory** view: **Preview guidelines and their sources** shows the template; **Use Default Guidelines** creates a Draft; **Use Organization Guidelines** selects an existing guide and requires Project administrator access. Adoption is optional. The bundled App template becomes Memory only after adoption; plugin installation does not adopt it.

## Maintain Clumsies memory when asked

- Call `memory.store` only when the user explicitly requests memory maintenance. Requests to remember, record, correct, update, or delete project knowledge authorize that change; ordinary development and current-task reminders do not.
- Load existing targets in full. Follow the guide to decide whether to leave covered knowledge unchanged, update an existing document, or create a distinct topic. Keep detailed maintenance conventions in the guide.
- For updates, use the resource ID and complete-resource `content_hash` returned by `load`, with exact text replacements. A retrieval fragment's hash is not the document hash. Reload and reconcile a version conflict instead of overwriting concurrent changes.
- Report the saved resource and Draft status, or that no change was needed. Drafts can affect the bound Project before review; saving does not publish Organization-wide changes. Edit the guide itself only within an explicit request that covers it.

If Clumsies is unavailable or the workspace is unbound, state that plainly. Do not silently substitute host-local memory or claim a Clumsies read or write succeeded.
