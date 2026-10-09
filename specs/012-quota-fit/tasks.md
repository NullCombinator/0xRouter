---

description: "Task list for 012-quota-fit"
---

# Tasks: Quota Fit, Outside Use and Leak Detection

**Input**: Design documents from `specs/012-quota-fit/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included. The spec's failure conditions and success criteria (SC-001 to SC-012) are
test outcomes: the extended simulated week, the 100-seed suite, unit tests of the fit math, and
the bench. Within each story, write the tests first, and expect them to fail until implemented.

**No local cargo** (project rule since 2026-10-06): don't run `cargo test`, `clippy`, `check` or
`build` on this machine. Commit in groups, push once with the user's OK, and read CI with the
github MCP. "Verify" below means in CI.

**Organization**: Tasks are grouped by user story, so each story can be implemented and tested
on its own. Task IDs are in execution order. US1 and US2 are both P1 and are built together as the
MVP, because a fit that outside use pulls off the truth fails the slice (spec, US2 "Why this
priority").

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US6)

## Path Conventions

- `crates/nullrouter-engine/src/quota/fit/`: NEW. The fit: pure functions of rows and `now`,
  except `store.rs` and `outside.rs`, which do file I/O through `crate::files`
- `crates/nullrouter-engine/src/route.rs`: the only routing call site that changes
- `crates/nullrouter-engine/tests/sim_week/`: slice 006's simulated world, extended
- `crates/nullrouter-registry/src/schema/`: plugin-level overrides in `config.toml`
- `crates/nullrouter-server/src/operator.rs`, `quota.rs`: socket ops
- `crates/nullrouter-cli/src/`: commands and text rendering

---

## Phase 1: Setup

- [X] T001 Create the module skeleton `crates/nullrouter-engine/src/quota/fit/mod.rs` with submodules `rows`, `model`, `linalg`, `test`, `classify`, `split`, `breaks`, `outside`, `store` (empty files with module doc comments citing research R2–R12), and declare `pub mod fit;` in `crates/nullrouter-engine/src/quota/mod.rs`
- [X] T002 [P] Add the `MeterNumber` enum (`Capacity`, `Weight(TokenClass)`, `Multiplier(String)`) with text forms `capacity`, `weight.input`, `weight.output`, `weight.cache_read`, `weight.cache_write`, `multiplier.<glob>`, `FromStr`/`Display` and serde as those strings, and `Source` (`AccountOverride`, `PluginOverride`, `Fit`, `Declared`; serialized `account_override`, `plugin_override`, `fit`, `declared`) in `crates/nullrouter-engine/src/quota/fit/mod.rs` (data-model § Meter number, § Source)
- [X] T003 [P] Add the test target stubs `crates/nullrouter-engine/tests/quota_fit.rs` and `crates/nullrouter-engine/tests/sim_suite.rs` (the latter's tests `#[ignore]`), with module docs naming the SCs each covers

---

## Phase 2: Foundational (blocking prerequisites)

**Purpose**: rows, the model, the noise, the test, and the meter in effect with its invariant. No story can start before these.

### Tests first

- [X] T004 [P] Write `linalg` tests in `crates/nullrouter-engine/src/quota/fit/linalg.rs` (`#[cfg(test)]`): Cholesky of a known SPD 4×4, solve, inverse times matrix ≈ identity (1e-9), and refusal (`None`) of a singular matrix
- [X] T005 [P] Write `rows` tests in `crates/nullrouter-engine/tests/quota_fit.rs`: from a hand-written history (three good entries and one failed in between) build rows. Assert the failed poll is bridged with tallies summed; a `resets_at` change or a falling `used` gives `SetAside(reset)`; `requests_usage_unreported > 0` gives `SetAside(usage_unreported)`; 100% used at either end gives `SetAside(exhausted)`; `x` is grouped by the first matching declared multiplier glob, else "no multiplier" (research R2)
- [X] T006 [P] Write model tests in `crates/nullrouter-engine/tests/quota_fit.rs`: on noiseless synthetic rows from known `k`, `ρ_o`, `ρ_r`, `ρ_w`, `μ`, `b`, Gauss–Newton recovers each within 1e-6 relative; with simulated whole-step rounding over 2,000 rows, the 95% range contains the truth in ≥ 93% of 500 seeded repetitions (coverage check of the sandwich covariance with the 1/6 diagonal and −1/12 adjacent terms, research R4)
- [X] T007 [P] Write confidence-sequence tests in `crates/nullrouter-engine/src/quota/fit/test.rs` (`#[cfg(test)]`): under the null, over 2,000 seeded runs of 1,000 sequential looks each, the boundary is crossed in ≤ 0.1% of runs (expect 0), and the measured rate is printed so `sim_suite` can report it beside the bound (research R5); under a factor-2 alternative with information growing like a day of sim traffic, it is crossed within the run
- [X] T008 [P] Write the meter-in-effect invariant test in `crates/nullrouter-engine/tests/quota_fit.rs`: for every bundled plugin's `[[routing.window]]` meters (load through `nullrouter_registry`), `in_effect(declared, None, None, &empty_fit)` equals the declared `MeterDecl` field for field (FR-011). With an account override of `weight.output`, only that field changes. An account override beats a plugin override, which beats a `Fitted` value, which beats the declaration (FR-012)

