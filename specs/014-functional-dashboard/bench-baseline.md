# Bench baseline: router overhead under dashboard changes (SC-006, T046)

SC-006: while the browser makes changes as fast as it can, client requests' router overhead stays
within the 013 baseline's noise (p95 no more than 10% above it).

Bench: `crates/nullrouter-dashboard/benches/overhead.rs` (`harness = false`). It lives in the
dashboard crate because the signed-in site, the form token (`/debug/form-token`, `testkit`) and the
post wiring are reachable there, and the crate already depends on the server and engine; no new
dependency. No stored 013 p95 exists, so A and B are measured in the same run on the same server.

Setup: client listener + dashboard + instant mock upstream (`mockco/m1`), one key. The client and
the posters run on their own one-thread runtimes, apart from the server's. Per condition, REQUESTS
whole and REQUESTS streamed chat requests, sequential, one client (REQUESTS was 1500 before the fix,
2500 after, so each condition lasts several seconds). Conditions: A idle; C ONE poster posting
sequentially as fast as it can (the spec's "browser as fast as it can"; gate); B 4 posters (stress,
not a gate); A2 idle again (drift). Posters loop `routing/window` (6h / default) and
`behaviour/set-break`, each a form-token GET plus a signed-in POST, each rewriting `config.toml`
and reloading the engine. Every post was a 303.

Machine: 4 cores (`nproc` 4), Linux 7.0.0-38-generic, release profile.

Command:
`export CARGO_HOME=$PWD/.cargo-home CARGO_TARGET_DIR=/mnt/storage/0router/target/main; nice cargo bench -j 2 -p nullrouter-dashboard --bench overhead`

## Cause (found with evidence)

The engine's HTTP client cache (`connection::clients::Clients`) lived in the engine snapshot, and
every reload built a fresh snapshot with an empty cache. So the first request after each dashboard
change built a new `reqwest::Client` (rustls roots, resolver, connector) synchronously on an async
worker and opened a new upstream connection. With a reload every ~40 ms, almost every request paid
that: median 1 ms became about 88 ms.

Evidence:
- One poster (C) gave the same latency as four (B) at the same post rate (about 19/s, serialised
  on the reload mutex), so it was not CPU saturation from the posters; the bench's own runtimes
  were also separated with no change (88 ms median in C, before the fix).
- Temporary timing in `reload_blocking` (removed): a reload takes 15 to 50 ms, all on the blocking
  pool (registry rebuild 6 to 35 ms, assemble about 10 ms, swap and the rest under 0.1 ms). Nothing
  the request path waits on is held: snapshot reads are ArcSwap loads.
- Calling `engine.reload()` in a loop (no dashboard at all, 35/s) reproduced the full slowdown
  (C 88 ms median, p95 127 ms), so the dashboard's pages and ops are not the cause.
- After the fix, the same bench shows no slowdown (below).

Fix (`nullrouter-engine`): `Clients::adopt_from` carries the old snapshot's built clients (they share
their pools) into the new one when the private-endpoint setting and every proxy definition are
unchanged (`Proxies::same_definitions`); `reload_blocking` calls it. A changed proxy or setting still
gets fresh clients.

## Results (ms)

Before the fix (REQUESTS 1500, one runtime):

| Run | Condition | whole p95 | stream_ttfb p95 | router_overhead p95 | posts |
|---|---|---|---|---|---|
| 1 | A idle | 3.168 | 3.053 | 0.975 | 0 |
| 1 | B 4 posters | 134.169 | 133.504 | 8.297 | 19.2/s |
| 1 | A2 idle | 2.280 | 2.137 | 0.665 | 0 |
| 2 | A idle | 3.361 | 3.291 | 1.119 | 0 |
| 2 | B 4 posters | 133.896 | 135.438 | 7.923 | 19.2/s |

Before the fix, C added (REQUESTS 500, separate runtimes): A 3.373 / 3.206 / 1.094; C 125.177 / 126.835 / 7.105 (87.9 ms median, 19.7 posts/s); B 140.206 / 124.798 / 8.071; A2 3.282 / 2.973 / 1.025.

After the fix (REQUESTS 2500, separate runtimes), whole p95 / stream_ttfb p95 / router_overhead p95:

| Run | A | C (1 poster) | B (4 posters) | A2 |
|---|---|---|---|---|
| 1 | 3.029 / 2.891 / 0.944 | 3.030 / 2.811 / 1.044 | 5.270 / 4.844 / 2.708 | 2.701 / 2.354 / 0.746 |
| 2 | 3.082 / 3.047 / 0.954 | 3.056 / 2.675 / 1.164 | 5.031 / 4.334 / 2.528 | 2.805 / 2.749 / 0.850 |

C/A p95: whole 1.00 / 0.99, stream_ttfb 0.97 / 0.88, router_overhead 1.11 / 1.22. Medians C vs A:
whole 1.12 vs 1.42 and 1.12 vs 1.32 (C is not slower); router_overhead 0.32 vs 0.37 and 0.32 vs 0.35.
Posts: 24/s in C, 19 to 20/s in B. Idle drift A2/A is 0.75 to 0.89 on router_overhead p95.

## Verdict

C on e2e (whole, stream_ttfb): PASS, p95 equal to A. C on router_overhead p95: 1.11 and 1.22 times A,
at or just over the 1.10 line, though the median is lower than A's. The remaining cost is the
reload's own CPU on a 4-core box shared with the bench's threads (15 to 50 ms of registry rebuild and
assemble, 24 times a second, about half a core), which shows in the tail of a sub-millisecond number
(an absolute p95 gap of 0.1 to 0.2 ms). B (stress) is 2.9 times A on router_overhead p95 and 1.7
times on whole; reported, not a gate. Before the fix C and B were 37 to 40 times A on e2e.
Open: cutting the reload cost (skip the registry rebuild when no plugin or config input changed)
would close the last tail gap; it is a larger change and was not made.

## Orchestrator rerun (2026-10-23, after the client-cache fix)

Same command and machine. The engine suite ran just before: 62 `test result: ok`, 0 failed.

| Condition | whole p95 | stream_ttfb p95 | router_overhead p95 | posts/s |
|---|---|---|---|---|
| A idle | 3.221 ms | 3.041 ms | 0.992 ms | 0 |
| C 1 poster | 2.877 ms | 2.633 ms | 1.056 ms | 24.6 |
| B 4 posters (stress) | 5.377 ms | 5.034 ms | 2.845 ms | 18.7 |
| A2 idle | 2.559 ms | 2.402 ms | 0.736 ms | 0 |

C/A p95 is 0.893 (whole), 0.866 (stream_ttfb) and 1.064 (router_overhead). **SC-006 passes in this
run.** The earlier runs had router_overhead at 1.11 and 1.22 of A, an absolute gap of 0.1–0.2 ms.
Idle-to-idle drift (A2/A, 0.74–0.89) is larger than the 10% bar, so router_overhead sits at the
edge of the noise. B, four browsers posting flat out, stays 1.7× (whole) and 2.9× (router_overhead)
of idle. It is reported as stress, not a gate. A cheaper reload that skips the registry rebuild
when no input changed is the next lever if it ever matters.
