# Research: Quota Fit, Outside Use and Leak Detection

There is no 9router oracle for this slice: 9router only displays quota
(`ref/9router/src/app/(dashboard)/dashboard/usage/components/ProviderLimits/QuotaProgressBar.js`,
`QuotaTable.js`). Every decision below is 0router's own, and is tested against the simulated
week (R14) rather than against parity fixtures.

## R1. Where the fit plugs into routing

**Decision**: The fit never touches the decision core. Before `meter::quota_for` runs,
`route.rs` builds the **meter in effect** for each window: a `MeterDecl` with each number taken
from account override, plugin override, significant fit, or declaration (FR-012). It passes that
meter where it passes `routing.windows` today. `cost`, `cost_spent`, `quota_for` and `place` are
unchanged.

**Rationale**: FR-011 and SC-002/SC-005 require placements equal to today's until a number is
significant. If no number is significant and no new override is set, the meter in effect is the
declared meter, field for field, so the decision core gets the same input as today. That makes
placement equality structural rather than tested-for, and it keeps the decision core pure
(slice 006's R2).

**Alternatives considered**: (a) Teach `cost_spent` about fits. That spreads fit lookups through
the hot path and makes equality depend on every branch. (b) Rewrite the plugin's meters in the
registry snapshot. That breaks FR-006 in spirit, and a reload would drop fits.

## R2. What one row of evidence is

**Decision**: A **poll interval** is a pair of consecutive good poll entries of one account in
`quota/<provider>/<account>.jsonl` (slice 005), with no reset of the window in between. A failed
poll in between is bridged: the interval runs from the last good entry to the next one, and the
tally is the sum of all entries' tallies in between (the entries already carry it). One row per
window per interval:

- `y`: the reported use change, in the window's report unit (percent points, or units for a
  counted or balance window), from `used` or `100 − remaining`.
- `x`: 0router's traffic by **multiplier group** and token class. A multiplier group is the
  first model glob of the declared meter that matches the tally's upstream model, or "no
  multiplier" (factor 1). For each group: `input`, `output`, `cache_read`, `cache_write`
  (`quota/tally.rs`'s non-overlapping categories), plus `requests` for request-counted windows.
- `t`: the interval's length in hours, for the steady outside rate.

A row is **set aside**, and is neither evidence nor outside use, when:
- the window reset inside it (its `resets_at` changed, or `used` fell);
- any attempt in it has usage the provider didn't report (`requests_usage_unreported > 0`). The
  view counts these (Edge Cases);
- the window is exhausted at either end (100% used): a full window hides use.

**Rationale**: Everything the fit needs is already on disk and durable (slice 005's history,
slice 006's tally). Rows are derived from history and never stored twice, so FR-018 is met by
the history itself plus a small state file (R12).

**Alternatives considered**: A separate interval log. It would duplicate the history and could
drift from it.

## R3. The model

**Decision**: For one window of one plugin, the expected use change of a row is

```
percent window:   E[y] = k_a · Σ_g μ_g · (I_g + ρ_o·O_g + ρ_r·R_g + ρ_w·W_g) + b_{a,q}·t
counted/balance:  E[y] =       Σ_g μ_g · (w_i·I_g + w_o·O_g + w_r·R_g + w_w·W_g) + b_{a,q}·t
request-counted:  E[y] = N + b_{a,q}·t                   (N = requests in the interval)
```

- `k_a = 100 · w_i / C_a`: the account's percent per input-weighted token. The **yardstick**
  (clarify Q1) holds `w_i` at its declared or overridden value, so `C_a = 100 · w_i / k_a` is
  the fitted capacity.
- `ρ_c = w_c / w_i`: the other weights relative to input. They are reported as weights:
  `w_c = ρ_c · w_i`.
- `μ_g`: the multiplier of glob `g`. Models no glob matches have factor 1, fixed.
- `b_{a,q} ≥ 0`: the account's steady unexplained rate in **part of the day** `q`, in report
  units per hour (Story 2 scenario 2). There are six parts of 4 hours (00–04 … 20–24, operator
  local time from the system clock). A row spanning two parts splits `t` between them. Idle rows
  pin each part's rate. This is the answer to outside use with a daily rhythm (spec Edge Cases,
  analysis C1). A flat rate is the case of six equal parts. The view reports the parts' rates
  and the largest of them as "steady".

A request-counted window reported in percent uses `E[y] = k_a · N + b_{a,q} · t`, which is the same
capacity fit with no weights.

**Capacity of a window reported in absolute units.** When the poll reports a `limit`, the limit
*is* the capacity, known to one unit. Its "fit" is the reported value, with a range of ±1 unit.
It goes through the R5 test like any other number: each reported limit is one reading, with
rounding variance `1/12` unit², so two or three agreeing polls that differ from the declared
value by more than a unit are enough to reject it. A window that reports
`used` but no `limit` gives no information about capacity, and the view shows "capacity: not
reported by the provider". Balance windows have no capacity number (Edge Cases).

Pooled numbers (`ρ`, `μ`, and on counted or balance windows `w`) are shared by every non-split
account of the plugin (FR-002). `k_a` (capacity) and `b_a` are per account. A split account
(R8) gets its own `ρ`, `μ`.

The estimate is weighted nonlinear least squares in log parameters (positivity), solved by
Gauss–Newton from the meter in effect as the starting point. Every poll refits from all rows of
the current epoch (R11). Size: a plugin with 3 accounts, 2 multiplier globs and 4 classes has
about 3 + 3 + 8 parameters. A month of 10-minute rows is about 4,300 rows per account, so a refit
costs well under a millisecond, once per poll, off the request path.

**Rationale**: This is the smallest model that contains every meter number the spec names
(FR-001) and the steady outside rate (FR-021). Log parameters keep weights positive and make the
ranges multiplicative, which matches how plugin numbers are wrong (3×, half).

**Alternatives considered**: Sufficient statistics with online updates. They are exact only for
a linear model, and the pooled model is bilinear (`k_a · ρ`). They also can't drop a row
retroactively, which outside-use classification (R6) needs while the fit is still wide.

## R4. Noise, and the 95% range

**Decision**: Each reading is rounded to the provider's resolution (1 point on a percent window,
1 unit on a counted window). The model treats the reading error as uniform within one step:
variance `1/12` per reading. A row's `y` is a difference of two readings, so its noise has
variance `1/6`, and adjacent rows share a reading, with covariance `−1/12`. On top of that is an
unknown extra variance `σ²_e` (requests that span a poll, provider lag), estimated from the
residuals and floored at 0.

