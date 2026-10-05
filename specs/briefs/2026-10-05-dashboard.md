# Scope brief: dashboard (slice A of two)

Shaped with `/shape-spec` on 2026-10-05. The user chose the dashboard over Claude's
recommendation (model tests and latency view), building 004 first, or combos. In
`/speckit-clarify` and `/speckit-plan`, an answer that contradicts a confirmed row below means
stop and revisit this brief. Don't accept it.

## Core map (at `224a11b`, branch `006-routing-decision`)

| Capability | Status | Evidence |
|---|---|---|
| Provider plugins, unified models, four client API styles | shipped | 002, 003 |
| Request execution, retry, fallback, informational errors, non-text types | shipped (live checks T060, T087, T096 open) | 003 |
| Account sign-in, quota polling | shipped (live checks T045, T056, T078, T097 open) | 005 |
| Cache-aware routing, per-agent isolation, windowed amortization | shipped, not merged to `main` | 006 97/99; T085 and T091 open |
| Persistent request records with latency | shipped, not merged | 006 journal |
| Latency summaries and trends | absent | → slice B |
| Dashboard | absent | → **this slice** |
| Model tests, combos, nested combos | absent | → later (placeholders here) |
| Harness adapters (hermes, WASM) | specified, 0/98 built | 004 |
| Quota fit, leak detection | absent | → later, no slice number |
| Community sign-in | absent | → later |

## Playback (confirmed)

The dashboard is a read-only web view, written in Rust and served by the same `nullrouter
serve` process on its own localhost-only port. You get a dashboard token from the CLI and the
browser asks for it once. Its look is 9router's, from a style guide taken from 9router's code
(colors, type, spacing, component styles); the layout is new. English only. Four pages:
accounts and quota, routing and requests, models and providers, keys and behaviour. Model
tests and Combos appear as placeholders saying the feature isn't built yet. You reload to
refresh, and each page shows an "as of" time. Latency is whatever records already carry. The
dashboard can't slow client requests. It fails if a page disagrees with the CLI or the look
drifts from the style guide. Slice B follows: latency summaries with trends, in the CLI and
dashboard together.

## Ledger

| # | Tag | Claim | Source |
|---|---|---|---|
| 1 | U | Shape the dashboard next | "Dashboard with claude design" |
| 2 | U | Look from 9router, layout changes a lot | "Same look as 9router but we have so many changes in layout" |
| 3 | U | UI written in Rust | "I want the UI be written in rust too" |
| 4 | C✓ | Server-rendered (axum + Rust templates), no WASM | "Server-rendered Rust" |
| 5 | C✓ | Four pages: accounts and quota; routing and requests; models and providers; keys and behaviour | all four selected |
| 6 | U | Look only; changes stay in the CLI | "Look only" |
| 7 | C✓ | Localhost only, dashboard token issued by the CLI | "This machine only, with a token" |
| 8 | C✓ | Look fixed by a style guide from 9router's code; Claude Design optional, later | "Style guide from 9router's code" |
| 9 | U | Fails if it disagrees with the CLI, or doesn't look like 9router | picked both |
| 10 | C✓ | Reload to refresh, with an "as of" time | "No, reload to refresh" |
| 11 | C✓ | Network and HTTPS access → later | "OK, later slice" |
| 12 | C✓ | English only | "OK, English only" |
| 13 | C✓ | Model tests and Combos shown as placeholders | "Visible placeholder pages" |
| 14 | C✓ | Same process, own port, isolated from the request path | "OK, same process, own port" |
| 15 | C✓ | Look = tokens and components, not pixel copies | "OK, tokens and components" |
| 16 | U | Latency view with trends is wanted | "No, add a latency view", "Add trend over time" |
| 17 | C✓ | Split: dashboard first (existing latency data), latency summaries with trend second | "Two slices: dashboard first" |
| 18 | K | Secrets and prompts never shown; plugins see no secrets | constitution Principle I |
| 19 | M | Latency is surfaced in the dashboard | constitution VIII |
| 20 | C✓ | Quota fit and leak detection: later, slice number no longer fixed | "Yes, compose the command" after the renumber note |

