# Implementation Plan: Dashboard summaries and landscape

**Branch**: `010-dashboard-summaries` | **Date**: 2026-10-06 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/010-dashboard-summaries/spec.md`

**Scope brief**: [specs/briefs/2026-10-05-dashboard.md](../briefs/2026-10-05-dashboard.md) (slice 2
of two). A plan decision that contradicts a confirmed ledger row means stop and revisit the brief.

**Builds on** spec 009's plan, which still holds: one `views::*` builder per fact, pages in
`nullrouter-dashboard`, no scripts, the page guard, the style guide.

## Summary

Two new read-only views, `usage` (request and token totals and Est. Cost for one period) and
`latency` (the last 24 hours per agent and per provider), each with a CLI command (`nullrouter
usage [--period]`, `nullrouter latency`) and each read by the pages, so pages and CLI agree by
construction (research R1). Both take a read time `at`; a page passes its "as of" (R2).

The engine gains `journal::summary`. It reads only the day segments a window needs, and the server
caches each finished day's totals (R3). Tokens are counted without double counting cache reads
(R4). Est. Cost prices each record at its serving account at arrival time, through a new
`price::entry_at` that `price_now` now uses too (R5). Latency uses nearest-rank percentiles; a
provider's time to first token is its own wait, measured from the attempt that was running when
the first token arrived (R6, clarify Q1).

Keys gain an optional harness tag, set with `keys issue --harness` or `keys tag` (R7). The pages
fill spec 009's slots: the landscape with gauges and agent colours on Endpoint & Key (R8, R9), the
period filter, five cards and topology graph on Usage (R10), "requests today" and "last response"
(R11).

## Technical Context

**Language/Version**: Rust (workspace edition and MSRV, unchanged).

**Primary Dependencies**: existing only. `nullrouter-server` gains the `jiff` workspace dependency
(already used by `nullrouter-dashboard`) for local midnight (R2). No new crate.

**Storage**: `keys.toml` gains an optional `harness` field (R7). Summaries read the journal
(`records/*.jsonl`) and write nothing. The segment totals cache is in server memory only (R3).

**Testing**: `cargo test` in GitHub Actions (engine unit tests, view tests, the hand-figure
fixture, dashboard agreement and twin suites, tag neutrality); local Criterion benches
(`usage_totals_100k`, `latency_24h_100k`, `pages`, `engine`); the opt-in isolation test; the
user's side-by-side review.

**Target Platform**: Linux and macOS, as spec 009.

**Project Type**: additions to an existing Rust service, CLI and server-rendered web surface.

**Performance Goals**: each view and each page that uses it under 1 s on 100,000 records (SC-004);
`usage` warm under 50 ms (R12). At most 1 ms added to client p95 while views and pages are read
(SC-005). No regression in the `engine` bench from `price::entry_at`.

**Constraints**: no scripts, no animation, no external fetch; exact percentiles over every value
(FR-013); colours only from the style guide with 9router sources (FR-030); no new write path
except `keys tag`; no secrets or prompt text in any output.

**Scale/Scope**: 2 new CLI commands (`usage`, `latency`), 1 new subcommand (`keys tag`), 1 changed
(`keys issue`, `keys list`); 2 new operator ops (`usage.totals`, `latency.summary`); 2 new views,
1 changed (`keys`); 3 pages changed (Endpoint & Key, Providers, Usage); 6 slots filled; about 10
new style tokens.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How |
|---|---|---|
| I. Plugin Safety | Pass | Plugins gain nothing and see nothing new. Est. Cost reads the prices plugins already declare, in the core. The harness tag is operator data in `keys.toml`; no plugin or adapter receives it. |
| II. Routing Fidelity | Pass | No routing decision changes. `price_now` is re-expressed through `entry_at` with the same result (R5); `rank_price` is untouched. The `engine` bench guards it. |
| III. Unified Models and Provider Entities | Pass | Rows are per agent and per provider, the provider entity. Per-unified-model latency is deferred to the latency slice (clarify Q2), within this principle. |
| IV. Scope Discipline | Pass | No request content is read or changed. The tag changes nothing about handling (FR-025, SC-006). |
| V. Streaming-Native SSE | Pass | Not touched. TTFT is read from records as the relay already sets it (`text.rs:206`). |
| VI. Reference-Informed Behavior | Pass | Periods match 9router's (`usageRepo.js:15`); missing cache rates fall back as 9router's `calculateCostFromTokens` does; the topology edge styles and agent palette trace to 9router files (R5, R8, R10). Two departures are deliberate and recorded: no double count of cached tokens (R4), and no guess for a missing output price (R5). |
| VII. Trustworthy Model Tests | Pass | Not touched. |
| VIII. Latency Observability | Pass | This slice surfaces TTFT and router overhead per agent and per provider on the CLI and dashboard. Per unified model and windowed total time go to the latency slice (clarify Q2); the records still measure both per request. |
| Architecture: Rust only | Pass | maud and inline SVG rendered on the server; no JS. |
| Architecture: no blocking on executor | Pass | Both ops and the cold reads run on the blocking pool (R12). |
| Architecture: secrets isolation | Pass | No secret is read. The tag isn't a secret. 009's sentinel scan covers the new outputs (SC-010). |
| Performance gate | Pass | `price_now` sits on the placement path, so the `engine` bench and the placement benches must not regress. The summaries are read-side; new benches `usage_totals_100k` and `latency_24h_100k` set their baselines. |
| Workflow: parity-audit gate | N/A | No module inheriting 9router request behaviour changes. |

**Post-design re-check (after Phase 1)**: no change. The design adds two views, two ops, one engine
module, one key field and one CLI subcommand; no row moved. There are no violations, so Complexity
Tracking is empty.

## Project Structure

### Documentation (this feature)

```text
specs/010-dashboard-summaries/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── cli.md
│   └── dashboard.md
├── checklists/requirements.md
└── tasks.md             # /speckit-tasks
```

### Source Code (repository root)

```text
crates/
├── nullrouter-engine/
│   ├── src/journal/summary.rs            # NEW: window selection, totals, latency, nearest_rank, segment cache (R3–R6)
│   ├── src/journal/mod.rs                # + pub mod summary
│   ├── src/routing/price.rs              # + Rates, entry_at; price_now via entry_at (R5)
│   ├── src/keys.rs                       # + harness field, check_harness, set_harness (R7)
│   ├── src/state.rs                      # + segment totals cache, cleared on reload (R3)
│   ├── tests/summary.rs                  # NEW: hand-figure fixture (SC-002)
│   ├── tests/fixtures/summaries/         # NEW: home + expected.toml
│   └── benches/{usage_totals_100k,latency_24h_100k}.rs   # NEW (R12)
├── nullrouter-server/
│   ├── Cargo.toml                        # + jiff (workspace)
│   └── src/
│       ├── views/usage.rs                # NEW: period → window, names joined, text/JSON (R1, R2)
│       ├── views/latency.rs              # NEW (R1, R6)
│       ├── views/keys.rs                 # + harness
│       ├── views/mod.rs                  # + modules; request() args for the new ops
│       └── operator.rs                   # + usage.totals, latency.summary (ring merge)
├── nullrouter-dashboard/
│   ├── src/page.rs                       # + ViewName::{Usage, Latency}
│   ├── src/landscape.rs                  # NEW: SVG layout, gauges, pipes (R9)
│   ├── src/topology.rs                   # NEW: SVG ellipse graph (R10)
│   ├── src/pages/{endpoint,providers,usage}.rs   # slots filled (R11)
│   ├── style/{tokens.toml,tokens.css,dashboard.css}   # agent-1..7, gauge, pipe, topology
│   ├── tests/                            # agreement, twins, secrets extended
│   └── benches/pages.rs                  # + /usage?period=all, /
└── nullrouter-cli/src/
    ├── main.rs                           # + Usage, Latency commands
    └── cmd/{usage,latency}.rs            # NEW: text renderers
    └── cmd/keys.rs                       # + --harness, tag subcommand, HARNESS column
docs/
├── operator-config.md                    # + usage, latency, keys tag; downgrade note (R7)
└── dashboard/style-guide.md              # + agent palette, gauge, pipe, topology (sources)
```

**Structure Decision**: no new crate. The engine holds the computation (it owns the journal and
prices), the server holds views and ops, the dashboard holds the SVG rendering. The dependency
order is unchanged: cli → dashboard → server → engine → registry.

## Phases for tasks

`tasks.md` orders the work by story priority; its order wins.

1. **Engine core** (CLI tests are the gate): `price::entry_at` with the `engine` bench check;
   `journal::summary` with window selection, totals, cost and latency; `nearest_rank`; the
   hand-figure fixture (US1, US2).
2. **Views, ops, CLI**: `usage` and `latency` views with `at`; the two ops with the cache and ring
   merge; `nullrouter usage` and `nullrouter latency` (US1, US2).
3. **Harness tag**: the key field, `check_harness`, `keys issue --harness`, `keys tag`, the keys
   view and `keys list` column, the tag neutrality test (US5).
4. **Style tokens**: agent palette, gauge, pipe and topology tokens with 9router sources; the
   style guide doc.
5. **Pages**: Usage (filter, cards, topology) → Endpoint & Key (landscape, requests today, badge)
   → Providers (last response) (US1, US3, US4).
6. **Gates**: agreement and twin suites for every period and the latency view (SC-001); secrets
   scan (SC-010); benches (SC-004); isolation run (SC-005); docs; the user's side-by-side review
   (SC-008).

## Complexity Tracking

No violations.
