# Implementation Plan: Quota Fit, Outside Use and Leak Detection

**Branch**: `012-quota-fit` | **Date**: 2026-10-07 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/012-quota-fit/spec.md` (clarified 2026-10-07, five
answers). Scope brief: [specs/briefs/2026-10-07-quota-fit.md](../briefs/2026-10-07-quota-fit.md).

## Summary

0router learns each polled account's real quota meter from its poll history and routes on the
learned numbers once they are significant. It recognises quota it didn't spend, and keeps that
out of the fit. On accounts the operator declares exclusive, it alerts on that use.

The approach:

- **The fit sits beside routing, not inside it.** For each account, `route.rs` reads a precomputed
  **meter in effect**: the declared `MeterDecl`, with each number replaced by account override,
  plugin override, or significant fit, in that order. The decision core is unchanged. With
  nothing significant and no new override, the meter in effect *is* the declared one, so
  placements equal today's by construction (FR-011)
  ([R1](research.md#r1-where-the-fit-plugs-into-routing), [R15](research.md#r15-performance-sc-011)).
- **Evidence is the history that already exists.** Rows are consecutive good polls with slice
  006's per-interval tally, read from `quota/<provider>/<account>.jsonl`. A restart replays them
  ([R2](research.md#r2-what-one-row-of-evidence-is), [R11](research.md#r11-epochs-and-restarts-fr-017-fr-018)).
- **One small model per plugin window.** Per-account capacity and a steady outside rate per
  4-hour part of the day (so outside use with a daily rhythm isn't read as a wrong weight), plus pooled
  weights relative to the input yardstick and pooled multipliers. It is fitted by Gauss–Newton in
  log space. Its noise model is exact for whole-step rounding (variance 1/6 per row, −1/12
  between adjacent rows) ([R3](research.md#r3-the-model), [R4](research.md#r4-noise-and-the-95-range)).
- **One always-valid test for every decision.** A normal-mixture confidence sequence at α = 0.001
  over each number's whole life, Bonferroni within the window. It decides significance,
  split-off, breaks, busy-time alerts and steady-rate alerts
  ([R5](research.md#r5-the-significance-test-fr-010-clarify-q2)).
- **Rows are classified before they count.** Idle drops beyond one step are outside use at once.
  Busy excess beyond the fit's predictive range is provisional for an hour, so a rule change
  shows as a break and not as a leak ([R6](research.md#r6-classifying-a-row-fr-007-fr-009),
  [R9](research.md#r9-breaks-fr-015-fr-016), [R10](research.md#r10-leak-alerts-fr-023-fr-027-clarify-q3)).
- **Evidence is slice 006's simulated week**, extended with true meters that differ from the
  plugin's, rounding, outside use, a halving and restarts. A 100-seed suite covers the right-plugin
  guarantee ([R14](research.md#r14-evidence-the-simulated-week-extended)).

## Technical Context

**Language/Version**: Rust 1.89 (workspace `rust-version`), edition 2024.

**Primary Dependencies**: Existing workspace crates only: `serde`, `serde_json`, `toml`, `sha2`
(meter hash), `ulid` (entry and alert ids), `arc-swap` (meter in effect), `tracing`, `indexmap`.
The linear algebra is small (≤ ~30 parameters). Gauss–Newton, a Cholesky solve and the inverse
are written in `quota/fit/linalg.rs`, about 100 lines, so no `nalgebra`. That choice is recorded
in Complexity Tracking.

**Storage**: Files under `$NULLROUTER_HOME`:
- `quota/fit/<provider>.json`: fit states, breaks, splits, prior;
- `quota/<provider>/<account>.outside.jsonl`: outside use, alerts, acks;
- `accounts.toml`: exclusive use, per-account weight and multiplier overrides;
- `config.toml`: plugin-level overrides.

See [contracts/state-files.md](contracts/state-files.md).

**Testing**: `cargo test` per crate in CI. The extended `sim_week` integration test is the
evidence; `sim_suite` (100 seeds) runs `--ignored` in release, in a new CI step. Unit tests cover
the fit math against closed-form cases: a linear model with known noise, and the coverage of the
mixture boundary. No local cargo.

**Target Platform**: Linux and macOS operator machines (unchanged).

**Project Type**: Rust workspace: library crates, an HTTP server and a CLI.

**Performance Goals**: No measurable placement regression (SC-011): the request path adds one
`arc-swap` load per candidate. A refit per poll per plugin window takes under 5 ms for a month of
10-minute rows. It runs on the poll task, never on a request.

**Constraints**:
- Fits and outside-use data never leave the machine (FR-030).
- Plugins stay data (FR-031).
- No forecast between polls (FR-014).
- Disk full keeps state in memory and warns.

**Scale/Scope**: Up to ~10 plugins with polled accounts, ~20 accounts, 1–3 windows each, months
of history. Six user stories and 34 FRs.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Plugin Safety | Plugins stay data; no plugin reads or writes a fit, a declaration or an alert; no secret in the new files, views or logs | **Pass**. The fit is core code. It reads poll history and tallies (no secrets) and writes 0600 files. Plugin TOML is read, never written (FR-006). SC-010's scan is extended to the new files |
| II. Routing Fidelity (v3.1.0) | Precedence: override > significant fit > declaration; declared value unchanged until significant; outside use never evidence | **Pass**. The meter in effect implements the precedence: account override > plugin override > fit > declaration (clarify Q5 refines "operator override" into two levels, both above the fit). Learning numbers keep the declared or overridden value (R1). Outside use is excluded (R6). Warm priority and per-agent isolation are untouched, since the decision core is unchanged |
| III. Unified Models and Provider Entities | Fits per provider entity's windows; no per-modality split | **Pass**. Pooling is per plugin (one provider entity), per window |
| IV. Scope Discipline | No content change | **Pass**. Nothing touches request content |
| V. Streaming-Native SSE | No buffering | **Pass**. Nothing on the stream path |
| VI. Reference-Informed Behavior | Oracle where inherited | **N/A**. 9router has no fit (research preamble); no inherited behaviour changes |
| VII. Trustworthy Model Tests | — | **N/A** |

Re-check after Phase 1 design: **Pass**. The data model and contracts add no plugin-facing
surface. The record field is absent unless a non-declared source is in effect, and the view
and socket carry no secrets.

## Project Structure

### Documentation (this feature)

```text
specs/012-quota-fit/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── cli.md
│   ├── operator-socket.md
│   ├── state-files.md
│   └── record.md
├── checklists/requirements.md
└── tasks.md             # /speckit-tasks
```

### Source Code (repository root)

```text
crates/nullrouter-registry/src/schema/
├── config.rs                    # ProviderSettings.meter: plugin-level weight/multiplier overrides
└── routing.rs                   # shared check_weights/check_multiplier wording (exported)

