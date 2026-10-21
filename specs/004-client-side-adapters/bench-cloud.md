# Cloud A/B bench: does slice 004 slow the no-adapter path? (FR-030)

- **Date**: 2026-10-21, 08:18–09:17 UTC (A/B rounds), then the adapters run
- **Machine**: cloud session container, 4 vCPU (`nproc` = 4), `Intel(R) Xeon(R) Processor @ 2.80GHz`,
  15 GB RAM, Linux 6.18.44, rustc 1.97.0. Nothing else ran during the benches.
- **Load**: 1-minute load average 0.48 before the first round (the script waited for it to drop below 0.5
  after the build) and 1.02 after the last. During the rounds it stayed at 1.0–1.2, which is the
  single bench process itself.
- **A** = `108bb26` (main: 012 merged, 004 not). **B** = `ed6274b` (`origin/004-client-side-adapters`,
  main merged in), so A and B differ only by slice 004.

## Method

1. Each side was checked out in its own worktree and built with its own `CARGO_TARGET_DIR`:
   `cargo bench --no-run -p nullrouter-engine -p nullrouter-server -p nullrouter-wire`.
2. The three request-path bench binaries (`engine`, `server`, `wire`) ran directly as
   `<binary> --bench --noplot`, in the order A1, B1, A2, B2, A3, B3. Each round had a fresh
   `CRITERION_HOME` (`crit/A1` … `crit/B3`), so Criterion had no saved baseline to compare against.
   One round took about 10 minutes. The engine crate's four read-model benches
   (`records_page`, `keys_last_used_100k`, `usage_totals_100k`, `latency_24h_100k`) are not on the
   request path and were not run. The bench sources for engine, server and wire are byte-identical
   in A and B.
3. For each bench: the round mean is Criterion's `mean.point_estimate`. The table shows the median of
   the three round means per side, B/A as the ratio of those medians, and each side's min–max.
   The noise floor is per bench: the larger of the two sides' round-to-round spreads
   ((max − min) / median), with a 5% minimum. Across all benches and sides the median spread was
   14.2%. "B>A rounds" pairs round *i* of A with round *i* of B.
   A bench counts as **regressed** only if B was slower than A in all three rounds and B/A > 1 + floor.
   It counts as **faster** only under the mirror-image condition: faster in all three rounds and
   B/A < 1 − floor.

**Gap: `placement/meters_in_effect` did not run.** It is the engine binary's last bench, and its
setup panics on both A and B (exit 101 in every round). The fixture declares a routing window with
unit `weighted_tokens`, and the registry rejects that unit (`allowed: percent, credits, requests,
tokens`). This predates 004, because the bench file is identical on both sides. Every other engine
bench had already finished when the panic hit.

## Results (52 benches)