## P notes for research.md

- 9router's tokens live in `ref/9router/src/app/globals.css` (brand scale around `#E56A4A`) and
  its components in `ref/9router/src/shared/components/` (Button, Card, Badge, Input, Modal,
  Drawer and others). Extract tokens and component styles only; don't copy page structure.
- Data comes from the operator socket and engine state that the CLI already uses, so
  "agrees with the CLI" can be tested by comparing both against one snapshot.
- Dashboard faults must stay off the request path: a separate task and listener, bounded work.
- Token handling: stored and protected like the operator's other secrets; the browser cookie
  or header scheme and CSRF are plan decisions. The dashboard has no write routes.
- The user did not pick "leaks or opens up" or "can change something" as failure signals.
  Both still stand as constraints (rows 6, 7, 18).
- Claude Design: `DesignSync` only syncs design-system projects through the user's
  `/design-sync` skill; it can't make a design. Not used.

## Final command

```
/speckit-specify Dashboard: a read-only web view of 0router, so the operator can see accounts, quota, routing, requests, models and keys in a browser. It is written in Rust, server-rendered, and served by the running `nullrouter serve` process on its own port, bound to this machine only. The operator gets a dashboard token from the CLI and the browser asks for it once; no page opens without it. The dashboard is look only: every change (accounts, keys, priorities, behaviour) stays in the CLI. Its look is 9router's: a style guide extracted from 9router's code (colors, type, spacing, component styles such as buttons, cards, badges and tables) is written into this repo and the dashboard's styling is built from it; the layout is new and not 9router's. The dashboard is English only. It has four pages: accounts and quota (accounts, sign-in status, which account needs signing in again, polled or estimated quota with reset times, priority); routing and requests (the routing view with pace, share and deficit, and request records with the reason for each placement, latency and token usage); models and providers (providers and plugins, unified models and their members, model types, the limits notes); keys and behaviour (agent access keys and the operator's behaviour settings). Model tests and Combos appear as entries that say the feature isn't built yet and point to the CLI. Each page shows the state at load time with an "as of" time, and the operator reloads to refresh. Latency appears as the records already carry it. A fault in the dashboard never slows or breaks client requests. A page never shows a secret or a prompt, and plugins see nothing they didn't before. The slice fails if: a page disagrees with the CLI for the same moment; the look visibly departs from the style guide taken from 9router. Out of scope: latency summaries and trends over time → the next slice (latency view, CLI and dashboard together); acting from the dashboard (changing accounts, keys, priorities or behaviour) → later; reaching the dashboard from another machine, HTTPS and network binding → later; translations and language switching → later; live auto-refresh → later; model tests and combos themselves → later, with their own slices; fitted quota weights and leak detection → later. Scope brief: specs/briefs/2026-10-05-dashboard.md
```

## Trace

| Sentence | Rows |
|---|---|
| Purpose; read-only; Rust, server-rendered; same process, own port, this machine only | 1, 3, 4, 6, 7, 14 |
| Token from the CLI, asked once, no page without it | 7 |
| Look only | 6 |
| Style guide, 9router's look, new layout | 2, 8, 15 |
| English only | 12 |
| Four pages and their contents | 5 |
| Placeholders for Model tests and Combos | 13 |
| "As of" time, reload to refresh | 10 |
| Latency as records carry it | 17, 19 |
| Dashboard faults never slow requests | 14 |
| No secret or prompt shown; plugins see nothing new | 18 |
| Failure list | 9 |
| Out of scope: latency trends | 16, 17 |
| Out of scope: acting, network, translations, live refresh | 6, 11, 12, 10 |
| Out of scope: tests/combos themselves, quota fit and leak detection | 13, 20 |
