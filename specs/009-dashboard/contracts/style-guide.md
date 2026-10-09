# Contract: Style guide

Two files, which agree:

- `docs/dashboard/style-guide.md`: for people. Sections: Palette, Type, Spacing, Radii, Shadows,
  Page (background, grid, content column), Components, Status colors, Icons, What is not taken.
- `crates/nullrouter-dashboard/style/tokens.toml`: for the build and the tests.

## Components

From 9router: sidebar (translucent `.bg-vibrancy`, 288 px) with its entries, group heading and
current-entry mark; page header; button (primary, secondary, disabled); card; badge; table;
input and select; modal window; side panel with its collapse chevron; round floating button and
its panel; empty state.

Built from 9router's components for 0router's pages: provider card, quota card (window bars),
key card, request row. Each names the 9router component it composes.

New, with no 9router precedent, from existing tokens only: the **slot** (card style, dashed
`border`, muted text) and the **disabled-control hint** (muted small text beside a disabled
button).

## `tokens.toml`

```toml
schema = 1
tailwind = "4.x (ref/9router/package.json)"

[token.color-brand-500]
value = "#E56A4A"
source = "src/app/globals.css:24"

[token.radius-card]
value = "14px"
source = "src/shared/components/Card.js:29"
class = "rounded-[14px]"

[token.sidebar-width]
value = "288px"
source = "<sidebar component file>:<line>"
class = "w-72"

[component.badge-success]
uses = ["green-600", "green-500-10", "text-xs", "space-1", "space-2-5"]
source = "src/shared/components/Badge.js:8"
```

Rules (tested; SC-007):
1. Every token has `value` and `source`, a file and line under `ref/9router` that contains the
   value or, when `class` is set, the class.
2. Light theme only: no source line is inside `.dark { … }` (FR-048).
3. `style/tokens.css` is generated from this file and never hand-edited; a test regenerates it
   and fails on any difference.
4. `style/dashboard.css` uses only `var(--<token>)`, CSS keywords, `0`, and percentages in
   `width`/`flex` layout. Every `var(--x)` names a token. Every component class is a
   `[component.*]` entry and uses only that entry's tokens.

## Status → badge

| Status shown | Badge |
|---|---|
| active, signed in, polled, served, loaded | success |
| needs sign-in, refused, failed, records not kept, error notice | error |
| cooling, stale, pending first poll, estimated, fallback, warning notice | warning |
| pay-as-you-go, disabled, revoked, default, never, not built yet | default |
| in progress, note notice | info |

Each variant's 9router source is recorded in the guide (FR-047).

## Not taken from 9router

- The dark theme and its switch (FR-048).
- Animations and anything that needs a script (copy buttons, hover-only content, live updates).
- Sample content drawn in the mockups (spec, Terms).
