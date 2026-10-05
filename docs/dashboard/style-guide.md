# Dashboard style guide (draft baseline)

Taken from 9router's code, light theme only (spec FR-030 to FR-034). Paths are under `ref/9router/src`. This is the human-readable baseline written before slice 007's `tokens.toml` exists. T022 turns every row into a token with a verified source line, and a test fails if a line no longer contains its value. Where a line number is not given below, T022 fills it in.

Not taken: page structure, navigation entries, routes, the dark theme, and anything that needs JavaScript (animations, hover-only affordances).

## Palette (light)

Source: `app/globals.css`, the `:root` block (about lines 14 to 67). The `.dark` block (about lines 72 to 116) is not used.

| Role | Token | Value |
|---|---|---|
| Brand | brand-50 / 100 / 200 / 300 / 400 | #fdf1ed / #fadccf / #f4b59c / #ee8d6a / #ea7855 |
| Brand | **brand-500** (primary) | #E56A4A |
| Brand | brand-600 (hover) / 700 / 800 / 900 | #cc5236 / #a64027 / #7a2f1d / #4d1e12 |
| Page | bg | #FDFAF6 |
| Page | bg-alt | #F7F3EE |
| Card | surface | #ffffff |
| Fills | surface-2 / surface-3 | #f4f4f5 / #e7e7e9 |
| Sidebar | sidebar | rgba(244, 241, 236, 0.85) |
| Lines | border / border-subtle | #e5e7eb / #f1f1f3 |
| Text | text-main | #0a0a0a |
| Text | text-muted / text-subtle | #6B7280 / #9CA3AF |
| Status | danger / success / warning / info | #cf222e / #10B981 / #F59E0B / #3B82F6 |

The status tokens above exist but 9router's badges do not use them. Badges use Tailwind palette colors (see Status colors).

## Type

- Family: Inter, then the system stack (`globals.css`, `--font-sans`, line 199). Embedded in the dashboard, never fetched (FR-011).
- Page title: `text-base lg:text-2xl font-semibold tracking-tight` (`Header.js`).
- Card title: `font-semibold` at the base size; subtitle `text-sm text-text-muted` (`Card.js`).
- Nav label: `text-[13px] font-medium` (`Sidebar.js`).
- Table header: `uppercase text-xs text-text-muted` (usage `UsageTable.js`).
- Badge: `text-xs` (md), `text-[10px]` (sm), `text-sm` (lg), always `font-semibold`.
- Code and keys: the monospace stack at `globals.css` line 594.
- Icons: Material Symbols Outlined, 18 px in nav and buttons, 20 px in card headers, 14 px in badges.

## Spacing and shape

- Card: `rounded-[14px]`, `border border-border-subtle`, `bg-surface`, padding `p-6` (md), `p-4` (sm), `p-3` (xs), `p-8` (lg) (`Card.js`, radius at line 29).
- Card section (nested panel): `p-4 rounded-[10px] bg-bg border border-border-subtle`.
- Card icon tile: `p-2 rounded-[10px] bg-bg`, icon 20 px.
- Radius scale in use: 8 px (small button), 10 px (button, input, select, tile), 14 px (card), full (badge).
- Sidebar item: `px-3 py-1 rounded-lg gap-3`, list gap `space-y-0.5`, container `px-4 py-2` (`Sidebar.js`).

## Shadows

`--shadow-soft: 0 1px 2px 0 rgba(0,0,0,0.04)` on cards. `--shadow-warm: 0 2px 12px -2px rgba(229,106,74,0.18)` on the logo tile and hover cards. `--shadow-focus: 0 0 0 3px rgba(229,106,74,0.18)` for focus. `--shadow-elev` for raised cards (`globals.css` lines 60 to 67).

## Components

**Button** (`shared/components/Button.js`): `font-semibold`, `gap-2`, 150 ms transition. Sizes: sm `h-7 px-3 text-xs rounded-[8px]`, md `h-9 px-4 text-sm rounded-[10px]`, lg `h-11 px-6 text-sm rounded-[10px]`. Variants: primary (brand-500 fill, white text, hover brand-600), secondary (surface-2 fill, border), outline (border, hover surface-2), ghost (muted text), danger and success (not needed: the dashboard is look-only). The dashboard uses buttons only for sign-in and paging links.

**Card**: see Spacing and shape. Optional header row with an icon tile, title, subtitle and an action on the right.

**Badge** (`Badge.js` lines 6 to 17): pill (`rounded-full`), optional leading dot 6 px, optional icon.

**Input and Select** (`Input.js`, `Select.js`): `py-2.5 px-3 text-sm bg-surface-2 rounded-[10px]`, transparent border, focus ring `ring-2 ring-brand-500/30` with `border-brand-500/40`; label `text-sm font-medium`; hint `text-xs text-text-muted`; error `text-red-500`.

**Table** (usage `UsageTable.js`): header `uppercase text-xs text-text-muted` on a faint fill; cells `px-6 py-3`; numbers right-aligned in muted text; the key column `font-medium`.

**Navigation** (`Sidebar.js`): item `text-text-muted`, hover `bg-surface-2 text-text-main`, active `bg-primary/10 text-primary` with a filled icon. Logo tile 36 px, `rounded-[10px]`, brand gradient 500 to 700.

**Empty state**: icon tile, a bold title, a muted one-line hint, centered in a card (seen on Combos and Quota).

## Status colors

| Variant | Classes | Used for |
|---|---|---|
| default | `bg-surface-2 text-text-muted` | pay-as-you-go, disabled, revoked, default, not built yet |
| primary | `bg-brand-500/10 text-brand-600` | emphasis, not a status |
| success | `bg-green-500/10 text-green-600` | active, signed in, polled, served |
| warning | `bg-yellow-500/10 text-yellow-600` | cooling, stale, pending first poll, estimated |
| error | `bg-red-500/10 text-red-600` | needs sign-in, refused, failed |
| info | `bg-blue-500/10 text-blue-600` | in flight, notes |

The `dark:` classes in the source are dropped.

## Corrections this extraction made to slice 007's documents

- The style-guide contract's example used `radius-card = 10px` from `Card.js:40`. The card radius is **14 px**, at `Card.js:29`. 10 px is the button, input, and tile radius. The example in `specs/007-dashboard/contracts/style-guide.md` needs fixing.
- The faint grid behind pages is `.landing-grid` in `app/globals.css` (about lines 464 to 471): 40 px cells of 1 px lines in the brand color at 8% opacity. A soft coral glow, `.dot-grid-bg` (lines 451 to 456), also exists. Both are decoration; they are listed under Page below.

## Page

- Background: `bg` (#FDFAF6), with the optional grid overlay above.
- Content column: centered, about 1000 px wide on the pages seen (Endpoint, Usage, Combos), with the sidebar fixed at about 240 px on the left.

## Open

- Whether to keep the grid overlay (it is pure CSS, so it fits the no-JS rule). SC-006 is judged side by side, so you decide when you see both.
- The Overview and layout decisions live in `docs/dashboard/9router-inventory.md`.
