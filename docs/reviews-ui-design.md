# Reviews UI Design (macOS App)

The Reviews section follows the native workspace navigation hierarchy:
the global app sidebar stays in place, the Review list is the root page, and a
selected Review is pushed onto a `NavigationStack`. The detail is a file-review
workspace rather than a permanently visible third app column.

This document replaces both the earlier web-style Review page and the temporary
three-column master-detail variant.

## 1. Navigation model

The outer shell has two stable roles:

```
global sidebar | NavigationStack (Review list -> Review detail)
```

- Entering Reviews opens the list, not an automatically selected Review.
- A native `NavigationLink` opens a Review. The system owns pointer, keyboard,
  VoiceOver activation, Back, and navigation transition behavior.
- Returning from a detail restores the list and its filter context.
- Search and newly created Reviews may deep-link to a stable `reviewId`.
- The global sidebar remains visible according to the user's existing sidebar
  preference and is not replaced by Review navigation.

## 2. Review list

Use a native inset `List` with information-rich rows and visible row separators.
A single click pushes the Review detail. The scroll area fills its content
region without drawing an outer border; record boundaries come from system
separators, focus, selection, hover, and inactive-window behavior. Do not enable
alternating row backgrounds: AppKit continues their stripes through empty table
space, making nonexistent Reviews look like blank rows. Pin the separator's
leading alignment to the row rather than allowing trailing metadata such as the
fixed update time to shorten it.

The list is a review queue. Each row answers four scan questions: what changed,
where it belongs, who submitted it, and what needs attention next. Keep the row
to two lines:

```
[lifecycle icon] review title                   optional queue signal
Submitted by author for project                 Updated <local date and time>
```

- Format updatedAt as a fixed local date and time, explicitly labeled Updated. It is the
  last Review record update, not creation time or a live elapsed-time counter.
- Keep the complete description in the detail page; do not add a list excerpt.
- Express Project and author as muted metadata on the left and the fixed
  Review record update time independently on the right. The author is the
  submitter, not necessarily the person who last updated the record, so never
  label the update as theirs. If a Project name or timestamp cannot be
  resolved, omit the value; never expose an opaque Project ID or raw protocol
  timestamp.
- Put one lifecycle icon before the title. Open uses the pull-request icon,
  Merged uses the merge icon, and Rejected uses a red pull-request icon. The
  icon has an accessibility status name and semantic color, so color is never
  the only signal. Do not repeat Open or Merged as row text.
- Resolve an optional queue signal separately. Precedence is `Conflicts`,
  `Update Required`/`Out of Date`, then legacy `Ready to Merge`, or
  `Resubmit`/`Awaiting Author`. Stable lifecycle labels such as `Needs Review`,
  `Approved`, and `Merged` are not repeated on the right.
  A merged Review never displays stale merely because the merge advanced the
  current Project ref. `Ready to Merge` applies only to historical two-step
  approvals and also requires a nonempty approved result hash; capability alone
  does not make a legacy approval mergeable.
- A queue signal is a plain system label with semantic foreground color. It is
  neither a button nor a capsule. At narrow widths, the label may reduce to its
  symbol so the primary Review title retains priority. Lifecycle and queue
  signals remain separate accessibility elements with explicit names.
- Let macOS draw separators, focus, hover/press feedback, and inactive-window
  state. Do not draw an outer list border, per-row cards, or empty-space zebra
  stripes. `NavigationLink` and the stack path are the only navigation state;
  do not add a parallel `List(selection:)` binding that can push the same route
  twice during a programmatic deep-link.
- The status Filter menu belongs to the list page's leading/navigation toolbar
  area. Its collapsed label communicates the selected scope; counts remain in
  the menu, help, and accessibility value. It defaults to Open and contains
  Open, Rejected, Merged, and All with counts. Historical Approved records
  remain available through All; they do not retain a dedicated filter.
