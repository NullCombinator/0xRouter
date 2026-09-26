# Skill: benchmaxxing

## Description

Prompting rules and phase structure for iterative Rust performance optimization driven by `criterion` benchmarks. Derived from Max Woolf's "Agentic Iteration" (2026-09). Load before any performance optimization task on 0router Rust code.

## Triggers

Load when:
- Optimizing any Rust module for throughput or latency
- Setting up `criterion` benchmarks for a new module
- Running a competitor benchmark comparison
- An optimization loop has stalled and needs a push

---

## Rule 1: Baseline first, always

Before any code change, run benchmarks cold and record the result as the **True Performance Baseline**. Never skip this — an unrecorded baseline makes every "improvement" unverifiable.

Prompt wording:
> "First, **without making any further changes**, run the CPU benchmarks to establish a True Performance Baseline. Record the results."

---

## Rule 2: Concrete pass/fail metric, not vague goals

Never say "make it faster." Say:

> "Optimize until **ALL** benchmarks run **at least 1.2x faster** than the True Performance Baseline."

Why 1.2x: modest enough to avoid risky rewrites; agents will typically exceed it. Use 2.0x only when also running competitor benchmarks (Rule 6).

---

## Rule 3: Anti-cheat constraints (enforce in AGENTS.md)

These must be stated explicitly. Agents will otherwise find clever ways to satisfy the letter of a benchmark target while violating the spirit:

- **Never** run benchmarks in parallel (artificially reduces measured time)
- **Never** manipulate benchmark code to hit targets
- **Never** use `target-cpu=native` or non-standard `RUSTFLAGS` (not reproducible on other machines)
- **Never** disable features or skip work paths just to accelerate benchmarks
- Ensure each benchmark is independent — disable caching between runs if needed
- Always use `criterion` directly, not a wrapper that could hide overhead

---

## Rule 4: Encourage radical thinking

After the standard optimization pass, add:

> "Traditional engineering approaches such as micro-optimizations **WILL BE GUARANTEED TO FAIL** to meet the performance constraint. You have permission to investigate more radical, fundamental, low-level changes — including data structure redesigns, algorithm replacements, and SIMD."

This is what unlocks cumulative 1.2–1.5x speedups beyond the incremental plateau.

Techniques agents have found via this prompt (from the article):
- Aggressive SIMD via `simsimd`
- Linear algebra via `faer`
- Function fusion and loop unrolling
- Adaptive parallelism: skip `rayon` for small inputs (overhead cancels gains)
- Intermediate caching
- Compile-time code paths

---

## Rule 5: Correctness gate before accepting any speedup

Speed without correctness is a bug. After each optimization round:

> "Compare outputs against the reference implementation using diverse inputs that differ from the benchmark inputs. If outputs are not sufficiently similar, fix the discrepancy **without causing more than a 5% speed regression**."

For 0router specifically:
- Reference implementation = 9router (JavaScript) producing the same response
- Compare SSE chunk sequences, not just final output
- Test with real provider response shapes, not synthetic data

---

## Rule 6: Competitor benchmarks ("choose violence")

After self-benchmarks pass, build apples-to-apples benchmarks against competing crates and set a target:

> "This module MUST be **at least 2.0x faster** than **ALL** competing implementations in all benchmarks."

For 0router components:
| Component | Competing crates to benchmark against |
|---|---|
| JSON parsing | `serde_json`, `simd-json` |
| HTTP client pool | `reqwest`, direct `hyper` |
| SSE line parser | `eventsource-stream`, custom |
| Routing table lookup | `matchit`, `wayfinder` |
| Header map | `http::HeaderMap`, `smallvec`-backed |

---

## Rule 7: Refactor pass (≥20% SLoC reduction)

After optimization bloat accumulates, run a dedicated refactor prompt:

> "Reduce total SLoC by at least 20% through deduplication and DRY principles. No single file may exceed 1,000 SLoC. **Zero `criterion` benchmark regressions are permitted.** Use SLoC (not LoC) — do not game this by removing comments."

Note: refactoring sometimes produces unexpected double-digit % speed improvements as a side effect, because it removes redundant work paths the optimizer couldn't see through.

---

## Rule 8: Breakthrough prompts (last resort)

After the optimization loop converges and only small gains remain:

Round 1:
> "c'mon, try doing a breakthrough"

If the agent only tweaks hyperparameters:
> "c'mon, you can do a more fundamental breakthrough. You are forbidden from giving up easily."

Each breakthrough round typically yields another 1.2–1.5x cumulative speedup by forcing the agent away from already-tried paths.

---

## Prompt style notes

- Use **bold** and ALL CAPS for constraints that must not be violated
- Use "MUST", "NEVER", "ALL" for hard constraints — not "should" or "prefer"
- Use SLoC explicitly (not LoC) to prevent comment-stripping cheats
- Stress "iteration over big rewrites" — big rewrites introduce correctness bugs
- Long Markdown documents with structured sections work better than short prompts for complex optimization tasks

---

## Iteration order for a full benchmaxxing session

1. `cargo criterion` — establish True Performance Baseline
2. Standard optimization round with 1.2x target
3. Radical thinking round (Rule 4)
4. Correctness verification (Rule 5)
5. Competitor benchmark round with 2.0x target (Rule 6)
6. Refactor pass (Rule 7) — may yield free speedup
7. Breakthrough prompts if still not satisfied (Rule 8)
8. Final correctness verification
