# Contract: Style guide

Two files, which agree with each other:

- `docs/dashboard/style-guide.md`: for people. Sections: Palette, Type, Spacing, Radii, Shadows,
  Components (Button, Card, Badge, Table, Input, Select, Navigation, Sidebar, Empty state, Page
  header), Status colors, What is not taken from 9router.
- `crates/nullrouter-dashboard/style/tokens.toml`: for the build and the tests.

## `tokens.toml`

```toml
schema = 1
tailwind = "4.x (ref/9router/package.json)"   # the scale Tailwind classes resolve against

[token.color-brand-500]
value = "#E56A4A"
source = "src/app/globals.css:24"            # relative to ref/9router

[token.radius-card]
value = "14px"
source = "src/shared/components/Card.js:29"
class = "rounded-[14px]"                     # present when the value came from a Tailwind class

[token.green-600]                            # Tailwind palette colors resolve against the v4 theme (oklch)
value = "oklch(62.7% 0.194 149.214)"
source = "src/shared/components/Badge.js:8"
class = "text-green-600"

[component.badge-success]
uses = ["green-600", "green-500-10", "text-xs", "space-1", "space-2-5"]
source = "src/shared/components/Badge.js:8"
```

Rules (tested; SC-005):
1. Every token has `value` and `source`. `source` names a file and line under `ref/9router`.
   That line contains `value` or, when `class` is set, the class.
2. Only light-theme values. No token's source line is inside the `.dark { … }` block of
   `globals.css` (FR-034).
3. `style/tokens.css` is generated from this file (`:root { --<name>: <value>; … }`) and is
   never hand-edited. A test regenerates it and fails on any difference.
4. `style/dashboard.css` may use only `var(--<token>)`, CSS keywords, `0`, and percentages in
   `width`/`flex` layout. Every `var(--x)` names a token. Every component class in the CSS is a
   `[component.*]` entry, and uses only that entry's tokens.

## Status → badge

9router's badges use Tailwind palette colors (`bg-green-500/10 text-green-600` and so on), not
its `--color-success` tokens. The guide records the palette colors the badges actually use.

| Status shown | Badge | 9router precedent |
|---|---|---|
| active, signed in, polled, served | success | Badge `success` variant |
| needs sign-in, refused, failed, records not kept | error | Badge `error` variant |
| cooling, stale, pending first poll, estimated, warning | warning | Badge `warning` variant |
| pay-as-you-go, disabled, revoked, default, not built yet | default (neutral) | Badge `default` variant |
| in flight, info notes | info | Badge `info` variant |

The exact variant names and their sources are recorded in the guide (FR-032).

## Not taken from 9router

- Page structure, navigation entries and routes: 0router's own (FR-033).
- The dark theme (FR-034).
- Animations and hover-only affordances that need JavaScript.
