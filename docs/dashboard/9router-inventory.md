# 9router dashboard inventory

Source: `ref/9router/src/app/(dashboard)/dashboard/` and `src/shared/components/Sidebar.js`, read at the ref SHA the fixtures use. Screenshots of 9router running locally are noted per section when taken.

Each section carries the **proposal** (keep, drop, change, add) that was put to the operator. They are now decided by [spec 009](../../specs/009-dashboard/spec.md) (see "Decided" below); where the spec differs from a row, the spec wins. Where the constitution already settles it, the row says so.

Status of 0router counterparts: **have** = a CLI command exists today; **007** = planned in slice 007; **later** = named in a brief row but not in 007; **none** = no plan.

## What 9router's sidebar shows

Main: Endpoint & Key (also the home page), Providers, Combo & Vision Adapter, Usage, Quota Tracker, Token Saver, CLI Tools.
Media providers: Embedding, Image, Video, TTS, STT, Web Fetch & Search, SystemOne.
Debug: Console Log, Translator (hidden unless enabled in settings).
System: Proxy Pools, Skills.
Footer: Profile (settings), an update banner, a link to 9english.net, a promo modal for NineRemote.
Hidden in code: Basic Chat, PXPIPE, MITM page.

## Section by section

| # | 9router section | What it does | 0router counterpart | Proposal | Why |
|---|---|---|---|---|---|
| 1 | **Endpoint & Key** (home) | Shows the local endpoint URL and API keys (create, delete, "require API key" toggle); tunnel and Tailscale Funnel to reach the dashboard from outside | `keys list` (have); client base URL (have, in docs) | **Change** | Keep: endpoint URL and the key list (name, created, last used, revoked). Drop: tunnel and Tailscale (brief: remote access is later). Drop: create and delete (look-only). |
| 2 | **Providers** | Per-provider connections (OAuth and API key), add, bulk add, test all, auto-ping, proxy pool per connection, capabilities, "allow China-hosted models" | `accounts list`, `providers`, `plugins list` (have) | **Change** | Keep: status badges, which account needs sign-in, capabilities. Drop: add, edit, bulk add, test (look-only; tests are a later slice). Re-cut by 0router's entity model: one provider = one plugin with per-modality sections. |
| 3 | **Combos & Vision Adapter** | Named model lists with fallback or round-robin, drag to reorder, judge model for "fuse" panels; vision adapter | none (combos later) | **Change** | Combos stay a "not built yet" entry in 007. Round-robin and "judge fusion" conflict with 0router's routing decision, so propose dropping them. Vision adapter: decide (it rewrites request content; Principle IV needs a record of every change). |
| 4 | **Usage** (tabs: Overview, Logs, Details) | Token totals by provider and model, cost estimate, charts per period; request log table; per-request payload in four stages (client request, provider request, provider response, client response) | `records list/show` (have); latency summaries (next slice) | **Change** | Keep: request table with reason for placement, latency, tokens (007 `/records`). Add later: per-provider and per-model summaries and trends (the latency slice). Drop: the four-stage payload view, because pages never show prompts (FR-029). Cost estimate: decide (needs a price table; "Estimated, not actual billing" in 9router). |
| 5 | **Quota Tracker** | Per-account quota bars, reset times, refresh, sort, filter by status, "Codex reset credit expiry" | `quota` (have) | **Keep** | Already in 007's accounts page. Drop: Codex-specific credit expiry modal unless the plugin carries it. |
| 6 | **Token Saver** and **PXPIPE** | Compresses tool results and images before sending upstream | none | **Drop** | Constitution: optimizer surfaces are out of scope. |
| 7 | **CLI Tools** | Detects installed clients (Claude, Codex, Cline, Kilo, OpenCode, Hermes, ...), writes their config to point at 9router, MCP list, MITM interception with DNS tricks and a certificate | harness tests in `tests/harness`; hermes adapter (built in) | **Change** | Drop: MITM and DNS interception (not 0router's model; large security surface), and writing configs (look-only). Candidate **add**: a read-only "Connect a client" page with the base URL, key placeholder and per-harness snippets, plus the harness adapters in use. |
| 8 | **Console Log** | Live server log stream over SSE | `records` (have); a log command: none | **Drop for now** | Live streaming is later; records cover what happened to requests. Revisit with live refresh. |
| 9 | **Translator** | Debug view of request translation between formats | none (the wire crate records changes per Principle IV) | **Drop** | Shows prompts. Candidate for later: a record's "what we changed" list, which carries no content. |
| 10 | **Proxy Pools** | Deploy relays on Cloudflare, Vercel, Deno; proxy lists; strict mode | none | **Drop** | Not core routing. Outbound proxy, if wanted, is a config setting, not a page. |
| 11 | **Skills** | Marketing page: "paste this to your AI", GitHub link | none | **Drop** | Not an operator surface. |
| 12 | **Profile** (settings) | Password, SSO (SAML, OIDC), language, routing strategy (round robin, sticky), observability toggle, database location, outbound proxy | `behaviour show` (007), `check` (have) | **Change** | Keep: behaviour settings and where data lives (read-only). Drop: password and SSO (the dashboard token replaces them), language (English only), routing strategy toggles (the routing decision is fixed by design), observability toggle. |
| 13 | **Media providers** (embedding, image, video, TTS, STT, web) | A page per modality with provider connections, a test form, example requests | `unified`, `model`, `providers` (007) | **Change** | Constitution III: unified models and model types, not per-modality pages. Drop the per-modality pages and the test forms (tests are a later slice). Keep the idea of showing kind and limits per model. Web fetch and search: decide whether 0router has them at all. |
| 14 | **Basic Chat** | Chat box (hidden in 9router) | none | **Drop** | Hidden upstream; a write surface. |
| 15 | **Footer chrome** | Update banner, version check, external promo links | none | **Drop** | Phones home. 007 fetches nothing outside the machine (FR-011). |

## Candidates to add (not in 9router)

| Idea | Counterpart | Slice |
|---|---|---|
| Overview home: what needs attention now (accounts needing sign-in, cooling accounts, `check` warnings, dashboard status) | `check` (have) | 007, if you want it |
| Latency view: per-provider and per-unified-model summaries, trends | records | next slice |
| Routing explain: pace, share, deficit per account, with the reason for each placement | `routing` (have) | 007 |
| Record detail: attempts, stay-warm decisions, the changes made to a request (no content) | `records show` (have) | 007 |
| Plugin review state: installed, bundled, community, harness adapter review | `plugins list` (have) | 007 partial |
| Model tests (testable combos) | none | later |

## Decided

[Spec 009](../../specs/009-dashboard/spec.md) settles the questions this section used to ask:

1. **Rows 1 to 15.** The pages and their contents are FR-026 to FR-040 and the Out of Scope list. Row 3: Combo is an entry that says combos aren't built and names `[[unified_model]]` and `nullrouter unified` (FR-037); combos, round-robin, Fusion and the vision adapter come later in their own slices. Row 4: Usage shows the Requests table and the request window; the stat cards, Est. Cost, the topology graph and the period filter are slots that arrive with the next dashboard slice. Row 7: no "Connect a client" page; Endpoint & Key shows the endpoint URL and a Client adapters side panel (FR-027). Rows 8 and 10: Console Log and Proxy Pools stay in the sidebar as entries that say they aren't built yet and name what exists instead. Row 13: Token Saver, CLI Tools, Translator, Skills, Media pages and Basic Chat are not planned.
2. **Landing page.** No Overview page: `/` opens Endpoint & Key, as in 9router. What needs attention is the housekeeping panel (User Story 6) and the notices above each page.
3. **Sidebar.** 9router's names under the title "0Router Proxy": Endpoint & Key, Providers, Combo, Usage, Quota Tracker, then under "System" Proxy Pools, Console Log and Settings (FR-026). This replaces spec 007's four-page layout.

## Seen running (9router v0.5.86, local, light mode, empty data)

Pages opened: Settings, Providers, Endpoint, Usage, Quota Tracker, Combos, Token Saver (text only), CLI Tools (still loading). Media providers and Console Log were not captured.

- The sidebar is longer than the code's `navItems`: it also shows **Media Providers** (collapsible), **9Remote** and **9English**, plus a **Donate** button and an "update available" banner with an `npm i -g` command. All are outside 0router's scope.
- **Endpoint** is a single card with the local URL, a Tunnel button and a Tailscale button, then a key list with a per-key on/off switch. Two "Default Key" rows appeared on first load.
- **Providers** is a grid of cards grouped OAuth, Free Tier, then API key; each card shows only a name and "No connections" or a status. It is a catalogue of what exists, not a status board.
- **Usage** has tabs Overview and Details (no Logs tab), five total tiles (requests, input, cached, output, estimated cost), a provider flow map, a recent-requests panel, and Tokens/Requests/Cost chart tabs.
- **Quota Tracker** is empty without OAuth connections.
- **Combos** has three strategies, not two: Fallback, Round Robin and **Fusion** (asks all models in parallel, a judge model merges the answers, and the page warns it bills every panel model plus the judge). The Vision Adapter covers images and also **audio**.
- **Token Saver** has four optimizers: RTK, Headroom (shows "Running"), Caveman (terse output prompt) and Ponytail (minimal-code prompt). All are dropped.
- **Settings** has a Light/Dark/System switch, backup download and import, language, require login, SSO, routing strategy, outbound proxy, observability, shutdown and logout.
- The look: cream page background with a faint grid, white cards with a hairline border, coral `#E56A4A` primary, a pale coral active nav item, small uppercase section labels. Card-heavy and airy.

Two changes to my table from this: row 3 gains "Fusion" (drop it; it multiplies cost and is not a routing decision), and row 12 gains backup, import and shutdown (drop: write actions).

## Not yet done

- The style guide is extracted: [style-guide.md](style-guide.md), with its tokens in `crates/nullrouter-dashboard/style/tokens.toml` (spec 009 FR-045 to FR-048).
- 9router's dev server is stopped. To look again: `cd ref/9router && DATA_DIR=/tmp/claude-1000/9r-data npm run dev`, then open `http://localhost:20127/dashboard`. The throwaway data dir has login turned off. `ref/9router/node_modules` and `.env` are gitignored.
