# Research: Routing Decision and Persistent Request History (slice 006)

Phase 0 output of `/speckit-plan`. Every item is a technical decision Claude made, per the
user's standing direction that technical choices are Claude's. Items that change something the
user can see are marked **(user-visible)**. Decisions the user made are cited, not re-decided:
the approved routing design (2026-09-28), the quota meter scheme (2026-09-28), the scope brief's
ledger, and spec Clarifications Q1–Q4 (2026-10-03).

Sources:
- the scope brief `specs/briefs/2026-10-03-routing-decision.md` (ledger rows 1–30, P notes);
- spec Clarifications 2026-10-03 (Q1 durability, Q2 full disk, Q3 fingerprint expiry, Q4 5-hour
  amortization default);
- agentmemory: "CONSOLIDATED unified-model routing design" and its 2026-09-28 amendments, the
  quota meter scheme, agent identity layers, request-size conversion;
- the current 0router code at `7cd82d9` (`plan.rs`, `attempt.rs`, `records.rs`, `quota/`,
  `cooldown.rs`, `accounts.rs`, registry `schema/`);
- `ref/9router` at `39e36d3`: `src/sse/services/auth.js` `getProviderCredentials` (fill-first,
  sticky round-robin) and `open-sse/services/combo.js` (fallback, round-robin).

No NEEDS CLARIFICATION remains.

---

## R1. Dependencies

**Decision**: no new crates.

| Need | Covered by |
|---|---|
| Prefix fingerprints | `sha2` (engine already), salted (R3) |
| Salt for fingerprints | `getrandom` |
| Record ids, time-sortable | `ulid` (slice 003) |
| Journal lines | `serde_json` |
| Background writer | `std::thread` + `std::sync::mpsc`, acks over `tokio::sync::oneshot` |
| File locks | `std::fs::File::lock` (MSRV 1.89, slice 005) |
| Price schedule times | UTC or a fixed offset (`+08:00`), parsed by hand; no time-zone database |
| Seeded randomness in the simulation | a 20-line SplitMix64 in the test crate |

**Rationale**: the work is arithmetic over a few accounts, hashing, and appending lines to
files. A database (SQLite) would add a C build dependency and a second durability model next
to slice 005's JSONL history. A time-zone crate is unnecessary because providers publish
off-peak prices in UTC.

**Alternatives**: `rusqlite` (rejected: C dependency, and range scans over daily JSONL segments
are enough at tens of thousands of records a day); `blake3` (kept in reserve if the hashing
bench misses its budget, R19); `chrono-tz` (rejected: fixed offsets cover every published
schedule).

## R2. Where the work lives

**Decision**:

| Crate | Additions |
|---|---|
| `nullrouter-registry` | plugin `[routing]` section (cache, windows, price schedule) with gate rules; `config.toml` `[routing]` (amortization default, per-target overrides); the unified-model limits note in the load report |
| `nullrouter-engine` | `routing/` (pure decision core, no I/O: fingerprints, warm store, meters, pace, ledger, placement, prices); `journal/` (writer thread, record segments, routing-state journals, recovery, prune); `plan.rs` reduced to candidate discovery; `attempt.rs` walks the placement's order and writes decisions |
| `nullrouter-server` | operator ops `routing.view`, `routing.reload-state`; records ops read the journal |
| `nullrouter-cli` | `routing` command (view, set, window); `accounts priority`; `records` reads disk, adds `prune` and `forget` |

The decision core takes `now: SystemTime` and a state snapshot as arguments and returns a
`Placement`. It never reads a clock, a file or the network, so the simulated week (R17) drives
it directly at full speed, and the engine wires it to the real clock and journal.

## R3. Prompt-prefix fingerprints

**Decision**:
- **Boundaries**: a request's prefix chain is `h0 = H(salt, model, tools, system)`, then
  `hk = H(h(k−1), message k)` for each message. Each `hk` is one fingerprint with its prefix
  length in estimated tokens. The tools and system block alone is `h0`, so a new session that
  reuses them is warm (spec US1 scenario 2).
- **Input**: the IR after the client style's decode (`ir::Request`: `tools`, `system`,
  `messages`), hashed field by field with type tags. Hashing the IR rather than raw bytes makes
  the chain style-independent, so an agent that switches client style keeps its warm state.
  `cache_control` markers aren't hashed: they don't change what the provider caches as content.
