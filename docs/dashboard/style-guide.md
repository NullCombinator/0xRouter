# Dashboard style guide

Taken from 9router's code, light theme only (spec 009 FR-045 to FR-048). Paths are under `ref/9router/src`. This is the human-readable guide. `crates/nullrouter-dashboard/style/tokens.toml` holds every value as a token with its verified source line, and `tests/style_guide.rs` fails when a line no longer contains its value, when a line sits inside `.dark {}`, or when `dashboard.css` uses anything but tokens (FR-046).

Not taken: routes and data (the dashboard shows 0router's facts, not 9router's), the dark theme (FR-048), and anything that needs JavaScript (animations, copy buttons, hover-only content, live updates).

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
| Sidebar | vibrancy | rgba(255, 255, 255, 0.72) with a 20 px blur (`.bg-vibrancy`, `globals.css:308-312`). The `--color-sidebar` token exists but the sidebar does not use it. |
| Lines | border / border-subtle | #e5e7eb / #f1f1f3 |
| Text | text-main | #0a0a0a |
| Text | text-muted / text-subtle | #6B7280 / #9CA3AF |
| Status | danger / success / warning / info | #cf222e / #10B981 / #F59E0B / #3B82F6 |

The status tokens above exist but 9router's badges do not use them. Badges use Tailwind palette colors (see Status colors).

## Type

- Family: Inter, then the system stack (`globals.css`, `--font-sans`, line 199). Embedded in the dashboard, never fetched (FR-015).
- Page title: `text-base lg:text-2xl (24 px) font-semibold tracking-tight` (`Header.js`).
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
- Sidebar: 288 px wide (`w-72`), `border-r border-border-subtle`. Under 1024 px it is a fixed drawer that slides in over a `bg-black/20` overlay (`DashboardLayout.js`).
- Sidebar item: `px-3 py-1 rounded-lg gap-3`, list gap `space-y-0.5`, container `px-4 py-2` (`Sidebar.js`).

## Shadows

`--shadow-soft: 0 1px 2px 0 rgba(0,0,0,0.04)` on cards. `--shadow-warm: 0 2px 12px -2px rgba(229,106,74,0.18)` on the logo tile and hover cards. `--shadow-focus: 0 0 0 3px rgba(229,106,74,0.18)` for focus. `--shadow-elev` for raised cards (`globals.css` lines 60 to 67).

## Components

**Button** (`shared/components/Button.js`): `font-semibold`, `gap-2`, 150 ms transition. Sizes: sm `h-7 px-3 text-xs rounded-[8px]`, md `h-9 px-4 text-sm rounded-[10px]`, lg `h-11 px-6 text-sm rounded-[10px]`. Variants: primary (brand-500 fill, white text, hover brand-600), secondary (surface-2 fill, border), outline (border, hover surface-2), ghost (muted text), danger and success (not needed: the dashboard is look-only). The dashboard uses buttons only for sign-in and paging links.

**Card**: see Spacing and shape. Optional header row with an icon tile, title, subtitle and an action on the right.

**Badge** (`Badge.js` lines 6 to 17): pill (`rounded-full`), optional leading dot 6 px, optional icon.

**Input and Select** (`Input.js`, `Select.js`): `py-2.5 px-3 text-sm bg-surface-2 rounded-[10px]`, transparent border, focus ring `ring-2 ring-brand-500/30` with `border-brand-500/40`; label `text-sm font-medium`; hint `text-xs text-text-muted`; error `text-red-500`.

**Table** (usage `UsageTable.js`): header `uppercase text-xs text-text-muted` on a faint fill; cells `px-6 py-3`; numbers right-aligned in muted text; the key column `font-medium`.

**Navigation** (`Sidebar.js`): item `text-text-muted`, hover `bg-surface-2 text-text-main` and the icon turns primary (`group-hover:text-primary`), active `bg-primary/10 text-primary` with a filled icon. Logo tile 36 px, `rounded-[10px]`, brand gradient 500 to 700.

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

## Frame

**Sidebar** (`Sidebar.js:112`): `w-72` (288 px), `border-r border-border-subtle`, `.bg-vibrancy` (`globals.css:308` to 312: white at 72% under a 20 px blur). Brand row `px-6 py-4`: the 36 px gradient tile with `shadow-warm`, the name in `text-lg font-semibold tracking-tight`, the version in `text-xs text-text-muted`. Entries as under Navigation, `text-[13px] font-medium` with 18 px icons, in 9router's order and names. The "System" heading (`Sidebar.js:187`): `text-xs font-semibold uppercase tracking-wider`, muted text at 60%.

**Page header** (`Header.js:230` to 296): `px-8 pt-3 pb-2`, a bottom border in border-subtle; the page icon in primary at 24 px, the title `text-2xl font-semibold tracking-tight`, the subtitle `text-sm text-text-muted`. The dashboard adds an "as of" chip on the right: when the facts were read.

**Modal** (`Modal.js`): fixed, `z-50`, a black 50% backdrop under a 2 px blur; the box `max-w-4xl`, surface, `rounded-[14px]`, `shadow-elev`; the header `border-b` with the title `text-lg font-semibold` and a close button `p-1.5 rounded-[10px]`; the body `p-6`, at most `calc(85vh - 100px)` tall. The dashboard opens a modal by its address (a provider window, a record); the backdrop and the close button are links back.

**Side panel** (`Drawer.js`): 400 px, surface, `border-l border-border-subtle`; the header `p-6 border-b` with the title `text-lg font-semibold`; the body `p-6`. The dashboard keeps it in the page's flow on the right (Client adapters, Provider plugins) and folds it with a `<details>` chevron, so it needs no script.

**Floating button and panel** (new, from existing tokens): a 56 px round button at the bottom right in the brand tile's gradient and `shadow-warm`, `z-40`; above it, the housekeeping panel in the modal box's look (`max-w-sm`, `rounded-[14px]`, `shadow-elev`). It lists `check`'s notices and the accounts that need action.

**Background grid** (`globals.css:465` to 471, `.landing-grid`): 40 px cells of 1 px lines in the accent color at 8% opacity, fixed behind every page.

**Slot** (new, from existing tokens): a place kept for slice 2, in the card shape (`rounded-[14px]`, `p-6`) with a dashed border, muted text and no numbers (`combos/page.js:1067`'s dashed box).

**Disabled control with hint** (new, from existing tokens): a control the dashboard can't offer (FR-014) is drawn disabled at 50% opacity (`Button.js:7`), with the CLI command that does it beside it in the input hint style (`Input.js:61`).

## Page

- Background: `bg` (#FDFAF6), with the grid above.
- Content column: `max-w-7xl` (1280 px), centered, `p-10`, sections `gap-8`.

## Decisions

- The grid overlay is kept on every page: it is pure CSS, so it fits the no-script rule.
- One status → badge mapping, in `specs/009-dashboard/contracts/style-guide.md` "Status → badge". Each variant's source is `Badge.js:6` to 11, with its dot at lines 41 to 46.
- Buttons appear only for sign-in, filters and paging links, size sm (md for sign-in).
- The card radius is 14 px (`Card.js:29`). 10 px is the button, input and tile radius.
- The Overview and layout decisions live in `docs/dashboard/9router-inventory.md`.
