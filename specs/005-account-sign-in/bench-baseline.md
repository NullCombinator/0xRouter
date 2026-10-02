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
