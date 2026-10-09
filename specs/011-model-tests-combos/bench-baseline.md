# 011 bench baseline (T054, SC-009)

SC-009: skipping BROKEN pairs and resolving combos adds no measurable regression to the placement
of a request. Target: each case that exists on both sides within 5 % of before.

Before is the branch point `1f94e08` (011 was cut from `009-dashboard` there, so `main` lacks
009's benches); after is the head of `011-model-tests-combos`. Run both on the same machine, with
`nice` and `-j 2`:

```bash
export CARGO_HOME=$PWD/.cargo-home
git switch --detach 1f94e08
nice cargo bench -j 2 -p nullrouter-engine --bench engine -- --save-baseline before-011
nice cargo bench -j 2 -p nullrouter-registry --bench resolve -- --save-baseline before-011
git switch 011-model-tests-combos
nice cargo bench -j 2 -p nullrouter-engine --bench engine -- --baseline before-011
nice cargo bench -j 2 -p nullrouter-registry --bench resolve -- --baseline before-011
```

Baseline: **not recorded yet.** Local cargo is off for this slice and cloud timings don't compare
with local ones (CLAUDE.md), so the first local run fills these tables, with the machine named.

Machine:

## Engine (`nullrouter-engine --bench engine`)

| Case | Before | After | Change | Within 5 % |
|---|---|---|---|---|
| `ttfb/direct` | | | | |
| `ttfb/same_style` | | | | |
| `ttfb/cross_style` | | | | |
| `ttfb/signin` | | | | |
| `tally/attempt` | | | | |
| `plan/verdicts_0` | new | | | |
| `plan/verdicts_1000` | new | | ≤ 5 % over `plan/verdicts_0` | |
| `plan/combo_3_levels` | new | | | |

## Registry (`nullrouter-registry --bench resolve`)

| Case | Before | After | Change | Within 5 % |
|---|---|---|---|---|
| `load` | | | | |
| `resolve/direct` | | | | |
| `resolve/direct_suffix` | | | | |
| `resolve/unified` | | | | |
| `resolve/combo` | new | | | |
| `resolve/uncatalogued` | | | | |
| `resolve/not_found` | | | | |

The `plan` group and `resolve/combo` are new in this slice, so they have no before. For them the
check is within the slice: `plan/verdicts_1000` (1,000 verdicts on the board) against
`plan/verdicts_0` (none).

The resolve bench's home gained two combos in this slice, so `load` after also loads and checks
them; a small rise there is the combos' own cost, not a regression in placement.