| Bench | A median | B median | B/A | A min–max | B min–max | B>A rounds | Floor | Verdict |
|---|---|---|---|---|---|---|---|---|
| `auth/unknown` | 1.26 µs | 1.1 µs | 0.872 | 1.16 µs–1.31 µs | 1.08 µs–1.2 µs | 0/3 | 12% | faster |
| `auth/valid` | 1.3 µs | 1.15 µs | 0.884 | 1.19 µs–1.32 µs | 1.12 µs–1.59 µs | 1/3 | 41% | no change |
| `e2e/stream_ttfb` | 1.24 ms | 1.21 ms | 0.976 | 1.18 ms–1.43 ms | 1.16 ms–1.26 ms | 1/3 | 21% | no change |
| `e2e/whole` | 1.19 ms | 1.14 ms | 0.956 | 1.16 ms–1.3 ms | 1.11 ms–1.16 ms | 0/3 | 11% | no change |
| `plan/combo_3_levels` | 7.12 µs | 7.63 µs | 1.072 | 6.28 µs–8.53 µs | 7.11 µs–8 µs | 2/3 | 32% | no change |
| `plan/verdicts_0` | 7.29 µs | 7.62 µs | 1.046 | 7.1 µs–10.1 µs | 7.23 µs–7.86 µs | 2/3 | 42% | no change |
| `plan/verdicts_1000` | 11.9 µs | 11.2 µs | 0.945 | 11.7 µs–14.8 µs | 11.2 µs–13.2 µs | 1/3 | 25% | no change |
| `request/anthropic-messages->anthropic-messages` | 168 µs | 175 µs | 1.042 | 167 µs–210 µs | 170 µs–199 µs | 2/3 | 25% | no change |
| `request/anthropic-messages->gemini` | 182 µs | 195 µs | 1.067 | 182 µs–263 µs | 185 µs–246 µs | 2/3 | 44% | no change |
| `request/anthropic-messages->openai-chat` | 202 µs | 179 µs | 0.886 | 177 µs–204 µs | 174 µs–193 µs | 1/3 | 14% | no change |
| `request/anthropic-messages->openai-responses` | 214 µs | 214 µs | 0.998 | 193 µs–227 µs | 190 µs–233 µs | 1/3 | 21% | no change |
| `request/gemini->anthropic-messages` | 238 µs | 190 µs | 0.796 | 182 µs–243 µs | 189 µs–241 µs | 1/3 | 27% | no change |
| `request/gemini->gemini` | 204 µs | 197 µs | 0.966 | 204 µs–207 µs | 187 µs–271 µs | 1/3 | 43% | no change |
| `request/gemini->openai-chat` | 174 µs | 208 µs | 1.192 | 174 µs–233 µs | 205 µs–217 µs | 2/3 | 34% | no change |
| `request/gemini->openai-responses` | 195 µs | 199 µs | 1.025 | 184 µs–228 µs | 185 µs–228 µs | 2/3 | 23% | no change |
| `request/openai-chat->anthropic-messages` | 169 µs | 164 µs | 0.972 | 163 µs–176 µs | 161 µs–171 µs | 1/3 | 7% | no change |
| `request/openai-chat->gemini` | 170 µs | 172 µs | 1.010 | 150 µs–218 µs | 155 µs–177 µs | 1/3 | 40% | no change |
| `request/openai-chat->openai-chat` | 191 µs | 161 µs | 0.840 | 136 µs–223 µs | 148 µs–189 µs | 1/3 | 45% | no change |
| `request/openai-chat->openai-responses` | 187 µs | 160 µs | 0.858 | 151 µs–211 µs | 156 µs–165 µs | 1/3 | 32% | no change |
| `request/openai-responses->anthropic-messages` | 211 µs | 186 µs | 0.884 | 193 µs–247 µs | 183 µs–210 µs | 0/3 | 26% | no change |
| `request/openai-responses->gemini` | 228 µs | 225 µs | 0.984 | 196 µs–272 µs | 192 µs–240 µs | 1/3 | 33% | no change |
| `request/openai-responses->openai-chat` | 234 µs | 194 µs | 0.826 | 183 µs–236 µs | 173 µs–228 µs | 1/3 | 29% | no change |
| `request/openai-responses->openai-responses` | 230 µs | 222 µs | 0.966 | 187 µs–235 µs | 188 µs–229 µs | 1/3 | 21% | no change |
| `route/chat` | 1.66 µs | 1.52 µs | 0.915 | 1.53 µs–1.77 µs | 1.52 µs–1.62 µs | 0/3 | 15% | no change |
| `route/gemini` | 3.66 µs | 3.72 µs | 1.017 | 3.59 µs–3.89 µs | 3.66 µs–3.96 µs | 3/3 | 8% | no change |
| `route/messages` | 1.4 µs | 1.46 µs | 1.042 | 1.36 µs–1.43 µs | 1.38 µs–1.55 µs | 2/3 | 12% | no change |
| `route/none` | 1.57 µs | 1.33 µs | 0.848 | 1.42 µs–1.58 µs | 1.3 µs–1.46 µs | 0/3 | 12% | faster |
| `stream/anthropic-messages->anthropic-messages` | 1.59 ms | 1.36 ms | 0.857 | 1.51 ms–1.64 ms | 1.34 ms–1.63 ms | 0/3 | 21% | no change |
| `stream/anthropic-messages->gemini` | 1.6 ms | 1.49 ms | 0.925 | 1.5 ms–1.85 ms | 1.48 ms–1.71 ms | 0/3 | 22% | no change |
| `stream/anthropic-messages->openai-chat` | 1.69 ms | 1.71 ms | 1.009 | 1.55 ms–1.92 ms | 1.59 ms–1.77 ms | 1/3 | 21% | no change |
| `stream/anthropic-messages->openai-responses` | 1.8 ms | 1.66 ms | 0.922 | 1.61 ms–2.01 ms | 1.64 ms–1.89 ms | 1/3 | 22% | no change |
| `stream/gemini->anthropic-messages` | 1.55 ms | 1.74 ms | 1.124 | 1.55 ms–1.71 ms | 1.55 ms–1.75 ms | 2/3 | 11% | no change |
| `stream/gemini->gemini` | 1.65 ms | 1.7 ms | 1.034 | 1.64 ms–1.86 ms | 1.65 ms–1.75 ms | 2/3 | 13% | no change |
| `stream/gemini->openai-chat` | 1.83 ms | 1.78 ms | 0.974 | 1.73 ms–1.92 ms | 1.72 ms–1.89 ms | 2/3 | 10% | no change |
| `stream/gemini->openai-responses` | 1.88 ms | 1.92 ms | 1.018 | 1.79 ms–1.93 ms | 1.9 ms–2 ms | 3/3 | 7% | no change |
| `stream/openai-chat->anthropic-messages` | 1.67 ms | 1.8 ms | 1.081 | 1.47 ms–2 ms | 1.5 ms–1.89 ms | 1/3 | 31% | no change |
| `stream/openai-chat->gemini` | 1.77 ms | 1.85 ms | 1.044 | 1.58 ms–2.03 ms | 1.6 ms–1.86 ms | 1/3 | 25% | no change |
| `stream/openai-chat->openai-chat` | 1.81 ms | 1.75 ms | 0.969 | 1.72 ms–1.87 ms | 1.75 ms–2.15 ms | 2/3 | 22% | no change |
| `stream/openai-chat->openai-responses` | 1.98 ms | 1.9 ms | 0.958 | 1.67 ms–2.17 ms | 1.69 ms–1.99 ms | 1/3 | 26% | no change |
| `stream/openai-responses->anthropic-messages` | 1.19 ms | 1.22 ms | 1.024 | 1.15 ms–1.37 ms | 1.19 ms–1.23 ms | 2/3 | 19% | no change |
| `stream/openai-responses->gemini` | 1.38 ms | 1.46 ms | 1.058 | 1.37 ms–1.54 ms | 1.45 ms–1.51 ms | 2/3 | 12% | no change |
| `stream/openai-responses->openai-chat` | 1.4 ms | 1.4 ms | 1.005 | 1.38 ms–1.78 ms | 1.37 ms–1.47 ms | 1/3 | 28% | no change |
| `stream/openai-responses->openai-responses` | 1.42 ms | 1.57 ms | 1.105 | 1.36 ms–1.59 ms | 1.55 ms–1.78 ms | 3/3 | 16% | no change |
| `tally/attempt` | 255 ns | 245 ns | 0.959 | 246 ns–290 ns | 234 ns–267 ns | 0/3 | 17% | no change |
| `ttfb/cross_style` | 751 µs | 782 µs | 1.040 | 748 µs–795 µs | 758 µs–783 µs | 2/3 | 6% | no change |
| `ttfb/direct` | 266 µs | 254 µs | 0.957 | 263 µs–269 µs | 244 µs–257 µs | 0/3 | 5% | no change |
| `ttfb/same_style` | 752 µs | 736 µs | 0.979 | 719 µs–762 µs | 704 µs–807 µs | 2/3 | 14% | no change |
| `ttfb/signin` | 810 µs | 751 µs | 0.927 | 716 µs–811 µs | 734 µs–794 µs | 1/3 | 12% | no change |
| `usage/anthropic-messages` | 753 ns | 761 ns | 1.012 | 714 ns–765 ns | 665 ns–823 ns | 1/3 | 21% | no change |
| `usage/gemini` | 730 ns | 719 ns | 0.984 | 703 ns–802 ns | 715 ns–822 ns | 2/3 | 15% | no change |
| `usage/openai-chat` | 993 ns | 1.1 µs | 1.104 | 990 ns–1.02 µs | 1.01 µs–1.13 µs | 2/3 | 11% | no change |
| `usage/openai-responses` | 800 ns | 865 ns | 1.082 | 799 ns–929 ns | 805 ns–1.02 µs | 3/3 | 24% | no change |