### Implementation

- [X] T009 [P] Implement `crates/nullrouter-engine/src/quota/fit/linalg.rs`: a small dense `Mat` (row-major `Vec<f64>`, n ≤ 64), `cholesky`, `solve`, `inverse`, `mul`, `transpose`; return `None` on a non-positive pivot; no external crate (plan Complexity Tracking)
- [X] T010 Implement `crates/nullrouter-engine/src/quota/fit/rows.rs`: `Row { account, window, start, end, y, x: BTreeMap<(Group, TokenClass), u64>, requests, hours, class }` and `rows_from(history: &[Entry], meter: &MeterDecl, since: SystemTime) -> Vec<Row>`, following research R2 exactly. `y` is from `used`, else `100 − remaining` (percent), in report units. Classes `Evidence`, `Idle`, `OutsideProvisional { until }`, `Outside`, `SetAside(Reset | UsageUnreported | Exhausted)`
- [X] T011 Implement `crates/nullrouter-engine/src/quota/fit/model.rs`: parameter layout per plugin window (per account `log k_a` and six part-of-day outside rates `b_{a,q} ≥ 0` via log or clamp, parts 00–04 … 20–24 local time, a row spanning two parts splitting its hours; pooled `log ρ_o`, `log ρ_r`, `log ρ_w`, `log μ_g`; on counted or balance windows `log w_c` instead of `k_a`; request-counted `E[y] = k_a·N + b_{a,q}·t`). Add prediction, Jacobian and Gauss–Newton from the meter in effect as the start (≤ 20 iterations, step halving). Covariance is `J⁻¹ (J_r + σ²_e·J₀) J⁻¹`, with the rounding term built from the row order per account (1/6 diagonal, −1/12 between adjacent rows) and `σ²_e` estimated from residuals and floored at 0. Add a 95% range per number in natural units. Absolute windows that report a `limit` treat each reported limit as one capacity reading with rounding variance 1/12, tested by FR-010's test like any other number (research R3, R4)
- [X] T012 Implement `crates/nullrouter-engine/src/quota/fit/test.rs`: `fn rejects(est_log: f64, null_log: f64, se: f64, m: usize) -> bool`, using the normal-mixture boundary of research R5 with `ALPHA = 0.001` and a `RHO` constant derived in closed form from the information per row of the sim world's traffic mix (tightest near the information a factor-2 error reaches in a day), with the derivation in the doc comment. Never tune it by running the sim week (research R5)
- [X] T013 Implement `in_effect` and the `Fits` container in `crates/nullrouter-engine/src/quota/fit/mod.rs`. `in_effect(declared: &MeterDecl, plugin: Option<&MeterOverride>, account: Option<&WindowOverride>, fit: &WindowFit, account_name) -> (MeterDecl, BTreeMap<MeterNumber, Source>)` keeps the plugin's glob order and the yardstick (`weight.input` on a percent window is never taken from the fit, clarify Q1). `Fits` is keyed by provider → window. Add a number-state enum with the variants of data-model § Number state, including progress `{ intervals, half_width }`
- [X] T014 Hold the meters in effect in engine state: in `crates/nullrouter-engine/src/state.rs`, add `meters_in_effect: ArcSwap<HashMap<(String, String), Arc<[MeterDecl]>>>` (provider, account) and a `rebuild_meters(&self)` that recomputes from the registry, overrides and `Fits`. Call it at start and on reload (research R15)
- [X] T015 In `crates/nullrouter-engine/src/route.rs` `candidate_of`, pass the account's meters in effect as `MeterInput.declared` instead of `routing.windows`, falling back to `routing.windows` when absent. Change nothing else in that function (coordination with slice 011, plan § Coordination)
- [X] T016 Extend the simulated world in `crates/nullrouter-engine/tests/sim_week/world.rs`: a `TrueWindow` gains its own true meter (capacity, weights, multipliers) separate from the plugin's; reported readings are rounded to whole steps (`Rounding::HalfUp` and `Rounding::Floor`); add an outside-use injector (idle bursts, a steady rate, an office-hours rate that follows the same daily shape as the sim agents' traffic, and bursts during traffic) that logs every injected interval. Injected idle drops meant to alert are at least 2.0 true steps; a separate set of sub-step and 1-step drops must not alert; add `set_true_capacity(at, value)` for the day-4 halving. Keep slice 006's existing world behaviour as the default, so its checks still pass

