# 009 look review (T058, SC-008)

Side by side, as quickstart step 8 says: 9router's dashboard in light mode
(`cd ref/9router && DATA_DIR=/tmp/claude-1000/9r-data npm run dev`, then
`http://localhost:20127/dashboard`), the mockups in `docs/dashboard/mockups`, and this dashboard
(`nullrouter serve`, then `nullrouter dashboard token`). Each component is compared for color, type,
spacing and shape.

**Status: draft, not judged.** Claude prepared this from the code; it could not render this
dashboard (local cargo is off for this slice), so nothing below has been compared on screen yet.
The user fills the last two columns, and SC-008 passes only on the user's word.

## What the code already guarantees

- Every value in `dashboard.css` is a token from `style/tokens.toml`, and each token's source line
  in `ref/9router` contains its value (`tests/style_guide.rs`, green in CI #37).
- Each rule uses only the tokens of the 9router component its selector names.
- Checked while drafting: 9router's table header class `bg-bg-subtle/30` names a color its
  `globals.css` doesn't define, so 9router draws no header tint; ours has none either.

## Components

| Component | 9router source | Mockup | Difference seen | Fixed / accepted |
|---|---|---|---|---|
| Sidebar, nav item | `Sidebar.js:112`, `:167` | any page | | |
| Card | `Card.js:28` | `endpoint.html`, `providers.html` | | |
| Badge | `Badge.js:31` | `quota.html`, `providers.html` | | |
| Button (sm, md, primary, secondary) | `Button.js:35` | `providers.html` | | |
| Table | `UsageTable.js:163` | `usage.html` | | |
| Input | `Input.js:41` | sign-in page | | |
| Window (modal) | `Modal.js:47` | `providers.html` (provider window), `usage.html` (request window) | | |
| Side panel | `Drawer.js:51` | `endpoint.html`, `providers.html` | | |
| Housekeeping button and panel | `Sidebar.js:123`, `Modal.js:57` | any page with `?notices` | | |
| Background grid | `globals.css` | any page | | |

## Known departures (decided, not to be judged as defects)

- One light theme, no theme switch (FR-048).
- No animations, copy buttons or hover-only content: the dashboard runs no script.
- Disabled controls (Add, Test All, plugin switches, chat box) look disabled and carry a hint.
- Slots stand where the next dashboard slice's totals, graph and period filter will go.

## Verdict

SC-008: _not yet judged._