- The fixed list header provides native Author and Project menus. These filters
  combine with status and search. Do not add a Label filter until Labels exist
  in the Review domain model.
- Search is an independent window-level action and remains the trailing-most
  Review tool. Sync and decision actions are not grouped with Filter.
- Loading without cached Reviews uses a labeled `ProgressView`. Existing cached
  rows remain visible during refresh. Empty and filtered-empty states use
  `ContentUnavailableView`; a search miss uses the native search-empty state.
  Other filtered-empty states include `Clear Filters`, which resets status,
  author, Project, and search together.

The GitHub pull-request list informs the information order — title first,
scope/author/time second, and a small number of workflow signals — but not its
Web chrome. Do not copy blue links, colored pills, PR numbers, avatar stacks,
comment counters, or pagination. The
Server does not currently provide unread counts, unresolved-thread counts, or a
true last-activity timestamp, and the macOS client must not invent them from
`updatedAt`.

## 3. Review detail

The pushed detail contains an independent split:

```
Review metadata + overall update status
changed-file navigator | selected file unified diff
```

The file navigator reuses the path hierarchy, folder expansion, file symbols,
and native row styling extracted from the Memory file tree. It owns only Review
file selection; it must not inherit Memory rename, delete, or open side effects.

Build the tree from the Review's `drafts[]` metadata as soon as it arrives. Each
Draft contributes its final file path; operation history does not create extra
files. Load snapshot content and calculate the diff only for the selected file,
off the main thread. Share completed and in-flight commit requests within the
loaded Review revision. File loading and retry errors stay in the detail pane,
so the navigator remains usable. Replacing the Review revision or leaving the
page cancels its loader; late responses cannot replace the current selection.

The commit endpoint still returns a complete snapshot. Its first download is
required for the selected diff, but does not block the file tree. Client logs
record directory readiness and individual file load duration separately.

The main pane contains only information needed to make the decision:

1. title and plain status;
2. author, project, and update time;
3. description when present;
4. actionable stale/conflict state when present;
5. decision result and audit metadata after a decision;
6. unified diff.

Do not show a `Changes` heading or summaries such as `Create path · 20 changed
lines`. The file navigator already communicates the path and the diff directly
communicates insertions/removals. Delete-only and metadata-only Reviews retain a
short explicit empty state because the diff cannot communicate those outcomes.

The file tree marks behind files and detected conflicts. These markers only describe
individual files; selecting a current file never redirects an update action to
another file. The overall Review header and stale explanation sit above the split.

The author opens **Update Review** from the symbol toolbar. One independent native
window contains every behind file, with standard close, minimize, and zoom/full-screen
controls. The detail page remains usable in the main window. File selection stays
inside the Review update window, and a draggable divider separates comparison from
result. Closing, signing out, or quitting confirms unsaved resolution edits; saving
blocks closing. Applying updates refreshes the Review detail.

- Prepare all behind files against one shared reference. Show “Merges
  automatically”, “Needs resolution”, or “Resolved” for each file; omit a separate
  file-count summary strip.
- Show Remote and Draft conflict sections side by side as unified diffs against
  their common original, with each choice button beside its heading. Selecting either change
  updates the merged result below while preserving automatic changes elsewhere.
  Once content conflicts are chosen, the result can be edited before saving.
- Show path and deletion choices explicitly. Whole-file replacement is a secondary
  menu action that confirms replacement of the entire result.
- A file becomes ready when its choices are complete, its path is valid, and no
  generated conflict markers remain. There is no separate Mark Resolved step.
  Switching files preserves choices and edits.
- **Save All Drafts** sends the complete ordered Review draft set. The Server
  checks membership, versions, candidates, and the shared reference in one
  transaction. A failure on any file rolls back all updates.
- Applying updates does not publish. Reload the Review, remove the update tool
  when current, then enable the separate approval/publication action when the
  loaded detail is current and readable.
