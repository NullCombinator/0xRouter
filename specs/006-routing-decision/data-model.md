# Data Model: Routing Decision and Persistent Request History (slice 006)

Extends [slice 005's data model](../005-account-sign-in/data-model.md) and
[slice 003's](../003-request-pipeline/data-model.md). Research items are cited as R-numbers from
[research.md](research.md).

## Provider account (extended)

| Field | Type | Notes |
|---|---|---|
| `priority` | number ≥ 0 | new; default 1; 0 = never cold work (FR-027) |
| `routing` | account overrides, optional | new; see below |
| everything else | | unchanged from slice 005 |

**Account overrides** (`[account.routing]`, R13):

| Field | Type | Overrides |
|---|---|---|
| `cache_lifetime` | duration | plugin `routing.cache.lifetime` |
| `reserve` | percent | every window's reserve floor |
| `price` | `{input, output?, cache_read?, cache_write?}` per Mtok | the plugin's whole price schedule |
| `window.<name>` | `{capacity?, length?, reserve?}` | that window's meter fields |

**Derived kind** (R5): `polled` | `estimated` | `pay-as-you-go`, plus `pending first poll` and
`stale` as display states of `polled`. Not stored.

## Routing declaration (plugin `[routing]`, new; R16)

| Part | Fields |
|---|---|
| `cache` | `mode`: `explicit` \| `automatic` \| `none`; `lifetime`: duration; `min_tokens`: integer (default 1024, `automatic` only) |
| `window[]` (meter) | `name`; `length`: duration; `unit`: `weighted_tokens` \| `requests`; `capacity`: number, optional; `token_weights`: `{input, output, cache_read, cache_write}`, default all 1; `model_multiplier`: glob → factor; `reserve`: percent; `reset`: `rolling` \| `fixed` \| `first_use` (with `anchor` for `fixed`), only for unreported windows; `admission`: bool, derived as `length < 1h` |
| `price[]` | `input`, `output`, `cache_read`, `cache_write` (per Mtok); `when`: `{days?, from?, to?, offset?}` |

Validation follows contracts/routing-schema.md. The declaration holds no URL, header or secret.

## Routing settings (`config.toml` `[routing]`, new; R13)

| Field | Type | Default |
|---|---|---|
| `amortization` | duration | `5h` (Clarifications Q4) |
| `amortization_for` | target → duration | none |

## Quota window state (in memory, derived; R5–R7)

One per `(account, window)`, rebuilt from the latest poll, the tally and the meter at each
decision.

| Field | Source |
|---|---|
| `name`, `unit` | report, else meter |
| `length` | meter |
| `capacity` | meter (account override first); `assumed: true` when taken from the median (R5) |
| `remaining_at_poll`, `polled_at` | slice 005 poll |
| `remaining_now` | `remaining_at_poll − cost(tally since poll)`, or estimated from the reset (R6) |
| `resets_at` | report; rolled forward by `length` when passed (FR-022) |
| `reserve` | override → meter → 5% |
| `pace` π, `rate` r | R7 |
| `role` | `pacing` \| `admission` \| `balance` (no reset: admit only) |

## Warm fingerprint (`routing/warm.jsonl`, new; R3, R12)

| Field | Type | Notes |
|---|---|---|
| `agent` | key id | per-agent isolation (FR-007) |
| `hash` | 128-bit hex | salted SHA-256 of the prefix chain position; never content |
| `provider`, `account`, `model` | ids | where it's cached |
| `prefix_tokens` | integer | usage-reported input tokens at this boundary, when known |
| `last_used` | RFC 3339 ms | refreshed on each warm hit |

Identity: `(agent, hash, provider, account, model)`.

Lifecycle: written after a successful attempt (per the provider's cache mode), refreshed on use,
and deleted once `now − last_used > lifetime` (Clarifications Q3), or when its account is removed
or its agent is forgotten.

`routing/salt`: 32 random bytes, 0600, created once.

## Deficit ledger (`routing/ledger.jsonl`, new; R8, R12)

| Field | Type | Notes |
|---|---|---|
| `target` | unified name or `provider/model` | |
| `tier` | `subscription` \| `payg` | separate ledgers (R9) |
| `window_start` | RFC 3339 | epoch-aligned multiple of the target's amortization length |
| `deficits` | account → tokens (input + cache read + cache write + output, unweighted; R8) | sum 0 while no clamp engages and no account was reset mid-window; each clamped to ±2,000,000 |
| `at` | RFC 3339 ms | |

State transitions: debit at placement (estimate), then settle at attempt end (actual − estimate),
or reverse on failure. A settlement or reversal for a past `window_start` is dropped. Everything
resets to 0 at `window_start + length`. Lines from a past window
are ignored at load.

## Placement decision (per request, in the record; R9, R15)

| Field | Type |
|---|---|
| `kind` | `warm` \| `cold` \| `overflow` \| `none` |
| `at` | RFC 3339 ms |
| `amortization_window` | `{start, length}` |
| `size_tokens` | integer, the estimate used (R7) |
| `warm` | `{provider, account, model, prefix_tokens, idle_s, stayed, moved_because?}`, optional |
| `candidates[]` | `{provider, account, model, tier, eligible, why_not?, quota_source, pace?, rate?, priority, weight?, share?, deficit_before?, price_now?}` |
| `order` | candidate indexes in attempt order |

`why_not`: `out_of_service`, `cooling`, `admission`, `reserve_floor`, `priority_zero`,
`not_a_subscription`.

## Attempt (extended)

| Field | Type | Notes |
|---|---|---|
| `placement` | `{reason, rank}` | new; `reason` ∈ `warm`, `cold_by_deficit`, `moved_for_capacity`, `left_pay_as_you_go`, `overflow`, `last_resort`, `retry`, `fallback` |
| everything else | | unchanged (latency, usage, dropped, forced) |

## Request record (extended; R11)

| Field | Notes |
|---|---|
| `decision` | new, above |
| `outcome` | adds `interrupted` (a crash cut it short; set at recovery) |
| storage | journal lines in `records/YYYY-MM-DD.jsonl`, folded by id (contracts/record-journal.md) |

Lifecycle: `open` (in progress), then `attempt`* and `decision`, then `close` (succeeded, failed,
refused, cancelled), or `interrupted` at recovery. The record is kept until `records prune` or
`records forget` (FR-035).

## Journal health (in memory; R11)

| Field | Notes |
|---|---|
| `kept` | bool |
| `since` | when writing first failed |
| `unkept_requests` | count of records not written |
| `last_sync` | last successful `fdatasync` |

Shown by `routing`, `check` and the log (FR-037a).

## Unified-model limits note (load report; R14)

| Field | Notes |
|---|---|
| `unified` | name |
| `limit` | `context_length` \| `max_output_tokens` |
| `values` | member → value or `undeclared` |
