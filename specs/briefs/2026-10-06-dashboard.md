# Scope brief: dashboard (slice 1 of two) and dashboard summaries and landscape (slice 2)

Shaped with `/shape-spec` on 2026-10-06, in an Opus 5.5 `high` session. It replaces
`2026-10-05-dashboard.md`, whose four-page layout the user overrode. The user asked for the
**whole mockup** (`docs/dashboard/mockups`) and then accepted a cut into two slices in build order,
dropping nothing. Slice 1 uses only reads that exist; slice 2 adds the summaries the landscape and
the Usage cards need.

In `/speckit-clarify`, `/speckit-plan` and `/speckit-analyze`, an answer that contradicts a
confirmed row below means stop and revisit this brief. Don't accept it.

## Core map (at `main` `b541985`, with `007-dashboard` rebased on it)

| Capability | Status | Evidence |
|---|---|---|
| Provider plugins, unified models, four client API styles | shipped | 002, 003 |
| Request execution, retry, fallback, non-text types | shipped (live checks T060, T087, T096 open) | 003 |
| Account sign-in, quota polling | shipped (live checks open) | 005 |
| Cache-aware routing, per-agent isolation, windowed amortization | shipped, merged | 006, PR #1 |
| Request records with tokens, latency, agent | shipped | 006 journal |
| Shared read model; `unified`, `behaviour show`, `records list --before` | shipped, merged | 008 |
| Plugin-declared prices (input, output, cache, schedule) | shipped | `routing.price` in the registry schema |
| Latency and usage summaries | **absent** | → slice 2 |
| Dashboard | **absent** (mockups only) | → slice 1 |
| Latency trends over time | absent | → latency slice |
| Client-side adapters | specified, 0/98 built | 004 |
| Model tests, combos, quota fit | absent | → later |

## Playback (confirmed)