**Checkpoint**: Fit math, rows and the meter in effect exist; routing reads the meter in effect and still places exactly as before.

---

## Phase 3: User Stories 1 + 2 — corrections, and outside use kept out (Priority: P1) 🎯 MVP

**Goal (US1)**: Wrong numbers are corrected once significant, right ones never; weights are pooled per plugin, capacity per account, and a disagreeing account is split off.
**Goal (US2)**: Use 0router didn't cause is listed as outside use, moves no fitted number, and raises nothing.

**Independent test**: The extended simulated week: the 3×-off mock is corrected (capacity and `weight.output` significant by day 7, within 10%); the right mock never is; with injected outside use the fitted values stay within the clean run's ranges and ≥ 90% of injected intervals are listed.

### Tests for US1 + US2

- [X] T017 [P] [US1] In `crates/nullrouter-engine/tests/sim_week.rs`, add the 3×-off run: plugin `token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }`, true output and cache weights 3× relative to input, and true capacity 0.6× declared, at 10-minute whole-percent polls. Assert capacity and `weight.output` become `Fitted` within 7 simulated days, final values within 10% of truth (SC-001), and every 95% range contains the truth at the end (US1 scenarios 1, 3). Run a second plugin whose declared capacity is half the true one (true = 2× declared, US1 scenario 3 as written) and assert the same. After capacity is `Fitted`, assert each window's `remaining_now` between polls equals the last poll's fraction × the fitted capacity minus 0router's traffic since, costed with the meter in effect, with no outside-use term (FR-014)
- [X] T018 [P] [US1] In `crates/nullrouter-engine/tests/sim_week.rs`, add a fitting-off baseline run of the same seed. Assert every placement of the 3×-off run before its first significance equals the baseline's (SC-005), and that every placement of the right-plugin run equals its baseline (US1 scenario 2)
- [X] T019 [P] [US1] In `crates/nullrouter-engine/tests/sim_week.rs`, add three accounts of one plugin with the same true weights and different capacities. Assert the weights are pooled (one estimate) and each capacity is fitted per account (US1 scenario 4). Then make one account's true output weight 2×. Assert it is split off with a reason naming `weight.output`, and the pooled fit no longer uses its rows (US1 scenario 5)
- [ ] T020 [P] [US2] In `crates/nullrouter-engine/tests/sim_week.rs`, rerun the 3×-off and right runs with injected outside use (idle bursts, a steady rate, an office-hours rate, bursts during traffic). Assert every final fitted value lies inside the clean run's 95% range, ≥ 90% of injected intervals beyond one step are listed (SC-004), the right run has 0 significant numbers, including with the office-hours rate, and reports a steady rate (US2 scenario 2; spec Edge Cases: outside use with a daily rhythm), and an idle 1-step change is not listed (US2 scenario 5)
- [X] T021 [P] [US2] In `crates/nullrouter-engine/tests/quota_fit.rs`, add classification unit tests: idle `y = 6` is listed as outside use and excluded (US2 scenario 1); idle `y = 1` is evidence; busy excess beyond the predictive upper range plus one step is `OutsideProvisional` for 6 rows, then `Outside`; a refit that narrows the range re-classifies an earlier busy row with its original times (research R6)
- [X] T022 [P] [US1] In `crates/nullrouter-engine/tests/quota_fit.rs`, add separability tests: with cache writes always 1/10 of cache reads, `weight.cache_write` is `NotSeparable { partner: weight.cache_read }` and stays declared; a multiplier group with no traffic is `Learning` with 0 intervals, not `NotSeparable` (FR-005, research R7)
- [ ] T023 [P] [US1] Write `crates/nullrouter-engine/tests/sim_suite.rs` (`#[ignore]`): 100 fixed seeds of right-plugin weeks, each with and without outside use. Seeds include office-hours outside use. Assert 0 significant numbers, 100% placements equal to baseline (SC-002), and per-number range coverage ≥ 93% over at least 1,000 checks (SC-003). Print the measured null rejection rate from synthetic rows with the sim's noise beside the 0.1% bound. On failure, print the seed and number; the doc comment says a failing seed is investigated, never replaced (research R5)
- [X] T024 [P] [US1] In `crates/nullrouter-engine/tests/decision_record.rs`, assert `meter_sources` is absent when all numbers are declared, and is `{"<window>": {"capacity": "fit"}}` after a capacity becomes significant (FR-013, contracts/record.md; US1 scenario 6)