No bench was slower in all three rounds by more than its floor. Four benches were slower in all
three rounds, but each by less than its floor:

- `route/gemini`: +1.7%, floor 8%
- `stream/gemini->openai-responses`: +1.8%, floor 7%
- `stream/openai-responses->openai-responses`: +10.5%, floor 16%
- `usage/openai-responses`: +8.2%, floor 24%

`auth/unknown` and `route/none` were faster in all three rounds, by more than their floors. Across
all benches the B/A ratios are centred on 1.0 (median 0.98).

## SC-010 on B (`cargo bench -p nullrouter-adapters --bench adapters -- claude_code_request`, one run)

The built claude-code WASM adapter plus the guardrail, on an anthropic-messages conversation.
Per-iteration time is `times[i] / iters[i]` from `sample.json`, with 100 samples per bench. The
p95 is over these per-sample averages (nearest-rank), not over single requests. Load average was
0.48 before the run and 0.75 after.

| Bench | Mean | p95 | SC-010 (p95 ≤ 5 ms at ≤ 100 KB) |
|---|---|---|---|
| `adapters/claude_code_request/100kb_clean` | 0.941 ms | 1.341 ms | pass |
| `adapters/claude_code_request/100kb_edit` | 1.677 ms | 2.559 ms | pass |
| `adapters/claude_code_request/1mb_clean` | 9.592 ms | 13.751 ms | n/a (above 100 KB) |
| `adapters/claude_code_request/1mb_edit` | 19.167 ms | 27.406 ms | n/a (above 100 KB) |

