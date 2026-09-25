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
