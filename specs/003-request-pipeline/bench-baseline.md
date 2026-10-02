# Bench baseline: slice 003 (T138, SC-013)

- **Date**: 2026-10-02
- **Machine**: Intel Core i7-7500U @ 2.70 GHz, 4 threads, Linux 7.0; desktop session running
  (load average 4–7 during the runs)
- **Toolchain**: rustc 1.93.1, `cargo bench` (release profile)
- **Baseline name**: `slice-003`, saved in `target/criterion` (local only)

```bash
cargo bench -p nullrouter-wire   --bench wire   -- --save-baseline slice-003
cargo bench -p nullrouter-engine --bench engine -- --save-baseline slice-003
cargo bench -p nullrouter-server --bench server -- --save-baseline slice-003
# compare later with --baseline slice-003
```

Criterion's figure is the median of its sample means. The latency benches also time every
request on its own and print that distribution's median and p95, given in the last
columns.

## SC-013: time 0router adds before the first byte

Target: p95 ≤ 10 ms. Every request goes over loopback to an instant mock.

| Bench | Criterion median | Per-request median | Per-request p95 | Requests |
|---|---|---|---|---|
| `engine` `ttfb/direct` (reqwest straight to the mock, the floor) | 303.7 µs | 175.8 µs | 1.05 ms | 33 441 |
| `engine` `ttfb/same_style` (attempt loop, openai-chat → openai-chat) | 566.7 µs | 301.6 µs | 1.89 ms | 19 245 |
| `engine` `ttfb/cross_style` (anthropic-messages → openai-chat) | 592.5 µs | 312.8 µs | 1.91 ms | 23 341 |
| `server` `e2e/stream_ttfb` (client → 0router → mock, first chunk) | 1.083 ms | 642.9 µs | 3.12 ms | 14 195 |
| `server` `e2e/whole` (client → 0router → mock, full body) | 854.5 µs | 488.0 µs | 2.63 ms | 19 245 |

The engine adds about 0.13 ms at the median and 0.85 ms at p95 over a direct request. The
full server path stays well inside the target.

The first run measured `e2e/stream_ttfb` at **40.9 ms** (p95 45.7 ms). The server's
accepted sockets had no `TCP_NODELAY`, so Nagle held each response's first frame until
the client's delayed ACK. `serve::run` now sets it, and
`tests/timing.rs::first_stream_frames_are_sent_without_delay` guards against a return.
The row above is the run after the fix.

## Wire (`nullrouter-wire`, no I/O)

Request: decode plus encode of an agent turn (system prompt, 10 exchanges each with a tool
call and its result, 2 tools). Rows are the client style; columns are the wire.

| from ↓ / to → | anthropic-messages | openai-chat | openai-responses | gemini |
|---|---|---|---|---|
| anthropic-messages | 461.6 µs | 502.5 µs | 580.9 µs | 560.3 µs |
| openai-chat | 417.4 µs | 398.3 µs | 457.4 µs | 427.2 µs |
| openai-responses | 601.5 µs | 474.1 µs | 493.0 µs | 400.2 µs |
| gemini | 382.4 µs | 344.7 µs | 419.5 µs | 397.3 µs |

Stream: framing, reading and rewriting one whole answer (thinking block, 200 text deltas,
one tool call, usage).

| from ↓ / to → | anthropic-messages | openai-chat | openai-responses | gemini |
|---|---|---|---|---|
| anthropic-messages | 2.411 ms | 2.717 ms | 2.903 ms | 2.813 ms |
| openai-chat | 2.714 ms | 2.998 ms | 3.155 ms | 3.085 ms |
| openai-responses | 2.221 ms | 2.570 ms | 2.644 ms | 2.434 ms |
| gemini | 2.862 ms | 3.488 ms | 3.498 ms | 3.042 ms |

That is about 11–17 µs per event. A same-style stream is relayed as frames and
isn't rewritten this way on the request path.

Usage extraction from a whole response:

| anthropic-messages | openai-chat | openai-responses | gemini |
|---|---|---|---|
| 1.157 µs | 1.644 µs | 1.443 µs | 1.191 µs |

## Server (`nullrouter-server`)

| Bench | Median |
|---|---|
| `auth/valid` (digest lookup of a presented key) | 1.882 µs |
| `auth/unknown` | 1.794 µs |
| `route/chat` (`POST /v1/chat/completions`) | 943.3 ns |
| `route/messages` | 846.5 ns |
| `route/gemini` (`:streamGenerateContent` with a `{model*}` capture) | 1.414 µs |
| `route/none` (no match) | 767.4 ns |