From 100 KB to 1 MB the mean grows 10.2× (clean) and 11.4× (edit), roughly linear in body size, as
SC-010 requires above 100 KB. Criterion warned that `1mb_edit` could not fit 100 samples in 5 s
(it needed 8.1 s); the run still completed with 100 samples.

**FR-030 verdict: no regression. Across 52 request-path benches, slice 004 slows none of them beyond
its noise floor, and the claude-code adapter meets SC-010 at 100 KB (p95 2.56 ms vs 5 ms).**

## For memory

- A/B on a quiet 4-vCPU cloud container (A, B, A, B, A, B; fresh `CRITERION_HOME` per round) still
  shows a median 14% round-to-round spread on these benches. A single A-vs-B Criterion comparison
  can't detect changes under about 10%, so FR-030 regression checks need interleaved rounds plus a
  per-bench noise floor.
- FR-030 for slice 004: no regression on the no-adapter path (108bb26 vs ed6274b).
- `engine` bench: `placement/meters_in_effect` panics in setup on main and on 004. The fixture
  declares a routing window with unit `weighted_tokens`, which the registry no longer accepts.
  Needs a fixture fix: probably `tokens`, or whichever unit replaced it.
- SC-010 cloud numbers (B): claude-code adapter p95 is 1.34 ms clean and 2.56 ms edit at 100 KB,
  and 13.8/27.4 ms at 1 MB, roughly linear.
- Bench run script and analysis: alternate the prebuilt bench binaries directly with
  `--bench --noplot`, then read `*/new/estimates.json` (`mean.point_estimate`) and `benchmark.json`
  (`full_id`).