The covariance of the estimate is the sandwich `J⁻¹ (J_r + σ²_e · J₀) J⁻¹`, where:
- `J₀ = Σ g_i g_iᵀ` over the row gradients;
- `J_r` applies the known tridiagonal rounding covariance (`1/6` on the diagonal, `−1/12`
  between adjacent rows of one account);
- `J = J₀` (Gauss–Newton).

The **95% range** of a number is `exp(log θ̂ ± 1.96 · se)`. It is shown, and it is the range
FR-004 and SC-003 talk about.

**Rationale**: FR-004 requires rounding to be part of the noise. Its structure is known exactly,
so it is modelled, not estimated. The `−1/12` covariance matters: ignoring it overstates the
noise of sums of rows by about 2×, and so slows significance for nothing.

**Alternatives considered**: (a) Regress cumulative levels within each reset period. The errors
would be i.i.d., but one outside-use burst would shift every later level of the period. (b) An
HAC (Newey–West) estimate. Unneeded when the dominant noise's structure is known.

## R5. The significance test (FR-010, clarify Q2)

**Decision**: A number becomes significant when a **normal-mixture confidence sequence**
(Robbins; Howard, Ramdas, McAuliffe and Sekhon 2021) excludes its declared or overridden value.
With information `V = 1/se²` of its log estimate and `Z = (log θ̂ − log θ₀) · V`:

```
significant  ⇔  |Z| ≥ sqrt( (V + ρ) · ( ln((V + ρ)/ρ) + 2 · ln(m / α) ) )
```

- `α = 0.001`: the clarified lifetime false-correction bound.
- `m`: the number of numbers tested at once in the window's fit. The Bonferroni split gives
  each number `α/m`, so the window as a whole stays under 0.1%.
- `ρ`: the mixture's tuning, set so the boundary is tightest at the information a factor-2
  error reaches in about a day of typical traffic. It is a constant in code, derived in closed
  form from the information per row of the sim world's traffic mix (no sim run is used to tune
  it, so the week that grades SC-001 doesn't also choose it), and recorded with its derivation.

