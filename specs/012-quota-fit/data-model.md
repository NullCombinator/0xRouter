# Data Model: Quota Fit, Outside Use and Leak Detection

Types named here are Rust types in `nullrouter-engine` unless noted. Stored shapes are in
[contracts/state-files.md](contracts/state-files.md).

## Meter number

A name for one number of one window's meter. Used as the key everywhere below.

| Variant | Applies to | Notes |
|---|---|---|
| `Capacity` | per account | percent windows, and absolute windows that report a `limit` (research R3) |
| `Weight(Input)` | pooled | **yardstick** on percent windows: never fitted there (clarify Q1) |
| `Weight(Output \| CacheRead \| CacheWrite)` | pooled | `weighted_tokens` windows only |
| `Multiplier(glob)` | pooled | one per declared `model_multiplier` glob |

Text form (CLI, records, JSON): `capacity`, `weight.input`, `weight.output`, `weight.cache_read`,
`weight.cache_write`, `multiplier.<glob>`.

## Source (FR-012)

`AccountOverride` > `PluginOverride` > `Fit` > `Declared`. `PluginOverride` never applies to
`Capacity` (clarify Q5). Serialized `account_override`, `plugin_override`, `fit`, `declared`.

## Number state (FR-028)

```text
            enough rows, test rejects θ₀
 Learning ─────────────────────────────▶ Fitted(since)
    ▲  ▲                                   │
    │  │ break detected (R9)               │
    │  └── Relearning(since) ◀─────────────┘
    │          │ (same as Learning, with the break time shown)
    │          └── test rejects θ₀ ──▶ Fitted(since)
    │
    └── meter changed at load ── Restarted(since)  (shown as Learning, with the reason)

 NotSeparable(partner)   VIF > 50 or |corr| > 0.98 (R7); re-evaluated each refit; the partner
                         may be a part-of-day outside rate
 Yardstick               weight.input on a percent window; constant
 NotReported             capacity of an absolute window with no reported limit
 NotFitted(reason)       account with no quota reports, or pay-as-you-go (FR-029)
```

`SplitOff(since, reason)` is a state of an **account** within a plugin's window, not of a number.
A split account's pooled numbers run their own state machine.

`Learning` carries its **progress**: rows used, and the current relative half-width of the 95%
range (e.g. `312 intervals, ±40%`).

## Poll interval (row)

Derived from history, never stored separately (R2).

| Field | Type | Rule |
|---|---|---|
| `account` | provider/account | |
| `window` | string | the report's window name |
| `start`, `end` | `SystemTime` | consecutive good entries' `at`; failed polls in between are bridged |
| `y` | f64 | reported use change, in report units |
| `x` | per multiplier group × class → u64 | summed tallies of every entry after `start` up to `end` |
| `requests` | u64 | |
| `hours` | f64 | `end − start` |
| `class` | `Evidence \| Idle \| OutsideProvisional(until) \| Outside \| SetAside(reason)` | R6. Set-aside reasons: `reset`, `usage_unreported`, `exhausted` |

## Fit (per plugin × window)

| Field | Notes |
|---|---|
| `epoch` | `SystemTime`: rows before it don't count (R11) |
| `meter_hash` | SHA-256 of the declared `MeterDecl` (canonical TOML); a mismatch at load restarts |
| `pooled` | estimates, covariance, state per pooled number |
| `accounts` | per account: `epoch` (set when (re-)added), `capacity` estimate and state, `rates` (six part-of-day `b_{a,q}` estimates), `split: Option<Split>`, own pooled-number estimates when split |
| `prior` | optional folded prior from pruned rows: mean and information matrix |
| `breaks` | list of `Break` |
| `in_effect` | `Arc<[MeterDecl]>` per account: the meter in effect, rebuilt on any state or override change (R15) |

Only `epoch`, `meter_hash`, states with their times, splits, breaks, the prior and alerted rates
are persisted (`quota/fit/<provider>.json`). Estimates are rebuilt from history (R11).

## Break

| Field | Type |
|---|---|
| `at` | `SystemTime`: the start hour the detector chose |
| `detected_at` | `SystemTime` |
| `window` | string |
| `number` | meter number; capacity breaks also name the account |
| `replaced` | the fitted value that stopped being used |

## Outside-use entry (FR-021)

| Field | Type | Notes |
|---|---|---|
| `id` | ULID | |
| `account`, `window` | | |
| `kind` | `Idle \| Busy \| SteadyRate` | |
| `start`, `end` | `SystemTime` | `end` absent for an ongoing steady rate |
| `amount` | f64, report units | excess beyond the fit's upper range plus one step, for `Busy`; per hour for `SteadyRate`, with its part of the day |
| `found_at` | `SystemTime` | |

## Exclusive-use declaration (FR-023)

`Account.exclusive_use: Option<SystemTime>`, in `accounts.toml`, set and withdrawn by the CLI.
Validation: the account's provider must declare a `[quota]` report for it. Otherwise it is refused
with `account <p>/<a>: exclusive use needs quota polls; <p> reports no quota for this account`.

## Leak alert (FR-024 – FR-027)

| Field | Type | Notes |
|---|---|---|
| `id` | ULID | short form shown in the CLI |
| `entry` | outside-use entry id | |
| `raised_at` | `SystemTime` | |
| `acknowledged_at` | `Option<SystemTime>` | set by `quota ack` |
| `text` | string | built from the entry, never says "leak" for the cause (FR-025) |

Raised only when the account had an exclusive-use declaration at the entry's `start`. An
acknowledged alert leaves the alert list. Its entry stays in the outside-use list.

## Meter override (FR-019)

`WindowOverride` (accounts.rs) gains:

| Field | Type |
|---|---|
| `token_weights` | `Option<PartialTokenWeights>`: each of the four classes optional |
| `model_multiplier` | `IndexMap<String, f64>`: globs must be declared by the plugin's meter |

A new `ProviderSettings.meter: BTreeMap<String, MeterOverride>` (registry `schema/config.rs`) holds
plugin-level overrides with the same two fields and no capacity. Both are checked by the plugin
rules (`check_weights`, `check_multiplier`, wording reused). A glob the plugin doesn't declare is
refused, like an unknown window name today.

## Meter in effect

For each account and declared window, a `MeterDecl` equal to the declared one with each number
replaced according to Source. Its `token_weights` and `model_multiplier` keep the plugin's glob
order. It is built by `quota::fit::in_effect(&declared, &plugin_override, &account_override,
&fit)`, and is pure.

**Invariant (FR-011, SC-002, SC-005)**: with no overrides of the new kinds and no `Fitted`
number, `in_effect(..) == declared` field for field.
