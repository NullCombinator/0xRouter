# Research: Dashboard summaries and landscape

Decisions for [spec.md](spec.md), slice 2 of the [dashboard brief](../briefs/2026-10-05-dashboard.md).
Spec 009's research ([R1 to R16](../009-dashboard/research.md)) still holds; this file adds to it.
Code references are to `crates/` at `e4ed8ac`.

## R1. Two new views, built once for the CLI and the pages

**Decision**: Two new read-model views beside slice 008's, in `nullrouter-server/src/views/`:

| View | CLI twin | Pages |
|---|---|---|
| `usage` | `nullrouter usage [--period P]` | Usage (cards, topology graph counts), Endpoint & Key ("requests today", period `today`) |
| `latency` | `nullrouter latency` | Endpoint & Key (landscape), Providers (window "last response"), Usage (graph's last response) |

Each is a `build(home, args, live)` with its `NEEDS`, like every view (`views/mod.rs:33`). The
dashboard's `ViewName` (`page.rs:22`) gains `Usage` and `Latency`. Pages only arrange and format
the view JSON, as 009 R1 decided; the twin test (009 R16) covers the new values.

**Rationale**: the slice fails if a page disagrees with the CLI. One builder makes agreement
structural.

**Alternative rejected**: computing summaries inside the page modules. That is a second rendering
of each number and can drift.

## R2. One "read time" passed to both views (FR-031)

**Decision**: Both views take an optional `at` argument (RFC 3339). The CLI leaves it out, and the
view uses `clock::now()`. A page passes its `as_of` (`page.rs:124`), so "today", "last 24 h" and
every rolling period end at the time the page prints. The view JSON echoes `at` and the resolved
window, `from` and `to`.

The view resolves a period to `[from, to)` itself, in this machine's zone (`jiff`,
`TimeZone::system()`, as 009 R13 did for display). The server crate gains the `jiff` workspace
dependency. The engine and the operator ops see only UTC instants and know nothing of zones.

| Period | `from` | `to` |
|---|---|---|
| `today` | local midnight of `at`'s local date (`jiff` `start_of_day`, safe across a daylight-saving change) | `at` |
| `24h`, `7d`, `30d`, `60d` | `at` minus 24 h, 7, 30 or 60 × 24 h of elapsed time | `at` |
| `all` | none | `at` |

A record belongs to a window when `from <= arrived < to` (spec edge case: arrival decides).

**Rationale**: a page builds in about a second; without a shared `at`, a request arriving during
the build would make the cards and the "as of" line disagree. Resolving zones in the view keeps
the CLI and the server on one rule; both run on this machine, so both read the same zone.

**Alternative rejected**: letting the operator op resolve periods. It would put zone logic in the
engine and give two clocks (CLI process and server) a chance to differ.

## R3. Reading a window: by day segment, with a per-segment cache in the server

**Facts** (`journal/records.rs`): records live in `records/YYYY-MM-DD.jsonl`, one UTC day of
arrivals per segment, as append-only event lines folded by `fold`. The segment index
(`journal/index.rs`) is in memory, keeps up to 8 segments, and has no arrival range or provider
data. `Filter.since` is a per-record check that doesn't skip segments.

**Decision**:
- A new engine module, `journal::summary`, has one function per view:
  `totals(home, from, to, pricing) -> Totals` and `latency(home, from, to) -> Latency`. It selects
  the segments whose day can hold arrivals in `[from, to)` by file name (`segments()`, `day_of`),
  folds each, and keeps records whose `arrived` falls in the window.
- Totals are sums, so a day segment that lies wholly inside the window adds the same numbers
  however often it is read. The server keeps a **segment totals cache**: per path, keyed by inode,
  length and the pricing generation (R5). It holds that segment's totals, with per-agent and
  per-provider counts. Only the (at most two) boundary segments, the first day (partial) and
  today's growing segment, are folded record by record on each read. A cached segment is
  recomputed when its file grows (a late `close` line) or is rewritten (prune, forget: new inode).
- Latency needs exact percentiles over every value (FR-013), so it is never cached as a sum. Its
  window is 24 h, which is at most two day segments.
- **Ring merge**: new records reach the disk in batches, so the server merges its live ring over
  the folded disk records by id, as `records.list` already does (`operator.rs:249-289`). With no
  server, the disk is the whole truth; `settle_open` (`views/records.rs:29`) marks unfinished
  records interrupted, as `records list` does.
- **Transports**: two new operator ops, `usage.totals {from, to}` and `latency.summary {from,
  to}`, answer from the cache and the ring. With no server, the view calls the same
  `journal::summary` function in its own process without the cache (cold path), like
  `keys.last_used` (009 R9).