The boundary holds at every poll for the number's whole life, so re-checking at every poll
costs nothing (the reason for the clarification). The same test (same `α`, own `m`) decides
split-offs (R8), breaks (R9), a leak alert's steady rate, and busy-time leak alerts (R10).

**Rationale**: A fixed-sample 95% test re-checked about 1,000 times a week rejects a true
declaration almost surely. The confidence sequence is the standard always-valid answer. The
reading noise is bounded (uniform), so its sub-Gaussian assumption holds conservatively.

**Known risk, recorded for the tests**: SC-002 runs 100 seeded weeks over about 4 windows each.
The bound allows up to about 0.4 false significances across the suite in the worst case. Mixture
boundaries are conservative in practice, but this is a bound, not a guarantee. If a seed trips,
that is a finding to investigate (is the noise model wrong?). It is never a reason to change the
seed (anti-cheat). To tell the two apart, the suite also reports the **measured** null rejection
rate of the test on synthetic rows with the sim's noise (T007), beside the bound.

**Alternatives considered**: (a) Plain sequential probability ratio tests per number. They need a
fixed alternative, and the error size is unknown. (b) Alpha-spending over a fixed horizon. Fits
have no horizon. (c) A Bayesian posterior threshold. It has no lifetime error guarantee, which
is what the user chose.

## R6. Classifying a row (FR-007 – FR-009)

**Decision**: Each new row is classified against the fit **before** it joins it:

1. **Idle** (0router sent nothing in the interval): `y ≤ 1 step` is rounding noise and stays an
   evidence row (it informs `b_a`). `y ≥ 2 steps` (beyond one step, FR-009) is **outside use**:
   listed with its start, end and amount, and excluded.
2. **Busy**: let `ŷ⁺` be the upper end of the row's 95% predictive range under the current fit,
   plus one step. If `y ≤ ŷ⁺`, the row is evidence. Otherwise the excess `y − ŷ⁺` is
   **provisionally outside use**. It is excluded from the fit and stays provisional for one
   **settle span** (6 rows, about an hour at 10-minute polls).
3. At the end of the settle span, a provisional row becomes final outside use unless the break
   detector (R9) has fired over it. If it has, the row was a sign of new rules: it becomes
   evidence for the new epoch, and is never listed or alerted.

While a number is still learning, its range is wide, so few busy rows are excluded. Outside use
that slips in then is diluted, and is re-tested at each refit: every refit re-classifies the
epoch's busy rows against the current fit (cheap, R3). A row that turns out beyond the new
range moves to the outside-use list then, with its original times.

**Rationale**: FR-008 defines outside use as use beyond what any meter within the fit's ranges
could explain. The predictive range is exactly that. The settle span is what keeps a capacity
halving (proportional excess, rows in a row) from being listed as outside use and alerted as a
leak. Idle drops need no settle span, because no rule change can make an idle interval drain:
SC-007 asks for those alerts at the first poll that shows them.

**Alternatives considered**: (a) Robust (Huber) weights without explicit exclusion. Outside use
would still pull the fit a little, and SC-004 compares fits with and without it. (b) Classifying
once and never revisiting. Early outside use would stay in the fit forever.

## R7. Numbers the traffic can't separate (FR-005)

**Decision**: After each refit, for each number, compute its variance inflation factor (VIF)
from the information matrix, and its most collinear partner (the largest absolute correlation in
the inverse). The partner may be a part-of-day outside rate `b_{a,q}`: then the view names that
part of the day ("not separable from outside use 08–12: 0router is never idle then"). A number is **not separable** when its VIF exceeds 50, or its partner correlation
exceeds 0.98. It is then held at the value in effect, excluded from the test, and the view names
the partner. A number with no traffic at all (no rows with that class or group) is **learning**
with 0 rows, not "not separable".

**Rationale**: The typical case is cache writes always arriving in fixed proportion to cache
reads, or a multiplier group whose model is the only one sent. Fitting them anyway gives huge
ranges that the test would never reject, but the view would read as "learning forever". Naming
the partner tells the operator why (Story 3 scenario 3).

## R8. Pooling and split-off (FR-002, FR-003)

**Decision**: Weights and multipliers are fitted jointly over all non-split polled accounts of
the plugin (R3). For each account, a **split test** asks whether its own rows reject the pooled
`ρ` and `μ`. It compares the account's own fit of `ρ, μ` with the pooled fit, using the
confidence sequence of R5 with `m` covering every account and number tested. When it rejects:

