# Inbox

Inbox is the personal notification entry in the macOS sidebar. It spans accessible projects and offers Inbox, Unread, and Archived filters, message-type filtering, and search. Types are Review Requests, Review Comments, Review Results, Remote Updates, Sync Errors, Welcome, and Access Changes. Projects remain source labels on messages. Archive hides the displayed revision; a later event returns the subject as unread. Neither read nor archive changes a Review or Memory.

The list reuses `ToolbarFilterMenu` for status and message type in the leading navigation toolbar. Read/unread, archive/restore, and refresh sit on the trailing side, followed by the shared `ClassicSearchField` at the right edge, using the workspace's macOS 14/26 placement rules. ⌘K focuses search while preserving filters.

Rows have two lines: title and absolute timestamp, followed by the event summary, source, and a named action. Text truncates rather than adding a third line; the tooltip exposes the full summary. Notifications are ordered newest first. Unread messages use a dot, a heavier title, and an accessibility label. There is no per-row archive icon, generic disclosure chevron, or inline expansion.

Clicking or selecting a single notification marks it read without navigating. In the Unread filter, that selected row remains visible until selection changes, so its action stays available. Explicit Mark as Unread remains effective while the row stays selected. Native range/multiple selection preserves unread state for bulk toolbar and context-menu commands. **Mark as Read / Unread** (⇧⌘U) changes only the read state. **Archive** (⌃⌘A) hides the displayed revision without marking it read; **Move to Inbox** restores it with its read state intact. Archive/restore registers a native Undo action for the successful items. A partial failure stops the batch, leaves the failed item visible, and shows the error. Undo cannot cross an account change or overwrite a new notification revision.

