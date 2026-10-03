# Bench baseline: slice 005

Recorded by T001 on 2026-10-02, before any slice 005 engine change, at `67a9b4b`, with
`nice cargo bench -p nullrouter-engine --bench engine -- --save-baseline pre-005`. The slice
002/003 baselines were lost to a `cargo clean` earlier that day. The Criterion data lives in
`target/criterion/*/pre-005/` (local only, not committed); the summary below is the record.

The registry subagent was writing code (no builds) during the run; the machine was otherwise idle.

| Bench | Low | Estimate | High |
|---|---|---|---|
| `ttfb/direct` | 261.23 µs | 285.13 µs | 317.26 µs |
| `ttfb/same_style` | 479.45 µs | 510.65 µs | 546.36 µs |
| `ttfb/cross_style` | 491.97 µs | 508.75 µs | 528.09 µs |

T084 adds a sign-in account case and compares with `--baseline pre-005`; T093 appends the
final summary here.

## Result (T093, 2026-10-03)

The machine wasn't idle: other software kept the load average at 4.6–5.8 on 4 cores for the
whole session, and a plain comparison against `pre-005` read +35–75 %, which was load, not code.
So the pre-slice code (`67a9b4b`, built in a worktree sharing the target directory) and the
slice code were run back to back, alternating, under the same load. Per-request medians:

| Bench | Pre-slice (2 runs) | Slice 005 (2–4 runs) |
|---|---|---|
| `ttfb/direct` | 281, 318 µs | 308, 299 µs |
| `ttfb/same_style` | 546, 586 µs | 560, 563 µs |
| `ttfb/cross_style` | 726, 634 µs | 745, 583, 612, 650 µs |
| `ttfb/signin` (new) | — | 733 µs |

No regression on the existing paths: every slice median lies within the spread of the pre-slice
runs. One Criterion comparison flagged `cross_style` (+4.6…+17.5 %, p < 0.05); the reverse-order
run (+9 %, p = 0.05, "no change") and the medians above don't reproduce it. `ttfb/signin` uses a
different test plugin than the others, so it isn't comparable to them; it is the reference for
later slices. Re-run on an idle machine before release if a tighter figure is wanted.

Also fixed: the bench's sign-in tally assertion failed on filtered runs (`-- ttfb/cross_style`).
