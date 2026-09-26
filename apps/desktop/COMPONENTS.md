# Components in the desktop client

The macOS client has a de-facto component library — the pieces it reuses across
screens — and this client has to answer the same questions for every one of
them: does the framework already ship it, do we compose it, or do we write it?
This file records the stack, the rule, and where each piece stands today.

## The stack

One dependency brings four layers, and knowing which layer a need belongs to is
most of the work:

| Layer | What it is | What it gives us |
| --- | --- | --- |
| `gpui_kit::*` | gpui-pre 0.3.6, the framework | Elements, layout, focus handles, actions and key bindings, text shaping, the window. |
| `gpui_kit::base` | gpui-base 0.6.6 | The theme and its tokens, the input engine (single-line, textarea, code editor with LSP), the tree, virtual lists, and the dialog, sheet and popover machinery. |
| `gpui_kit::component` | gpui-component 0.6.6 | The widget set: button, input, list, menu, tab, table, tree, dialog, sheet, notification, badge, avatar, spinner, markdown text, title bar, and about fifty more. |
| `gpui_kit::assets` | gpui-kit-assets 0.6.6 | The icon catalog and the asset source the SVGs resolve through. |

Beside it: `similar` (line-level diffing), `clumsiesd` (the daemon's wire
protocol, depended on rather than copied), and serde/reqwest/uuid for the
Server's own API.

We do not vendor or fork the library. Where its behaviour is wrong for us we
work around it in exactly one place and say so there — `components/fill.rs` is
the example — and a workaround good enough for everyone is worth offering
upstream rather than carrying.

## The rule

1. **The library first.** If `gpui_kit` ships the shape, use it, with theme
   tokens and no local copy. A second implementation of a button is a second
   button to keep in step.
2. **Compose in `apps/desktop/src/components/`** when the library has the
   pieces but not the arrangement, or when the arrangement is ours: a pane
   header, a tab chip, a filter chip, an empty state. A composition is a
   function, not a framework.
3. **Write a component** only when the library has nothing: today that is the
   diff view, the pane-filling measurement, and the reconciliation surfaces
   that come with Reviews.
4. **Port the tables, not the views.** What a macOS component *knows* — the
   metrics, the state-to-symbol-to-colour mappings, the sorting rules — is the
   part that carries over. Its AppKit plumbing does not, and this client's
   keyboard paths are its own.

## Where each macOS piece stands

Read from `apps/macos/Sources`; the right column is what this client does or
will do. Ordered the way the macOS client uses them.

| macOS piece (uses) | Its shape | Here |
| --- | --- | --- |
| `View.pageFeedback` + `feedbackHost` (16 files) | A screen reports a notice or failure without moving the layout; the window shows the newest one. | Compose. `ui::message` for a screen's own empty and failure states, the pane header's `header_status`/`header_notice` for what a document is doing, `logging.rs` for what nobody should see. A window-level corner is not built; the library's `notification` is the piece to build it on. |
| `SheetActionBar` (9 files) | Divider, progress label, Cancel and Confirm, bar background. | Library. `dialog`/`sheet` own their footers; the Review sheet uses them. Escape and Return come from the framework, so they are free. |
| `View.toolbarHelp` (6 files) | A tooltip that survives being hosted in the native toolbar. | Library. `tooltip::Tooltip`, built once per control. |
| `FormErrorMessage` (6 files) | A red line inside the form, with an optional Retry. | Compose. `ui::message(text, danger)` is the same line; a Retry belongs to the control that failed. |
| `ClassicSearchField` (5 files) | The one search box, with a focus token driven by Cmd-K. | Library. `input::Input` with a magnifier and its clear button, in the Memory list's header, filtering the tree in place. macOS keeps the same field in its window toolbar; this client keeps it in the list's own header, because its title bar carries no screen content. A palette over `command` is not built. |
| `ContentLoadingView` (5 files) | One centred spinner for a region awaiting its first result. | Library. `spinner`/`skeleton`; not built yet, and today a screen simply has nothing to show while it waits. |
| `UnifiedDiffView` + its metrics (4 files) | Monospaced diff, two or three gutters, hunk collapsing, inline comment threads. | Ours: `components/diff.rs`, on `similar`. Rows and colours are there; gutters, hunk collapse and comment threads are not. |
| `AvatarView`, `UserIdentityLabel` (3 files each) | Initials or image, one size ramp, one a11y element. | Library. `avatar`; the rail's foot draws an icon today. |
| `BrandLogoView` (3 files) | The brand mark, optionally breathing. | Ours later; it is an asset and a container. |
| `ReviewSymbolImage`, `ReviewStatusIndicator`, `InlineStatusBadge` (2 files each) | Review state as an icon, a colour and a capsule. | Port the table, use `badge`/`tag`. `ui.rs` holds the type and colour steps; the review-state table arrives with the Reviews screen. |
| `ToolbarFilterMenu`, `ProjectFilterMenu` (2 files each) | A funnel chip with the selected title, opening a menu. | Compose. `shell::project_picker` is already this chip; `popover` + `menu` are the library pieces behind it. |
| `PathTreeView`, `PathTreeRowLabel`, `FileSymbolView` (2 files) | A path-derived tree, one row shape, file-type icons. | Ours: `components/memory_tree.rs` builds the tree from paths; the library's `tree` draws it. Multi-select is ours on top, in `file_tree::Selection`: the set is the screen's, the painting and the modifier clicks are the component's. A context menu and inline rename are not built — the menu is Memory's business, and a rename is a dialog rather than a field in a row. |
| `MarkdownContentView`, `MarkdownPreview` (2 files each) | Markdown with the frontmatter split out, in a reading column. | Library. `text::TextView::markdown` with `FrontmatterPlugin`, wrapped in `components/markdown.rs`. |
| `NativeTextEditor` (1 screen) | The document body: plain text, undo, find, a centred column. | Library. `input::Textarea`, sized by `components/fill.rs`; the centred column is `DocumentContentMetrics`' idea and is not built. |
| `DocumentTabStrip` (1 screen) | Pill tabs, a close button revealed on hover, width-aware layout. | Compose, for now in `screens/memory.rs`: the library's `tab` has no per-tab close button. It moves to `components/` when a second screen wants tabs. |
| `FileTreeView` (1 screen) | The Memory navigator: multi-select, draft-coloured titles, rename, context menu. | Ours, partly. Selection works, one row or a set of them, with the row menu acting on the selection; one click both selects and expands (a known gap), and a draft is marked with a label rather than a colour. |
| `NativeAccountMenu` (1 screen) | The bottom-of-sidebar account control and its native menu. | Library. `menu` + `avatar`; the rail's foot is the place. |
| `ReviewRequestSheet` (3 files) | Title, description, batch confirmation, conflict steps. | Ours: `document::ReviewDialog` asks for one draft or for a whole selection's worth in a single Review; candidates and conflicts are unbuilt. |
| `ReviewCommentRow`, `ReviewCommentComposer` (2 files each) | One comment, and a 2–6 line composer. | Library pieces (`input`, `avatar`, `button`), composed when Reviews lands. |
| `DraftConflictView`, `DraftResolutionContent`, `DraftReconciliationView` (2 files each) | Choosing between remote and draft, per conflict. | Ours later. Nothing in the library is close; it is a domain surface and needs its own component. |