### Implementation for US1 + US2

- [X] T025 [US2] Implement `crates/nullrouter-engine/src/quota/fit/classify.rs`. `classify(row, fit) -> Class`, per research R6: idle `y ≤ 1 step` → `Idle` evidence, `≥ 2 steps` → `Outside`; busy above the predictive upper 95% plus one step → `OutsideProvisional { until: +6 rows }`. Add `reclassify_epoch(rows, fit)` for each refit, and `separability(info) -> Vec<(MeterNumber, Option<MeterNumber>)>` with the thresholds VIF > 50 or |corr| > 0.98, where the partner may be a part-of-day outside rate, named like `outside use 08–12` (research R7)
- [X] T026 [US1] Implement `crates/nullrouter-engine/src/quota/fit/split.rs`. Per account, fit its own `ρ, μ` from its rows and test it against the pooled values with `test::rejects` (`m` = accounts × numbers). On rejection, record `Split { since, reason }`, where the reason names the number and ratio (e.g. `weight.output 2.1× the pooled value`), and exclude the account's rows from the pool. Skip when the plugin has one polled account (research R8)
- [X] T027 [US1] Implement `Fits::observe(provider, account, history_tail, now)` in `crates/nullrouter-engine/src/quota/fit/mod.rs`: build the new rows, classify, refit the plugin window over the epoch's evidence rows, run separability and split, and move each number `Learning → Fitted(since)` when `test::rejects` holds against the value in effect (override or declaration). Then call `EngineState::rebuild_meters` when any state changed (FR-010, FR-011)
- [X] T028 [US1] Call `Fits::observe` after each good poll's entry is written, from the `PollHook` in `crates/nullrouter-engine/src/quota/history.rs`, after the entry is durably appended, on the poll task, never on a request (plan Performance Goals)
- [X] T029 [US2] Implement the outside-use list in `crates/nullrouter-engine/src/quota/fit/outside.rs`: append `entry` lines (`idle`, `busy`, `steady`) and `reclassified` lines to `quota/<provider>/<account>.outside.jsonl` through the history file's lock and `crate::files` (0600), and add `read(provider, account, since, limit)`. Log nothing for accounts without an exclusive-use declaration (FR-021, FR-022, contracts/state-files.md)
- [X] T030 [US1] Implement `crates/nullrouter-engine/src/quota/fit/store.rs`: load and save `quota/fit/<provider>.json` (atomic replace, 0600) holding epoch, `meter_hash` (SHA-256 of the canonical TOML of the declared `MeterDecl`), states with times, splits, breaks, prior and alerted rates. A file that doesn't parse is renamed `<provider>.json.bad-<time>`, the fit restarts and a warning is raised; it is never overwritten silently. At start, replay the history rows from each window's epoch. On disk full, keep state in memory and expose a warning (contracts/state-files.md, research R11, R12)
- [X] T031 [US1] Add `meter_sources: Option<BTreeMap<String, BTreeMap<String, Source>>>` to `CandidateRow` in `crates/nullrouter-engine/src/routing/mod.rs` (`skip_serializing_if = "Option::is_none"`), and fill it in `crates/nullrouter-engine/src/route.rs` from the source map `in_effect` returns, listing only non-declared numbers of the windows the candidate's quota read (contracts/record.md)
- [X] T032 [US2] Make the steady rate visible: when `b_a`'s confidence sequence excludes 0 (research R10), append one `steady` entry (rate per hour, since). Don't repeat it while the rate stays, and append a new one when it changes significantly (FR-021), in `crates/nullrouter-engine/src/quota/fit/outside.rs`
- [X] T033 [US1] Implement the edge rules in `crates/nullrouter-engine/src/quota/fit/mod.rs`. Pay-as-you-go and unpolled accounts are never fitted, and pooled weights don't apply to them. A capacity "assumed from peers" starts from the assumed value. Balance windows have no capacity number. When every account is split off, the view notes that no pooled fit remains. `plugins remove` deletes `quota/fit/<provider>.json` (spec Edge Cases). Account removal is T054