- **Key**: the upstream model is part of `h0`. Provider caches are per model, so a prefix warm on
  `claude-sonnet-4-5` is not warm for `claude-opus-4-1` on the same account.
- **Hash**: SHA-256 over `salt ‖ data`, truncated to 128 bits. The salt is 32 random bytes in
  `routing/salt` (0600), created on first start. Without the salt, a hash can't be checked
  against a guessed prompt on another machine (SC-012 "zero prompt text": hashes only).
- **What becomes warm**: after an attempt succeeds, the fingerprints written depend on the
  provider's declared cache mode (R16):
  - `explicit` (Anthropic Messages): only boundaries at or before the last part or tool that
    carried a cache marker in the request as sent. A request with no marker writes nothing,
    because the provider cached nothing;
  - `automatic` (OpenAI-style, Gemini implicit): every boundary whose prefix is at least the
    declared minimum (`min_tokens`, default 1024);
  - `none`: nothing is written, and the provider's requests are always cold.
- **Lifetime**: a fingerprint is warm while `now − last_used ≤ lifetime`. The lifetime is the
  account's override, else the plugin's `cache.lifetime`. Under `explicit` mode, a marker's own
  TTL wins when the style declares where it sits (Anthropic's `cache_control.ttl = "1h"`), read
  through a style-data path, not core code. Using a warm prefix refreshes `last_used`, as
  provider caches do.
- **Expiry** (Clarifications Q3): an expired fingerprint is deleted. A sweep runs every minute
  and at compaction (R12).
- **Longest prefix wins**: lookup walks the chain from the longest boundary down and stops at the
  first hit among the target's candidates. Equal lengths can't occur for one chain position; two
  accounts holding the same boundary are ordered by `last_used`, newest first.

**Rationale**: the boundary chain gives exact longest-prefix matching with one hash per message,
and the per-provider cache mode stops 0router from counting warmth a provider never created (an
Anthropic request without markers caches nothing).

**Alternatives**: hashing only system and tools (misses conversation warmth, the bulk of cached
tokens in long sessions); hashing raw client bytes (breaks across client styles and is sensitive
to key order); per-token rolling hashes (precise, but far more state for no routing gain).

## R4. Warm placement **(user-visible)**

**Decision**: for a request, look up the agent's longest warm prefix among the target's candidate
`(provider, account, model)` triples (R3). The warm candidate is used unless:

| Condition | Result | Record reason |
|---|---|---|
| account out of service (disabled, removed, needs sign-in, refused) | warm entry ignored | `warm_unusable` with the account state |
| account cooling down after 429/5xx, or a short window refuses (R5) | move | `moved_for_capacity: rate_limited` |
| any window at or below its reserve floor | move | `moved_for_capacity: reserve_floor` |
| warm account is pay-as-you-go and some subscription has weight > 0 | move | `left_pay_as_you_go` |

A moved request is placed as cold work (R8, R9), excluding the account it moved from. Priority
never moves warm work, and priority 0 doesn't either (spec edge case).

The record carries `warm: {provider, account, prefix_tokens, idle_s}` for every request with a
warm hit, whether it stayed or moved.

**Rationale**: brief rows 3, 8, 9 and FR-009 list exactly these moves. A move for a cooldown
mirrors slice 003's skip of a cooling account, now with a named reason.

## R5. Quota windows for routing

**Decision**: each account's windows come from up to two sources, matched by window name:
- **reported**: slice 005's polled windows (`remaining`, `limit`, `unit`, `resets_at`);
- **declared**: the plugin's `[[routing.window]]` meters (R16), with account overrides.

A meter supplies what a report lacks:
- `length`, which pace needs;
- `capacity` in weighted tokens or requests, which converts percent and credits into tokens;
- `token_weights` and `model_multiplier`, which give the cost of a request in the window's unit
  (the approved quota meter scheme);
- `reserve`, the floor;
- `reset` (`rolling`, `fixed` with an anchor, or `first_use`), for estimated windows only.

The account's kind follows:

| Account has | Kind | Routing view label |
|---|---|---|
| a `[quota]` report with at least one good poll | subscription | `polled` (`stale` when the last poll failed) |
| no report, and declared meters with a capacity | subscription | `estimated` |
| a report declared but not yet polled | subscription, on pace (π = 1), r from the declared capacity | `polled (pending first poll)` |
| neither | pay-as-you-go | `pay-as-you-go` |

**Short windows** (length under 1 hour, such as per-minute requests or tokens) only admit or
refuse. 0router counts its own traffic against declared short limits in a sliding minute, and
treats a 429 cooldown (slice 003) as a refusal. They never enter π or r (FR-017).

**Windows without a reset** (grok-cli's `prepaid` and `on-demand` credit balances) have no time
dimension. They admit while above their floor, and don't pace.

**Missing capacity**: a percent or credits window with no declared capacity is paced in its own
unit, which is enough between accounts of the same provider. Across providers, its r uses the
median capacity of the other declared windows of the same length, and the routing view marks it
`capacity assumed`. `nullrouter check` lists such windows.

**Reserve floor default**: 5% of the window when neither the plugin nor the operator sets one.

**Rationale**: polls give exact remaining fractions but not window lengths or token sizes, and
the routing math needs both. Declaring them per window in the plugin, overridable per account,
is the "plugins declare, operator overrides" constraint. The 1-hour threshold keeps every
known subscription window (5-hour and up) in pacing and every rate limit out.

**Alternatives**: inferring lengths from consecutive `resets_at` values (wrong for `first_use`
windows, and unavailable until a reset has been seen); requiring capacity everywhere (blocks
bundled providers whose sizes aren't published; 007 fits them).

## R6. Quota between polls **(user-visible)**

**Decision**:
- `remaining_now = remaining_at_poll − cost(traffic since the poll)`, where cost applies the
  window's meter to slice 005's per-model tally (input, output, cache read, cache write, request
  count). Percent windows convert through capacity. It never goes below 0.
- A poll replaces the estimate, and the tally restarts from that poll, as in slice 005.
- When `resets_at` passes before the next poll, the window counts as reset: full remaining,
  next `resets_at = old + length`. The next poll confirms or corrects it (FR-022).
- An estimated window starts from capacity at its reset and subtracts 0router's own counted cost
  since then. `rolling` windows subtract the cost inside the trailing `length`. `fixed` windows
  reset at `anchor + n × length`. `first_use` windows start at the first request after the
  previous window ended.
- A failed poll keeps the estimate running from the last good poll. The view shows the poll's
  age and `stale`.
- **Crash recovery**: slice 005 checkpoints the tally every 10 s. 006 recovers the remainder
  from the record journal: every `attempt` line carries provider, account, model and usage
  (R11), so start-up adds the usage of attempts after the checkpoint's time. A crash loses no
  counted traffic.

**Rationale**: brief row 12 and FR-021. The tally already has the token categories the meter
weighs, so the estimate is arithmetic over data 005 keeps.

## R7. Pace, rate and weight

**Decision** (the approved formulas, with the open constants fixed):
- Per window: `π = (remaining/capacity) ÷ (time_left/length)`, and
  `r = remaining_tokens ÷ time_left`. `time_left` has a floor of 60 s, so r stays finite just
  before a reset.
- Request-counted windows: `r_tokens = r_requests × size(request)` (brief row 18). `size` is the
  request's estimated input tokens (below), with a floor of 1 token.
- Per account: `π = min` over its pacing windows, and `r = min` over its pacing windows.
- `weight = r × min(π, 10) × priority`. The cap of 10 limits how far a nearly expired, unused
  window can draw cold work.
- Weight is 0 when any window is at or below its floor, an admission window refuses, the account
  is cooling down, or priority is 0.
- `share = weight ÷ Σ weights` over the target's eligible subscription accounts, recomputed at
  each decision.

**Request size**: the estimated input tokens are the token count recorded at the longest known
prefix of this agent's chain (the fingerprint store keeps the usage-reported input tokens per
boundary), plus `wire::estimate` over the remainder. A cold request uses `wire::estimate` alone.
The estimate is known at decision time, so no forecast is involved (FR-004).

**Rationale**: these are the 2026-09-28 formulas. The constants (cap 10, 60 s floor, size floor
1) were Claude's open technical choices in that design.

## R8. Deficits and cold placement **(user-visible)**

**Decision**:
- **Scope**: a ledger per `(target, account)`. Quota windows belong to the account and are shared
  across targets. Deficits are per target, because shares are computed among a target's
  candidates.
- **Cold work only**: only cold placements move deficits. This matches SC-001, which measures cold
  work against its target. Warm traffic still lowers an account's pace, and therefore its share,
  through its quota.
- **Unit of work**: plain tokens, `input + cache_read + cache_write + output`, unweighted and the
  same for every account. Shares are pure fractions, so any common unit works. Plain tokens keep
  accounts on different providers comparable, and each window's meter weights (R5) apply only to
  its own remaining quota. SC-001 counts work in the same unit.
- **Update**: when a cold request of size T is placed on account X, every eligible account gets
  `d += share × T`, and X gets `d −= T`. The sum stays 0 until a clamp engages or one account is
  reset mid-window (added, re-enabled or reprioritised). Ineligible accounts (share 0) don't
  accrue a deficit.
- **Tentative debit, settle on completion**: the update happens under the target's ledger lock at
  placement, using the estimated input size, so a concurrent request sees the debit. When the
  attempt ends, the same shares are reapplied to `T_actual − T_estimate`, where `T_actual` is the
  attempt's total tokens (unit above; output, unknown at placement, arrives here). A failed
  attempt reverses its debit, and the next candidate is debited instead. A settlement or reversal
  whose `window_start` is no longer the current window is dropped: the request stays debited in
  the window where it was placed (spec edge case).
- **Bound**: each deficit is clamped to ±2,000,000 tokens, larger than any single
  request, so a long block can't build a burst larger than that.
- **Amortization windows** (Clarifications Q4): 5 hours by default, aligned to multiples of the
  length since the Unix epoch (UTC), so the boundary is the same after a restart. At a boundary,
  every deficit of the target resets to 0. The default and per-target values live in
  `config.toml` (R13).
- **Choice**: the largest deficit among eligible accounts. Ties go to the higher share, then the
  operator's `order`, then the account name (FR-016).

**Rationale**: these are the approved deficit placement and reset rules. The tentative debit is
the brief's P note on concurrent placements. A fixed epoch alignment avoids storing the window
start and makes restarts deterministic.

## R9. Tiers and the attempt order (FR-025, FR-039) **(user-visible)**

**Decision**: one placement computes the request's whole attempt order from one snapshot:
1. the warm candidate, if it stays (R4);
2. **subscription tier**: the target's subscription accounts with weight > 0, by deficit order
   (R8);
3. **pay-as-you-go tier**: pay-as-you-go accounts with priority > 0, by their own deficit
   ledger, with shares `∝ priority ÷ price_now` (R10);
4. **last resort**: subscription accounts held back only by their reserve floor, in operator
   order. They serve rather than fail the client, and are recorded as `last_resort`.

Out-of-service accounts are recorded skips, as in slices 003 and 005. Priority-0 accounts are
left out of cold work entirely, including last resort and fallback (spec FR-039). When the order
is empty or every entry fails, slice 003's error lists every candidate with its `why_not`. Cooling accounts are recorded skips with their remaining time.
A retry or fallback takes the next entry. Deficits don't change on failure, so walking the
snapshot order equals placing again while excluding the accounts already tried.

Unified-model member order no longer decides placement. The operator's account `order` is the
final tie-break, and its equal-state behaviour matches 9router's fill-first (R20).

**Rationale**: FR-039 requires retry and fallback to follow these rules. Computing the order once
keeps the decision cost to one lock and one sort per request. The last-resort tier keeps slice
003's guarantee that a client isn't failed while some account can serve.

## R10. Pay-as-you-go price schedule **(user-visible)**

**Decision**: a plugin declares `[[routing.price]]` entries, each with per-million-token prices
(`input`, `output`, `cache_read`, `cache_write`) and an optional `when`: `days` (mon–sun) and
`from`/`to` times with a UTC or fixed offset. The first matching entry wins, and an entry without
`when` is the default. The operator may override the whole schedule per account with a flat
price. Ranking uses the input price in effect at decision time (output is unknown before the
call). An account with no price anywhere ranks as price 1, with a `check` warning.

**Rationale**: brief row 11. The input price is the one known quantity at decision time
(FR-004), and published off-peak schedules (DeepSeek, for example) are stated in UTC.

## R11. Persistent request records **(user-visible)**

**Decision**:
- **Files**: `records/YYYY-MM-DD.jsonl` (UTC day of arrival), 0600, directory 0700. One segment
  per day makes `prune --before DATE` mostly a file deletion.
- **Lines**: `open` (id, arrived, agent, style, op, type, target), one `attempt` line per ended
  attempt (with its placement reason, latency and usage), `decision` (the candidate table, R15,
  once per request), and `close` (outcome, served_by, usage, ttft, total, break handling, job).
  The record is the fold of its lines (contracts/record-journal.md).
- **Writer**: one dedicated thread owns every journal file. Request tasks send lines over a
  channel, and never block the async executor (Architecture constraint). `open` and `close` wait
  for the writer's ack, which comes after `write(2)` returns. `attempt` and `decision` lines don't
  wait.
- **"Finished" means written**: the `close` line is acked before the final byte goes to the
  client (the last SSE event or the end of a non-stream body). A 0router crash after a client saw
  its answer therefore can't lose the record (Clarifications Q1). The cost is one channel round
  trip plus one page-cache write per request. The bench measures it (R19).
- **Power loss** (Q1): the writer calls `fdatasync` on dirty files at most every second, and once
  at shutdown.
- **Recovery**: at start, the server scans the last two segments for `open` lines without a
  `close`, and appends `close {outcome: "interrupted"}` for each. Those records keep their open
  and attempt lines (spec US5 scenario 2).
- **Torn lines**: a crash mid-write can leave a partial last line. Readers skip a line that
  doesn't parse, and recovery truncates a torn final line before appending (FR-037).
- **Full disk** (Q2): on `ENOSPC` or `EDQUOT`, the writer holds up to 10,000 pending lines in
  memory, counts dropped records beyond that, and retries every 5 s. `routing view`, `check` and
  the log show `records not kept since T (N requests)` until a write succeeds. Serving never
  waits on a failing disk: acks resolve with an error, and the request continues.
- **Reading**: the CLI reads segments directly, newest first, folding by id. It needs no server.
  An `open` without a `close` shows as `in progress` when the server answers on the socket, and
  `interrupted` otherwise.
- **Pruning** (FR-035): `records prune --before DATE` deletes whole older segments and rewrites
  the boundary day. `records forget --account P/N | --agent KEY` rewrites the segments that
  contain them. Rewrites take the journal lock, which the writer also takes per write batch, and
  use write-temp-then-rename.
- **In-memory ring**: slice 003's `RecordStore` stays as the live working set for in-progress
  updates and the socket's fast path. The journal is the durable copy.

**Rationale**: the same JSONL, 0600 and advisory-lock pattern as slice 005's history, already
proven in this codebase. Acking `close` before the final byte is the only way "a 0router crash
loses nothing" holds without an fsync per request.

**Alternatives**: one fsync per request (rejected by Q1's answer); writing the whole record only
at the end (an in-flight record would vanish in a crash instead of showing as interrupted); a
memory-mapped ring (complex torn-write handling and no gain at this volume).

## R12. Persistent routing state

**Decision**:
- `routing/warm.jsonl`: one line per fingerprint upsert (`agent`, `hash`, `provider`,
  `account`, `model`, `prefix_tokens`, `last_used`).
- `routing/ledger.jsonl`: one line per ledger change of a target (`target`, `window_start`, all
  deficits after the change).
- Both go through the same writer thread and 1-second `fdatasync` as records. Warm upserts and
  ledger lines don't wait for acks of their own. The writer handles lines in FIFO order, and a
  request's warm and ledger lines are enqueued before its `close`, so the `close` ack (R11)
  proves they were written. A 0router crash can therefore only lose the state changes of
  requests still in flight, and those records come back as interrupted. The in-memory state is
  updated under the same lock that enqueues the line, so the file order matches the order in
  which decisions saw the changes.
- **Compaction**: at start and every hour, each file is rewritten with only live state (unexpired
  fingerprints, the current window's last ledger line per target), then renamed into place.
- **Load**: at start, the files are replayed. A torn last line is dropped. Ledger lines from a
  past amortization window are ignored, as if reset.
- **Removal**: `accounts remove` drops the account's fingerprints and ledger entries. `records
  forget --agent` also drops that agent's fingerprints.

**Rationale**: FR-034 keeps warm state and deficits through crashes. Append-then-compact bounds
the files by live state, and fingerprint expiry (Q3) keeps the warm file small.

## R13. Operator settings **(user-visible)**

**Decision**:
- **Per account**, in `accounts.toml` (schema 2, new optional keys): `priority` (default 1, 0 to
  never take cold work) and a `[account.routing]` table of overrides (`cache_lifetime`,
  `reserve`, `price`, and per-window `capacity`, `length`, `reserve`). Unknown keys stay refused.
- **Amortization**, in `config.toml`: `[routing] amortization = "5h"` plus
  `[routing.amortization_for]` with target → duration entries.
- **CLI**: `accounts priority P N <n>`, `routing set P N <key>=<value>…`, `routing unset P N
  <key>…`, `routing window [<target>] <duration|default>`. Each writes atomically and reloads
  over the socket, as slice 005's mutating commands do. Changes apply to the next decision
  (FR-028).

**Rationale**: priorities and overrides belong to the account, and amortization belongs to the
target. Both files already have atomic-write and reload paths.

## R14. Unified-model limits note **(user-visible)**

**Decision**: at load and reload, for each unified model, compare the members' declared
`context_length` and `max_output_tokens` (registry `Model`). When they differ, the load report
gains a note, which `check`, the reload answer of every mutating command, and `resolve <name>`
print:

```text
note: unified model sonnet: members differ in context_length: kiro 200000, openrouter 128000
```

A member without a declared value is listed as `undeclared`. The note never fails the load
(FR-031). There's no request-time check (FR-032).

## R15. The placement in the record **(user-visible)**

**Decision**: each request record gains a `decision`:
- `kind`: `warm`, `cold`, `overflow` or `none`;
- `at`;
- `amortization_window` (start and length);
- `size_tokens`;
- `warm`: the hit, if any;
- `candidates`: per account, `provider`, `account`, `model`, `tier`, `eligible`, `why_not`,
  `pace`, `rate`, `weight`, `share`, `deficit_before`, `price_now`, `priority`, `quota_source`.

Each attempt gains a `placement` with:
- `reason`: one of `warm`, `cold_by_deficit`, `moved_for_capacity`, `left_pay_as_you_go`,
  `overflow`, `last_resort`, `retry` or `fallback`;
- `rank`: its position in the decision's order.

`records show` renders the table, and `--json` prints it whole. Recomputing a cold choice means
taking the eligible candidate with the largest `deficit_before`, using the tie order. That is
SC-006's check.

**Rationale**: FR-030 and SC-006. A candidate table holds a few accounts with a dozen numbers
each, about 1 KB per request.

## R16. The plugin `[routing]` section and bundled values **(user-visible)**

**Decision**: a new optional section with closed fields (contracts/routing-schema.md):
`[routing.cache]` (`mode`, `lifetime`, `min_tokens`), `[[routing.window]]` meters, and
`[[routing.price]]`. Gate rules:
- durations are positive;
- reserve is 0–50%;
- weights are ≥ 0;
- a window's `name` is unique and, when the plugin has `[quota]`, may name a reported window;
- `reset` is only for windows with no report;
- prices are ≥ 0.

The section holds no URL, header or secret, so any plugin may declare it, community plugins
included. The fit check doesn't restrict it.

Bundled values (initial; the operator overrides, and 007 fits them):

| Provider | cache | windows | price |
|---|---|---|---|
| anthropic | explicit, 5m (marker `ttl` honoured) | 5-hour (5h), weekly (7d), weekly per model (7d), capacities from 005 history | API-key accounts: published per-Mtok prices |
| xai | automatic, 5m, min 1024 | none reported; key accounts are pay-as-you-go | published prices |
| grok-cli | automatic, 5m | monthly included (fixed monthly), weekly SuperGrok (7d); prepaid and on-demand admit only | — |
| opencode-go, opencode-zen | automatic, 5m | rolling (5h), weekly (7d), monthly (30d) | — |
| openrouter | automatic, 5m | none; pay-as-you-go | per model where declared, else default |
| elevenlabs | none | none | per character expressed as input |

Each bundled value carries a comment naming its source (provider docs, or "fit from 005 history
on DATE"). A value can't be confirmed in CI. The live check (R18) compares the routing view with
real polls.

## R17. The simulated week

**Decision**: `crates/nullrouter-engine/tests/sim_week.rs` drives the decision core and the
journals on a simulated clock:
- **Accounts**:
  - two anthropic-shaped subscriptions (5-hour and weekly percent windows, different capacities);
  - one request-counted subscription (a daily request window);
  - one estimated account (declared windows, no report);
  - two pay-as-you-go accounts with peak and off-peak prices;
  - a per-minute admission window on one subscription.
- **Traffic**:
  - 12 agents (8 in long warm sessions, 4 sending cold one-shots);
  - working-hours bursts and quiet nights, generated from a fixed seed;
  - request sizes from 2k to 150k tokens;
  - a simulated provider that charges each window exactly by the meter and reports cache reads
    when the prefix is warm on that account.
- **Polls**: every 10 simulated minutes, with exact values. Between polls, the estimate runs.
- **Restart**: at a random point, the journals are flushed as a crash would leave them (written
  but not synced), the state is dropped and reopened from the files, and the run continues. A
  second run without the restart must place identically from that point (SC-005).
- **Checks**: SC-001 to SC-006. The run writes a report table: per window, target share, actual
  share, remaining at reset.
- **Runtime**: about 60,000 requests in under 2 minutes in release mode, under 5 in debug. CI
  runs it in release.

**Rationale**: brief row 21. Driving the pure core with the real journal covers the decision and
its durability without an HTTP stack. Real-clock end-to-end coverage comes from the integration
tests (R18).

## R18. Test strategy

**Decision**:
- **Unit tests** in `routing/`: fingerprint chains and cache modes; π, r and weight edge cases
  (zero time left, request windows, priority 0, floor); ledger sum-to-zero and clamp; tie order;
  tiers; price schedule matching; meter cost.
- **Engine integration** (`crates/nullrouter-engine/tests/`) with the mock upstream, extended to
  report cache reads per account:
  - `routing_warm.rs`: stays, the four move reasons, longest prefix;
  - `routing_cold.rs`: shares, admission, priority, reset;
  - `routing_overflow.rs`;
  - `records_journal.rs`: crash recovery with a killed child process, torn line, full disk via a
    tiny tmpfs or a fault-injecting writer, prune and forget;
  - `routing_state.rs`: restart keeps warm state and deficits.
- **Failure injection**: slice 003's matrix rerun with several accounts (SC-010).
- **Harnesses** (`tests/harness`, `NR_HARNESS=1`): the Python OpenAI SDK (chat style) and Claude
  Code (messages style) run warm, cold and overflow scenarios against mock accounts. The Node SDK
  runs the standing smoke. Checked: zero client errors, and cache reads on warm requests (SC-009).
- **Secret and prompt sentinel**: slice 003's sentinel extended to the journals, routing files
  and `routing` output. It also searches for a planted prompt string (SC-012).
- **Live check L7** (opt-in, the operator's real accounts): `nullrouter quota poll` on each
  polled account, then `nullrouter routing --json`. The remaining amounts must equal the polls.
  Then one tiny request per account, after which the estimate must drop by its metered cost
  (SC-007). Documented in `docs/operator-config.md` "Live checks".
- **Parity**: no placement oracle exists (R20). The deviation rows in `tests/parity/deviations.toml`
  assert that 9router's fill-first order is our tie order in equal state.

## R19. Performance

**Decision**: the engine bench gains:
- `route/decide` with 2, 8 and 32 candidates;
- `route/fingerprint` over 10, 100 and 400 messages (about 200k tokens);
- `journal/close_ack`;
- `ttfb/routed`, the full request path with the journal on.

The budget is SC-013 (≤ 5 ms at p95 added over slice 005, measured with the same mock). If
fingerprinting of 400 messages misses the budget, two options are open: per-message hashes cached
by `(agent, chain position)`, or `blake3`. The performance gate in the constitution applies.
Baselines go in `specs/006-routing-decision/bench-baseline.md`, as in slice 005.

## R20. 9router reference and deviations

**Decision**: 9router has no quota-paced placement, so there is no parity target for the rule
itself. What carries over:
- **Equal state**: fill-first by `priority` (`getProviderCredentials`) equals our tie order by
  account `order` when shares and deficits are equal.
- **Not adopted**: sticky round-robin (`stickyRoundRobinLimit`), because warm placement replaces
  stickiness, and combo round-robin, because deficits replace rotation.
- **Unchanged**: error classification, cooldowns and retry semantics from slice 003
  (Constitution VI). Only the order changes.

Deviation rows (asserted in `tests/parity/deviations.toml`):
- D-006-1: account order comes from quota pace, not fill-first;
- D-006-2: unified members aren't tried in declared order;
- D-006-3: no sticky round-robin.