These choices follow the selection-and-command pattern in [Apple Mail](https://support.apple.com/en-in/guide/mail/mlhlp1052/mac) and the HIG guidance on [context menus](https://developer.apple.com/design/human-interface-guidelines/context-menus) and [toolbars](https://developer.apple.com/design/human-interface-guidelines/toolbars). The exact notification grouping and read timing below are Clumsies product decisions.

| Message | Explicit action and destination | Read behavior |
| --- | --- | --- |
| Welcome | **Read Message** opens its original Markdown body in a native sheet. **Open Memory** is available when the user has an accessible project. | Mark read on single selection or when the message is presented. |
| Project added or role changed | **Open Project** opens that project’s existing Memory workspace, while access remains available. | Mark read on single selection or successful navigation. |
| Project access removed or organization role changed | Explain the change in the row, without a detail page or inaccessible destination. | Mark read on single selection, or use Mark as Read. |
| Review request, comment, or result | **Open Review** opens the existing Review page. | Mark read on single selection or successful navigation. |
| Remote updates | **Open Memory** switches to the corresponding project and opens files with remote changes in existing Diff tabs. If those changes are already synchronized, it simply opens the project's Memory workspace. | Mark read on single selection or successful navigation. |
| Sync problem | **Retry Sync** performs recovery directly; there is no invented message detail page. | Mark read on single selection or successful recovery; resolved local problems disappear. |
| Information without a destination | No open button or double-click navigation. The row remains selectable for read/archive commands. | Mark read on single selection, or use Mark as Read. |

Only the welcome message has original body content and its own reading view. Review, remote-update, and access notifications reuse existing destinations or remain informational rows; sync errors perform recovery directly. Navigation errors remain in the list; selection may already have acknowledged the notification. Opening Memory only displays its existing comparisons; reading, archiving, or undoing never publishes, discards, merges, or resolves files.

## Notification sources

| Source | Recipients and grouping | Destination |
| --- | --- | --- |
| First successful login | One welcome per user, persisted with the first session; no repeat across devices or later logins, and no retroactive notice for existing users with sessions | Original welcome body |
| Added to or removed from a project | The affected member, excluding self-actions; one notice per actual change | Existing project Memory when still accessible; removal is informational |
| Project or organization role changed | The affected member; previous and new roles and actor, with explicit scope; no notice for an unchanged role | Existing project Memory when applicable and accessible |
| Review submitted or resubmitted | Active owner/admin project members, excluding the actor; one entry per Review | Review detail |
| Review comment | Author, participants, and existing recipients who retain membership, excluding the actor | Review detail |
| Review approved, rejected, or merged | Author, participants, and existing recipients who retain membership, excluding the actor | Review detail |
| Remote Memory publication | Members of affected projects, excluding the actor; one entry per project | Existing Memory workspace and document Diff tabs |
| Sync failure or unavailable local service | This Mac; one entry per condition | Retry Sync |

Ordinary Draft creation, editing, renaming, deletion, upload, and submission state are not notification sources. Inbox does not fetch or aggregate the Draft inventory and has no Drafts filter or Memory Changes page. File colors and existing Memory/Review commands continue to express the user's own work.

Remote updates are emitted only after remote publication affects Memory selected by a project. The server finds those projects through resource selections and delivers to their active members, excluding the publisher. Merely advancing an unrelated organization Ref, saving a Draft, or belonging to a project that uses other Memory does not trigger a remote-update notification. Rename and deletion affect the projects referencing the resource before publication; newly published files notify the proposing project after they enter its selections.

Known remote changes without a corresponding server notification remain discoverable from the existing per-resource synchronization plan. Sync Errors cover failed uploads or downloads, degraded synchronization, and an unavailable local sync service. Pending uploads and ordinary Draft edits alone do not qualify. Resolved local sync problems disappear.

The file tree retains file-type icons, context menus, and change colors: green for additions, amber for modifications, and red for deletions, including submitted changes until they are merged. Trailing status icons are removed. Review, update, and reconciliation commands remain in the existing document and file menus; documents have no extra status strip. Background notification icons are removed from the workspace toolbar. Operation failures, including failed saves, remain visible where the operation occurred.

## Persistence and access

The server stores welcome, access-change, Review, and publication notifications in the business transaction that produces the event. Existing open Reviews are backfilled for eligible reviewers. Reads and receipt updates recheck the recipient and organization. Review and remote-update content additionally requires current project membership. Personal access notices retain only their event data and project-name snapshot after removal or project deletion; this does not grant access to the project or its content. The client also removes inaccessible source notifications and project actions when its project list changes. Direct merge emits one merged outcome; unsuccessful writes produce no notification.

`GET /api/v1/me/inbox` returns keyset pages including archived/read subjects. `PATCH /api/v1/me/inbox/{notification_id}` accepts the displayed `version` and `read`, `unread`, `archive`, or `restore`. A stale receipt cannot acknowledge a newer revision or undo a newer archive/read state. Read state and archive state are independent. See the public OpenAPI contract for wire fields.

Local receipts use preferences scoped to server, organization, and user. Authority changes clear visible state and invalidate in-flight requests. Cross-project navigation flushes pending edits before switching; remote updates download the current commit before comparing files.

Project membership currently takes effect immediately when an administrator adds a member. The notice says the user was added; it does not offer acceptance or rejection for an invitation workflow that does not exist. Organization account disabling revokes sign-in, so Inbox is not a delivery channel for a disabled account.

## First-version limits

- Refresh uses the existing 30-second workspace cadence and explicit refresh. This version has no system push notifications.
- Server notifications use the daemon's existing offline response cache. Read/archive writes require a connection and report failure; they are not queued offline. Local receipts remain usable offline.
- Local sync-error and remote-update fallback receipts do not synchronize across devices.
- Notification history aggregates by subject and reloads all pages. It is not an audit log. Comments open their Review, without scrolling to an individual comment.

`InboxTests` covers receipt persistence, revision races, authority invalidation, offline errors, and existing remote changes. `inbox_flow` exercises the HTTP API against isolated PostgreSQL, including the submission/comment/rejection/resubmission/merge lifecycle, remote publication, selection-scoped delivery for updates/renames/deletions, pagination, stale receipts, revoked access, welcome deduplication across sign-ins, unchanged roles, and personal notices surviving removal and project deletion.