**Rationale**: the 100,000-record target (SC-004) is met by never re-folding a finished day while
the server runs. The cold CLI path folds only the segments the period needs. All is the worst
case, measured by the bench (R12).

**Alternatives rejected**:
- A persisted summary file per day: a second record of what the journal already records, which
  must then agree with `records forget` and `prune`.
- Incremental counters on the request path: lost at restart, and they would touch the hot path
  (Performance gate).
- Bounding a window by ULID id range in the index: the index holds at most 8 segments and has no
  record content, so it saves no folding.

## R4. Tokens on the cards (FR-004)

**Facts**: `Usage` (`records.rs:38`) holds `input`, `output`, `cache_read`, `cache_write`,
`reasoning`, and `input_semantics`. With `IncludesCache`, `input` already includes both cache
counts (`Usage::reported`, `:51`). A record's `usage` is the serving attempt's, carried across a
continuation (`attempt.rs:1775`).

**Decision**: for each finished record with `usage`:
- **cached** = `cache_read`;
- **input** = input with cache reads taken out (normalized from `IncludesCache`), so cache writes
  stay in input;
- **output** = `output` (reasoning tokens are part of output as providers report them, and are
  not added again).

A record with no `usage` adds no tokens and counts as **not reported**, the word
`records list` prints (`cmd/records.rs:208`). Unfinished records count as **in flight** (they
add no tokens yet). `input + cached` is every prompt token, and nothing is counted twice.

**Rationale**: FR-004 and the spec's edge case. 9router's "Total Input Tokens" card includes
cached tokens and its "Cached Tokens" card counts them again (`usageRepo.js:66`, `:381`); 0router
doesn't copy that double count. The CLI labels the line "input (uncached)" so nobody compares it
with a provider's cache-inclusive figure by mistake.

## R5. Est. Cost: the price in effect at arrival (FR-005 to FR-007)

**Facts**: `routing/price.rs` has `PriceSpec { schedule, flat }`, `price_now(spec, now)` (the
input price only) and `rank_price` (unpriced counts as 1, for ranking). `holds(when, now)` is
private. Nothing reads output or cache prices today.

**Decision**:
- `price.rs` gains `pub fn entry_at(spec, at) -> Option<Rates>`, with `Rates { input, output,
  cache_read, cache_write }`. It returns the override if the account has one, else the first
  schedule entry whose `when` holds at `at`, using `holds` unchanged. `price_now` becomes
  `entry_at(..).map(|r| r.input)`, which is the same result; `rank_price` is untouched.
- A finished record with tokens is priced at its **serving account** (`served_by.provider`,
  `served_by.account`), at its `arrived` time. The `PriceSpec` is built as `route.rs:170` builds it,
  from the current registry and the account's current overrides.
- Missing rates follow 9router's `calculateCostFromTokens` (`open-sse/providers/pricing.js:420`):
  cache reads and cache writes without their own price use the input price. **No output price
  and output tokens above 0** has no 9router rule (it assumes an output price exists). Such a
  record is counted as **unpriced** rather than guessed.
- **Unpriced** (left out of Est. Cost, counted with a reason): no price spec at all for the
  account, the account no longer exists, or output tokens with no output price. The JSON gives
  the count per reason; the card shows the total and the CLI text lists the reasons.
- Prices are per million tokens, in US dollars. The view keeps full precision (`f64`); text and
  the card round to cents and prefix `~$`, as the mockup does.
- **Pricing generation**: the segment totals cache (R3) is keyed by the engine's reload
  generation, because a reload can change prices or accounts.
- The label "Estimated, not actual billing" and the note "Priced with today's declared prices;
  earlier price changes are not tracked" are constants in the view JSON (`label`, `note`), so the
  CLI and the card print the same words.

**Rationale**: the user confirmed "existing prices, unpriced left out and counted" (brief row 25).
Pricing at the serving account matches the tokens on the cards, which are the serving attempt's
(R4). A restart that moved to another account is priced at the final account; restarts are rare
and the note says the figure is an estimate.

**Alternatives rejected**:
- Pricing each attempt's own `usage` at its own account: the cards would then count tokens from a
  different set than the cost.
- Pricing a missing output rate at the input rate: output is usually several times dearer, so
  this would understate the cost without saying so.

## R6. Latency figures (FR-008 to FR-013, clarify Q1)

**Facts**: `ttft_ms` and `total_ms` are milliseconds from arrival. For relayed streams the server
sets `ttft_ms` at the first content handed to the client's socket (`text.rs:206`); a break
doesn't reset it. Attempts carry `started`, `ended`, `outcome` and `kind`. Nothing named
"overhead" is recorded.