crates/nullrouter-engine/src/
├── quota/
│   ├── fit/                     # NEW
│   │   ├── mod.rs               # Fits: per plugin × window; refit on poll; in_effect()
│   │   ├── rows.rs              # history → rows; set-aside rules (R2)
│   │   ├── model.rs             # parameters, prediction, Gauss–Newton, sandwich covariance (R3, R4)
│   │   ├── linalg.rs            # Cholesky, inverse, small dense matrices
│   │   ├── test.rs              # normal-mixture confidence sequence (R5)
│   │   ├── classify.rs          # idle/busy/provisional/outside (R6), separability (R7)
│   │   ├── split.rs             # split-off (R8)
│   │   ├── breaks.rs            # break detector (R9)
│   │   ├── outside.rs           # outside-use list, alerts, acks; .outside.jsonl (R10, R12)
│   │   └── store.rs             # fit/<provider>.json load/save, meter hash, prior (R11, R12)
│   ├── history.rs               # prune: fold pruned in-epoch rows into the prior; forget: outside file
│   └── poll.rs                  # after a good poll's entry: Fits::observe(account)
├── accounts.rs                  # WindowOverride.token_weights/model_multiplier; Account.exclusive_use
├── route.rs                     # candidate_of: meters in effect instead of routing.windows
├── routing/mod.rs               # CandidateRow.meter_sources
├── routing/view.rs              # AccountView.meter / outside_use / fit_note
└── state.rs                     # EngineState holds Fits and meters in effect

crates/nullrouter-server/src/
├── operator.rs                  # quota.outside/alerts/ack ops; routing.view additions
└── quota.rs                     # op handlers

crates/nullrouter-cli/src/
├── cmd/routing.rs               # weight/multiplier keys; set-plugin/unset-plugin
├── cmd/quota.rs                 # outside, alerts, ack
├── cmd/accounts.rs              # exclusive on|off; list column
├── cmd/check.rs                 # unacknowledged alerts
└── routing_text.rs              # meter block rendering

crates/nullrouter-engine/tests/
├── sim_week.rs, sim_week/       # extended world (R14)
├── sim_suite.rs                 # NEW: 100 seeds, #[ignore]
├── quota_fit.rs                 # NEW: fit math, classification, store, in_effect invariant
└── live.rs                      # live_fit_progress (R16)

.github/workflows/ci.yml          # step: sim_suite --ignored in release
docs/operator-config.md           # Quota fit, outside use, exclusive use, overrides
```

**Structure Decision**: The fit lives in `nullrouter-engine/src/quota/fit/`, beside the history
it reads. Routing gets only a precomputed meter. The server and CLI add surface by following the
existing op/command patterns. The dashboard is untouched (FR-032).

## Phasing (for /speckit-tasks)

1. **Foundation**: rows from history, model and noise, linalg, the confidence sequence, and
   `in_effect` with its invariant test. The `sim_week` world gains true meters and rounding.
2. **US1 + US2 (P1, MVP)**: refit on poll, significance, pooling and split-off, classification
   and the outside-use list, records' `meter_sources`, store and replay. Evidence: the 3×-off,
   right-plugin and outside-use runs, plus `sim_suite`.
3. **US3**: view and socket additions, CLI rendering.
4. **US4**: overrides at both levels, with parsing, checks and precedence.
5. **US5**: break detector, meter-change restart, account epochs, crash and restart evidence.
6. **US6**: exclusive use, alerts, ack, `check`, serve log.
7. **Polish**: docs, live check, bench, CI step, SC-010 secret scan.

## Coordination

- Slice 011 (`.worktrees/011`) edits `routing/` for BROKEN skips. The overlap is `route.rs`'s
  `candidate_of`. This slice changes only the `declared:` argument there.
- Slice 010 reads records. `meter_sources` is additive and optional.

## Complexity Tracking

| Choice | Why needed | Simpler alternative rejected because |
|---|---|---|
| Hand-written small linear algebra (`linalg.rs`) instead of a crate | ≤ 30×30 dense matrices; only Cholesky, solve and inverse | `nalgebra` is a large dependency for about 100 lines, and adds compile time on a 4-core machine |
| Always-valid confidence sequences instead of a fixed-sample test | Clarify Q2: re-checked at every poll for a number's whole life | A fixed 95% test re-checked each poll corrects right plugins almost surely (fails SC-002) |
| Busy outside use is provisional for 6 rows | A rule change looks like outside use for its first rows | Classifying at once raises false leak alerts at every provider rule change |
| Six part-of-day outside rates per account instead of one | Outside use often follows office hours, as 0router's traffic does (analysis C1, Principle II) | One flat rate lets daytime outside use be fitted as a too-low weight, i.e. counted against the plugin |
