---

description: "Task list for 006-routing-decision"
---

# Tasks: Routing Decision and Persistent Request History

**Input**: Design documents from `specs/006-routing-decision/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included. The spec's failure conditions and success criteria (SC-001 to SC-013) are
test outcomes: the simulated week, crash tests, harness runs, the sentinel and the bench. Within
each story, write the tests first and confirm they fail before implementing.

**Organization**: Tasks are grouped by user story, so each story can be implemented and tested
on its own.

**Phase order**: stories follow the spec's priorities, with one exception. US7 (P2, the
between-poll estimate and estimated accounts) comes before US6 (P1, the simulated week), because
the week checks estimated accounts and between-poll pacing and can't pass without them. Task IDs
are in execution order.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US8)

## Path Conventions

Cargo workspace at the repo root ([plan § Project Structure](plan.md#project-structure)):

- `crates/nullrouter-registry/`: `[routing]` schema, gate, config `[routing]`, limits note (sync, no tokio)
- `crates/nullrouter-engine/src/routing/`: the pure decision core. No I/O, no clock reads: `now` is
  an argument
- `crates/nullrouter-engine/src/journal/`: the writer thread, record segments, routing-state files
- `crates/nullrouter-engine/src/testkit/`: mock upstream and mock quota, behind the `testkit` feature
- `crates/nullrouter-server/`: operator socket ops, relay close ack, end-to-end tests
- `crates/nullrouter-cli/`: `routing`, `accounts priority`, `records` offline, `check`

Build commands need `export CARGO_HOME=$PWD/.cargo-home`. Respect the build budget: `-j 2`,
`nice`, one crate at a time, never workspace test and clippy chained.

**Live checks** (tasks marked *operator-run*) need the operator's real accounts. Ask the user to
run the given command with `! …` in the session. Never ask for tokens, codes or keys.

---

## Phase 1: Setup (Shared Infrastructure)

- [X] T001 Before any engine change, record a Criterion baseline of the current engine bench (`nice cargo bench -p nullrouter-engine -j 2 -- --save-baseline pre-006`) and write the `ttfb/direct` p95 into a new `specs/006-routing-decision/bench-baseline.md`. It is SC-013's reference ("measured against slice 005 on the same machine").
- [X] T002 [P] Create the engine module skeletons with `//!` headers only, wired into `crates/nullrouter-engine/src/lib.rs`: `src/routing/mod.rs` (`fingerprint.rs`, `warm.rs`, `meter.rs`, `pace.rs`, `ledger.rs`, `price.rs`, `place.rs`) and `src/journal/mod.rs` (`writer.rs`, `records.rs`, `state.rs`). Add a `no_io` unit test in `src/routing/mod.rs` that reads each `routing/*.rs` source with `include_str!` and fails if it mentions `std::fs`, `std::net`, `tokio` or `SystemTime::now` (plan: "pure, clock passed in").
- [X] T003 [P] Create `crates/nullrouter-registry/src/schema/routing.rs` with a `//!` header, re-exported from `schema/mod.rs`.
- [X] T004 [P] Create `crates/nullrouter-cli/src/cmd/routing.rs` with an empty `Command` enum and register the `routing` subcommand in `crates/nullrouter-cli/src/cmd/mod.rs`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**⚠️ No user story can start until this phase is complete.**

### Registry: plugin `[routing]` and config `[routing]`

- [X] T005 Define `RoutingDecl` in `crates/nullrouter-registry/src/schema/routing.rs` per [contracts/routing-schema.md](contracts/routing-schema.md), `deny_unknown_fields` everywhere: `cache` (`mode` = `explicit` | `automatic` | `none`, `lifetime` duration, `min_tokens` integer); `window[]` (`name`, `length`, `unit` = `weighted_tokens` | `requests`, `capacity?`, `token_weights` `{input, output, cache_read, cache_write}` defaulting to all 1, `model_multiplier` glob → factor, `reserve` percent, `reset` = `rolling` | `fixed` | `first_use`, `anchor`); `price[]` (`input`, `output?`, `cache_read?`, `cache_write?` per Mtok, `when` `{days?, from?, to?, offset?}`). Percent values parse from `"5%"`. Durations reuse `schema/duration.rs`.
- [X] T006 Add `routing: Option<RoutingDecl>` to `PluginFile` and `ProviderEntity` in `crates/nullrouter-registry/src/schema/plugin.rs`, and `ProviderEntity::routing()` returning the effective declaration with the contract's defaults: "`[routing.cache]` missing → mode `automatic`, lifetime 5m, min_tokens 1024"; reserve "5%"; no price → `None` (ranks as price 1 in the engine). `admission` is derived, never declared: "`length < 1h`".
- [X] T007 Gate rules in `crates/nullrouter-registry/src/validate/gate.rs`, from the contract's table: `cache.mode` "required when `[routing.cache]` is present"; `cache.lifetime` "> 0; ≤ 24h"; `cache.min_tokens` "≥ 0; only with `automatic`"; `window[].name` "unique within the plugin; 1–64 chars; may use `*` to match reported names"; `length` "> 0"; `unit` required; `capacity` "> 0"; `token_weights` "keys ⊆ `input`, `output`, `cache_read`, `cache_write`; values ≥ 0; only with `weighted_tokens`"; `model_multiplier` "numbers > 0"; `reserve` "0–50%"; `reset` "only for a window no `[quota]` rule names; `fixed` needs `anchor` (`HH:MM±hh:mm`, or `D HH:MM±hh:mm` for monthly or weekly)"; prices "≥ 0; `input` required"; `when.days` "⊆ `mon`…`sun`"; `when.from`/`to` `HH:MM`; `offset` `±hh:mm`; `price[]` "at most one entry without `when`, and it comes last". The fit check (`fit.rs`) stays unchanged: community plugins may declare `[routing]`.
- [X] T008 [P] Gate corpus: invalid cases under `crates/nullrouter-registry/tests/gate/invalid/providers/` (lifetime `25h`, `min_tokens` with `explicit`, duplicate window name, reserve `60%`, `token_weights` on a `requests` window, `reset` on a window a `[quota]` rule names, `fixed` without `anchor`, two default prices, default price not last, `days = ["funday"]`) with golden `.expected` messages; extend `crates/nullrouter-registry/tests/gate.rs`. Add a valid community plugin with `[routing]` to `crates/nullrouter-registry/tests/community.rs` and assert it installs.
- [X] T009 [P] `config.toml` `[routing]` in `crates/nullrouter-registry/src/schema/config.rs`: `amortization` (duration, default `5h`, Clarifications Q4) and `[routing.amortization_for]` (target → duration, target is a unified model name or `provider/model`). Durations > 0. A key naming no unified model or known `provider/model` is refused with the gate's message.

### Engine: accounts, core types, records

