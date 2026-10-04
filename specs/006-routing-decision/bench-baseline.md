# Bench baseline: slice 006

Recorded by T001 on 2026-10-03, before any slice 006 engine change in the request path, with
`--save-baseline pre-006` on the `engine` bench binary built from the engine at `7cd82d9` plus
empty module skeletons (T002). The Criterion data lives in `target/criterion/ttfb/direct/pre-006/`
(local only, not committed); the summary below is the record.

| Bench | Low | Estimate | High |
|---|---|---|---|
| `ttfb/direct` | 298.29 µs | 312.83 µs | 329.30 µs |

The bench's own per-request report for the same run: median 172.5 µs, **p95 1.105 ms** over
28,391 requests. SC-013 compares `ttfb/routed` p95 with `ttfb/direct` p95 on the same machine
within 5 ms, so the reference is the `ttfb/direct` p95 above.

The machine was not idle (other work was running; see the slice 005 baseline for the same
caveat), so compare runs taken back to back under the same load rather than against these
numbers alone.

T091 appends the final comparison here.
