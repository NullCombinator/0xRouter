---

description: "Task list for slice 010, dashboard summaries and landscape (slice 2 of two)"
---

# Tasks: Dashboard summaries and landscape

**Input**: Design documents from `specs/010-dashboard-summaries/`

**Prerequisites**: plan.md, spec.md, research.md (R1 to R13), data-model.md, contracts/cli.md,
contracts/dashboard.md, quickstart.md. Scope brief: `specs/briefs/2026-10-05-dashboard.md`.
Spec 009's tasks and code are the base; this slice changes them only where named.

**Tests**: requested by the spec (SC-001 to SC-010 and each story's Independent Test), so each
story has test tasks. Write them first; they must fail before the code exists.

**Build**: never run cargo on this machine (no test, clippy, check, build or bench). Commit, push
the group with the user's OK, and let GitHub Actions run. Benches and the isolation run are the
user's to run locally. The style-guide test (`crates/nullrouter-dashboard/tests/style_guide.rs`)
runs in CI, which clones `ref/9router`: every new token must trace to a light 9router line, and
`tokens.css` must match what the test generates from `tokens.toml`. Work only in `.worktrees/010`; check `git branch --show-current` is
`010-dashboard-summaries` before each commit. CLI goldens
(`crates/nullrouter-cli/tests/golden/`) change only in the commit that changes the read; mark them
for re-blessing (`NR_BLESS=1 cargo test -p nullrouter-cli --test read_golden`) in the commit
message when CI shows the expected diff.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an unfinished task)
- **[Story]**: US1 totals (`usage`, Usage cards), US2 latency (`latency`), US3 landscape, US4 key
  cards, provider window and topology graph, US5 harness tag

---

## Phase 1: Setup

