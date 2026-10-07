# Quickstart: validating slice 012

Local cargo is not run on this machine (project rule since 2026-10-06). The commands below are
what CI runs, or what an operator runs on a built binary. Push the slice's commits as one group,
with the user's OK, and read the results with the github MCP.

## 1. The simulated week (FR-033; SC-001, SC-004 – SC-008)

```bash
nice cargo test -p nullrouter-engine --release --test sim_week -j 2 -- --nocapture
```

Expected table rows (one per check, each `ok`):

| Check | Expected |
|---|---|
| 3×-off: capacity, weight.output significant | both by day 7; final values within 10% of truth (SC-001) |
| 3×-off: placements before first significance | 100% equal to the fitting-off baseline (SC-005) |
| right plugin: significant numbers | 0 |
| outside use injected: fitted values | inside the no-outside-use run's 95% ranges; ≥ 90% of injected intervals listed (SC-004) |
| capacity halved day 4 | break reported ≤ 1 simulated day; 0 placements after it use the old capacity (SC-006) |
| alerts | 0 on the non-exclusive account; 0 from sub-step and 1-step noise; every idle drop of ≥ 2.0 true steps alerted at the first poll; busy bursts alerted only when they pass the test (SC-007) |
| restart and crash | fits, states, breaks, outside use, declarations, alerts identical (SC-008) |
| meter change at load | that window's numbers restarted; no other number changed (SC-008) |

## 2. The right-plugin suite (SC-002, SC-003)

```bash
nice cargo test -p nullrouter-engine --release --test sim_suite -j 2 -- --ignored --nocapture
```

Expected: `significant numbers: 0 / 100 weeks` with and without outside use, including
office-hours outside use; `placements equal to baseline: 100%`; `range coverage ≥ 93%` over at
least 1,000 checks for every reported number; and the measured null rejection rate printed beside
the 0.1% bound. A failing seed is
investigated, never replaced (research R5).

## 3. Contracts and invariants (unit and integration tests)

- `quota::fit::in_effect` returns the declared meter unchanged with no override and no fitted
  number (the FR-011 invariant).
- Override parsing: per-account and `config.toml` plugin-level keys accepted and refused with
  the gate's wording (contracts/cli.md).
- `accounts exclusive … on` refused on an unpolled account (FR-023).
- Records: `meter_sources` absent when all numbers are declared, present otherwise (contracts/record.md).
- No secret in any fit file, outside-use file, view, alert or log line (SC-010): the existing
  `no_cloaking`/redaction scan is extended to the new files.

## 4. Routing benchmark (SC-011)

```bash
nice cargo bench -p nullrouter-engine --bench engine -- placement --baseline slice-006
```

Expected: no measurable regression. This is a local bench; cloud timings aren't comparable.

## 5. Operator walk-through (manual, on a built binary)

```bash
nullrouter serve &
nullrouter routing sonnet                       # meter blocks under each polled account
nullrouter routing set anthropic max window.weekly.weight.output=15
nullrouter routing sonnet --json | jq '.targets[0].accounts[0].meter'
nullrouter routing set-plugin anthropic window.weekly.weight.cache_read=0.1
nullrouter accounts exclusive anthropic max on
nullrouter accounts exclusive xai main on       # exits 2: no quota polls
nullrouter quota outside
nullrouter quota alerts
nullrouter quota ack all
nullrouter check
```

## 6. Live check (FR-034, SC-012)

```bash
NR_LIVE=1 NULLROUTER_HOME=~/.0router nice cargo test -p nullrouter-engine --test live live_fit_progress -j 2 -- --nocapture
```

Expected: one line per polled account, window and number, with a state and progress. It fails
when `NULLROUTER_HOME` is unset or no polled account exists. No time target.