**Decision**, over records that arrived in `[at − 24 h, at)` and have an agent (FR-012):
- **Router overhead** of a request = `started` of its first attempt whose kind isn't `Skipped`.
  Requests without such an attempt have no overhead value.
- **Agent row**: requests; router overhead p50/p95; time to first token p50/p95 (`ttft_ms`, the
  client's whole wait); last response.
- **First-token attempt** = the last non-skipped attempt with `started <= ttft_ms`. It is the
  attempt that was running when the first token reached the client, which holds for fallbacks,
  continuations and restarts alike, since a break comes after the first token.
- **Provider's own time to first token** = `ttft_ms − first_token_attempt.started`, credited to
  that attempt's provider (clarify Q1). This needs no new record field.
- **Provider row**: requests (records with at least one non-skipped attempt on it); its own time
  to first token p50/p95; requests per agent; last response.
- **Last response**, provider: the newest attempt on it whose outcome is `Ok` (resolved) or
  `Failed` (failed), timed at `arrived + ended`. `Skipped` and `Cancelled` don't count.
  **Agent**: the newest finished request: `Succeeded` is resolved; `Failed`, `Refused` and
  `Interrupted` are failed; `Cancelled` doesn't count.
- **Percentiles**: nearest rank over all values. Sort ascending; p = `v[ceil(q·n) − 1]`, with q =
  0.5 and 0.95. One value gives itself for both. One function, `journal::summary::nearest_rank`,
  is used by both views and tested against hand figures (SC-002).
- A row with no values says `none in the last 24 h` (text) or `null` (JSON), never 0 (FR-011).

**Rationale**: each hop measures its own wait (clarify Q1). Nearest rank always returns a real
observed value and is the simplest rule a person can check by hand.

**Alternative rejected**: interpolated percentiles. They return values nobody observed, and the
same records could give different figures under a different interpolation.

## R7. Harness tag (FR-023 to FR-025, clarify Q5)

**Decision**:
- `AgentKey` (`keys.rs:26`) gains `harness: Option<String>`, `#[serde(default,
  skip_serializing_if = "Option::is_none")]`. `keys.toml` stays at `schema = 1`. A file with no
  tags is byte-for-byte what it is today.
- Text rules, one function `keys::check_harness`: trimmed, 1 to 32 characters (Unicode scalar
  values), no control characters. The error says the limit.
- `keys issue <name> [--harness TEXT]`; new `keys tag <key> <TEXT>` and `keys tag <key> --clear`.
  `<key>` is a name or id, as `revoke` and `set-break` take. Each command loads, mutates, saves
  atomically (`files::write_private`) and sends `reload`, like the other key mutators. A refused
  tag writes nothing.
- The keys view rows gain `harness` (string or null); `keys list` prints a `HARNESS` column (`-`
  when none). The tag is never read by auth, routing, adapters or records.

**Rationale**: the tag is display only, so no other code path should see it. Optional and skipped
when empty keeps old files unchanged.

**Known edge**: `AgentKey` has `deny_unknown_fields`, so a binary older than this slice refuses a
`keys.toml` that carries a tag. Downgrading after tagging needs `keys tag <key> --clear` first.
`docs/operator-config.md` says so.

## R8. Agent colours (FR-019)

**Decision**: the style guide gains an agent palette, taken from 9router's chart series colours
(`src/app/(dashboard)/dashboard/usage/components/ProviderBarChart.js:17`: `#6366f1 #14b8a6
#f59e0b #ef4444 #8b5cf6 #06b6d4 #10b981 #f97316`). Red (`#ef4444`) is dropped from the agent order
because it is the "failed" colour (R10), which leaves seven. An agent's colour is `palette[i mod
7]`, where `i` is the key's position in `keys list` order (creation order, revoked keys
included), so a colour never moves when keys are added. The keys view's position is the only
input, so every page agrees.

**Rationale**: FR-030 requires every colour to come from 9router. The mockup's agent colours
were invented sample content.

## R9. The landscape on Endpoint & Key (FR-017 to FR-020)

**Decision**:
- Inline SVG rendered by the server (maud), laid out like `docs/dashboard/mockups/topo.py`'s
  gauges mode: agents in a left column, the router node in the centre, providers in a right
  column, each pipe a cubic Bézier. The positions are computed in Rust from the row counts.
- **Who appears** (FR-020): every unrevoked key (keys view) and every provider with an account
  (accounts view), plus any revoked key or other provider with a row in the latency view. Revoked
  keys carry the revoked badge, as their card does. A node with no traffic has a dim pipe and no
  gauge numbers.
- **Gauges**: a half dial per hop, its needle at the median. Full scale is 40 ms for router
  overhead and 3 s for time to first token. The zones are green for the first half, amber to 80%
  and red for the rest, from the mockup, with the status colours of the style guide. The p50, p95
  and request counts are printed beside every gauge, so neither a zone nor the needle carries a
  fact alone. A value beyond full scale pins the needle and still prints its number.
- **Provider pipes** show one thin stroke per agent that used them, in that agent's colour, with
  the per-agent counts printed in the provider's label block.
- **Hover**: a CSS-only `:hover`/`:focus-within` card that repeats the printed numbers. It adds
  nothing the page doesn't print (FR-027).
- The heading reads "Agent traffic · last 24 h · as of HH:MM:SS".
- With no traffic in the window, the landscape shows the nodes and "No requests in the last
  24 hours".

## R10. The topology graph on Usage (FR-016, clarify Q4)

**Decision**: inline SVG after 9router's `ProviderTopology` (`usage/components/ProviderTopology.js`):
the router in the centre, providers on an ellipse (`buildLayout`, `:263`), no agent nodes. The
providers and their counts come from the usage view's `providers` for the chosen period. The
edge style comes from 9router's `edgeStyle` (`:294`): red `#ef4444` when the provider's last
response (latency view) is failed, amber `#f59e0b` on the provider with the newest last response
overall, and the border colour otherwise. There is no animation (9router animates only in-flight
edges, and there is no in-flight count). Each node's label prints its count and "last response
resolved · HH:MM" or "failed · HH:MM", or "none in the last 24 h".

