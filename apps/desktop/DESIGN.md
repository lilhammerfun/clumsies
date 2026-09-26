# Desktop client design rules

The macOS client is the reference for *what* each screen does. This document is
about *how* the Windows and Linux client looks and behaves while we translate
it, so that every screen does not invent its own numbers.

COMPONENTS.md is the companion for *what to build it out of*: the framework's
layers, the rule for choosing between them, and where every reusable piece of
the macOS client stands here.

## Where the rules come from

| Platform | Reference |
| --- | --- |
| Windows | [Windows 11 app design](https://learn.microsoft.com/en-us/windows/apps/design/) — Microsoft's counterpart to the Apple HIG. Corner radii, type ramp and best practices below are quoted from it. |
| macOS | The Apple HIG, already embodied in `apps/macos`. We do not restyle macOS. |
| Linux | There is no single HIG. We follow the component library's own token system and the desktop's font and light/dark preference instead of inventing a third look. |

## Windows rules that bind us

### Geometry

| Corner radius | Use |
| --- | --- |
| 8px | Top-level containers: windows, dialogs, flyouts |
| 4px | In-page elements: buttons, inputs, list rows, bars |
| 0px | Straight edges that meet other straight edges |

### Type ramp

Sizes are px / line height. Weight is Regular for text, Semibold for titles.

| Style | Weight | Size |
| --- | --- | --- |
| Caption | Small | 12 / 16 |
| Body | Text | 14 / 20 |
| Body Strong | Text semibold | 14 / 20 |
| Body Large | Text | 18 / 24 |
| Subtitle | Display semibold | 20 / 28 |
| Title | Display semibold | 28 / 36 |
| Title Large | Display semibold | 40 / 52 |

Windows states a floor: **12px Regular and 14px Semibold are the smallest
legible sizes**, because smaller text breaks in some languages. That floor
matters more for us than for most apps, since every screen carries Chinese.

Other stated practices: left-align by default, use sentence case for all UI
text including titles, and prefer ellipsis over clipping.

### Color

Windows ships light and dark modes; both must work. Depth comes from surface
lightness — less important surfaces are darker, more important ones lighter —
not from extra borders.

## What the component library already gives us

Do not hardcode a color, a size or a radius that the theme has a token for. The
theme carries 98 semantic color tokens (`background`, `foreground`, `border`,
`primary`, `muted`, `muted_foreground`, `link`, `danger`, `list_hover`,
`list_active`, `ring`, …), plus:

| Token | Meaning |
| --- | --- |
| `radius` | general radius — our 4px controls |
| `radius_lg` | dialogs and overlays — our 8px containers |
| `font_family`, `font_size` | the UI font, 16px base |
| `mono_font_family`, `mono_font_size` | platform-appropriate mono (Menlo, Consolas, DejaVu Sans Mono), 13px |
| `mode` | light / dark, switchable at runtime |
| `focus_ring` | focused controls draw a ring outside their border |

Because both renderers read the same tokens, following them is also what keeps
Windows and Linux looking like the same product.

## Spacing

Spacing comes from a 4px grid, stepped 4 / 8 / 12 / 16 / 24 / 32. Named steps
live in `src/ui.rs`; screens use those names rather than picking a padding
per call site.

## Motion

Windows states the timing and easing rather than leaving them to taste, and
adjusts both to the purpose. The two we need:

| Purpose | Easing | Duration | Used for |
| --- | --- | --- | --- |
| Bare minimum | Linear | 83ms | Opacity |
| Direct entrance | cubic-bezier(0, 0, 0, 1) | 167ms | Position, scale, rotation |
| Direct exit | cubic-bezier(0, 0, 0, 1) | 167ms | The same, **always combined with a fade** |
| Existing elements | cubic-bezier(0.55, 0.55, 0, 1) | 167–333ms | Point-to-point movement |

So: hover and press feedback is a 83ms opacity change; a panel or overlay
enters at 167ms and leaves at 167ms with a fade. Nothing in this client should
animate for longer than a third of a second.

## Navigation structures

Windows names two and says when each fits: **flat/lateral** when the pages are
peers with no parent-child relationship and there are fewer than eight, and
**hierarchical** when pages have a parent and there are more. Two rules come
with it:

- **Do not go deeper than two levels** without a breadcrumb, or the user is
  stranded in a hierarchy they cannot leave.
- **Avoid "pogo-sticking"**: needing to go up a level and back down to reach
  related content means the structure is wrong.

Where this client sits: the rail is the flat level (Projects, and later
Reviews, Inbox, Settings as peers). The Memory screen is a **list/details**
pair, which is a second level but not a hierarchy to descend — the tree beside
the document. Adding a third pane of *navigation* would need a breadcrumb, so
the rule to keep is: no more than one navigable level inside a screen.

## List and details

Windows describes the pattern, and — the part worth copying — when it changes
shape. The two panes are side by side only while there is room for both:

| Available width | Style |
| --- | --- |
| 320–640 epx | **Stacked**: one pane at a time, selecting an item drills into it |
| 641 epx or wider | **Side by side**: the list keeps a selection visual and the detail pane follows it |

The Memory screen is side-by-side at every width today; below 641px it should
stack and grow a back affordance. This is a real gap, not a stylistic
preference, because the client runs in windows the user resizes.

**A tree row is selected the platform's way.** A plain click selects one row and
opens its document; Ctrl-click (Cmd on macOS) adds a row to the selection or
takes it out, and Shift-click takes everything between it and the row the range
grew from. Right-clicking a row outside the selection makes that row the
selection first, and right-clicking one inside it keeps the selection, so the
menu always acts on what the reader can see selected. The menu splits the way
macOS's does: the commands every file list has — open, rename, delete — and then,
after a divider, the ones only Memory knows, a Review and a discard. A command
that would change several documents at once says how many it would change, and
the ones that cannot be undone name the documents in the dialog they ask with.

## Forms

Windows is specific about forms, and the sign-in screen is one:

- **Mark required input with an asterisk on the label.**
- **Disable the submit action until the required input is filled.** A form that
  can be submitted invalid is a form that will be.
- Use the right control so invalid input is impossible, and a placeholder to
  show the expected shape.
- When a value is rejected, **mark the field and keep the user on the form**;
  do not clear what they typed.
- **Errors belong where they happened.** A field's problem is shown at the
  field; a connection problem is a message in the form. Windows says explicitly
  not to use a dialog for contextual errors.

The one step that leaves the app is the browser round trip to the identity
provider, and the form says so before it happens.

## Dialogs

- The title is the instruction and is optional; the content is the description
  and is required; there is always at least one button.
- **Escape maps to the safe action**, always, and the safe action is the
  rightmost button. The primary action is leftmost.

For this client that means: a discard or destructive confirmation can be a
dialog, and Escape must dismiss it without doing the destructive thing.

## The window

The window is one shell, and every screen fills two slots in it. macOS draws the
same shape as a NavigationSplitView in WorkspaceView: a sidebar of six
destinations, the open section's navigator beside it, and the work itself in the
detail. This client draws, from the top:

| Region | What it is |
| --- | --- |
| Title bar | The window's, not a screen's: the page navigation at the left and the window controls at the right, and nothing else. |
| Rail | The six destinations of the macOS sidebar, **icons only**, each named in a tooltip. Memory is the brain, as it is in macOS. |
| List column | The open section's list, under a header row carrying its filter — the Project picker here, which macOS calls MemoryProjectFilter and keeps in the same place — and, at the far right, at most one command rarer than the work itself. No heading: the rail already says which section this is. |
| Detail | The work, under **one** header row: the open documents as tabs on the left, the tools for the one in front on the right. |

**A document opens to be read.** The pane's default is the prose, and editing
and diffing are tools a reader turns on rather than a mode switch they start in;
turning one off returns to reading. The rare commands (request a review, and
whatever joins it) live behind the overflow at the end of that row, which is
where macOS keeps its Memory Actions menu.

**Every pane carries its own header row.** A pane header — WinUI calls the
control a command bar, VS Code a view header — holds the commands and the facts
that act on that pane, at the top of it. The window's title bar therefore stays
empty of screen content: a command belongs beside the region it acts on, and a
reader looking at the tree or at the text finds the commands for it directly
above what they are looking at. macOS keeps the document's name and its view
switch in the window toolbar instead; that is the one place this client
deliberately differs, for the reason above.

**Documents are tabs.** Memory's detail opens one tab per document — macOS's
`DocumentTabStrip`, which macOS draws at the top of its main pane in the same
place — and each tab holds the whole session: its own text, its view mode, its
draft, and what the engine has done with it. A tab the reader has left keeps its
text, so switching between two documents never loses an edit. The tab in front
is the one the tree marks, the header commands act on, and a keystroke is stored
against; an editor reports each keystroke itself, so a store follows the tab it
was typed in rather than whichever tab is in front by the time the pause ends.

The window's arrows walk the reader's history, which is macOS's
`navigationBackStack` and `navigationForwardStack`: opening or picking a tab
pushes where the reader came from and empties the forward stack, closing a tab
drops it from both, and an arrow with nowhere to go is drawn disabled. The two
stacks hold tabs, so the arrows only ever move between documents that are open.

A screen owns its list and its detail and nothing else about the layout. A
section with no screen yet says so in both slots, and names the macOS view it
will be translated from.

Three rules belong to the shell so that no screen repeats them:

- **A narrow window adapts once.** Below 641 epx the list stacks above the
  detail. No screen decides this for itself.
- **The window controls are drawn when the platform does not.** The component
  library skips them under server-side decorations; some compositors answer that
  request with "server" and then draw only a border, Hyprland among them, so the
  application draws minimize, maximize and close itself in that case.
- **The keyboard reaches every region.** F6 moves the keyboard to the next
  region of Memory's window — the list, the work, the actions over it — and
  Shift+F6 to the previous one; the focused region shows a ring. Enter or Space
  runs the focused action. F6 is the Windows key for moving between a window's
  regions, and it is the only way into the list and the actions at all, because a
  document editor consumes Tab. Inside the list the arrow keys move the
  selection, which is what opens a document; the document keys are this
  platform's — Alt+Left and Alt+Right for the history, Ctrl+Tab to cycle the open
  documents, Ctrl+W to close the one in front.

The theme follows the system's light or dark preference, and keeps following it:
`Theme::sync_system_appearance` is called when the window opens and on every appearance
change. Tokens come from the theme, so nothing here is a literal in one mode and
wrong in the other.

## Rules for translating a macOS screen

1. **Read the macOS screen first**, and write down what the user can see and do
   there. The information architecture is not negotiable; it is the same
   product.
2. **Re-express it in the platform's metrics.** Windows: Fluent radii, the type
   ramp above, Fluent control heights. Linux: the component library defaults.
3. **Use theme tokens, never literals**, so light and dark both work.
4. **Keep the platform's own interaction conventions.** Where Windows and macOS
   disagree about how a control behaves — a tree row, a context menu, a
   keyboard shortcut — follow the platform, because that is what the user's
   hands already know.
5. **Write the keyboard path where it earns its place.** Anything a reader does
   all day is reachable by keyboard, and the shortcut appears next to the command
   that has one. Not every command needs one: the ones a reader uses rarely, and
   the ones whose keyboard form would mean inventing a widget the component
   library does not have, wait until they are asked for. The product owner's call
   (2026-09-26) is that the keyboard is not a priority for this client yet, so
   what exists stays — it is also how this client gets tested on a machine that
   cannot synthesize pointer events — and nothing new is added for its own sake.

## Known gaps

Places where the client does not yet meet the rules above, with the reason and
the fix. Anything that is merely unbuilt is not listed: a screen that does not
exist yet is not a deviation.

| Gap | Where | Why, and what fixes it |
| --- | --- | --- |
| Long diff lines are clipped, not wrapped | Diff tab | The rows are virtualized, so a variable-height row would break the window. Fix: a horizontal scroll region sized to the longest line. |
| Single click both selects and expands a tree folder | memory tree | The chevron should toggle while the row selects, but the tree element owns that handler and exposes no separate toggle, so this waits on a component change or a custom row. |
| A folder cannot be expanded from the keyboard | memory tree | The arrow keys move the selection, and Enter on a folder does nothing: the tree component expands a folder in its own click handler and exposes no command for it. Fix: a component change, or a custom row that toggles. |
| A document that exists only as a proposal is not in the tree | memory tree | `project_checkout` lists published resources, and a draft that creates a file has no resource yet, so `New file…` writes a draft the tree never shows. Fix: build the row from the draft — `list_drafts` carries the path it would be created at — and send the create operation for its later edits, so that discarding it is what deleting it means. |
| Closing a tab discards unsaved text without asking | document strip | macOS asks before closing a tab whose text the Server has not accepted. This client stores after a 600ms pause and closes straight away, which loses at most that pause. Fix: a confirmation when the pane is dirty. |
| Tabs are not restored between runs | document strip | macOS keeps its tabs in the workspace model, which this client does not have yet. Fix: remember the open documents with the selected Project. |