**Checkpoint**: The MVP. Run the sim week and the suite in CI. Placements before any significance equal slice 006's.

---

## Phase 4: User Story 3 — the operator sees which number routing uses, and why (Priority: P2)

**Goal**: One routing-view call shows, per polled account, window and number: declared value, override, fitted range, value in use, source, state and progress.

**Independent test**: During the simulated week, one `routing.view` call answers every number's in-use value, source, state and progress (SC-009).

### Tests for US3

- [ ] T034 [P] [US3] In `crates/nullrouter-engine/tests/routing_state.rs`, assert the view's `meter` per account and window carries `declared`, `account_override`, `plugin_override`, `fit {value, low, high}`, `in_use`, `source`, `state`, `since`, `progress {intervals, half_width}`, `partner` and `reason`, for learning, fitted, not-separable and yardstick numbers (Story 3 scenarios 1–4, contracts/operator-socket.md § WindowMeterView)
- [ ] T035 [P] [US3] In `crates/nullrouter-cli/tests/routing.rs`, add a golden-text test of the meter block for one learning, one fitted, one yardstick and one not-separable number, plus the `not fitted: provider reports no quota` line (contracts/cli.md § The routing view; Story 3 scenario 5)

### Implementation for US3

- [ ] T036 [US3] Add `meter: Vec<WindowMeterView>`, `outside_use: OutsideSummary` and `fit_note: Option<String>` to `AccountView` in `crates/nullrouter-engine/src/routing/view.rs`, filled from `Fits` and the outside-use list, with shapes exactly as in contracts/operator-socket.md (including `set_aside` counts per reason)
- [ ] T037 [US3] Carry the new fields through the `routing.view` op in `crates/nullrouter-server/src/operator.rs`, and add the warnings `… provider rules changed around …`, `fit state not saved since … (disk full)` and `…: N unacknowledged usage alerts (nullrouter quota alerts)` to the view's warnings
- [ ] T038 [US3] Render the meter block under each polled account in `crates/nullrouter-cli/src/routing_text.rs`, in the line format `name  declared · fit <low>–<high> · in use <value> <source> <state>` (leave out `in use` when it equals the declared value), the `outside use …` summary line, and one `usage alert` line per unacknowledged alert with its text, as in contracts/cli.md (FR-027)

**Checkpoint**: US1–US3 work; the operator can see every correction.

---

## Phase 5: User Story 4 — the operator's override wins (Priority: P2)

**Goal**: Overrides of every meter number, per account and (weights and multipliers) per plugin; account beats plugin beats fit beats declaration; the fit keeps learning.

**Independent test**: Override `weight.output` on the 3×-off mock before and after significance. Routing uses the override both times, and the view shows the fitted range beside it.

### Tests for US4

- [X] T039 [P] [US4] In `crates/nullrouter-engine/src/accounts.rs` tests, cover parsing and refusal of `window."<name>" = { token_weights = { output = 15.0 }, model_multiplier = { "claude-opus-*" = 1.5 } }`: refusals use the plugin gate's wording, a glob the plugin doesn't declare is refused, and token weights on a `requests` window are refused (Story 4 scenario 4)
- [X] T040 [P] [US4] In `crates/nullrouter-registry/src/schema/config.rs` tests, cover `[provider.<id>.meter."<window>"]` with `token_weights` and `model_multiplier` accepted, and `capacity` refused with `capacity is per account; use routing set <provider> <account>` (contracts/state-files.md § config.toml)
- [ ] T041 [P] [US4] In `crates/nullrouter-engine/tests/sim_week.rs`, override `weight.output` on the 3×-off mock before significance and after it. Routing uses the override both times, and the view shows `source: account_override` with the fit range. After removing the override, the fit (if significant) or the declaration is used. A multiplier override applies only to matching models. A plugin-level override applies to every account except one with its own account override (Story 4 scenarios 1–3, 5; FR-020)

### Implementation for US4

