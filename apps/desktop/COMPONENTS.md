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
protocol, depended on rather than copied), serde/reqwest/uuid for the Server's
own API, and chrono/iana-time-zone for the reader's calendar — the Dashboard is
bucketed in their time zone and answered in Unix seconds. Both were already in
the workspace's lock file through the Server's own use of them.

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
| `ClassicSearchField` (5 files) | Content search. | Deferred until the desktop search scope and behavior are defined. No search field or search icon is currently exposed. |
| `ContentLoadingView` (5 files) | One centred spinner for a region awaiting its first result. | Library. `spinner`/`skeleton`; not built yet, and today a screen simply has nothing to show while it waits. |
| `UnifiedDiffView` + its metrics (4 files) | Monospaced diff, two or three gutters, hunk collapsing, inline comment threads. | Ours: `components/diff.rs`, on `similar`. Rows and colours are there, and a line wider than the pane is reached by scrolling the view sideways rather than wrapping — the rows are virtualized, so a wrapped line would break their height. Gutters, hunk collapse and comment threads are not built. |
| `AvatarView`, `UserIdentityLabel` (3 files each) | Initials or image, one size ramp, one a11y element. | Library `avatar` when a screen names a member; the rail's foot draws the account mark, because 36 points of rail has no room for a name. |
| `BrandLogoView` (3 files) | The brand mark, optionally breathing. | Ported: `assets.rs` carries the mark in front of the library's icon set, and the sign-in page draws it at macOS's 40 points. The breathing state is not built: the mark only moves while the page is busy in macOS, and this client says what it is doing in words instead. |
| `ReviewSymbolImage`, `ReviewStatusIndicator`, `InlineStatusBadge` (2 files each) | Review state as an icon, a colour and a capsule. | Port the table, use `badge`/`tag`. `ui.rs` holds the type and colour steps; the review-state table arrives with the Reviews screen. |
| `ToolbarFilterMenu`, `ProjectFilterMenu` (2 files each) | A filter icon and selected project name, opening a checked menu. | `components/project_filter.rs` composes GPUI Button and PopupMenu in the list header, shared by Memory and Reviews. No content search is mixed into project selection. |
| `PathTreeView`, `PathTreeRowLabel`, `FileSymbolView` (2 files) | A path-derived tree, one row shape, file-type icons. | Ours: `components/memory_tree.rs` builds the tree from paths; the library's `tree` draws it. Multi-select is ours on top, in `file_tree::Selection`: the set is the screen's, the painting and the modifier clicks are the component's. A context menu and inline rename are not built — the menu is Memory's business, and a rename is a dialog rather than a field in a row. |
| `MarkdownContentView`, `MarkdownPreview` (2 files each) | Markdown with the frontmatter split out, in a reading column. | Library. `text::TextView::markdown` with `FrontmatterPlugin`, wrapped in `components/markdown.rs`. |
| `NativeServerAccessView` + `NativeServerAccessModel` (1 screen) | The sign-in page: the brand mark over a title, what the Server says it offers, the first-run fields, the Server address folded away, the failure in the form. | Ported: `screens/sign_in.rs` and `sign_in.rs`. Two ways in, both the Server's own answer: a local password (`password/sessions`), and the browser round trip (PKCE over a loopback listener, the tokens handed to the daemon). An invitation and a reset are the same call with a one-time credential, and the first run creates a local owner or hands itself to the identity provider. Google's mark travels with the button when that is the provider (`assets/google-g.png`, with its notice). The two links carry the theme's accent, which is what macOS's `Color.accentColor` is there — the library's text button draws the foreground colour, so the colour is asked for. Metrics stay the library's, per DESIGN.md's platform rule, and layout stays macOS's: the buttons fill the column the way `SignInButtonStyle` makes them, the title is the ramp's 20-point step, and the local fields name themselves in placeholders while the first-run fields carry labels — which is macOS's split, and leaves DESIGN.md's asterisk rule applying to the labels that exist. The Server's DTOs are snake_case and always have been; the mirrors here are pinned by tests, because a mirror that renames them reads nothing. Not built: macOS's administrator recovery, which signs in without the daemon — this client is built around it. |
| `DashboardPage`, `DashboardView` + Swift Charts (1 screen) | Six metric cards, six panels of charts and bars, a period picker in the window toolbar, an About sheet per panel. | Ported: `screens/dashboard.rs`. The page is macOS's: six cards across the top, the panels below two to a row, **no navigator column beside it** (macOS's split view is the sidebar next to that one page) and no page title (macOS's has none), so the Project filter and the period picker sit in the page's own header. The cards, the panels and the charts are composed — the library's `chart` draws one series per value and cannot stack a day's outcomes, so a day is a column here. Hovering a chart draws macOS's `RuleMark` and the day's numbers beside it; the pointer's day comes from one invisible cell per day rather than from measured geometry. Each panel names the peak its chart is scaled by instead of drawing a y axis. |
| `NativeAccountMenu` (sidebar foot) | The identity, then `Settings…`, then `Sign Out` under a rule. | Ported: the rail's foot, with the same three things in the same order. |
| `SettingsWindowView` + `SettingsNavigation` + `AccountSecurityView` | A window with a sidebar: the account block **is** the Account pane, then General, Agents, Support, and the Organization's administration for a role that grants it. The Account pane is what the account signs in with, with the password and the identity provider each opening a form in place. | Partly ported: `screens/settings.rs` is a dialog over the work — this client has one window — with the Account pane, General and Support. Both account changes answer with a fresh session, which goes to the daemon before it is reported, because changing a password signs every other session out. **Not built:** the Agents pane, the administration panes, and General's language and update controls, because this client has neither a translation nor an updater. |
| `DashboardModel.demoSnapshot` (1 screen) | A Dev Instance's `fixtures/dashboard.json`, read instead of the live statistics and badged "Demo data". | Ported: `engine::demo_snapshot`. The file is the macOS client's spelling, so its camelCase keys and `TimeInterval` seconds are translated once where it is read rather than kept in a second set of types. |
| `DashboardModel` + `DashboardSummary` (1 screen) | Two reads joined: the Server's published inventory, and the daemon's local retrieval telemetry. | Ported: `engine::dashboard` asks the Server through the proxy and the engine over its socket, with the Server's own day boundaries so both halves describe the same days. The daemon does not export the nested types of its answer, so the fields this client draws are mirrored in `engine.rs` and pinned by a test. |
| `NativeTextEditor` (1 screen) | The document body: plain text, undo, find, a centred column. | Library. `input::Textarea`, sized by `components/fill.rs`; the centred column is `DocumentContentMetrics`' idea and is not built. |
| `DocumentTabStrip` (1 screen) | Pill tabs, a close button revealed on hover, width-aware layout. | Compose, for now in `screens/memory.rs`: the library's `tab` has no per-tab close button. It moves to `components/` when a second screen wants tabs. |
| `FileTreeView` (1 screen) | The Memory navigator: multi-select, draft-coloured titles, rename, context menu. | Ours, partly. Selection works, one row or a set of them, with the row menu acting on the selection and reporting the draft's synchronization; a folder is opened by its own control or by Left/Right rather than by a click on the row, and a draft is marked with a label rather than a colour. |
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
- `TimestampFormatting` — absolute and relative timestamps. Started:
  `timestamps.rs` names the system's IANA zone and writes a local `Sep 26`,
  which is what the Dashboard's axes are labelled with. Relative timestamps are
  not built; Reviews still slices RFC 3339 text.
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

## Header geometry

`components/header.rs` owns the shared 44px pane row, 40px transparent, borderless
group, and 32px rounded controls. Project scope, document tabs, document tools
and project settings use this same composition; selected tabs use a fill, not
a separate border.

Surface roles are defined in `ui.rs`: page chrome surrounds the base content
canvas; header groups are transparent layout wrappers; active tabs use the
selected surface. Only interaction states add control backgrounds. All screens must use these roles instead of assigning background
tokens independently.

## File rows and menus

The reusable path tree composes GPUI Tree, ListItem, Icon, Tooltip and Tag.
Rows use the library's small text scale and radius, with folder/file icons,
truncated names and a trailing secondary status tag. Folder icons toggle expansion and show open/closed state without a separate
arrow; multi-selection uses the same ListItem color tokens as single
selection. Keep our path projection and selection behavior, not a second set
of widget styles.

All action and project menus use PopupMenu unchanged. The shared theme maps
its semantic popup surface at startup and on appearance changes, using GPUI's
existing colors; no per-menu palette or hand-mixed colors are allowed.