## R11. Requests today and last response in the existing slots

- **Key card** "Requests today": the usage view for period `today`, `agents[<key id>].requests`,
  or 0 (FR-021).
- **Provider window** "Last response": the latency view's `providers[<id>].last` (FR-022).
- The Usage period filter is a `GET` form (`?period=7d`), as 009 R4 does filters. An unknown value
  falls back to `today` and says so.

Every 009 slot is replaced: `slot(...)` calls at `endpoint.rs:31`, `:82`; `providers.rs:253`;
`usage.rs:192-195`. The `slot` component stays for future slices.

## R12. Bounds, isolation and benches (SC-004, SC-005, Performance gate)

- The ops run on the blocking pool; the pages are under 009's guard (2 builds at once, 10 s each).
  No new lock is taken on the request path; the cache is the server's own.
- `price_now` now calls `entry_at`. `place.rs` runs it on the routing path, so the existing
  `engine` bench and slice 006's placement benches must not regress (Performance gate).
- New Criterion benches in `nullrouter-engine/benches/`, reusing `keys_last_used_100k`'s fixture
  builder:
  - `usage_totals_100k`: periods `today`, `30d` and `all`, warm (cache filled) and cold (no
    cache). Target: warm under 50 ms, cold under 1 s.
  - `latency_24h_100k`: on 100,000 records over 30 days and on 100,000 in one day (the worst
    case for a 24 h window). Target: under 1 s.
- The dashboard `pages` bench gains `/usage?period=all` and `/` (landscape).

## R13. Testing

- **Unit** (engine): `nearest_rank`; the first-token attempt over fallback, continuation and
  restart records; window selection by day segment across midnight and daylight-saving changes;
  `entry_at` against the schedule rules (`holds` cases) and the override; cost with missing cache
  and output rates; the cache invalidated on growth, rewrite and generation change; the ring merge.
- **Unit** (server views): period resolution with a fixed zone; text and JSON of both views;
  `check_harness`.
- **Hand-figure fixture** (SC-002): a home whose records have totals, costs and percentiles worked
  out by hand in the test file. Both views must match it exactly, cost to the cent.
- **Agreement** (SC-001): extend 009's dashboard agreement suite. For each period and for the
  latency view, the page values equal the CLI's `--json` at the same `at`.
- **Tag neutrality** (SC-006): the same requests with a tagged and an untagged key give equal
  placements, upstream bodies and responses (mock provider).
- **Style** (SC-007): the new tokens (agent palette, gauge, pipe, graph edge) carry 9router
  sources; the style-source test covers them.
- **Secrets** (SC-010): 009's sentinel scan covers the new pages and both CLI outputs.
- **CI**: everything except the benches and the style-source test runs in GitHub Actions; no
  local cargo (project rule).

## Brief rows touched

Row 9 (R1, R2), 20 (R11, slots replaced), 23 (R3, R6), 24 (R2, R3, R4), 25 (R5), 26 (R7), 27 (R8,
R9), 28 (R10, R11), 29 (out of scope; per-unified-model latency deferred by clarify Q2). Clarify Q5
extends row 26 with `keys tag`; that adds to the row and contradicts none.