### Primitives worth porting as tables

These are not views and have no interactivity, so they port as Rust constants
and enums — the cheapest part of the macOS client to reuse and the part that
keeps two clients looking like one product:

- `WorkspaceSection` — the six destinations. Done: `shell::Section`.
- `DocumentContentMetrics` (reading width 760, minimum inset 36) and the tab and
  diff metrics — partly done: `ui.rs` holds the scale, `shell.rs` the widths.
- `MemoryTreeProjection` — items to rows. Done: `components/memory_tree.rs`.
- `TimestampFormatting` — absolute and relative timestamps. Not built; the
  Reviews and Activity screens need it.
- The Review status tables (state to title, symbol, tone) and the review queue's
  filter set. Not built; they arrive with the Reviews screen.
- `ClientDiagnostics` — redaction and export. Partly done: `logging.rs` records
  identifiers and never document text.
- The menu and shortcut table in `AppDelegate` — Cmd-W closes a tab, Cmd-[ and
  Cmd-] walk panes, Cmd-K searches. Ours are this platform's keys: Ctrl+W,
  Alt+Left and Alt+Right, F6; the search palette is not built.

## What the framework does differently, and why that is not a port

The macOS pieces that carry the most AppKit with them are the ones whose
behaviour this client has to re-express rather than translate:

- **First responder and focus.** SwiftUI and AppKit hand focus around through
  `NSViewRepresentable` and `@FocusState`. Here every region is a `FocusHandle`
  and the window owns the keys that move between them (`F6`), because a text
  editor consumes Tab.
- **Hover affordances.** NSCursor, hover-revealed close buttons and hover
  toolbars have no equivalent; a control that only appears on hover is a
  control a keyboard user cannot reach, so anything hidden here must still be
  reachable another way.
- **Preferences and environment injection.** `PreferenceKey` and
  `@EnvironmentObject` have no counterpart: state lives in `DesktopApp`, and a
  screen reports upward by calling it.
- **Native controls.** `NSSearchField`, `NSMenu`, `NSCollectionView` and
  `NSTextView` are the macOS client's chrome; here they are library widgets or
  compositions, themed from tokens rather than from the platform.

## Next, in the order the screens need them

1. A shared review-status table plus `badge`/`tag`, so Reviews and Activity can
   render the same states.
2. `components/tabs.rs`: the strip moves out of the Memory screen once Reviews
   wants document tabs of its own.
3. An empty/loading pair (`empty`, `spinner`) so a region that is waiting says
   so, which is what `ContentLoadingView` is for in macOS.
4. Diff parity: gutters, hunk collapsing, and the comment threads the review
   flow needs.
5. The window-level feedback corner, on `notification`.
