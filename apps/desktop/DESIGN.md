# Desktop client design rules

The macOS client is the reference for *what* each screen does. This document is
about *how* the Windows and Linux client looks and behaves while we translate
it, so that every screen does not invent its own numbers.

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
5. **Write the keyboard path.** Anything reachable by mouse is reachable by
   keyboard, and the shortcut appears next to the command that has one.

## What the first pass violates

The prototype predates this document. Its known deviations, to fix as screens
are rebuilt:

| Deviation | Where | Fix |
| --- | --- | --- |
| Selected row painted with `rgb(0x2f3542)` | project list, tree tabs | `theme().list_active` / `list_hover` |
| Diff colors as literals | `components/diff.rs` | success / danger tokens, so dark mode works |
| Ad-hoc padding and gaps | every screen | `ui::` spacing steps |
| Ad-hoc text sizes | every screen | the type ramp |
| A development input probe in product UI | projects column | move behind a debug flag |
| Single click both selects and expands a tree folder | memory tree | chevron toggles, row selects — but the tree element owns that handler and exposes no separate toggle, so this waits on a component change or a custom row |
| No dark mode, no keyboard path, no empty states | all | part of each screen's rebuild |
