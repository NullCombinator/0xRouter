# Bench baseline: slice 004 (T092, SC-010, FR-030)

- **Date**: 2026-10-20 (the second run; the first ran under `.cargo/capped`'s `nice 19` on a
  busy machine and is discarded)
- **Machine**: Intel Core i7-7500U @ 2.70 GHz, 4 threads, Linux 7.0; desktop session running.
  Load average about 6 during engine, server and wire (15:18 to 15:44), and 3.1 to 3.4 during
  adapters (16:22 to 16:23)
- **Toolchain**: rustc 1.93.1, `cargo bench` (release profile)
- **Baselines**: `pre-004` (built from `f7518cd`, main without 004) and `slice-004`, in
  `CRITERION_HOME=/mnt/storage/0router/criterion-004` (local only). Raw output is in
  `/mnt/storage/0router/bench-004/`.

The commands are in [operator-runs.md](operator-runs.md), T092.

## SC-010: adapter plus guardrail

Target: at most 5 ms p95 per request, and at most 1 ms per stream event. The p95 is taken over
Criterion's per-iteration sample times (`p95.txt`).

| Bench | What it times | Mean | p95 |
|---|---|---|---|
| `adapters/guard_request/10kb` | request guardrail, 10 KB anthropic-messages conversation, one removal | 25.3 µs | 0.036 ms |
| `adapters/guard_request/100kb` | the same, 100 KB | 278.2 µs | 0.408 ms |
| `adapters/guard_request/1mb` | the same, 1 MB | 3.757 ms | 4.405 ms |
| `adapters/guard_event` | event guardrail, one content-block delta | 11.1 µs | 0.016 ms |
| `adapters/selector_extract` | `messages[*].content[*].type` over 1 MB | 1.296 ms | 2.449 ms |
| `adapters/hermes_request` | hermes selectors and edits, 40 turns with images | 255.9 µs | 0.394 ms |
| `sandbox/call_noop` | fresh instance and call, no parts (the floor) | 12.5 µs | 0.018 ms |
| `sandbox/call_claude_code` | the built claude-code module, one 100 KB message | 230.9 µs | 0.329 ms |

**Request side: met as the suite measures it, but not shown for a 1 MB request.**
`guard_request/1mb` plus `call_claude_code` gives 4.405 + 0.329 = 4.73 ms p95, 0.27 ms under
the target. Adding p95s overstates the p95 of the sum, so the suite's own figure passes. It
doesn't measure one 1 MB request through the adapter, for three reasons:

1. `call_claude_code` sends a 100 KB body as one string. A 1 MB conversation of many messages
   is ten times the bytes, and the module parses and re-serialises structure, not one string.
2. Before the call, the runner extracts claude-code's selectors (`messages[*]` among them, a
   copy of every message) and serialises the call input. Neither is benched at 1 MB.
   `selector_extract` walks 1 MB without copying the messages, and alone takes 2.45 ms p95.
3. In production, the guardrail's verdict decodes the original body again
   (`runner.rs`, `Side::verdict`). The bench decodes it once, outside the timing.

At 1 MB, each of those is a full pass over the body, and the parts the suite doesn't time would
take up most of the 0.27 ms margin, or more. So SC-010 at 1 MB is not established: neither
met nor missed.

### End to end: `adapters/claude_code_request`

Added after the run above to settle that question. It runs the Claude Code adapter's whole
request side as `serve` does: extraction, call input, sandbox call, edit check and copy, and
the verdict. `edit` has one mid-conversation system turn the adapter converts; `clean` has none,
so the module answers no edits and the verdict is skipped. Same machine, busy (load 3 to 5).

| Bench | Mean | p95 |
|---|---|---|
| `claude_code_request/100kb_edit` | 2.58 ms | 3.72 ms |
| `claude_code_request/100kb_clean` | 1.92 ms | 2.65 ms |
| `claude_code_request/1mb_edit` | about 23 ms | 26.9 ms |
| `claude_code_request/1mb_clean` | about 18 ms | 20.9 ms |

**Met up to 100 KB; missed at 1 MB by about five times.** The cost is linear, about 25 µs per
KB. At 1 MB, timing the parts apart (median): extraction 3.0 ms, call input 8.0 ms (overstated,
built with `json!`), sandbox call 11.6 ms, edit check and copy 2.8 ms, decoding the original
4.7 ms, and the guardrail 5.6 ms. Each is a full pass over the body. SC-010 is revised to 5 ms
up to 100 KB and linear above (spec, Clarifications; research R14).

The sandbox call's share is guest time and counts against the deadline. At about 12 ms per MiB,
the fixed 20 ms request deadline sent any session past about 2 MB on unedited
(`failed{deadline}`). The deadline now adds 40 ms per MiB of call input (`loader::sized_deadline`);
`a_four_megabyte_conversation_runs_within_the_production_deadline` fails without it.

**Event side: met.** `guard_event` p95 is 0.016 ms against 1 ms. Claude Code reads no
responses (`events = false`), so no sandbox call adds to it.

## FR-030: the no-adapter path against `pre-004`

**Inconclusive at this machine's noise level; not a pass.** At load about 6, the changes
reported against `pre-004` run from −38% to +74% in both directions, often between neighbouring
benches that share the code paths 004 touches:

| Bench | Regressed | Improved | No change | Within noise |
|---|---|---|---|---|
| engine | 2 | 3 | 2 | 1 |
| server | 2 | 3 | 2 | 1 |
| wire | 14 | 8 | 12 | 2 |

For example, `route/chat` improves by 36% and `route/none` regresses by 31% in the same run.
`ttfb/direct`, which goes straight to the mock and doesn't run any 0router code, moves by
−22%. No regression repeats in a way a code change would explain. A judgement needs a rerun on
an idle machine (load below about 1). A regression counts only if it holds across reruns.