- the account is split off from that time, with the reason ("output weight 2.1× the pooled
  value");
- its rows stop counting in the pooled fit;
- its own `ρ, μ` are fitted from its own rows.

A split is permanent until a break or restart of that window (R9, R11). A rejoin command was
considered and left out: it isn't in the spec, and a break or meter change already rejoins.
With one account, there is nothing to split from (Edge Cases). The split test is also the guard
for outside use that tracks 0router's traffic on one account (spec Edge Cases): that account
disagrees with its peers and is split off, so the pooled weights stay clean.

**Rationale**: The promotion case (Story 1 scenario 5) is exactly a rejection of the pooled value
by one account. Using the same always-valid test keeps a right account from being split off by
chance.

## R9. Breaks (FR-015, FR-016)

**Decision**: For each **significant** number, 0router keeps a **break detector**: confidence
sequences of R5 started at each whole hour over the last 48 hours. Each tests the number fitted
from rows after that hour alone against the current fitted value. `m` counts the 48 starts and
the numbers. When one rejects:

- a **break** is recorded at that start ("provider rules changed around Tue 14:00");
- every fitted number of that window (for capacity, only that account's) goes back to learning,
  and routing uses the override or declaration (FR-015);
- the fit's new epoch starts at the break, and rows before it no longer count;
- the break goes to the routing view and the `serve` log.

Rows provisionally classified as outside use inside the settle span (R6) are scored by the
detector, so a proportional excess shows as a break, not a leak. A burst that doesn't track
traffic moves the traffic-direction score little and doesn't fire (Story 5 scenario 4).

**Rationale**: SC-006 asks for detection within a day of a halving. A halving is a factor-2 error
on `k_a`, about as large as the corrections the fit is tuned for (R5), so hourly starts detect it
within hours. 48 hours of starts bounds the cost and the `m` penalty.

**Alternatives considered**: (a) A CUSUM with a fixed shift size. It needs the shift known in
advance. (b) Comparing the last N rows to the rest. Its error rate under repeated checks is
uncontrolled.

## R10. Leak alerts (FR-023 – FR-027, clarify Q3)

**Decision**: Only accounts with an **exclusive-use declaration** raise alerts.

- **Idle drop**: an idle row of outside use (R6 rule 1) raises an alert at once.
- **Busy excess**: a busy row that becomes final outside use (R6 rule 3) raises an alert when its
  excess passes the R5 test against zero. The test's information is that of the row's
  predictive variance, and `m` counts the account's windows.
- **Steady rate**: when `b_a`'s confidence sequence excludes 0, one alert reports the rate and
  since when, and it isn't repeated while the rate stays.

The alert text states facts: `anthropic/max: 4% of weekly used 02:10–02:30 with no traffic from
0router`. It never uses "leak" for the cause. It changes nothing on the account (FR-026).
Acknowledging removes it from the alert list. The outside-use list keeps the entry (FR-027).

**Rationale**: This follows the clarified rules exactly. Idle drops alert at the first poll that
shows them, as SC-007 requires.

## R11. Epochs and restarts (FR-017, FR-018)

**Decision**: Each window of each plugin has an **epoch**, a start time from which rows count. It
is set by:
- the first poll after this slice ships;
- a break (R9);
- a change of the window's declared meter. The state file keeps a hash of the declared
  `MeterDecl` per window. A different hash at load restarts that window's epoch, and the view
  says "restarted: plugin meter changed".

An account also has its own **account epoch**, set when it is (re-)added: its rows before that
time count for nothing, neither its capacity nor the pooled weights' new evidence. The pooled
weights keep what its earlier rows gave only through the prior folded at removal (spec Edge
Cases). At `accounts remove`, its `.outside.jsonl` is renamed `.outside.jsonl.removed-<time>`:
kept on disk, never shown or read again.

At start, the fit is rebuilt by replaying the history rows of the current epoch. Fits are
deterministic in their rows, so a restart and a crash give the same fits (SC-008).

When the operator prunes history from inside an epoch (`quota prune`), the fit loses those rows.
Before pruning, the CLI folds them into the state file as a **prior** (spec Edge Cases: pruned
history): the estimate and
information matrix of the pruned rows, added to the refit as one Gaussian term. Nothing else is
pruned automatically (slice 005).

**Rationale**: History is already durable (slice 005's write-ahead and checkpoint rules), so
replay is the simplest correct durability. Replaying a month of rows for every plugin at start
costs milliseconds.

## R12. What is stored, where

**Decision**:
- `quota/fit/<provider>.json` (0600, atomic replace): per window, the epoch, the meter hash,
  each number's state and state times ("fitted since", "relearning since", split accounts and
  reasons), breaks, any folded prior, and alerted steady rates. Estimates aren't stored: they
  are rebuilt (R11). The view reads the in-memory fit.
- `quota/<provider>/<account>.outside.jsonl` (0600, append-only, beside the history): one line
  per outside-use entry or steady rate, one per alert raised, one per acknowledgement. It is
  write-ahead like the history (slice 005's writer and lock).
- `accounts.toml`: `exclusive_use = <RFC 3339 time>` on the account (FR-023). Per-account weight
  and multiplier overrides go in `[account.routing] window."<name>"` beside `capacity`.
- `config.toml`: plugin-level overrides go under `[provider.<id>.meter."<window>"]`, as
  `token_weights = { output = 15.0 }` and `model_multiplier = { "claude-opus-*" = 1.5 }` (the
  existing per-provider settings table).

**Disk full** (Edge Cases): fit state stays in memory, and the view warns, as slice 006 does for
the routing journal (`files::FileError::Full`).

**Rationale**: Each fact lives with its owner. Overrides go with the other overrides, alerts with
the account's history, and the fit state with the plugin. `quota forget` removes the
account's outside-use file with its history, and `plugins remove` removes `fit/<provider>.json`
(Edge Cases).

## R13. Request records (FR-013)

**Decision**: Each `CandidateRow` of the decision record gets `meter_sources`: for each window the
placement read, a compact map from number to source, where a number isn't `declared`, e.g.
`{"weekly": {"capacity": "fit", "weight.output": "plugin_override"}}`. The field is absent when
every number is declared, which keeps today's records byte-identical until a fit is significant
or an override is set.

**Rationale**: FR-013 and brief row 21 want "why it landed there" after the fact. Leaving the
field out in the all-declared case costs nothing, and keeps old records and read-model queries
(slice 008) valid.

## R14. Evidence: the simulated week, extended

**Decision**: Extend `crates/nullrouter-engine/tests/sim_week/` (slice 006's world: injected
clock, true accounts that charge exactly by their meters, 10-minute polls) with:

- true meters that differ from the plugin's. The **3×-off** mock has output and cache weights 3×
  the plugin's relative to input, and capacity 0.6× the declared. The **right** mock is exact;
- whole-percent rounding of every reported reading (round half up, the common provider
  behaviour; floor is also run, to show the period intercept absorbs it);
- outside use: idle bursts, a steady background rate, and bursts during traffic, with a log of
  injected intervals for SC-004/SC-007;
- a capacity halving on day 4 (SC-006), and a meter change at load (SC-008);
- a restart and a crash mid-week, replayed from the files (SC-008);
- a **baseline run** of the same seed with fitting disabled, for the placement equality of
  SC-002/SC-005.

A second test target, `sim_suite`, runs 100 fixed seeds of right-plugin weeks, with and without
outside use, and counts significances and range coverage (SC-002, SC-003). It is gated behind
`--ignored` and runs in CI in release mode. Locally, cargo isn't run (project rule).

**Rationale**: The world already exists and is trusted (slice 006's SC-001 – SC-006). Reusing it
keeps the evidence comparable.

## R15. Performance (SC-011)

**Decision**: The only new work on the request path is building the meter in effect for each
candidate window. It is precomputed when a fit's state or an override changes, and stored in the
engine state as an `Arc<[MeterDecl]>` per account, read through `arc-swap` like the rest of the
snapshot. `route.rs` swaps `routing.windows` for that slice, which costs a pointer load. The fit
itself runs on the poll task. The `engine` bench's placement case is the gate (no measurable
regression against the `slice-006` baseline).

## R16. Live check (FR-034)

**Decision**: Add `live_fit_progress` to `crates/nullrouter-engine/tests/live.rs` (opt-in with
`NR_LIVE=1`, as slice 006's L7). It reads the operator's real `NULLROUTER_HOME` history, runs the
fit offline, and prints per account, window and number its state and progress. It fails only if a
polled account's number has no state (SC-012). Per the 2026-10-05 lesson, it **fails** when
`NULLROUTER_HOME` is unset or no polled account exists, instead of passing vacuously.
