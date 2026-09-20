# macOS Memory UI design

The macOS Memory surface edits project-scoped Draft overlays while showing organization authority as read-only context. It cannot bypass Review or publish directly to the organization Ref.

## Product boundary

- The tree presents effective Memory for the selected Project.
- A resource opens the authoritative body plus its project-carried Draft overlay.
- Create, rename, update, and delete actions produce Draft proposals.
- Review is the only path from a proposal to organization authority.

## State and synchronization

Project selection controls effective Memory, Drafts, and Review context. A project or session change invalidates stale document work before asynchronous results publish. Desktop and MCP use the same daemon and Draft contract; revision conflicts, offline state, and sync failures remain visible instead of appearing as publication success.

The UI restores a selection only when it remains valid. Loading, conflict, offline, selection, and error states do not rely on color alone, and keyboard navigation remains available.

File-name colors describe unpublished additions, modifications, and deletions. A gray Draft icon marks unsubmitted edits; a green pull-request icon marks submitted Drafts. Its tooltip identifies the Review, and clicking it or choosing **View Review** opens that Review, including for files in a multi-Draft Review. Merged and discarded Drafts have no file-tree status marker.

When synchronization is idle and changes are submitted, the sync status reports **Synced · N changes in review** and links to the Reviews. Synchronization does not submit or merge a Review. Failures and stale data take priority over this completion message.

File menus use **Rename…** and **Delete…**. Their confirmations explain that the change is saved as a Draft and affects every project referencing the file after review and merge. **Remove from Project** only removes that project's reference.

## Review requests

Directory and multi-selection Review requests include open Organization Drafts carried by the selected Project. Every Draft must be synced and have a Server ID; directory operations and document synchronization also block the entry point.

Behind Drafts, including those with conflicts, can open the request sheet. The sheet loads reconciliation candidates and asks the user to resolve conflicting files before updating the Drafts and creating one Review in the same transaction. The entry point does not require freshness to be current.

## ZIP export

The top toolbar exports all Memory in the current Project or Organization view, independent of the search filter. The file-tree context menu exports a file, multiple selected files, or every descendant of a selected folder, including files hidden by search. Mixed file/folder selections are deduplicated into one ZIP. Memory Actions also exports the open file.

Exports preserve the original relative paths and UTF-8 contents, including local Draft renames, edits, new files, and pending editor text. Deletion Drafts are excluded. The native save dialog chooses the ZIP destination; Finder reveals the completed archive. Export uses the captured workspace view without publishing changes or refreshing it to a newer shared version.

Unloaded bodies go through the existing version/hash-validated reader. Missing bodies, unsafe paths, or file-path collisions fail the export rather than omit files or overwrite a conflicting entry. Loading Draft inventory and active document synchronization block export. Compression runs off the main thread using macOS `ditto`, and the destination is replaced only after the archive is complete.

This is a file snapshot. The org-admin `/api/v1/admin/memory-export` JSON endpoint separately exports migration state, including Draft operations, selections, and Bundles.

## Implementation boundary

SwiftUI lives under `apps/macos/`; Draft persistence, synchronization, and authority checks live in the shared daemon and Server contracts. Planned interactions are tracked as gaps, not documented as shipped behavior.

## Resolving remote changes

The Memory conflict action opens a separate native window with standard close,
minimize, and zoom/full-screen controls. The main workspace remains usable; closing
the source tab does not discard the resolution. The window remembers its frame,
and a draggable divider separates the independently scrolling comparison and result.
Remote and Draft conflict sections appear side by side as unified diffs against
their common original, with the choice button in each side's header. Each choice updates the merged result
without dropping automatically merged changes. After choosing the content changes,
the result remains editable. Path and deletion conflicts require explicit choices.
Save to Draft stays disabled until all choices are complete; it does not approve or
publish. Whole-file replacement is a secondary menu action with confirmation.
Cancel, the close button, and Command-W confirm edited input; saving blocks closing.
Signing out or quitting also checks these windows before discarding unsaved edits.