- [X] T001 Add `jiff.workspace = true` to `crates/nullrouter-server/Cargo.toml` (research R2) (the two bench declarations moved to T048: cargo refuses a `[[bench]]` whose file doesn't exist yet, which would fail CI for every commit in between)
- [X] T002 [P] Create the empty module `crates/nullrouter-engine/src/journal/summary.rs` with a module doc naming research R3 to R6, and add `pub mod summary;` to `crates/nullrouter-engine/src/journal/mod.rs`

---

## Phase 2: Foundational (blocking)

Prices and the shared record rules every view uses. No story starts before this phase is done.

### Price in effect at a time (research R5)

- [X] T003 [P] Unit tests in `crates/nullrouter-engine/src/routing/price.rs` (`#[cfg(test)]`): `entry_at` returns the override's rates when the account has one; otherwise the first schedule entry whose `when` holds at the given time (days, `from`/`to` wrapping past midnight, `offset`), else the default entry; `None` with no schedule and no override; `price_now(spec, t) == entry_at(spec, t).map(|r| r.input)` for every case
- [X] T004 Add `pub struct Rates { input: f64, output: Option<f64>, cache_read: Option<f64>, cache_write: Option<f64> }` and `pub fn entry_at(spec: &PriceSpec, at: SystemTime) -> Option<Rates>` to `crates/nullrouter-engine/src/routing/price.rs`, using the private `holds` unchanged; re-express `price_now` through `entry_at`; leave `rank_price` untouched

### Record rules (research R4, R6)

- [X] T005 [P] Unit tests in `crates/nullrouter-engine/src/journal/summary.rs`: `nearest_rank` (sorted ascending, p = `v[ceil(q·n) − 1]`, q = 0.5 and 0.95; one value gives itself; empty gives `None`); `tokens_of(usage)` gives uncached input with `IncludesCache` normalized (cache reads and writes taken out of input, cache writes added back to input), `cached = cache_read`, `output` as reported, reasoning not added; `first_attempt` skips `kind = skipped`; `first_token_attempt` is the last non-skipped attempt with `started <= ttft_ms`, checked on a fallback, a continuation and a restart record; `own_ttft = ttft_ms − first_token_attempt.started`
- [X] T006 Implement `nearest_rank`, `tokens_of`, `first_attempt`, `first_token_attempt` and `own_ttft` in `crates/nullrouter-engine/src/journal/summary.rs`

### Window selection (research R2, R3)

- [X] T007 [P] Unit tests in `crates/nullrouter-engine/src/journal/summary.rs`: `segments_for(home, from, to)` returns exactly the `records/YYYY-MM-DD.jsonl` files whose UTC day lies in `[day_of(from), day_of(to)]` (all when `from` is `None`); `records_in(home, from, to)` folds them and keeps records with `from <= arrived < to`, across a UTC midnight boundary
- [X] T008 Implement `Window { from: Option<SystemTime>, to: SystemTime }`, `segments_for` and `records_in` in `crates/nullrouter-engine/src/journal/summary.rs`, using `records::segments`, `day_of` and `fold`

### Hand-figure fixture (SC-002)

- [X] T009 Create the fixture home `crates/nullrouter-engine/tests/fixtures/summaries/` (built as `keys.toml` with three keys, one revoked; `prices.toml`, a table of per-account price specs the tests turn into `PriceSpec`s, with a time-of-day schedule, a flat override, one account with no output rate, one with no price and one gone, in place of a registry home with plugins, which would test the registry rather than the summaries; `records/` over three UTC days) and `expected.toml` with the hand-worked totals per period, Est. Cost to the cent with unpriced counts per reason, and p50/p95 per agent and per provider. Cover: cached tokens with `IncludesCache` and `ExcludesCache`, an account override, an account with no price, a deleted account, output with no output price, no usage reported, one record in flight, a fallback (A failed, B served), a continuation, a restart, a request refused before a key matched, a cancelled attempt. Write the derivation of each figure as a comment beside it

**Checkpoint**: prices and record rules exist; stories can start.

---

## Phase 3: User Story 1 — Totals for a period, in the CLI and on Usage (Priority: P1) 🎯 MVP

**Goal**: `nullrouter usage [--period]` and Usage's period filter and five cards.

**Independent Test**: the fixture home; `usage` for each period equals `expected.toml`; Usage with
each period equals `usage --json` at the page's `as of`.

### Tests for User Story 1

- [X] T010 [P] [US1] Engine test `crates/nullrouter-engine/tests/summary.rs`: `summary::totals` over the fixture for each window in `expected.toml` matches requests, `in_flight`, `not_reported`, input, cached, output, `cost_usd` (to the cent) and `unpriced` per reason (`no_price`, `account_gone`, `no_output_price`)
- [X] T011 [P] [US1] View tests in `crates/nullrouter-server/src/views/usage.rs` (`#[cfg(test)]`): period resolution with a fixed zone (`Europe/Berlin`): `today` starts at local midnight of `at`, including on the two daylight-saving change days; `24h`/`7d`/`30d`/`60d` subtract elapsed time; `all` has no start; an unknown period is a `ViewError` naming the six periods; the JSON shape of data-model.md (`label` "Estimated, not actual billing", the `note`, `agents` and `providers` sorted by requests then id, names joined from `keys.toml`, null for a deleted key)
- [X] T012 [P] [US1] CLI test `crates/nullrouter-cli/tests/usage.rs` (exact lines on a home it builds; no golden files, because `read_golden` would need blessed files on four fixture homes, which can't be generated without running cargo): text per contracts/cli.md (compact counts `612k`, `1.3M`; `~$` cost to cents; label always printed; "not priced" line only above 0; note line always; empty period wording; exit 1 on `--period week`), and `--json` equal to the view
- [X] T013 [P] [US1] Server test `crates/nullrouter-server/tests/usage_totals.rs`: with a running server, `usage.totals` equals the cold path on the same disk; a record still in the live ring and not yet on disk is counted; a finished day is served from the cache and recomputed after the file grows, after `records forget` (new inode) and after `reload` (new generation)
- [X] T014 [P] [US1] Dashboard agreement test `crates/nullrouter-dashboard/tests/agreement/usage.rs`: for each period, the five cards, the in-flight, not-reported and not-priced counts, the label and the note equal `usage --json` with the page's `as_of` as `at`; `?period=bogus` shows Today with the notice "Unknown period; showing Today"; an empty period shows zeros and "No requests in this period"

### Implementation for User Story 1

- [X] T015 [US1] Implement `pub struct Totals` (fields of data-model.md) with `add`, and `summary::totals(home, window, prices) -> Totals` in `crates/nullrouter-engine/src/journal/summary.rs`: count every record in the window; `in_flight` for `in_progress`; `not_reported` for finished records with no `usage`; tokens by `tokens_of`; cost by `entry_at` at `arrived` for the `served_by` account, missing cache rates at the input rate, output above 0 with no output rate → unpriced `no_output_price`, no spec → `no_price`, account not in `accounts.toml` → `account_gone`; per-agent counts by `agent.key`; per-provider counts for records with a non-skipped attempt on the provider. `prices` is a lookup built from the registry and accounts as `crates/nullrouter-engine/src/route.rs:170` builds a `PriceSpec`
- [X] T016 [US1] Add the segment totals cache (built as a process-wide cache in `crates/nullrouter-engine/src/journal/summary.rs`, like the segment index, instead of in `state.rs`; `totals_cached` takes the generation): one `Totals` per segment path, keyed by inode, length and engine generation; used only for segments whose whole UTC day lies inside the window; dropped on inode change or reload
- [X] T017 [US1] Add the operator op `usage.totals {from, to}` in `crates/nullrouter-server/src/operator.rs` (protocol table in the doc comment; runs on the blocking pool; merges the live ring over disk by id as `records.list` does) and its args line in `views::request` in `crates/nullrouter-server/src/views/mod.rs`
- [X] T018 [US1] Implement the `usage` view in `crates/nullrouter-server/src/views/usage.rs` (`NEEDS = ["usage.totals"]`; args `period`, `at`; resolves the window with `jiff` in `TimeZone::system()`; without a server calls `summary::totals` in process and settles open records through the engine's `running` flag (`summary::totals(.., running)`), so `views::records::settle_open` stays private; joins key names; adds `check`'s "records not kept" warning when the window touches one) and register it in `crates/nullrouter-server/src/views/mod.rs`
- [X] T019 [US1] Add the `usage` command (`--period`, default `today`) to `crates/nullrouter-cli/src/main.rs` and its text renderer in `crates/nullrouter-cli/src/cmd/usage.rs` per contracts/cli.md; declare the module in `crates/nullrouter-cli/src/cmd/mod.rs`
- [X] T020 [US1] Add `ViewName::Usage` (needs and builder) in `crates/nullrouter-dashboard/src/page.rs`, with `build_with` setting `at` to the page's as-of for views that take one; add the `usage-stats` and `usage-stat` components to `tokens.toml` and their rules, and `border`/`font-family`/`cursor` for button items, to `dashboard.css` (the filter reuses `kind-filter`, 9router's SegmentedControl); on Usage (`crates/nullrouter-dashboard/src/pages/usage.rs`) replace the "Period filter" and "Requests, input, cached, output, Est. Cost" slots with the period `GET` form (Today, 24h, 7D, 30D, 60D, All; chosen one marked) and the five cards of contracts/dashboard.md, passing the page's `as_of` as `at`; style with the existing card and filter components

**Checkpoint**: US1 complete: the totals are on the CLI and on Usage.

---

## Phase 4: User Story 2 — Latency over the last 24 hours (Priority: P1)

**Goal**: `nullrouter latency`, per agent and per provider.

**Independent Test**: the fixture home with `at` fixed; every figure equals `expected.toml`.

### Tests for User Story 2

- [x] T021 [P] [US2] Engine test in `crates/nullrouter-engine/tests/summary.rs`: `summary::latency` over the fixture's last 24 h matches `expected.toml`: agent requests, overhead and TTFT p50/p95 with `n`; provider requests, own TTFT p50/p95, per-agent counts; last response (provider: newest `Ok`/`Failed` attempt at `arrived + ended`, with the HTTP status when failed; agent: newest finished request, `Succeeded` resolved, `Failed`/`Refused`/`Interrupted` failed, `Cancelled` ignored); the refused-before-key record in no row; a row with no values gives `None`, not 0; records older than 24 h excluded
- [x] T022 [P] [US2] View and CLI tests: `crates/nullrouter-server/src/views/latency.rs` (`#[cfg(test)]`, JSON shape of data-model.md, `window` = "last 24 h", window `[at − 24 h, at)`) and `crates/nullrouter-cli/tests/latency.rs` with golden `crates/nullrouter-cli/tests/golden/latency.txt` (ms under a second, seconds with one decimal above; `none`; `own ttft` heading; "no requests in the last 24 h"; local times with a date when not the read's date)
- [x] T023 [P] [US2] Server test `crates/nullrouter-server/tests/latency_summary.rs`: `latency.summary` with a running server equals the cold path, and includes a record finished in the ring but not yet on disk

### Implementation for User Story 2

- [x] T024 [US2] Implement `Pct`, `Last`, `AgentLatency`, `ProviderLatency`, `Latency` and `summary::latency(home, window) -> Latency` in `crates/nullrouter-engine/src/journal/summary.rs` per research R6 and data-model.md; never cached
- [x] T025 [US2] Add the operator op `latency.summary {from, to}` in `crates/nullrouter-server/src/operator.rs` (blocking pool, ring merge) and its args line in `crates/nullrouter-server/src/views/mod.rs`
- [x] T026 [US2] Implement the `latency` view in `crates/nullrouter-server/src/views/latency.rs` (`NEEDS = ["latency.summary"]`; arg `at`; cold path in process, settling open records with `views::records::settle_open` as `usage` does; names joined) and register it
- [x] T027 [US2] Add the `latency` command to `crates/nullrouter-cli/src/main.rs` and its text renderer in `crates/nullrouter-cli/src/cmd/latency.rs` per contracts/cli.md
- [x] T028 [US2] Add `ViewName::Latency` (needs and builder) in `crates/nullrouter-dashboard/src/page.rs`

**Checkpoint**: US1 and US2 complete: both views on the CLI.

---

## Phase 5: User Story 5 — Tag a key with its harness (Priority: P3, built here because US3 and US4 show the tag)

**Goal**: `keys issue --harness`, `keys tag`, the tag in `keys list` and on cards.

**Independent Test**: issue, tag, change, clear, refuse; the secret keeps working; tagged and
untagged keys are handled the same.

### Tests for User Story 5

- [x] T029 [P] [US5] Engine unit tests in `crates/nullrouter-engine/src/keys.rs`: `check_harness` accepts text that, trimmed, is "1 to 32 Unicode scalar values, no control characters" and refuses the rest with a message stating the limit; `set_harness` and `clear_harness` change only `harness` (digest, id, created, revoked and break behaviour byte-identical after save); a file with no tags round-trips byte for byte
- [x] T030 [P] [US5] CLI test in `crates/nullrouter-cli/tests/accounts_keys.rs`: `keys issue x --harness claude-code`; `keys tag <name|id> codex`; `keys tag <key> --clear`; a tag with a tab exits 1 and leaves the old tag; TEXT and `--clear` together exit 1; an unknown key exits 2 with `no key "<key>"`; a revoked key can be tagged and stays revoked; `keys list` shows the `HARNESS` column (`-` when none) and `--json` rows carry `harness`; the golden `crates/nullrouter-cli/tests/golden/keys_list*.txt` gains the column
- [x] T031 [P] [US5] Server test `crates/nullrouter-server/tests/harness_tag.rs` (SC-006): the same requests against a mock provider with a tagged and an untagged key give equal placements, upstream bodies and responses, and the secret issued before `keys tag` still authenticates after it

### Implementation for User Story 5

- [x] T032 [US5] Add `harness: Option<String>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` to `AgentKey`, and `check_harness`, `set_harness(name_or_id, text)` and `clear_harness(name_or_id)` to `crates/nullrouter-engine/src/keys.rs`; `issue` takes an optional harness
- [x] T033 [US5] In `crates/nullrouter-cli/src/cmd/keys.rs`: `--harness` on `issue`; a `tag` subcommand (`<key> <TEXT>` or `<key> --clear`, mutually exclusive) that loads, mutates, saves and calls `apply`, printing `<name>: harness <text>` or `<name>: no harness` then `applied`; the `HARNESS` column in `list`; `harness` in the `--json` outputs
- [x] T034 [US5] Add `harness` to the keys view rows in `crates/nullrouter-server/src/views/keys.rs`, and the harness badge on key cards in `crates/nullrouter-dashboard/src/pages/endpoint.rs` (badge component, nothing when null)

**Checkpoint**: US5 complete.

---

## Phase 6: Style tokens for the new parts (US3, US4)

- [x] T035 [US3] Add `[token.agent-1]` to `[token.agent-7]` to `crates/nullrouter-dashboard/style/tokens.toml` (`#6366f1 #14b8a6 #f59e0b #8b5cf6 #06b6d4 #10b981 #f97316`, source `ref/9router/src/app/(dashboard)/dashboard/usage/components/ProviderBarChart.js:17`, red dropped, research R8), and `[component.gauge]`, `[component.pipe]` and `[component.topology]` with their 9router sources (`ProviderTopology.js` `edgeStyle` :294, node styles :35–:86; gauge zones from the existing status colours); write `crates/nullrouter-dashboard/style/tokens.css` in the same commit exactly as `style_guide.rs:57` generates it (no local cargo; if CI shows a diff, take CI's output); add the rules for the period filter, gauge, pipe, topology graph and harness badge to `crates/nullrouter-dashboard/style/dashboard.css`, each using only the tokens of the component its selector names; add the new sections to `docs/dashboard/style-guide.md`
- [x] T036 [US3] Add `pub fn agent_colour(index: usize) -> &'static str` (`agent-(index mod 7) + 1`) in `crates/nullrouter-dashboard/src/components.rs`, with a unit test that colours never change when keys are appended

---

## Phase 7: User Story 3 — The traffic landscape on Endpoint & Key (Priority: P2)

**Goal**: agents left, router centre, providers right, gauges, last 24 h.

**Independent Test**: the landscape equals `latency --json` at the page's `as of`, with scripts off.

### Tests for User Story 3

- [x] T037 [P] [US3] Unit tests in `crates/nullrouter-dashboard/src/landscape.rs` (`#[cfg(test)]`): node set per FR-020 (every unrevoked key, every provider with an account, plus revoked keys and other providers with a latency row, marked); needle at p50 on a 40 ms (overhead) or 3 s (TTFT) full scale, pinned beyond it; p50, p95 and request counts printed as text beside every gauge; one stroke per agent on a provider pipe in that agent's colour; the empty state "No requests in the last 24 hours"; a long key name or harness tag is shown as spec 009 shows long names (in full, or shortened with the full text in the node's hover card); no `<script>`, no animation, no external reference
- [x] T038 [P] [US3] Dashboard agreement test `crates/nullrouter-dashboard/tests/agreement/endpoint.rs`: every number in the landscape equals `latency --json` at the page's `as_of`; the heading says "last 24 h"; each agent's colour is the same on two loads and after a key is appended

### Implementation for User Story 3

- [x] T039 [US3] Implement `crates/nullrouter-dashboard/src/landscape.rs`: layout from row counts (agents in a left column, router centre, providers right, cubic Bézier pipes, after `docs/dashboard/mockups/topo.py` gauges mode), half-dial gauges (green to 50%, amber to 80%, red above, from the style guide), labels, a CSS-only `:hover`/`:focus-within` card repeating the printed numbers and the full name of a shortened node, rendered with maud as inline SVG
- [x] T040 [US3] On Endpoint & Key (`crates/nullrouter-dashboard/src/pages/endpoint.rs`) read `Latency` and `Accounts` (with `at` = `as_of`) and replace the "Agent traffic" slot with the landscape, headed "Agent traffic · last 24 h · as of HH:MM:SS"

**Checkpoint**: US3 complete.

---

## Phase 8: User Story 4 — Key cards, provider window and topology graph (Priority: P2)

**Goal**: fill "requests today", "last response" and the topology graph.

**Independent Test**: each value equals `usage --period today`, `latency` and `usage --period <p>`.

### Tests for User Story 4

- [x] T041 [P] [US4] Dashboard agreement tests: `crates/nullrouter-dashboard/tests/agreement/endpoint.rs` (each card's "requests today" equals `usage --period today` for that key, 0 when absent); `crates/nullrouter-dashboard/tests/agreement/providers.rs` (the window's "last response" equals `latency`'s `last`, or "None in the last 24 hours"); `crates/nullrouter-dashboard/tests/agreement/usage.rs` (for each period the graph's providers and counts equal `usage --period <p>`, edges red for a failed last response and amber on the newest, no in-flight count)
- [x] T042 [P] [US4] Unit tests in `crates/nullrouter-dashboard/src/topology.rs`: providers on an ellipse after 9router's `buildLayout` (`ProviderTopology.js:263`), router centred, no agent nodes, labels printed, no animation or script

### Implementation for User Story 4

- [x] T043 [US4] Fill "requests today" on each key card in `crates/nullrouter-dashboard/src/pages/endpoint.rs` from `Usage` with `{"period":"today","at":as_of}` (`agents[<id>].requests`, else 0)
- [x] T044 [US4] Fill "last response" in the provider window in `crates/nullrouter-dashboard/src/pages/providers.rs` from `Latency` ("resolved · <local time>", "failed · <local time> · <status>", or "None in the last 24 hours")
- [x] T045 [US4] Implement `crates/nullrouter-dashboard/src/topology.rs` (inline SVG) and replace the "Topology graph" slot in `crates/nullrouter-dashboard/src/pages/usage.rs`, providers and counts from `Usage` for the chosen period, last response from `Latency`, labelled "last 24 h"

**Checkpoint**: every spec 009 slot is filled; `slot(` is no longer called on Endpoint & Key, Providers or Usage.

---

## Phase 9: Polish & cross-cutting gates

- [ ] T046 [P] Twin test `crates/nullrouter-dashboard/tests/twins.rs`: register the `Usage` and `Latency` views for Endpoint & Key, Providers and Usage; it fails if a page renders a value absent from those views' JSON (SC-001)
- [ ] T047 [P] Secrets scan `crates/nullrouter-dashboard/tests/secrets.rs` and `crates/nullrouter-server/tests/secrets.rs`: cover the new pages, `usage`, `latency` and `keys list` output (SC-010)
- [ ] T048 [P] Benches (written here; the user runs them locally): declare `usage_totals_100k` and `latency_24h_100k` (`harness = false`) in `crates/nullrouter-engine/Cargo.toml` with their files; `crates/nullrouter-engine/benches/usage_totals_100k.rs` (`today`, `30d`, `all`, warm and cold; reuse `keys_last_used_100k`'s fixture builder) and `crates/nullrouter-engine/benches/latency_24h_100k.rs` (100,000 records over 30 days and in one day); add `/usage?period=all` and `/` to `crates/nullrouter-dashboard/benches/pages.rs`; leave `specs/010-dashboard-summaries/bench-baseline.md` with empty rows for the user's run, including the `engine` bench before/after `entry_at`
- [ ] T049 [P] Docs: `docs/operator-config.md` gains `nullrouter usage`, `nullrouter latency`, `keys issue --harness`, `keys tag`, what Est. Cost includes and leaves out (the three unpriced reasons; cache tokens with no cache rate at the input rate), that the per-agent rows can sum to less than the total because requests refused before a key matched have no agent, and the downgrade note (a tagged `keys.toml` needs `keys tag <key> --clear` before an older binary reads it)
- [ ] T050 Run the quickstart's CI-covered steps through GitHub Actions on the pushed branch (user's OK to push); record the run number in `specs/010-dashboard-summaries/tasks.md` beside this task
- [ ] T051 The user's side-by-side review of the cards, filter, topology graph and landscape against 9router and the mockups (SC-008), with scripts off, and whether Endpoint & Key alone tells which agents sent traffic, to which providers, and which hop is slowest (SC-009); record the verdict in `specs/010-dashboard-summaries/look-review.md`
- [ ] T052 The isolation run (SC-005, local, user): `cargo test -p nullrouter-dashboard --release --features fault -- --ignored isolation` with the landscape and Usage pages in the load mix (add them to `crates/nullrouter-dashboard/tests/isolation.rs`)

---

## Dependencies & execution order

- **Setup (T001–T002)** → **Foundational (T003–T009)** → stories.
- **US1 (T010–T020)** and **US2 (T021–T028)** depend only on Foundational; they touch the same
  files (`summary.rs`, `operator.rs`, `views/mod.rs`, `main.rs`, `page.rs`), so run them one after
  the other: US1 first (MVP).
- **US5 (T029–T034)** is independent of US1 and US2 and can run beside them. It comes before US3
  and US4 because their pages show the tag.
- **Style (T035–T036)** before US3 and US4.
- **US3 (T037–T040)** needs US2 and the style tasks.
- **US4 (T041–T045)** needs US1, US2 and the style tasks.
- **Polish (T046–T052)** last.

Within a story: tests first (they fail), then engine → op → view → CLI → page.

## Parallel examples

- Foundational: T003, T005 and T007 (tests in different modules) together; then T004 beside T006
  and T008 (different files, `price.rs` versus `summary.rs`, though T006 and T008 share a file and
  run in order).
- US1: T010 to T014 together (five test files).
- US5 beside US1: T029 to T031 while US1's implementation runs, since `keys.rs`, `cmd/keys.rs` and
  `views/keys.rs` aren't touched by US1.
- Polish: T046 to T049 together.

Project rule: no parallel Opus subagents unless the user asks; "[P]" marks what may run at once,
not what must.

## Implementation strategy

1. **MVP = US1**: `nullrouter usage` and the Usage cards and filter. Push, let CI run, stop for
   the user.
2. **US2**: `nullrouter latency`. Push, CI.
3. **US5**: the harness tag (small, independent).
4. **Style, then US3 and US4**: the landscape, key cards, provider window, topology graph. Push,
   CI.
5. **Polish**: twins, secrets, benches written, docs; then the user's benches, isolation run and
   look review.