- [X] T042 [US4] Extend `WindowOverride` in `crates/nullrouter-engine/src/accounts.rs` with `token_weights: Option<PartialTokenWeights>` (each class optional) and `model_multiplier: IndexMap<String, f64>`. Parse and write them in `[account.routing] window."<name>"`, and check them in `RoutingOverrides::problem` with the registry's weight and multiplier rules (export them from `crates/nullrouter-registry/src/schema/routing.rs` if needed)
- [X] T043 [US4] Add `meter: BTreeMap<String, MeterOverride>` to `ProviderSettings` in `crates/nullrouter-registry/src/schema/config.rs`, with `MeterOverride { token_weights, model_multiplier }` and no capacity. Validate it against the plugin's declared windows and globs at load, and report errors as `config.toml:L:C provider.<id>.meter."<window>".<field>: <rule>`
- [X] T044 [US4] Pass plugin and account overrides into `in_effect` from `EngineState::rebuild_meters` (`crates/nullrouter-engine/src/state.rs`), and rebuild on reload, so the next placement uses a new override (Story 4 scenario 1)
- [X] T045 [US4] In `crates/nullrouter-cli/src/cmd/routing.rs`, accept `window.<name>.weight.<class>=V` and `window.<name>.multiplier.<glob>=V` in `routing set`/`unset`. Add `routing set-plugin <provider> KEY=VALUE…` and `routing unset-plugin <provider> KEY…`, which write `config.toml` through the existing config writer; `set-plugin … capacity` is refused with the contract's text (contracts/cli.md § Overrides)

**Checkpoint**: Overrides at both levels win, and the fit keeps learning underneath.

---

## Phase 6: User Story 5 — provider rule changes are noticed (Priority: P2)

**Goal**: A significant, consistent disagreement with a fitted number is recorded as a break: routing falls back, the fit relearns from the break, and the view and log say so. A plugin meter change restarts the window's fit. Everything survives restarts.

**Independent test**: Halve one account's true capacity on day 4. The break is reported within one simulated day, and no placement after the report uses the old fitted capacity.

### Tests for US5

- [ ] T046 [P] [US5] In `crates/nullrouter-engine/tests/sim_week.rs`, halve the 3×-off account's true capacity on day 4. Assert a break is recorded within 1 simulated day, 0 placements after the report use the old fitted capacity (SC-006), only rows after the break count for the new fit (Story 5 scenario 2), and the busy rows around the halving are not listed as outside use and raise no alert
- [ ] T047 [P] [US5] In `crates/nullrouter-engine/tests/sim_week.rs`, restart cleanly and crash (drop without shutdown) mid-week. Assert fits, states, breaks, outside-use entries, exclusive-use declarations and alerts are identical after replay (SC-008, Story 5 scenario 5)
- [ ] T048 [P] [US5] In `crates/nullrouter-engine/tests/quota_fit.rs`, change one window's declared meter between loads. Assert that window's numbers become `Restarted` with reason `plugin meter changed` and no other number changes (SC-008, FR-017). Also cover the fit file that doesn't parse: it is renamed, the fit restarts, and a warning is raised
- [ ] T049 [P] [US5] In `crates/nullrouter-engine/tests/quota_fit.rs`, inject an outside-use burst against a fitted model. Assert the break detector does not fire (Story 5 scenario 4)

### Implementation for US5

- [ ] T050 [US5] Implement `crates/nullrouter-engine/src/quota/fit/breaks.rs`: for each `Fitted` number, confidence sequences started at each whole hour over the last 48 hours, each testing the number fitted from rows after its start against the current fitted value (`m` = 48 × numbers). Score `OutsideProvisional` rows too. On rejection, record `Break { at, detected_at, window, number, replaced }`, move that window's fitted numbers (for capacity, only that account's) to `Relearning(since)`, start the epoch at `at`, and reclassify that span's provisional rows as evidence with `reclassified` lines (research R6, R9)
- [ ] T051 [US5] Emit `WARN quota.fit: provider rules changed provider=… account=… window=… number=… around=… replaced=…` with `tracing` when a break is recorded (`crates/nullrouter-engine/src/quota/fit/breaks.rs`, FR-016)
- [ ] T052 [US5] Restart on a meter change: in `crates/nullrouter-engine/src/quota/fit/store.rs`, compare each window's `meter_hash` at load and at reload. On a difference, set a new epoch with `restarted { at, reason: "plugin_meter_changed" }` (FR-017)
- [ ] T053 [US5] Fold pruned in-epoch rows into the prior before pruning, in `crates/nullrouter-engine/src/quota/history.rs` `prune`: compute the pruned rows' estimate and information, and save them through `store.rs`. Make `forget` also delete `<account>.outside.jsonl` (research R11, contracts/state-files.md)
- [ ] T054 [US5] Implement account epochs in `crates/nullrouter-engine/src/quota/fit/store.rs` and `mod.rs`. On `accounts remove` (hook in `crates/nullrouter-engine/src/accounts.rs` removal path), fold the account's rows into the pooled prior, drop its capacity fit, and rename `quota/<p>/<a>.outside.jsonl` to `<a>.outside.jsonl.removed-<time>`. On (re-)add, set `account_epochs[<a>]` to now, so earlier history rows count for nothing. Test in `crates/nullrouter-engine/tests/quota_fit.rs`: remove and re-add an account with history; its capacity is `Learning` with 0 intervals and `quota outside` is empty (spec Edge Cases, research R11)