- Failed requests retain input. Check Latest Again explicitly warns before
  replacing edited resolutions. Cancel also confirms edited input; applying
  disables cancellation. Background detail refresh preserves the update model;
  an account/authority reset clears it and ignores late responses.
- Non-authors see that the author must update the Review.

## 4. Diff and comments

- Replacement rows render both the removal and insertion.
- Long lines scroll horizontally. Unchanged regions remain collapsed unless
  expanded or required to reveal an anchored comment.
- A line anchor is the final/new-side `(path, line)` pair. Removal rows are not
  comment targets until the API gains an explicit side field.
- A line thread renders immediately after its exact diff row. It is never moved
  to the top of the diff.
- Review-wide comments have no path or line. They live in an explicitly labeled,
  user-opened `Review comments` area and must never look like line feedback.
- Anchored comments for an earlier path remain discoverable in that area with
  their original `path:line` label instead of silently disappearing.
- Comment creation uses the version of the Review detail that produced the
  visible diff. If that version is stale, the strict Server contract rejects it
  and the detail reloads rather than anchoring a comment to unrelated content.

Historical comments created before the strict Server anchor deployment may be
General because the old Server discarded unknown path/line fields before the
client failed to decode its response. The client cannot infer the lost line;
the UI labels these honestly rather than pretending they are inline comments.

## 5. Toolbar decisions

The toolbar owns the whole-Review update action and decisions. All use icons
with tooltips and accessibility labels. Update Review uses
arrow.trianglehead.2.clockwise.rotate.90 and appears only for the author of a
behind, editable Review. It is disabled while updating. There is no update
button inside an individual file detail.

Decision actions retain menu-command parity:

- Open: Org owners/admins with `review:decide` and `review:merge` see Reject
  (`xmark`) and Approve (`checkmark`) in the standard toolbar style. Approve records the
  decision and merges into authority in one Server transaction; ordinary
  members remain read/comment participants and see neither authority action.
- Approved: historical records retain Merge (`arrow.triangle.merge`) when
  permitted and the Server supplied a nonempty approved result hash. Legacy
  approvals without that immutable result identity remain visible but cannot
  be merged.
- Rejected: Resubmit (`arrow.clockwise`) for the Draft author.

Filter belongs only to the list page. Decision tools belong only to an active
detail. Sync remains its own utility slot. Search remains independent and
trailing-most. Cross-section toolbar grouping and macOS 14-26 placement are
tracked as a separate workspace-wide design issue; Reviews must not reintroduce
one catch-all action group while that work is pending.

Reviews hide the redundant global `In Review` icon while retaining sync
progress, failures, and stale status.

## 6. State and accessibility

| State | Native handling |
| --- | --- |
| Loading | `ProgressView` |
| No Reviews/filter matches | Contextual `ContentUnavailableView` |
| Detail load failure | Explicit error and Retry; decisions stay unavailable |
| Stale/conflict | Concise semantic label and nearby action |
| Delete/metadata-only | Explicit main-pane result instead of an empty diff |
| Narrow window | Native outer sidebar behavior and toolbar overflow |

- Preserve system focus and link activation feedback; do not encode state by
  color alone.
- Every symbol-only control has `.help()` and an accessibility label.
- Verify list -> detail -> Back with mouse, keyboard, and Full Keyboard Access.
- Verify nested paths, long paths, CJK text, long diff lines, comments inside
  omissions, rename-only comments, stale details, and loading/error states.

## 7. Data boundary

A Review can contain multiple drafts from the server-provided `drafts[]`
metadata. Coordination is aggregated across its pending members. Update planning covers
every behind member; applying the complete ordered set is atomic. A discarded
member is detached and approval of the old set is invalidated. If the primary
member was discarded, the next surviving member becomes primary. When none
remain, the Review is rejected with its last member retained for history.
A database migration repairs previously stranded discarded memberships; the
client must not silently restore discarded content. The client does not infer
a multi-file commit history from draft operations. Legacy singular
detail fields remain supported by the current client contract.