Slice 1 is the dashboard you open in a browser on this machine. You enter a token from the CLI
once, and read-only pages show your real data, in the mockups' look, inside 9router's sidebar:
Endpoint & Key (URL, keys as cards, a Client adapters panel that says "not built yet"), Providers
(cards, detail windows, every model kind, plugin logos shipped as small PNGs), Quota Tracker
(each account with pace, share and deficit), Usage (the Requests table; a row opens that
request's attempts), Settings, and Combo, Console Log and Proxy Pools as "not built yet". A round
housekeeping button lists current notices from `check`. You reload to refresh; nothing can be
changed here; it can never slow requests. The landscape, usage totals with Est. Cost and harness
tags are drawn as "arrives next" and come in slice 2.

## Ledger

| # | Tag | Claim | Source |
|---|---|---|---|
| 1 | U | The whole mockup is the target; cut into two slices in build order, nothing dropped | "Whole mockup", "Two slices, in build order" (2026-10-06) |
| 2 | C✓ | Look only; every change stays in the CLI | "Look only" (2026-10-06) |
| 3 | M | Rust, server-rendered, no scripts; served by the `serve` process on its own port, this machine only | brief 2026-10-05 rows 3, 4, 14 |
| 4 | M | Token issued by the CLI, asked once; on by default; until one is issued, one page names the command; the operator can turn it off | brief row 7; clarify 2026-10-05 |
| 5 | M | Light theme only, English only; look from a style guide taken from 9router's code; constant grid, icons, bright colours, title "0Router Proxy", translucent 288 px sidebar | brief rows 8, 12, 15; directions 2026-10-05 |
| 6 | C✓ | 9router's sidebar and page names. Overrides spec 007 FR-033, old brief rows 2 and 5, and init.md line 29 | "OK, override both" (2026-10-06) |
| 7 | C✓ | Combo, Console Log, Proxy Pools are "not built yet" pages | "OK, keep all three" |
| 8 | C✓ | Token Saver, CLI Tools, Translator, Skills, Media pages, Basic Chat are left out | "OK, left out" |
| 9 | M | Every fact a page shows has a CLI twin; a page shows everything its CLI reads show; every `check` item appears on the page it concerns | clarify 2026-10-05 |
| 10 | M | Each page shows an "as of" time, in this machine's time zone, named; reload to refresh | brief row 10; FR-017a |
| 11 | C✓ | Right panel only on Endpoint (adapters) and Providers (plugins) | "OK, only those two" |
| 12 | C✓ | Adapters panel says "not built yet", no sample rows | "Drawn, says not built yet" |
| 13 | C✓ | Providers buttons "Add … Compatible" and "Test All" are disabled with a hint | "Disabled with a hint" |
| 14 | C✓ | Kind filter lists every kind the plugins declare; "Decisions" is dropped until a plugin declares one | "OK, every declared kind" |
| 15 | C✓ | A plugin may ship a PNG logo, at most 64 KiB and 256 px; the core checks it; a bad logo is ignored, the plugin loads with its text icon, and `check` says why | "Plugin ships a logo file", "PNG only, 64 KiB, 256 px" |
| 16 | C✓ | Quota cards show pace, share and deficit; a Requests row opens a modal with that request's attempts, stay-warm decision and changes | "Quota cards and a record modal" |
| 17 | C✓ | Housekeeping panel lists current notices from `check` and the accounts; its chat box is disabled "not built yet" | "Current notices from check" |
| 18 | C✓ | Plugin switches (enable, disable, hide) are disabled "not built yet"; every plugin shows active | "Later, switches drawn as not built" |
| 19 | C✓ | Usage page shows only what the mockup draws: no extra charts or breakdown tables | "OK, as drawn" |
| 20 | C✓ | Slice-2 areas (landscape, stat cards, topology graph, period filter) are drawn as "arrives next" slots; no harness badge in slice 1 | "Drawn as 'arrives next' slots" |
| 21 | K | No secret or prompt shown; plugins see nothing new; a dashboard fault never slows or breaks client requests | constitution I; FR-012 |
| 22 | C✓ | Adds `dashboard token` and `dashboard status`, and a `subject` field on each `check` item (T011) | plan 2026-10-05; "Yes, matches" |
| 23 | C✓ | Slice 2: a read of latency over the last 24 hours per agent and per provider (router overhead, median and 95th-percentile time to first token, request counts, last response resolved or failed), with a CLI twin | "Summaries in, trends later"; "Last 24 hours, fixed" |
| 24 | C✓ | Slice 2: a read of request and token totals for Today, 24h, 7D, 30D, 60D, All, with a CLI twin | same |
| 25 | C✓ | Slice 2: Est. Cost uses the prices plugins already declare and the operator's account overrides, priced at the time of the request; unpriced requests are left out and counted; always "Estimated, not actual billing" | "Existing prices, unpriced left out" |
| 26 | C✓ | Slice 2: a key carries a free-text harness tag, `keys issue <name> --harness <text>`, shown in `keys list` and on agent cards; display only; no list of client names in the core | "Free-text label" |
| 27 | M | Slice 2: the landscape has agents on the left, the router in the centre, providers on the right, one pipe per agent and per provider, a gauge on each hop, a colour per agent | directions 2026-10-05 |
| 28 | M | Slice 2: the Usage page has a period filter, five stat cards (requests, input, cached, output, Est. Cost) and a circular topology graph | directions 2026-10-05 |
| 29 | U | Latency trends over time stay in the later latency slice | row 17 of the old brief, narrowed 2026-10-06 |

## P notes for research.md

- **The mockups hold invented sample content.** Don't ship it: "hermes · always on", the third-party
  adapter rows, "Pruned 214 request records" (0router has no pruning), the "Decisions" kind filter
  with no data, and the sample counts and gauge numbers.
- **`init.md` lists "decision" as a first-class model type.** It has no plugin data yet; the filter
  shows it once a plugin declares one (row 14).
- **Logos.** Bundled plugins are single `.toml` files, so the logo field needs a place to point
  (a file beside the plugin, embedded for bundled and community ones). The mockup's 146 PNGs come
  from 9router; all but one are within 64 KiB, and the 790 KB outlier needs shrinking. Serve with
  `X-Content-Type-Options: nosniff`, `img-src 'self'` in the CSP, and no SVG.
- **Router overhead** is derivable from a record: first attempt start minus arrival. Confirm in plan.
- **Price at request time.** Plugins' price schedules carry time-of-day rules, so the price at the
  request's time can be computed. A plugin edit after the fact is not tracked; say so on the card.
- **24-hour summaries over a large journal** need the same early-stop discipline as 008's records
  page and a bench on a 100,000-record journal. This is a plan-level target, not a scope row.
- **CLI twins to confirm in plan** for two facts the mockups draw: the endpoint URL (Endpoint &
  Key) and "where the data lives" (Settings). If no CLI command shows them today, add the fact to
  an existing read (for example `check`) rather than drop it, as the 2026-10-05 direction says.
- **Dashboard token and cookie** decisions from the old plan (research R6 to R8, open Low L1)
  carry over to the new plan.
- **Spec directory.** Speckit numbers specs itself (next is `009`); `specs/007-dashboard` is the old
  four-page spec and is retired when slice 1 is specified. Its `tasks.md` T011 (`subject`) carries
  over into this brief's row 22. `init.md` line 29 is updated by this brief's commit.
- **Claude Design** can't make a design (`DesignSync` only syncs design-system projects). Not used.
- **Not decided here:** per-model prices, plugin enable/disable/hide, a housekeeping agent runtime,
  writes from the dashboard, live refresh, panels on Usage, Quota Tracker and Settings. All later.

## Final command: slice 1

```
/speckit-specify Dashboard: a read-only web view of 0router in a browser, so the operator can see their endpoint, keys, providers, quota, routing and requests on pages that look like the dashboard mockups. It is written in Rust, server-rendered with no scripts, and served by the running `nullrouter serve` process on its own port, bound to this machine only. It is on by default. Until the operator issues a dashboard token with `nullrouter dashboard token`, its only page names that command; the operator can turn the dashboard off, and `nullrouter dashboard status` shows its state. The browser asks for the token once; no other page opens without it. The dashboard is look only: every change (accounts, keys, priorities, behaviour, plugins) stays in the CLI. Its look is 9router's: a style guide extracted from 9router's code (colors, type, spacing, component styles) is written into this repo and the styling is built from it, with one light theme, English only, the constant background grid, icons and bright colors. Its sidebar and page names are 9router's (Endpoint & Key, Providers, Combo, Usage, Quota Tracker, and under System: Proxy Pools, Console Log, Settings) under the title "0Router Proxy"; this replaces spec 007's four-page layout. Every fact a page shows has a CLI command that shows the same fact, a page shows everything its matching CLI reads show, and every `check` warning, note and error appears on the page it concerns. Each page shows an "as of" time in this machine's time zone, named, and the operator reloads to refresh. Endpoint & Key shows the endpoint URL and the agent keys as cards (as `keys list` shows them), and a Client adapters side panel that says client-side adapters aren't built yet. Providers shows provider cards, a detail window per provider with its accounts and all its models under a filter that lists every model kind the plugins declare, and a Provider plugins side panel; the buttons "Add … Compatible" and "Test All" and the plugin enable, disable and hide switches are disabled and say they aren't built yet or name the CLI command. A plugin may ship a PNG logo, at most 64 KiB and 256 px; the core checks it, and a bad logo is ignored so the plugin still loads with its text icon, and `check` says why. Quota Tracker shows every account with its sign-in status, quota with reset times (polled or estimated), priority, and its pace, share and deficit. Usage shows the Requests table, newest first, 50 at a time, with the reason for each placement, latency and result; a row opens a window with that request's attempts, stay-warm decision and changes. The traffic landscape, the usage stat cards and topology graph, and the period filter appear as slots that say they arrive with the next dashboard slice. Settings shows the operator's behaviour settings, where the data lives, and the dashboard's own status. A round housekeeping button opens a panel that lists the current notices from `check` and the accounts; its chat box is disabled and says it isn't built yet. Combo, Console Log and Proxy Pools are entries that say the feature isn't built yet and name what exists instead. This slice adds `nullrouter dashboard token`, `nullrouter dashboard status` and a subject on each `check` item so notices reach their page. A page never shows a secret or a prompt, plugins see nothing they didn't before, and a fault in the dashboard never slows or breaks client requests. The slice fails if: a page disagrees with the CLI for the same moment; the look visibly departs from the style guide taken from 9router. Visual reference only (where it disagrees with this text, this text wins): docs/dashboard/mockups. Out of scope: changing anything from the dashboard, including plugin switches, adapter review and the housekeeping chat → later; the traffic landscape and gauges, usage totals per period, Est. Cost, the topology graph, the period filter, and the harness tag on keys → the next dashboard slice (summaries and landscape); latency trends over time → the latency slice; extra Usage charts and breakdown tables → later; live auto-refresh → later; side panels on Usage, Quota Tracker and Settings → later; reaching the dashboard from another machine, HTTPS and network binding → later; translations → later; client-side adapters → slice 004; model tests and combos themselves → later, with their own slices; fitted quota weights and leak detection → later; 9router's Token Saver, CLI Tools, Translator, Skills, Media pages and Basic Chat → not planned. Scope brief: specs/briefs/2026-10-06-dashboard.md
```

## Final command: slice 2

Run after slice 1 ships. Re-read this brief first; if slice 1 changed a row, revisit it.

```
/speckit-specify Dashboard summaries and landscape: fills the slots the dashboard left for them, with real numbers, and the CLI shows the same numbers. Two new read-only CLI views, each with --json: latency over the last 24 hours per agent and per provider (router overhead, median and 95th-percentile time to first token, request counts, and whether the last response resolved or failed), and request and token totals for a chosen period (Today, 24h, 7D, 30D, 60D, All). The Usage page fills its period filter, its five stat cards (requests, input, cached, output, Est. Cost) and its circular topology graph. Est. Cost uses the prices plugins already declare and the operator's account overrides, priced at the time of each request; requests with no price are left out of the total and counted on the card; it is always labelled "Estimated, not actual billing". The Endpoint page fills the traffic landscape: agents on the left, the router in the centre, providers on the right, one pipe per agent and per provider, a gauge on each hop, a colour per agent, with gauge numbers and request counts for the last 24 hours, labelled so. An agent key gains a free-text harness tag: `nullrouter keys issue <name> --harness <text>`, shown in `keys list` and on the agent cards; it is display only, existing keys show none, and the core keeps no list of client names. The dashboard stays look only and without scripts; every number a page shows equals the CLI's for the same moment; a page never shows a secret or a prompt; a fault in the dashboard never slows or breaks client requests. The slice fails if: a page disagrees with the CLI for the same moment; the look visibly departs from the style guide taken from 9router. Out of scope: latency trends over time → the latency slice; extra Usage charts and breakdown tables → later; per-model prices → later; changing anything from the dashboard → later; live auto-refresh → later; client-side adapters → slice 004. Scope brief: specs/briefs/2026-10-06-dashboard.md
```

## Trace

| Sentence of the slice-1 command | Rows |
|---|---|
| Read-only web view, pages that look like the mockups; endpoint, keys, providers, quota, routing, requests | 1, 2, 5 |
| Rust, server-rendered, no scripts; `serve` process, own port, this machine only | 3 |
| On by default; token from the CLI; only page names the command; can be turned off; `dashboard status`; asked once | 4, 22 |
| Look only; every change stays in the CLI | 2 |
| Style guide, light theme, English, grid, icons, colours | 5 |
| 9router's sidebar and names, title, replaces the four-page layout | 5, 6 |
| CLI twin for every fact; everything the CLI reads show; `check` items on their page | 9 |
| "As of" time, time zone, reload | 10 |
| Endpoint & Key: URL, key cards, adapters panel | 11, 12, playback (URL) |
| Providers: cards, detail window, kind filter, plugin panel, disabled buttons and switches | 11, 13, 14, 18 |
| Plugin logo and its limits | 15 |
| Quota Tracker with pace, share and deficit | 9, 16 |
| Usage Requests table and the record window | 9, 16 |
| Slice-2 slots on Endpoint and Usage | 20 |
| Settings | 9, playback |
| Housekeeping button and panel | 17 |
| Combo, Console Log, Proxy Pools | 7 |
| New commands and the `check` subject | 22 |
| No secret or prompt; plugins see nothing new; faults never slow requests | 21 |
| Failure list | 9, 5 |
| Visual reference sentence | 1 (mockups); P notes (invented content) |
| Out of scope: writes and switches | 2, 17, 18 |
| Out of scope: landscape, totals, Est. Cost, graph, filter, harness tag → slice 2 | 20, 23 to 28 |
| Out of scope: trends → latency slice | 29 |
| Out of scope: extra charts; live refresh; panels; network; translations; adapters; tests/combos; quota fit | 19, 10, 11, 3, 5, 12, 7 |
| Out of scope: 9router pages left out | 8 |

| Sentence of the slice-2 command | Rows |
|---|---|
| Fills the slots, same numbers as the CLI | 20, 9 |
| Latency view per agent and per provider, 24 hours | 23 |
| Totals per period | 24 |
| Usage filter, five cards, topology graph | 28 |
| Est. Cost basis and labels | 25 |
| Landscape, gauges, last 24 hours | 23, 27 |
| Harness tag | 26 |
| Look only, no scripts, equals the CLI, no secrets, no slowdown | 2, 3, 9, 21 |
| Failure list | 9, 5 |
| Out of scope | 29, 19, 2, 10, 12 |
