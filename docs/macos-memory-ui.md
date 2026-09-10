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

## Review requests

Directory and multi-selection Review requests include open Organization Drafts carried by the selected Project. Every Draft must be synced and have a Server ID; directory operations and document synchronization also block the entry point.

Behind Drafts, including those with conflicts, can open the request sheet. The sheet loads reconciliation candidates and asks the user to resolve conflicting files before updating the Drafts and creating one Review in the same transaction. The entry point does not require freshness to be current.

## Implementation boundary

SwiftUI lives under `apps/macos/`; Draft persistence, synchronization, and authority checks live in the shared daemon and Server contracts. Planned interactions are tracked as gaps, not documented as shipped behavior.
