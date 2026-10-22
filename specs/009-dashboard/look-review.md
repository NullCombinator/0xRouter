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
| Sidebar, nav item | `Sidebar.js:112`, `:167` | any page | rows 4px shorter than drawn | fixed: `nav-item-leading` 24px (e0820dd) |
| Card | `Card.js:28` | `endpoint.html`, `providers.html` | provider cards taller, title 16px | fixed: padding 12/16, title 14px, grid gap 12 (e0820dd) |
| Badge | `Badge.js:31` | `quota.html`, `providers.html` | revoked key drawn as a badge in the mockup | accepted: plain text, a test forbids a badge without a harness |
| Button (sm, md, primary, secondary) | `Button.js:35` | `providers.html` | Add/Test All drawn enabled | accepted: disabled with hints (FR-014) |
| Menu | `HeaderMenu.js:81` | `providers.html` (All kinds) | absent | fixed: details menu, no script |
| Table | `UsageTable.js:163` | `usage.html` | recent list used "in N out N" and clock times | fixed: `in↑ out↓` in colour, "2s ago" |
| Input | `Input.js:41` | sign-in page | none seen | |
| Window (modal) | `Modal.js:47` | `providers.html` (provider window), `usage.html` (request window) | none seen | |
| Side panel | `Drawer.js:51` | `endpoint.html`, `providers.html` | tab missing | fixed (95d152b) |
| Housekeeping button and panel | `Sidebar.js:123`, `Modal.js:57` | any page with `?notices` | none seen | |
| Background grid | `globals.css` | any page | none seen | |

## Page by page against the mockups (2026-10-21, Claude in Chrome, 1568x762)

Branch `look/mockup-parity` (95d152b, 22aece6, 5e3fe44, 8fddbc8, e0820dd), sample home with placeholder secrets.

- Every page: content column starts 24px under the header as drawn (was 40); empty tiles tightened; sidebar logo.
- Endpoint & Key: compact key cards, URL field fills the row, landscape as glass pipes with dials, collars and a legend, provider logos (text icon without one), gradient avatars ringed in each agent's colour, the logo mark in the router . Departures: latency and request count printed at each dial rather than on hover (FR-027: hover may show nothing the page doesn't show); "requests today N" (FR-029a wording); the agent's harness under its name (010 FR-026).
- Providers: search right-aligned with the "All kinds" menu (a details menu of links, `?has=<kind>`, the kinds `providers` prints), boxed empty row, compact cards. Departure: the Add and Test All buttons are disabled with their hints (FR-014); the mockup's OAuth section has no counterpart in the fixture's categories.
- Usage: Overview / Details control (Details jumps to the Requests table), five stat columns, segmented period control, framed topology with logos and the router mark, Recent Requests as `in↑ out↓` in colour. Recent Requests times read "2s ago" from the page's as-of (the Requests table keeps the full local time, FR-023). Departures: no animated edges or zoom buttons (no script, FR-027).
- Quota Tracker: several notices fold under one counted line; footer boxed; the "Show" button stays (no script to submit the filters).
- Combo, Proxy Pools: as drawn. Console Log: hint as drawn, the "Open Usage" button dropped.
- Settings: a rule under each head, plain monospace values in Dashboard, token hint under its label. Departures: the Status row stays (it carries off and not-listening states); no description under `break_behaviour` (`behaviour show` prints none); no dot in the routing badge.
- Sidebar shows the CLI's version (FR-026) and the "as of" line names the zone (FR-023).

## Known departures (decided, not to be judged as defects)

- One light theme, no theme switch (FR-048).
- No copy buttons or hover-only content: the dashboard runs no script. Motion is CSS only: the Endpoint pipes carry moving bubbles (user, 2026-10-21), stopped under `prefers-reduced-motion`.
- Disabled controls (Add, Test All, plugin switches, chat box) look disabled and carry a hint.
- Slots stand where the next dashboard slice's totals, graph and period filter will go.

## Verdict

SC-008: _not yet judged._

## Providers and the side panels (user asks of 2026-10-21, done)

- Side panels on Providers and Endpoint & Key follow the mockup: a head with a search (disabled,
  "not built yet") and a gear; rows with logo, set · category, and a status badge; no switches in
  the rows. Endpoint & Key lists hermes as built in and points to `nullrouter adapters list` for
  third-party adapters.
- The gear opens a settings window over the open panel (CSS checkbox toggle, no script, no form):
  "Provider plugin settings" (Show on page / Enabled switches, Uninstall, install row) and "Client
  adapter settings" (hermes, Add an adapter). Every control is disabled and says where it lives
  in the CLI.
- The provider window follows the mockup's pm6: slim head, big logo and name, "N connections · M
  models", a Connections card with the last response, Available Models with kind chips (zero
  counts dimmed) and compact model rows. Departures: no "Get API Key" link and no "schema N" (no
  view carries them); the panel rows show the category instead of a model count for the same
  reason.

## Quota Tracker (user ask of 2026-10-21: "does not look the same", done in 6594c18)

- Filters are the mockup's pills: "All providers" and "All accounts" open menus of links, "Expiring first" is a link that turns on and off. The native selects and the Show button are gone.
- The card head has the sign-in badge with a small line under it: "polled 18:40", "estimated from usage", "pay-as-you-go" or "quota not reported".
- Windows are 9router's compact rows: a coloured dot, the name, a 4 px bar, the CLI's figure with the percent left beside it in the bar's colour, and the reset. Colours follow 9router's bounds (green above 70 %, yellow from 30 %, red below); they were 50/20 before.
- A card with no window says why, centred under an icon. An account that needs signing in shows the red error icon and the command, as in the mockup.
- Departures: the targets (pace, share, deficit), the "priority · order · kind" line and the amortization window stay, because `routing` and `accounts` show them. There's no "Page 1 / 1": the page doesn't paginate. The scratch home has no successful poll, so the rows were checked with sample rows put into the page in the browser, not with live data.

## Combo, Proxy Pools, Console Log, Settings (2026-10-21, compared after Quota Tracker)

- Combo, Proxy Pools and Console Log match their mockups. The only difference is the faint background on the code in Combo's hint, which is the dashboard's style for inline code everywhere.
- Settings: `break_behaviour` now has its one-line description and its value is in a badge with a dot, as in the mockup (commit after 6594c18). The Dashboard card keeps its "Status" and "Address" rows, which `dashboard status` shows and the mockup leaves out.