**Checkpoint**: Breaks and restarts are handled; the simulated week's SC-006 and SC-008 pass.

---

## Phase 7: User Story 6 — opt-in leak alerts (Priority: P3)

**Goal**: Accounts declared as used only through 0router raise factual alerts for idle drops beyond rounding, for busy-time excess that passes the test, and for a steady rate. Alerts change nothing and can be acknowledged.

**Independent test**: Inject idle drops and rounding noise on a declared and an undeclared account. Alerts fire only for drops beyond one step, and only on the declared account.

### Tests for US6

- [ ] T055 [P] [US6] In `crates/nullrouter-engine/tests/sim_week.rs`, inject idle drops, 1-step noise and busy-time bursts on one exclusive-use and one non-exclusive account. Assert 0 alerts on the non-exclusive account, 0 alerts from 1-step noise, every idle drop of ≥ 2 steps alerted at the first poll that shows it, 0 alerts on busy intervals without injected use, and an alert for each busy burst whose excess passes the test (SC-007, Story 6 scenarios 1, 3, 4, 8)
- [ ] T056 [P] [US6] In `crates/nullrouter-engine/tests/quota_fit.rs`, assert alert text matches `"<provider>/<account>: N% of <window> used HH:MM–HH:MM with no traffic from 0router"` (idle) and contains no "leak" or "key". Assert the account's state, priority and routing are unchanged after an alert, and that withdrawing the declaration stops new alerts (FR-025, FR-026; Story 6 scenarios 5, 7)
- [ ] T057 [P] [US6] In `crates/nullrouter-cli/tests/accounts_keys.rs`, `quota.rs` and `check.rs`, assert `accounts exclusive xai main on` on an unpolled account exits 2 with `account xai/main: exclusive use needs quota polls; xai reports no quota for this account` (FR-023); `quota ack <id>` removes the alert from `quota alerts` and keeps the entry in `quota outside` (Story 6 scenario 6); `check` lists unacknowledged alerts as warnings and its exit status is unchanged by them (0 with no errors)

### Implementation for US6

- [ ] T058 [US6] Add `exclusive_use: Option<SystemTime>` to `Account` in `crates/nullrouter-engine/src/accounts.rs` (TOML key `exclusive_use`, RFC 3339). Refuse it at load and in the CLI for an account whose provider declares no quota report for it, with the FR-023 wording
- [ ] T059 [US6] Raise alerts in `crates/nullrouter-engine/src/quota/fit/outside.rs`, only when the account was exclusive-use at the entry's `start`: idle `Outside` rows at once; busy rows when they become final `Outside` and their excess passes `test::rejects` against 0 (`m` = the account's windows); a steady rate when `b_a`'s confidence sequence excludes 0, once per established rate. Append `alert` lines with text built per data-model § Leak alert, and emit `WARN quota.alert: unexplained use …` (FR-024, FR-025, FR-027, clarify Q3)
- [ ] T060 [US6] Implement acknowledgement: `ack(provider, account, id | all)` appends `ack` lines in `crates/nullrouter-engine/src/quota/fit/outside.rs`; unacknowledged alerts are alerts with no `ack` line (FR-027)
- [ ] T061 [US6] Add the `quota.outside`, `quota.alerts` and `quota.ack` ops in `crates/nullrouter-server/src/quota.rs` and `crates/nullrouter-server/src/operator.rs`, with shapes as in contracts/operator-socket.md
- [ ] T062 [US6] Add `quota outside [provider [account]] [--since T] [--limit N]`, `quota alerts` and `quota ack <id>|all [provider [account]]` to `crates/nullrouter-cli/src/cmd/quota.rs`. Read the files offline, and use the socket for `ack` when a server runs. Text formats as in contracts/cli.md
- [ ] T063 [US6] Add `accounts exclusive <provider> <account> on|off` and the `exclusive` column of `accounts list` in `crates/nullrouter-cli/src/cmd/accounts.rs`
- [ ] T064 [US6] In `crates/nullrouter-cli/src/cmd/check.rs`, list each unacknowledged alert as `warn  <text> (nullrouter quota ack <id>)`, as a warning that leaves the exit status alone (contracts/cli.md § check, FR-026)