- [X] T010 Account fields in `crates/nullrouter-engine/src/accounts.rs` per [data-model § Provider account](data-model.md#provider-account-extended): `priority` "number ≥ 0; default 1; 0 = never cold work" and `[account.routing]` overrides `cache_lifetime` (duration), `reserve` (percent), `price` `{input, output?, cache_read?, cache_write?}` ("the plugin's whole price schedule"), `window.<name>` `{capacity?, length?, reserve?}`. `parse` keeps schema 2 and refuses unknown keys; `to_toml` writes the keys only when set. Override values are validated with the T007 rules; "a window name that matches no window is refused" at engine load (it needs the provider's declaration). Unit tests for defaults, round trip and refusals.
- [X] T011 Core types in `crates/nullrouter-engine/src/routing/mod.rs`, serialized exactly as in [contracts/record-journal.md](contracts/record-journal.md): `CandidateKey {provider, account, model}`; `Tier` (`subscription` | `payg`); `QuotaSource` (`polled` | `estimated` | `pay-as-you-go`, with display states `pending first poll` and `stale`); `WhyNot` (`out_of_service`, `cooling`, `admission`, `reserve_floor`, `priority_zero`, `not_a_subscription`); `PlacementReason` (`warm`, `cold_by_deficit`, `moved_for_capacity`, `left_pay_as_you_go`, `overflow`, `last_resort`, `retry`, `fallback`); `WarmHit {provider, account, model, prefix_tokens, idle_s, stayed, moved_because?}` with `moved_because` ∈ `warm_unusable`, `rate_limited`, `reserve_floor`, `left_pay_as_you_go`; `CandidateRow` (every field of the data model's `candidates[]`); `Decision {kind = warm | cold | overflow | none, at, amortization_window {start, length}, size_tokens, warm?, candidates, order}`; `Placement {decision, steps: Vec<(CandidateKey, PlacementReason, rank)>}`; and a `RoutingInput` snapshot struct (candidates with their account, plugin declaration, poll windows, tally, cooldown state, priority, overrides) so every routing function takes `(&RoutingInput, now: SystemTime)`.
- [X] T012 Records (after T011, whose `Decision` type it uses) in `crates/nullrouter-engine/src/records.rs`: `RequestRecord.decision: Option<Decision>`; `Attempt.placement: Option<{reason, rank}>`; `Outcome::Interrupted` ("a crash cut it short; set at recovery"). Serde shapes match the journal contract.
- [X] T013 Window state in `crates/nullrouter-engine/src/routing/meter.rs` per [data-model § Quota window state](data-model.md#quota-window-state-in-memory-derived-r5r7) and research R5, without the between-poll estimate (US7): match reported windows (slice 005 polls) to declared meters by name (`*` globs allowed); `role` = `pacing` | `admission` (length under 1 h) | `balance` (no reset: admit only); `reserve` = "override → meter → 5%"; account kind per the R5 table (`polled`, `polled (pending first poll)` with π = 1 and r from the declared capacity, `pay-as-you-go`); `at_floor()`; admission from a sliding-minute count of 0router's own traffic against declared short limits, and from slice 003 cooldowns (`cooldown.rs`). Unit tests for each role and kind.

### Engine: journal writer and state

- [X] T014 Journal writer in `crates/nullrouter-engine/src/journal/writer.rs` and the `Journal` handle in `journal/mod.rs`, per research R11–R12 and [contracts/record-journal.md](contracts/record-journal.md): one dedicated `std::thread` reading a `std::sync::mpsc` channel; line types `open`, `decision`, `attempt`, `close`, `warm`, `ledger`, each `{"v":1,"t":…}`; `open` and `close` return a `tokio::sync::oneshot` ack sent after `write(2)` returns, other lines don't wait; FIFO across all files; record lines go to `records/YYYY-MM-DD.jsonl` by UTC day of arrival, routing lines to `routing/warm.jsonl` and `routing/ledger.jsonl`; files 0600, directories 0700; `records.lock` taken per write batch; `fdatasync` on dirty files "at most every second, and once at shutdown"; after creating a file (a new daily segment), `fsync` its parent directory so a power loss can't lose the whole file; on `ENOSPC` or `EDQUOT`, hold "up to 10,000 pending lines in memory", count records dropped beyond that, retry "every 5 s", resolve acks with an error; `Health {kept, since, unkept_requests, last_sync}`. A `testkit` fault hook makes writes fail on demand and can pause acks. Unit tests: ordering, ack after write, sync cadence with a fake timer, fault and recovery.
- [X] T015 Engine wiring in `crates/nullrouter-engine/src/state.rs` and `crates/nullrouter-server/src/serve.rs`: `Engine` gains `journal: Journal` and `router: routing::Router` (the warm store and ledgers behind locks, and the salt); `routing/salt` is "32 random bytes, 0600, created once" with `getrandom`; `serve` starts the writer before listening and on shutdown drains it and runs a final `fdatasync`. Tests and the CLI build an `Engine` over a temp `$NULLROUTER_HOME`.

### Engine: testkit

- [X] T016 Mock upstream cache simulation in `crates/nullrouter-engine/src/testkit/mock_upstream.rs`: per `(account key, model)`, remember the prefixes sent (by content hash, in the mock only) with their time; report `cache_read` tokens for the longest previously sent prefix within a per-test lifetime and `cache_write` for the rest, honouring explicit markers for Messages-style requests (cached only up to the last marker) and automatic caching otherwise; per-account scripted 429s (from the Nth request, for a duration); per-account counters of requests and cache reads, readable by tests.
- [X] T017 Mock quota windows (after T016, whose served usage it follows) in `crates/nullrouter-engine/src/testkit/mock_quota.rs`: a generic endpoint answering per account with windows `{name, unit, used, limit, resets_at}` that tests set and that drop automatically by the mock upstream's served usage, weighted by a per-test meter, so "the between-poll estimate equals the provider's own figure at every poll" is checkable (SC-008). Add target builders to `crates/nullrouter-engine/tests/common/mod.rs`: a unified model or direct target over N mock accounts with given kinds, priorities, meters and prices.

**Checkpoint**: `[routing]` loads or is refused with gate messages; accounts carry priority and
overrides; window state is derived from polls and meters; journal lines reach disk in order with
acks; the mock reports cache reads.

---

## Phase 3: User Story 1 — Warm requests stay where their cache is (Priority: P1) 🎯 MVP

**Goal**: Each agent's follow-up requests stay on the account holding its cached prefix. They
move only for capacity (rate limit, reserve floor, out of service) or to leave pay-as-you-go.

**Independent Test**: Two subscription accounts on the mock provider, two agents from two
different client harnesses through one unified model. Each agent's follow-ups stay on its first
account and the mock reports cache reads on them. Exhaust one account's rate limit: only the
agent on it moves, and its record names the rate limit.

### Tests for User Story 1 ⚠️

- [X] T018 [P] [US1] Fingerprint unit tests in `crates/nullrouter-engine/src/routing/fingerprint.rs`: the same conversation decoded from Chat Completions and Messages gives the same chain; the upstream model is part of `h0`; `cache_control` values don't change hashes; a different salt changes every hash; `explicit` mode yields only boundaries at or before the last marked part or tool, and nothing without a marker; `automatic` yields boundaries with prefix ≥ `min_tokens` (default 1024); `none` yields nothing; the serialized output is hex only and contains no substring of the prompt.
- [X] T019 [P] [US1] Warm store unit tests in `crates/nullrouter-engine/src/routing/warm.rs`: longest prefix wins; two accounts holding the same boundary → newest `last_used`; agents never see each other's entries (FR-007); warm while `now − last_used ≤ lifetime`, deleted once `now − last_used > lifetime` (Clarifications Q3); the account's `cache_lifetime` override beats the plugin's; a marker's `ttl = "1h"` beats both under `explicit`; a hit refreshes `last_used`.
- [X] T020 [P] [US1] Integration tests in `crates/nullrouter-engine/tests/routing_warm.rs` for US1 scenarios 1–7 with the mock upstream: stays whatever the deficits say (record reason `warm`); a new session with the same system prompt and tools is warm; agent 2 with identical prompts is cold; rate limit → `moved_for_capacity` with `rate_limited`; a window at its floor → `moved_for_capacity` with `reserve_floor`; warm on pay-as-you-go with an eligible subscription → `left_pay_as_you_go`; idle past lifetime → cold; longest prefix on B beats system-only on A. Edge cases: warm account disabled, removed or needing sign-in → `warm_unusable` and placed elsewhere; warm account with priority 0 stays; an embedding or image request is cold; the mock reports cache reads on every warm request.
- [X] T021 [P] [US1] Rewrite `crates/nullrouter-engine/tests/stay_warm.rs` from slice 003's last-account semantics to fingerprint semantics, keeping every scenario that still holds and deleting the ones the spec replaces ("The per-agent warm map of slice 003 … is replaced by persistent prefix fingerprints").

### Implementation for User Story 1

- [X] T022 [US1] Fingerprints in `crates/nullrouter-engine/src/routing/fingerprint.rs` per research R3: `h0 = H(salt, model, tools, system)`, `hk = H(h(k−1), message k)` over `nullrouter_wire::ir::Request`, fields hashed with type tags, `cache_control` skipped; SHA-256 (`sha2`) truncated to 128 bits; each boundary carries its estimated prefix tokens; `writable(chain, cache_mode, markers, min_tokens)` returns the boundaries to store after a success.
- [X] T023 [US1] Marker TTL through style data: declare where a marker's TTL sits in `styles/bundled/anthropic-messages.toml` (inside `cache_control`, key `ttl`), add the optional key to the style schema in `crates/nullrouter-registry/src/schema/style.rs` with its rule in `validate/style_gate.rs`, and read it in `routing/fingerprint.rs` from the IR part's `cache_control` value. No style-specific code in the core.
- [X] T024 [US1] Warm store in `crates/nullrouter-engine/src/routing/warm.rs`: entries keyed `(agent, hash, provider, account, model)` with `prefix_tokens` and `last_used`; `lookup(agent, chain, candidates, now)` walks the chain from the longest boundary down; `upsert`; `sweep(now)` deletes expired entries; `drop_account`, `drop_agent`.
- [X] T025 [US1] Warm step in `crates/nullrouter-engine/src/routing/place.rs` per research R4's table: the warm candidate stays unless out of service (`warm_unusable`, entry ignored), cooling or refused by an admission window (`rate_limited`), at or below a reserve floor (`reserve_floor`), or pay-as-you-go while a subscription is eligible (`left_pay_as_you_go`). A moved request is placed as cold excluding the account it moved from. Priority never moves warm work. Until US2, cold candidates are ordered by account `order`, then name.
- [X] T026 [US1] Reduce `crates/nullrouter-engine/src/plan.rs` to candidate discovery: the `(provider, account, model)` triples behind a unified or direct target, with out-of-service accounts as recorded skips (slice 005 reasons unchanged) and disabled accounts filtered. Remove `WarmMap` (`plan.rs:98-…`) and its uses in `state.rs:21,105,211`.
- [X] T027 [US1] Walk the placement in `crates/nullrouter-engine/src/attempt.rs` (`walk`, about line 568): fingerprint the IR once, take the `Router` lock, build `RoutingInput` and call `place` once, record `decision` on the request, and walk `Placement.steps` with slice 003's cooldown checks, classification and retries unchanged. Each attempt gets `placement {reason, rank}`; a same-account retry is `retry`, a step after a failure is `fallback`. On success (`succeed`, about line 1703), upsert the writable fingerprints with `prefix_tokens` from the attempt's usage, updating the store and enqueueing the `warm` line under the same lock (research R12).
- [X] T028 [P] [US1] `[routing.cache]` for all seven bundled plugins in `plugins/bundled/*.toml` per research R16, each value with a comment naming its source: anthropic `explicit`, `5m`; xai `automatic`, `5m`, `min_tokens = 1024`; grok-cli, opencode-go, opencode-zen, openrouter `automatic`, `5m`; elevenlabs `none`.
- [X] T029 [US1] Harness warm scenario in `tests/harness/run.sh`, `tests/harness/py/`, `tests/harness/claude.sh` and `crates/nullrouter-server/tests/harness.rs`: two mock subscription accounts behind one unified model; the Python OpenAI SDK (chat) and Claude Code (messages) each run a multi-turn session; assert zero client errors, each agent's follow-ups on one account, and mock cache reads on every warm request (SC-009, warm part).
- [X] T030 [US1] Green run: `nice cargo test -p nullrouter-registry -j 2`, then `-p nullrouter-engine`, `-p nullrouter-server`, `-p nullrouter-cli`, one at a time.

**Checkpoint**: warm work stays and moves only for the four reasons; cold work follows account
order (fill-first, as before).

---

## Phase 4: User Story 2 — Cold work spends subscription quota before it expires (Priority: P1)

**Goal**: Cold work is placed by pace-weighted shares and deficits over a 5-hour amortization
window, so each subscription window is spent before it resets.

**Independent Test**: `routing_cold.rs` with mock 5-hour, weekly and request-counted windows:
shares follow weights, admission and priority 0 exclude accounts, deficits reset at the
boundary, and ties are deterministic. The full week is US6.

### Tests for User Story 2 ⚠️

- [X] T031 [P] [US2] Pace unit tests in `crates/nullrouter-engine/src/routing/pace.rs`: `π = (remaining/capacity) ÷ (time_left/length)` and `r = remaining ÷ time_left` with `time_left` floored at 60 s; per account the minimum π and minimum r over pacing windows (scenario 2); `weight = r × min(π, 10) × priority`; weight 0 at or below a floor, when an admission window refuses, when cooling, or with priority 0; request-counted windows `r_tokens = r_requests × size` with size floored at 1 (scenario 4: the large request favours the request-counted account, the small one the token-counted account); shares sum to 1 over eligible accounts.
- [X] T032 [P] [US2] Ledger unit tests in `crates/nullrouter-engine/src/routing/ledger.rs`: work is counted in plain tokens (input + cache read + cache write + output, unweighted; research R8); deficits sum to 0 after each placement while no clamp engages and no account was reset mid-window; a separate case drives one account past "±2,000,000" and checks the clamp holds; share-0 accounts don't accrue; tentative debit then settle with `T_actual − T_estimate`, and reversal on failure, leave the same state as a single exact debit; a settlement or reversal arriving after the window has rolled is dropped and leaves the new window's deficits at 0 (spec edge case "Amortization window boundary during a request"); windows are aligned to multiples of the length since the Unix epoch (UTC) and reset to 0 at the boundary (scenario 6); a forward clock jump across a boundary resets once, and a backward jump never returns to an earlier window; an account added or reprioritised mid-window starts at 0; ties go to higher share, then account `order`, then name (scenario 7, FR-016).
- [X] T033 [P] [US2] Integration tests in `crates/nullrouter-engine/tests/routing_cold.rs` for US2 scenarios 1–7 with the mock upstream and mock quota: A (80% left, 20% time left) takes the larger share against B (50%/50%) until paces even out; a per-minute refusal skips the account with no deficit build-up (scenario 3); priority 0 receives no cold work while its warm work stays; concurrent cold requests (spawned together) don't all land on the largest deficit; the record's `decision` holds every candidate row and `order`. (Scenario 3's per-minute refusal is covered by a `place.rs` unit test: the mock has no own-traffic admission count until T076.)
- [X] T034 [P] [US2] Rerun slice 003's failure matrix with several accounts in `crates/nullrouter-engine/tests/fallback.rs`: injected 429, 5xx, timeout and connection failure on the placed account never reach the client while another account or provider could serve (SC-010); later attempts follow `Placement.steps`, excluding accounts already tried (FR-039), with reason `fallback`. Extend `crates/nullrouter-engine/tests/usage_records.rs` so each attempt's recorded usage equals the mock's reported usage on routed requests (SC-011).

### Implementation for User Story 2

- [X] T035 [US2] Pace, rate, weight and shares in `crates/nullrouter-engine/src/routing/pace.rs` per research R7, with the constants named: `PACE_CAP = 10.0`, `TIME_LEFT_FLOOR = 60 s`, `SIZE_FLOOR = 1`.
- [X] T036 [US2] Request size in `crates/nullrouter-engine/src/routing/pace.rs`: the warm store's recorded `prefix_tokens` at the agent's longest known boundary plus `nullrouter_wire::estimate::estimate` over the remaining messages; a cold request uses `estimate` alone. Recorded as `decision.size_tokens`.
- [X] T037 [US2] Deficit ledgers in `crates/nullrouter-engine/src/routing/ledger.rs` per research R8: one per `(target, tier)`, `debit(placed, shares, T)` (`d += share × T` for every eligible account, `d −= T` for the placed one, T in plain tokens), `settle` and `reverse` (both carry the debit's `window_start` and are dropped when it is no longer the current window), clamp, `window_start(now, length)`, lazy reset when `now` passes into a new window.
- [X] T038 [US2] Subscription tier and last resort in `crates/nullrouter-engine/src/routing/place.rs` per research R9: eligible subscription accounts with weight > 0 by largest deficit with the FR-016 ties; every other candidate gets a row with its `why_not`; priority-0 accounts are left out of cold work entirely, including last resort and fallback (FR-039); then "subscription accounts held back only by their reserve floor, in operator order", reason `last_resort` (FR-025a). The first cold step's reason is `cold_by_deficit`. When the order is empty or every step fails, extend slice 003's `Failure.tried` (`crates/nullrouter-engine/src/attempt.rs:166`) so the client's error lists every candidate that wasn't tried with its `why_not` (`priority_zero`, `reserve_floor`, `admission`, …), redacted as before (spec edge case "Every account blocked").
- [X] T039 [US2] Debits and settlement in `crates/nullrouter-engine/src/attempt.rs`: debit the placed account under the target's ledger lock at placement using `size_tokens`; at `end_attempt` (about line 1636) settle with the attempt's total tokens (input + cache read + cache write + output, unweighted; not the meter-weighted cost, which only lowers the window's remaining quota), or reverse on failure and debit the next step; enqueue each `ledger` line (target, tier, `window_start`, all deficits, `at`) under the same lock. The amortization length is `[routing.amortization_for]` for the target, else `[routing] amortization`.
- [X] T040 [P] [US2] `[[routing.window]]` meters for anthropic, grok-cli, opencode-go and opencode-zen in `plugins/bundled/*.toml` per research R16's table (anthropic `5-hour` 5h, `weekly` 7d, weekly per model 7d; grok-cli monthly included `fixed` monthly, weekly SuperGrok 7d, prepaid and on-demand as balance windows; opencode rolling 5h, weekly 7d, monthly 30d), names matching each `[quota]` rule, each capacity with a source comment ("provider docs" or "fit from 005 history on DATE"). xai, openrouter and elevenlabs declare none.
- [X] T041 [US2] Parity deviations D-006-1 ("account order comes from quota pace, not fill-first"), D-006-2 ("unified members aren't tried in declared order") and D-006-3 ("no sticky round-robin") in `tests/parity/deviations.toml`, asserted in `crates/nullrouter-engine/tests/routing_cold.rs`, plus an equal-state test: with equal shares and deficits, the order equals 9router's fill-first by priority (`ref/9router/src/sse/services/auth.js` `getProviderCredentials`) mapped to account `order`.
- [X] T042 [US2] Green run, crate by crate.

**Checkpoint**: cold work follows pace; the record holds the full candidate table.

---

## Phase 5: User Story 3 — Pay-as-you-go is overflow only (Priority: P1)

**Goal**: Pay-as-you-go accounts serve only when no subscription can, chosen by priority ÷
current price from the plugin's schedule or the operator's override.

**Independent Test**: Two subscriptions and two pay-as-you-go accounts with peak and off-peak
prices; traffic heavier than the subscriptions take. Pay-as-you-go serves only while no
subscription can, split by priority ÷ current price, switching when the schedule does.

### Tests for User Story 3 ⚠️

- [X] T043 [P] [US3] Price unit tests in `crates/nullrouter-engine/src/routing/price.rs`: first matching entry wins; `days`; `from`/`to` with "`to` before `from` wraps past midnight"; `offset` (default `+00:00`); the entry without `when` is the default; the account's flat `price` override replaces the whole schedule; no price anywhere ranks as 1.
- [X] T044 [P] [US3] Integration tests in `crates/nullrouter-engine/tests/routing_overflow.rs` for US3 scenarios 1–4: no pay-as-you-go placement while a subscription can serve; with every subscription rate-limited or at its floor, overflow goes to the largest priority ÷ price, and the record's rows give each subscription's `why_not`; crossing into the off-peak period changes the next decision; the override price is used; priority-0 pay-as-you-go accounts receive nothing. Scenario 5: with every subscription at its floor and every pay-as-you-go account rate-limited, a floor-held subscription serves with reason `last_resort`. With only priority-0 accounts able to serve, the client gets the error, and it lists each account with its `why_not` (`priority_zero` included).

### Implementation for User Story 3

- [X] T045 [US3] Price schedule in `crates/nullrouter-engine/src/routing/price.rs` per research R10: `price_now(decl, override, now)` with hand-parsed `HH:MM` and `±hh:mm`, no time-zone database; ranking uses the input price.
- [X] T046 [US3] Pay-as-you-go tier in `crates/nullrouter-engine/src/routing/place.rs`: after the subscription tier and before last resort, pay-as-you-go accounts with priority > 0 by their own `payg` ledger with shares ∝ priority ÷ `price_now`, reason `overflow`, `price_now` in their rows; `decision.kind = overflow` when the first step is pay-as-you-go. The warm `left_pay_as_you_go` move now uses "some subscription has weight > 0".
- [X] T047 [P] [US3] `[[routing.price]]` in `plugins/bundled/xai.toml`, `openrouter.toml` (per model where declared, else default), `anthropic.toml` (applies to API-key accounts) and `elevenlabs.toml` ("per character expressed as input"), with source comments.
- [X] T048 [US3] Harness cold and overflow scenarios in `tests/harness/` and `crates/nullrouter-server/tests/harness.rs`: three mock subscription accounts and one pay-as-you-go account; the Python SDK and Claude Code run cold one-shots and an overflow burst; the Node SDK runs the standing smoke; zero client errors (SC-009).
- [X] T049 [US3] Green run, crate by crate.

**Checkpoint**: all three tiers place; P1 routing behaviour is complete.

---

## Phase 6: User Story 4 — The operator sees and steers every decision (Priority: P1)

**Goal**: Priority and overrides from the CLI apply to the next decision; the routing view and
`records show` explain every placement.

**Independent Test**: Mixed warm, cold and overflow traffic; for sampled records the operator
names each attempt's reason from `records show` and `routing`; recomputing each cold decision
from its record gives the same account.

### Tests for User Story 4 ⚠️

- [X] T050 [P] [US4] CLI tests in `crates/nullrouter-cli/tests/routing.rs` (new) per [contracts/operator-cli.md](contracts/operator-cli.md): `accounts priority anthropic pro 0` prints `applied` with a server and `saved; applies at next start` without; `routing set … cache_lifetime=1h reserve=10% window.5-hour.capacity=12000000`, `routing unset`, `routing window sonnet 1h` and `routing window default` write the files atomically; an unknown key or bad value exits 1 with the gate's message; `routing` without a server exits 4; `accounts list` shows a `priority` column.
- [X] T051 [P] [US4] Server tests in `crates/nullrouter-server/tests/routing_view.rs` (new): the `routing.view` op returns, per target and account, pace, share, deficit, priority, cache lifetime in effect, each window's remaining amount, unit, reset time and reserve, and the quota source with the last poll time; pay-as-you-go rows carry `price_now`; `routing.health` returns the journal health; setting priority 2 changes the account's weight on the next decision without a restart (scenario 1).
- [X] T052 [P] [US4] Decision-record tests in `crates/nullrouter-engine/tests/decision_record.rs` (new): a request whose first attempt fails and second succeeds shows both attempts with reason, candidate rows, latency and usage (scenario 3); a moved warm request names the warm account and the capacity reason (scenario 4); for every cold and overflow record in a mixed run, taking the eligible candidate with the largest `deficit_before` under the tie order selects the served account's first attempt (SC-006).

### Implementation for User Story 4

- [X] T053 [US4] Routing view data in `crates/nullrouter-engine/src/routing/mod.rs`: `view(&RoutingInput, ledgers, now, target?) -> RoutingView` (pure), with per-account `cache_lifetime` in effect and per-window `unit`, `remaining_at_poll`, `cost_since_poll`, `remaining_now`, `capacity`, `reserve`, `resets_at` (FR-029). Until US7 (T076) lands, `cost_since_poll` is 0 and `remaining_now` equals `remaining_at_poll`; also emit warnings (`stale`, `capacity assumed`, records not kept). Socket ops `routing.view {target?}` and `routing.health` in `crates/nullrouter-server/src/operator.rs`, added to its `//!` op table.
- [X] T054 [US4] `nullrouter routing [target] [--json]` in `crates/nullrouter-cli/src/cmd/routing.rs`, text output exactly as in the contract (header line with amortization window and journal sync age; tier groups; a `cache` column; each window as remaining/capacity in its unit, floor and reset; warnings on their own lines); `--json` emits the full data.
- [X] T055 [US4] `routing set`, `routing unset` and `routing window` in `crates/nullrouter-cli/src/cmd/routing.rs`: keys `cache_lifetime`, `reserve`, `price.input`/`price.output`/`price.cache_read`/`price.cache_write`, `window.<name>.capacity`/`length`/`reserve`; validate with the registry gate rules; write `accounts.toml` or `config.toml` atomically; reload over the socket.
- [X] T056 [US4] `accounts priority <provider> <name> <n>` and the `priority` column of `accounts list` in `crates/nullrouter-cli/src/cmd/accounts.rs`.
- [X] T057 [US4] Reload in `crates/nullrouter-engine/src/state.rs`: a reload carries new priorities, overrides and amortization lengths into the next `RoutingInput` while keeping the `Router`'s warm store and ledgers; an account whose priority changed, or that was added or re-enabled, starts its deficit at 0 (spec edge case); a changed amortization length starts the target's new window at the next aligned boundary.
- [X] T058 [US4] `records show <id> [--json]` decision table in `crates/nullrouter-cli/src/cmd/records.rs`, text as in the contract (decision line, candidate table, attempts with `reason (rank n)`, the `warm on … · prefix … · idle … · stayed` and `moved: <reason> on <account>` forms).
- [X] T059 [US4] Green run, crate by crate.

**Checkpoint**: every placement is explainable from the CLI; overrides apply live.

---

## Phase 7: User Story 5 — History and routing state survive restarts and crashes (Priority: P1)

**Goal**: Records, warm fingerprints and deficits are on disk and survive a clean restart and a
crash; records are kept until pruned; the CLI reads them without a server.

**Independent Test**: Kill the server without warning mid-traffic and restart it. Every record
of a finished request is present, the in-flight one shows `interrupted`, warm agents stay on
their accounts, and deficits continue.

### Tests for User Story 5 ⚠️

- [X] T060 [P] [US5] Crash tests in `crates/nullrouter-engine/tests/records_journal.rs` (new) with a killed child process (the test re-executes its own binary as a mock-backed server and sends `SIGKILL` at random points): 100% of records whose client got its last byte are present and unchanged (scenario 1); in-flight records show `interrupted` with their open and attempt lines (scenario 2); a torn final line is truncated and the next line is whole (FR-037); a simulated power loss (files cut back to the last `fdatasync` offsets, and files created after the last directory `fsync` removed) loses at most the last ~1 s and nothing older, including just after a new daily segment starts; with the fault hook failing writes, clients still get answers, `Health` counts unkept requests, and writing resumes without a restart once the fault clears (FR-037a).
- [X] T061 [P] [US5] Restart tests in `crates/nullrouter-engine/tests/routing_state.rs` (new): an agent warm on A before a restart goes to A after it (scenario 3); deficits after a restart equal those before (scenario 4); expired fingerprints are dropped at load; ledger lines from a past amortization window are ignored; compaction keeps exactly the live state.
- [X] T062 [P] [US5] CLI tests in `crates/nullrouter-cli/tests/records.rs` (new): `records list` and `records show` without a server (scenario 5), with `--provider`, `--account`, `--agent`, `--model`, `--reason`, `--since`, `--limit`, `--json`; an `open` without `close` shows `in progress` with a server and `interrupted` without; `records prune --before DATE` removes only older records and prints the count (scenario 6); `records forget --account P/N` and `--agent KEY` remove those records, and `--agent` also drops that agent's fingerprints; both exit 1 when the journal lock isn't free within 10 s (shortened in test).
- [X] T063 [P] [US5] Close-ack ordering in `crates/nullrouter-server/tests/records_durable.rs` (new): with the writer's ack paused by the fault hook, a streaming client doesn't receive the final SSE event and a non-streaming client doesn't receive the end of the body; after release, the `close` line is on disk before the client sees its last byte.

### Implementation for User Story 5

- [X] T064 [US5] Record lines from the attempt loop in `crates/nullrouter-engine/src/attempt.rs`: `open` (id, arrived, agent, style, op, type, target) acked before the first upstream call; `decision` once per request; `attempt` per ended attempt (`end_attempt`); `close` (outcome, served_by, usage, ttft, total, break handling, job). An ack error (disk full) never fails or delays the request beyond the ack itself. `RecordStore` stays as the live ring (research R11).
- [X] T065 [US5] Close ack before the last byte in `crates/nullrouter-server/src/relay.rs` (hold the final SSE event until the `close` ack resolves) and `crates/nullrouter-server/src/text.rs` (the end of a non-stream body). The stream keeps relaying unbuffered up to the final event (Constitution V).
- [X] T066 [US5] Segment reader in `crates/nullrouter-engine/src/journal/records.rs`: list segments, read newest first, skip lines that don't parse, fold by id ("start from `open`, append each `attempt` in `n` order, set `decision`, then apply `close`. Later duplicates of a line type replace earlier ones"), filter by the contract's flags. No server needed.
- [X] T067 [US5] Recovery in `crates/nullrouter-engine/src/journal/records.rs`, run by `crates/nullrouter-server/src/serve.rs` before listening: scan the last two segments, truncate a torn final line, append `{"t":"close","id":…,"outcome":"interrupted","recovered_at":…}` for every `open` without a `close`.
- [X] T068 [US5] Routing state in `crates/nullrouter-engine/src/journal/state.rs`: load `routing/warm.jsonl` ("the last `warm` line per identity wins, with expired entries dropped") and `routing/ledger.jsonl` ("the last `ledger` line per `(target, tier)` wins if its `window_start` is the current window's") into the `Router` at start; compaction at start and every hour (write live state to a temp file, `fdatasync` it, rename into place, then `fsync` the `routing/` directory, under the journal lock); the expired-fingerprint sweep every minute.
- [X] T069 [US5] Prune and forget in `crates/nullrouter-engine/src/journal/records.rs`: `prune(before)` deletes whole older segments and rewrites the boundary day; `forget(account | agent)` rewrites the segments containing them; both take `records.lock` (10 s timeout) and write temp-then-rename. When a server is running, the CLI first sends the socket op `records.forget` ([contracts/operator-cli.md § Operator socket](contracts/operator-cli.md#operator-socket-added-and-changed-ops)) so the server drops the fingerprints (and, for an account, its ledger entries) from memory and `warm.jsonl` together; add the op to `crates/nullrouter-server/src/operator.rs` and its `//!` op table. Rewrites `fsync` the `records/` directory after the rename.
- [X] T070 [US5] `accounts remove` drops the account's fingerprints and ledger entries (`crates/nullrouter-cli/src/cmd/accounts.rs`, and `Router::drop_account` on reload in `crates/nullrouter-engine/src/state.rs`).
- [X] T071 [US5] Records from disk: `records.list` and `records.get` in `crates/nullrouter-server/src/operator.rs` serve the journal plus in-flight records from the ring (unchanged answer shape); `crates/nullrouter-cli/src/cmd/records.rs` reads segments directly with the contract's flags and adds `prune` and `forget`.
- [X] T072 [US5] Disk-full warnings (FR-037a): a `warn!` when writing first fails and when it resumes, plus one every minute while failing, with the unkept count; the `records not kept since T (disk full): N requests` line in the routing view (T053) and in `nullrouter check` (`crates/nullrouter-cli/src/cmd/check.rs`, through `routing.health` when a server answers).
- [X] T073 [US5] Green run, crate by crate.

**Checkpoint**: nothing a finished request produced is lost to a 0router crash; history is
readable offline and pruned only by the operator.

---

## Phase 8: User Story 7 — Quota is estimated between polls and without a quota report (Priority: P2)

**Goal**: Between polls, remaining quota is the last poll less 0router's metered traffic; each
poll replaces it. Providers without a report are paced from declared limits and shown as
"estimated".

**Independent Test**: The mock quota endpoint reports on demand. Between polls the view's
remaining equals the last report less counted traffic; at the next poll it equals the new
report. An account with declared limits and no report shows "estimated" and is paced; one with
neither is pay-as-you-go.

### Tests for User Story 7 ⚠️

- [X] T074 [P] [US7] Meter unit tests in `crates/nullrouter-engine/src/routing/meter.rs`: cost applies `token_weights` and the first matching `model_multiplier` glob; percent windows convert through capacity; request windows count 1 per request; `remaining_now` never goes below 0; a passed `resets_at` counts as reset with `resets_at = old + length` (FR-022); estimated windows reset `rolling` (cost inside the trailing `length`), `fixed` (`anchor + n × length`) and `first_use` (the first request after the previous window ended); a percent window with no capacity across providers takes the median capacity of other declared windows of the same length and is flagged `assumed`.
- [X] T075 [P] [US7] Integration tests in `crates/nullrouter-engine/tests/estimate.rs` (new) for US7 scenarios 1–5: the estimate drops by served traffic in the window's unit; a poll replaces it; a passed reset counts as reset; a no-report account with declared limits shows `estimated` and is paced; one with neither is pay-as-you-go; at every poll against the mock with no outside use, the estimate equals the mock's figure (SC-008); a failing poll keeps the estimate running and shows `stale` with the last good poll's age.

### Implementation for User Story 7

- [X] T076 [US7] Between-poll estimate in `crates/nullrouter-engine/src/routing/meter.rs`: `remaining_now = remaining_at_poll − cost(tally since poll)` over slice 005's per-model tally (`crates/nullrouter-engine/src/quota/tally.rs`); the tally restarts at each poll as in slice 005; reset roll-forward until the next poll confirms or corrects it. Fill in the view's `cost_since_poll` and `remaining_now` (T053 shows the poll's value until now) and extend `crates/nullrouter-server/tests/routing_view.rs` to assert them.
- [X] T077 [US7] Estimated accounts: the `estimated` kind in `routing/meter.rs` (no report, declared meters with a capacity); per-window counters since each window's reset kept next to slice 005's tally in `crates/nullrouter-engine/src/quota/tally.rs` (hourly buckets for `rolling` windows) and checkpointed with it by `quota/history.rs`.
- [X] T078 [US7] Tally recovery after a crash in `crates/nullrouter-engine/src/quota/history.rs`: at start, add the usage of journal `attempt` lines later than the last tally checkpoint (`CHECKPOINT_EVERY` 10 s), so "a crash loses no counted traffic" (research R6).
- [X] T079 [US7] `capacity assumed` and pending-first-poll in the routing view (T053) and `nullrouter check` (`crates/nullrouter-cli/src/cmd/check.rs`): list reported windows with no matching meter, paced in their own unit.
- [X] T080 [US7] Green run, crate by crate.

**Checkpoint**: decisions run on present-state quota between polls; providers without a report
are paced.

---

## Phase 9: User Story 6 — A simulated week proves the routing (Priority: P1)

**Goal**: A seeded simulated-clock week over real-shaped mock accounts proves SC-001 to SC-006;
an opt-in live check proves the view matches real polls.

**Independent Test**: `nice cargo test -p nullrouter-engine --release --test sim_week -j 2 -- --nocapture`
runs in under 2 minutes, passes, and prints the same report twice.

- [X] T081 [US6] Simulation model in `crates/nullrouter-engine/tests/sim_week.rs` per research R17: a 20-line SplitMix64; accounts (two anthropic-shaped subscriptions with 5-hour and weekly percent windows and different capacities, one request-counted subscription with a daily window, one estimated account, two pay-as-you-go accounts with peak and off-peak prices, a per-minute admission window on one subscription); 12 agents (8 in long warm sessions, 4 sending cold one-shots); working-hours bursts and quiet nights; request sizes 2k–150k tokens; a simulated provider charging each window exactly by its meter and reporting cache reads when the prefix is warm on that account; polls every 10 simulated minutes. It drives `routing::place`, settlement and the real journal over a temp home with an injected clock.
- [X] T082 [US6] Checks and report in `crates/nullrouter-engine/tests/sim_week.rs`: SC-001 (per account and amortization window, |cold work received − target| ≤ max(5% of all cold work placed in that window, its largest cold request); the target is the sum of share × work at each placement; work in plain tokens, input + cache read + cache write + output, unweighted); SC-002 (0 warm moves for reasons other than capacity or leaving pay-as-you-go, each move with its reason); SC-003 (0 pay-as-you-go placements while a subscription could serve); SC-004 (no subscription window of an account with priority above 0 resets above its floor while cold work in it went to pay-as-you-go or to another account while this one held the largest deficit and could serve; under excess demand every window resets within 5% of capacity of its floor); SC-006 (recompute every cold and overflow decision from its record). Print one row per account window: target share, actual share, remaining at reset.
- [X] T083 [US6] Restart and power loss in `crates/nullrouter-engine/tests/sim_week.rs`: at a seeded point, leave the journals as a crash would (written, not synced), drop all in-memory state, reopen from the files and continue; a second run without the restart must place identically from that point, and print `post-restart placements identical: yes` (SC-005). A power-loss variant cuts files back to the last sync and checks that at most the last simulated second is missing.
- [X] T084 [US6] Determinism and runtime: two runs print identical output; about 60,000 requests in under 2 minutes in release and under 5 in debug. If the release budget is missed, profile with the bench cases (T091) before changing the simulation.
- [ ] T085 [US6] *operator-run* Live check L7 in `crates/nullrouter-engine/tests/live.rs` (`live_routing_matches_polls`, `NR_LIVE=1`) and quickstart §5: poll each polled account, compare `routing --json` `remaining_now` with the poll's `remaining`, send one tiny request through each, and check the drop equals its metered `cost_since_poll` (SC-007). Ask the user to run `! CARGO_HOME=$PWD/.cargo-home NR_LIVE=1 cargo test -p nullrouter-engine --test live -- live_routing_matches_polls --nocapture`. Record mismatches as meter corrections in `plugins/bundled/*.toml` with a dated source comment.

**Checkpoint**: the routing rules are proven over a week and the view matches real polls.

---

## Phase 10: User Story 8 — Direct targets and mixed-limit unified models (Priority: P3)

**Goal**: Direct `provider/model` targets with several accounts route like unified models; mixed
member limits produce a note at check and reload.

**Independent Test**: A direct target with three accounts places identically to a unified model
of the same accounts. A unified model with 200k and 128k members prints a note and still loads.

### Tests for User Story 8 ⚠️

- [X] T086 [P] [US8] Direct-target test in `crates/nullrouter-engine/tests/routing_direct.rs` (new): `provider/model` with three accounts gets the same decisions as a unified model of the same accounts for a fixed warm and cold sequence, and the routing view lists the direct target (scenario 1). A request the placed member refuses for its size moves on through ordinary fallback (scenario 3, FR-032).
- [X] T087 [P] [US8] Limits-note tests in `crates/nullrouter-registry/tests/unified.rs`: members with different `context_length` or `max_output_tokens` produce `note: unified model sonnet: members differ in context_length: kiro 200000, openrouter 128000`; a member without a value is `undeclared`; the model loads. CLI tests in `crates/nullrouter-cli/tests/check.rs`: `check`, `resolve <unified>` and a command applied to a running server print the note.

### Implementation for User Story 8

- [X] T088 [US8] Limits note in `crates/nullrouter-registry/src/load.rs`: `LoadReport` gains `notes: Vec<LimitsNote {unified, limit, values}>` per [data-model § Unified-model limits note](data-model.md#unified-model-limits-note-load-report-r14), computed at load and reload; never an error.
- [X] T089 [US8] Print the note in `crates/nullrouter-cli/src/cmd/check.rs`, `cmd/resolve.rs`, and on stderr from `apply()` in `crates/nullrouter-cli/src/lib.rs` for every command that reloads a running server (`cmd/model.rs` only reads; unified models are edited in `config.toml`).
- [X] T090 [US8] Green run, crate by crate.

---

## Phase 11: Polish & Cross-Cutting Concerns

- [ ] T091 Bench cases in `crates/nullrouter-engine/benches/engine.rs` per research R19: `route/decide` with 2, 8 and 32 candidates; `route/fingerprint` over 10, 100 and 400 messages; `journal/close_ack`; `ttfb/routed` (the full path with the journal on). Compare with `pre-006` (T001) and append the results to `specs/006-routing-decision/bench-baseline.md`. `ttfb/routed` p95 must be within 5 ms of `ttfb/direct` (SC-013). If fingerprinting 400 messages misses, try per-message hashes cached by `(agent, chain position)` first, then `blake3`.
- [X] T092 [P] Secret and prompt sentinel in `crates/nullrouter-server/tests/secrets.rs`: plant sentinel secrets and a sentinel prompt string, run warm, cold and overflow traffic, then scan logs, `records/*.jsonl`, `routing/*.jsonl`, `routing` and `records show` output (text and JSON), operator socket answers and plugin-visible data: zero secrets and zero prompt text (SC-012).
- [X] T093 [P] `nullrouter check` additions in `crates/nullrouter-cli/src/cmd/check.rs`: a pay-as-you-go provider with no price; an `explicit` cache mode on a provider none of whose endpoints speaks a style with cache markers; modes of `records/`, `records.lock`, `routing/` and `routing/salt` (0600 files, 0700 directories).
- [X] T094 [P] `docs/operator-config.md`: priority and `[account.routing]` overrides, `[routing]` amortization, the `routing` view and its labels, `records` offline with `prune` and `forget`, durability per the journal contract's table, disk-full behaviour, and "Live checks" L7.
- [X] T095 [P] `docs/plugins.md`: the `[routing]` section for plugin authors (cache modes, meters, admission, balance windows, price schedule, defaults, gate rules), from [contracts/routing-schema.md](contracts/routing-schema.md).
- [ ] T096 `/rust-parity-audit` on `crates/nullrouter-engine/src/attempt.rs` and `src/plan.rs` against 9router's retry and fallback path (`open-sse/handlers/chatCore.js`, `src/sse/services/auth.js`, `open-sse/services/combo.js`); D-006-1..3 are deliberate and must not be reported as gaps; fix High findings.
- [ ] T097 Security review of the slice diff (`/security-review`): journal and routing file modes, `records.lock`, temp-then-rename rewrites, the salt, fingerprints holding no content, `forget`, redaction of error reasons in journal lines.
- [ ] T098 Run [quickstart.md](quickstart.md) §1–4 end to end; fix what fails.
- [X] T099 `cargo fmt --all`, then `nice cargo clippy -p <crate> -j 2 -- -D warnings` crate by crate.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: none. T001 must run before any engine change.
- **Foundational (Phase 2)**: depends on Setup; blocks every story.
- **US1 (Phase 3)**: depends on Foundational. The MVP.
- **US2 (Phase 4)**: depends on US1 (`place.rs`, the attempt walk, `plan.rs` reduction).
- **US3 (Phase 5)**: depends on US2 (tiers follow the subscription tier and its weights).
- **US4 (Phase 6)**: depends on US2 for the view's pace, share and deficit; price columns fill in
  once US3 lands.
- **US5 (Phase 7)**: record persistence needs only Foundational and US1's attempt walk; the
  routing-state load needs US1 (warm lines) and US2 (ledger lines). T072 extends the view from
  T053 (US4).
- **US7 (Phase 8)**: depends on US2 (meters feed pace).
- **US6 (Phase 9)**: depends on US1, US2, US3, US5 and US7; it proves them together.
- **US8 (Phase 10)**: the note (T087–T089) depends on Foundational only and can run any time;
  the direct-target test (T086) needs US2.
- **Polish (Phase 11)**: depends on every story.

### Story Graph

```text
Setup → Foundational ─┬─▶ US1 (MVP) ─▶ US2 ─┬─▶ US3 ─┐
                      │                     ├─▶ US4   │
                      │                     ├─▶ US7 ──┼─▶ US6 (the week) ─▶ Polish
                      │       US1+US2 ─▶ US5 ─────────┘
                      └─▶ US8 note (any time after Foundational)
```

### Within Each Story

- Tests first; watch them fail.
- Then the pure core (`routing/`), then the engine wiring (`attempt.rs`, `state.rs`, `journal/`),
  then plugin data, then server and CLI.
- Finish with the green-run task.
- Operator-run live checks come after the mock tests pass. A live result can only correct a
  declared value (a capacity, a lifetime); it never changes a rule.

---

## Parallel Examples

```text
# Phase 1 and 2: skeletons together, then independent foundations
T002 engine skeletons   T003 schema/routing.rs   T004 cmd/routing.rs
T008 gate corpus   T009 config [routing]
(T012 waits for T011, T017 for T016)

# US1: tests together, then plugin data alongside the core
T018 fingerprint   T019 warm store   T020 routing_warm   T021 stay_warm
T028 bundled [routing.cache]   (while T022–T027 proceed)

# US2: three test files together, plugin meters alongside
T031 pace   T032 ledger   T033 routing_cold   T034 fallback/usage   T040 bundled meters

# US4 and US7 can proceed in parallel after US2
T050 T051 T052 (US4 tests)   T074 T075 (US7 tests)

# US5 tests together
T060 records_journal   T061 routing_state   T062 cli records   T063 records_durable

# US8 note any time after Foundational; Polish docs together
T087 T088 T089   ·   T092 T093 T094 T095
```

---

## Implementation Strategy

### MVP (User Story 1)

1. Phases 1 and 2.
2. Phase 3. **Stop and validate**: two harnesses, two mock subscription accounts; warm follow-ups
   stay and the mock reports cache reads; a rate limit moves only the agent on that account, with
   the reason in its record. Cold work still follows account order, as before this slice.

The MVP meets objective 1 (no token waste from moving warm sessions). The slice's failure
conditions are all closed only after US1–US5, and proven by US6.

### Incremental Delivery

1. US1: warm placement. Objective 1.
2. US2: cold work by pace and deficit. Objective 2.
3. US3: pay-as-you-go as overflow. Every P1 placement rule in place.
4. US4: the operator steers and explains decisions.
5. US5: records, warm state and deficits survive crashes.
6. US7: between-poll and estimated quota.
7. US6: the simulated week and live check L7 prove it.
8. US8: direct-target proof and the limits note.
9. Polish: bench gate, sentinel, docs, parity audit, security review.

A commit per task or per logical group is fine. Bundled-plugin value changes (T028, T040, T047)
each go in their own commit, so a later fit can be compared against them.

---

## Notes

- Constraints quoted from the data model and contracts are binding:
  - priority "number ≥ 0; default 1; 0 = never cold work";
  - reserve "0–50%", default 5%;
  - `cache.lifetime` "> 0; ≤ 24h"; default cache "mode `automatic`, lifetime 5m, min_tokens 1024";
  - deficits in plain tokens (input + cache read + cache write + output, unweighted), "each
    clamped to ±2,000,000", sum 0 while no clamp engages and no account was reset mid-window;
  - a settlement or reversal for a past amortization window is dropped;
  - priority 0 is never cold work, last resort or fallback;
  - new files and renames are followed by a parent-directory `fsync`;
  - amortization default `5h`, epoch-aligned;
  - π cap 10, time-left floor 60 s, size floor 1;
  - full disk: "up to 10,000 pending lines", retry "every 5 s";
  - `fdatasync` "at most every second";
  - journal and routing files 0600, directories 0700; `routing/salt` "32 random bytes".
- `routing/` never does I/O or reads the clock; `now` is always an argument (T002's `no_io` test).
- Fingerprinting only reads the IR. Never add, move or remove cache markers or any other
  content (FR-011).
- Never store prompt content: fingerprints are salted, truncated hashes.
- Never put a secret, header value or prompt text in a journal, routing file, CLI output or
  socket answer.
- Never block the async executor on file I/O: request tasks only send to the writer and await
  acks.
- Never add `tokio` or `unsafe` to `nullrouter-registry`.
- Never edit `ref/9router/` or `tests/fixtures/9router/` by hand.