**Checkpoint**: All six stories work.

---

## Phase 8: Polish & cross-cutting

- [ ] T065 [P] Extend the secret scan (`crates/nullrouter-engine/tests/no_cloaking.rs`, or the redaction test that covers records and logs) to `quota/fit/*.json`, `*.outside.jsonl`, the view's JSON and the `quota.fit`/`quota.alert` log lines. Assert no account secret or token appears, and that no fitted value, interval or alert reaches a provider request or plugin input (SC-010, FR-030, FR-031)
- [ ] T066 [P] Add `live_fit_progress` to `crates/nullrouter-engine/tests/live.rs` (opt-in `NR_LIVE=1`). It reads `NULLROUTER_HOME`'s history, runs the fit offline, and prints state and progress for every number of every polled account. It **fails** when `NULLROUTER_HOME` is unset or no polled account exists (SC-012, research R16; lesson of 2026-10-05)
- [ ] T067 [P] Add a placement case with meters in effect to `crates/nullrouter-engine/benches/engine.rs`, compared against the `slice-006` baseline locally only (SC-011)
- [ ] T068 [P] Add a CI step in `.github/workflows/ci.yml` after the workspace tests: `cargo test -p nullrouter-engine --release --test sim_suite -- --ignored 2>&1 | tee sim_suite.log`, and pass its log to `tools/ci/annotate.sh`
- [ ] T069 [P] Document quota fit, outside use, exclusive use and alerts, the new override keys and `set-plugin`, and the meter block in `docs/operator-config.md` (§ Quota, § Account overrides, § The routing view), following contracts/cli.md
- [ ] T070 Run quickstart.md's checks in CI (push once, with the user's OK) and record the sim-week table and suite counts in the PR description

---

## Dependencies & Execution Order

- **Setup (T001–T003)** → **Foundational (T004–T016)** → stories.
- **US1 + US2 (T017–T033)**: the MVP; needs Foundational. T027 needs T025, T026 and T030; T028 needs T027.
- **US3 (T034–T038)**: needs the MVP's `Fits` (T027).
- **US4 (T039–T045)**: needs `in_effect` (T013) and `rebuild_meters` (T014). It can run in parallel with US3.
- **US5 (T046–T054)**: needs the MVP (T027, T030).
- **US6 (T055–T064)**: needs the outside-use list (T029) and the test (T012). It can run in parallel with US5.
- **Polish (T065–T070)**: after the stories it touches. T070 is last.

```text
Setup → Foundational → US1+US2 (MVP) ─┬─ US3
                                      ├─ US4
                                      ├─ US5
                                      └─ US6 → Polish
```

## Parallel Examples

- Foundational tests: T004, T005, T006, T007 and T008 together (different files or test modules).
- MVP tests: T017–T024 together, before T025–T033.
- After the MVP: US3 (T034–T038) and US4 (T039–T045) in parallel; US5 and US6 in parallel. Each story's tests marked [P] together.
- Polish: T065–T069 together.

Per the project's pace rule, run one Opus subagent at a time unless the user asks for parallel ones.

## Implementation Strategy

1. **MVP = Phases 1–3**: corrections with outside use kept out, durable, with records naming sources. Push the group and read CI: the sim week (SC-001, SC-004, SC-005) and the suite (SC-002, SC-003).
2. **Then US3 and US4**: the view and overrides. These make the MVP visible and controllable.
3. **Then US5**: breaks and restarts (SC-006, SC-008).
4. **Then US6**: opt-in alerts (SC-007).
5. **Polish**: secret scan, live check, bench, CI step, docs.

Commit after each task or small group. Push only in groups, with the user's OK; each push cancels the previous CI run.
