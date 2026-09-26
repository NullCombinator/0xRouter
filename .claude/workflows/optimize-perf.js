export const meta = {
  name: 'optimize-perf',
  description: 'Iterative Rust performance optimization loop (benchmaxxing). Phases: baseline → parallel hypothesis exploration → implement best → correctness gate → competitor benchmarks → refactor pass.',
  whenToUse: 'User wants to optimize a Rust module for throughput or latency. Requires criterion benchmarks to already exist (or creates them). Based on Max Woolf\'s agentic benchmaxxing technique.',
  phases: [
    { title: 'Baseline', detail: 'Run criterion benchmarks cold, record True Performance Baseline' },
    { title: 'Explore', detail: 'Spawn parallel hypothesis-explorer subagents, each investigating a distinct optimization direction' },
    { title: 'Implement', detail: 'Implement the most promising hypotheses, verify each clears the 1.2x target' },
    { title: 'Correctness', detail: 'Compare outputs against reference implementation; fix regressions with <5% speed cost' },
    { title: 'Compete', detail: 'Build competitor benchmarks; target 2x faster than all competing crates' },
    { title: 'Refactor', detail: '≥20% SLoC reduction with zero criterion regressions; harvest free speedup' },
  ],
}

// --- Input ---
// args: { target: string, benchmark_cmd?: string, reference_impl?: string, speedup_target?: number }
// target: Rust module path, e.g. "src/executor" or "src/translator"
// benchmark_cmd: how to run benchmarks, defaults to "cargo criterion"
// reference_impl: what to verify correctness against, e.g. "ref/9router/open-sse/executors/base.js"
// speedup_target: minimum speedup multiplier, default 1.2

function resolveArgs(a) {
  if (!a) return {}
  if (typeof a === 'string') return { target: a }
  return a
}
const { target, benchmark_cmd = 'cargo criterion', reference_impl, speedup_target = 1.2 } = resolveArgs(args)

if (!target) {
  log('No target provided. Call Workflow({ name: "optimize-perf", args: { target: "src/executor" } })')
  return { ok: false, reason: 'no-input' }
}

// --- Schemas ---

const BASELINE_SCHEMA = {
  type: 'object',
  required: ['benchmark_results', 'slowest_benchmarks', 'baseline_recorded'],
  properties: {
    benchmark_results: {
      type: 'array',
      items: {
        type: 'object',
        required: ['name', 'mean_ns', 'throughput'],
        properties: {
          name: { type: 'string' },
          mean_ns: { type: 'number' },
          throughput: { type: 'string', description: 'e.g. "1.2 GiB/s" or "450k ops/sec"' },
        },
      },
    },
    slowest_benchmarks: { type: 'array', items: { type: 'string' }, description: 'Top 3 bottlenecks by wall time.' },
    baseline_recorded: { type: 'boolean' },
    notes: { type: 'string' },
  },
}

const HYPOTHESIS_SCHEMA = {
  type: 'object',
  required: ['hypothesis', 'approach', 'estimated_speedup', 'risk', 'affected_benchmarks'],
  properties: {
    hypothesis: { type: 'string', description: 'One sentence: what change and why it should be faster.' },
    approach: { type: 'string', description: 'Concrete implementation plan (2-4 sentences).' },
    estimated_speedup: { type: 'string', description: 'e.g. "1.3-1.8x on routing lookup".' },
    risk: { type: 'string', enum: ['low', 'medium', 'high'], description: 'Correctness/complexity risk.' },
    affected_benchmarks: { type: 'array', items: { type: 'string' } },
    crates_needed: { type: 'array', items: { type: 'string' } },
  },
}

const IMPLEMENT_SCHEMA = {
  type: 'object',
  required: ['applied_hypotheses', 'new_benchmark_results', 'speedup_achieved', 'target_met'],
  properties: {
    applied_hypotheses: { type: 'array', items: { type: 'string' } },
    new_benchmark_results: {
      type: 'array',
      items: {
        type: 'object',
        required: ['name', 'mean_ns', 'speedup_vs_baseline'],
        properties: {
          name: { type: 'string' },
          mean_ns: { type: 'number' },
          speedup_vs_baseline: { type: 'number' },
        },
      },
    },
    speedup_achieved: { type: 'number', description: 'Minimum speedup across all benchmarks.' },
    target_met: { type: 'boolean' },
    notes: { type: 'string' },
  },
}

const CORRECTNESS_SCHEMA = {
  type: 'object',
  required: ['verdict', 'tests_run', 'regressions_found'],
  properties: {
    verdict: { type: 'string', enum: ['pass', 'fail', 'fixed'] },
    tests_run: { type: 'number' },
    regressions_found: { type: 'number' },
    speed_cost_of_fixes_pct: { type: 'number', description: 'Speed regression from correctness fixes (must be <5%).' },
    notes: { type: 'string' },
  },
}

const COMPETE_SCHEMA = {
  type: 'object',
  required: ['competitors', 'all_targets_met'],
  properties: {
    competitors: {
      type: 'array',
      items: {
        type: 'object',
        required: ['crate', 'benchmark', 'our_mean_ns', 'their_mean_ns', 'speedup', 'target_met'],
        properties: {
          crate: { type: 'string' },
          benchmark: { type: 'string' },
          our_mean_ns: { type: 'number' },
          their_mean_ns: { type: 'number' },
          speedup: { type: 'number' },
          target_met: { type: 'boolean' },
        },
      },
    },
    all_targets_met: { type: 'boolean' },
    notes: { type: 'string' },
  },
}

const REFACTOR_SCHEMA = {
  type: 'object',
  required: ['sloc_before', 'sloc_after', 'sloc_reduction_pct', 'regressions', 'speed_delta_pct'],
  properties: {
    sloc_before: { type: 'number' },
    sloc_after: { type: 'number' },
    sloc_reduction_pct: { type: 'number' },
    target_met: { type: 'boolean', description: 'sloc_reduction_pct >= 20' },
    regressions: { type: 'number', description: 'Must be 0.' },
    speed_delta_pct: { type: 'number', description: 'Positive = faster. Refactoring sometimes yields free speedup.' },
    notes: { type: 'string' },
  },
}

// ─── Phase 1: Baseline ────────────────────────────────────────────────────────

phase('Baseline')

const baseline = await agent(
  `You are establishing a True Performance Baseline for Rust criterion benchmarks.

Target module: ${target}
Benchmark command: ${benchmark_cmd}

**CRITICAL: Without making ANY code changes**, run the benchmarks now:
  ${benchmark_cmd} -- --noplot 2>&1

Record every benchmark result exactly as criterion reports it: name, mean time, throughput.
Identify the 3 slowest benchmarks — these are the primary optimization targets.

Rules:
- **NEVER** run benchmarks in parallel
- **NEVER** use target-cpu=native or non-standard RUSTFLAGS
- Run on a quiet machine with no competing load
- Run each benchmark at least twice; use the second run (warm disk cache, cold CPU cache)

Return the structured baseline results.`,
  {
    label: `Baseline: ${target}`,
    phase: 'Baseline',
    schema: BASELINE_SCHEMA,
    agentType: 'performance-engineer',
  }
)

if (!baseline?.baseline_recorded) {
  log('Baseline failed — cannot proceed without a True Performance Baseline')
  return { ok: false, reason: 'baseline-failed', target }
}

log(`Baseline recorded. Slowest: ${baseline.slowest_benchmarks.join(', ')}`)

// ─── Phase 2: Explore (parallel hypothesis subagents) ────────────────────────

phase('Explore')

// Launch 7 subagents, each investigating a distinct optimization direction
const hypothesisDirections = [
  'SIMD: identify hot loops that process bytes/floats and replace with explicit SIMD via simsimd or std::simd',
  'allocation reduction: profile heap allocations in the hot path; replace Vec/String with stack-allocated alternatives (SmallVec, ArrayVec, Cow)',
  'parallelism: identify embarrassingly parallel work; apply rayon where input size justifies the overhead (benchmark both paths)',
  'data structures: replace HashMap with a faster alternative (AHashMap, IndexMap, or a perfect hash for static key sets)',
  'algorithmic: identify O(n²) or repeated work; replace with a fundamentally better algorithm',
  'caching and memoization: identify repeated expensive computations; add intermediate caches with appropriate invalidation',
  'compile-time evaluation: identify values computed at runtime that could be const or computed at build time via build.rs or proc macros',
]

log(`Launching ${hypothesisDirections.length} hypothesis-explorer subagents in parallel...`)

const hypotheses = await pipeline(hypothesisDirections, (direction) =>
  agent(
    `You are a Rust performance hypothesis explorer. Your job is to investigate ONE specific optimization direction for a Rust module and return a concrete hypothesis — but you must NOT make any code changes.

Module: ${target}
Benchmark baseline (slowest paths): ${baseline.slowest_benchmarks.join(', ')}

Your direction: **${direction}**

Steps:
1. Read the relevant source files in ${target}
2. Identify the specific functions/loops that your direction applies to
3. Assess whether this direction is likely to yield a meaningful speedup for THIS code
4. If yes: write a concrete hypothesis with an estimated speedup range and implementation plan
5. If no: explain why this direction doesn't apply and return estimated_speedup "0x (not applicable)"

Constraint: Long-duration analysis is fine. Do NOT run benchmarks. Do NOT write code. Return a hypothesis only.`,
    {
      label: `Hypothesis: ${direction.split(':')[0]}`,
      phase: 'Explore',
      schema: HYPOTHESIS_SCHEMA,
      agentType: 'perf-hypothesis-explorer',
    }
  )
)

const viable = hypotheses.filter(h => h && !h.estimated_speedup?.startsWith('0x'))
log(`${viable.length}/${hypotheses.length} hypotheses viable:`)
for (const h of viable) {
  log(`  [${h.risk} risk] ${h.hypothesis} → ${h.estimated_speedup}`)
}

// ─── Phase 3: Implement ───────────────────────────────────────────────────────

phase('Implement')

const hypothesisSummary = viable.map((h, i) =>
  `${i+1}. ${h.hypothesis}\n   Approach: ${h.approach}\n   Estimated: ${h.estimated_speedup} (${h.risk} risk)\n   Crates: ${(h.crates_needed||[]).join(', ')||'none'}`
).join('\n\n')

const impl = await agent(
  `You are a senior Rust performance engineer implementing optimization hypotheses.

Module: ${target}
True Performance Baseline:
${baseline.benchmark_results.map(b => `  ${b.name}: ${b.mean_ns}ns (${b.throughput})`).join('\n')}

Viable hypotheses to implement (choose the highest-impact, lowest-risk combination):
${hypothesisSummary}

Target: ALL benchmarks must run **at least ${speedup_target}x faster** than the True Performance Baseline.

Rules:
- **Iteration over big rewrites.** Implement changes incrementally; run benchmarks after each significant change.
- **NEVER** manipulate benchmark code to hit the target.
- **NEVER** use target-cpu=native or non-standard RUSTFLAGS.
- **NEVER** run benchmarks in parallel.
- If traditional micro-optimizations hit a ceiling, you have permission to investigate radical fundamental changes — data structure redesigns, algorithm replacements, SIMD. **Traditional approaches WILL BE GUARANTEED TO FAIL to meet the target if the easy wins are already gone.**
- After implementing, run: ${benchmark_cmd} -- --noplot
- Record the new results and compute speedup vs. baseline for each benchmark.

If after exhausting the hypotheses the target is still not met:
> "c'mon, try doing a breakthrough — a more fundamental change than what you've tried so far. You are forbidden from giving up easily."

Return: applied_hypotheses, new_benchmark_results with speedup_vs_baseline, whether target was met.`,
  {
    label: `Implement optimizations: ${target}`,
    phase: 'Implement',
    schema: IMPLEMENT_SCHEMA,
    agentType: 'performance-engineer',
  }
)

if (!impl) {
  log('Implementation phase failed')
  return { ok: false, reason: 'impl-failed', baseline }
}

log(`Implementation done. Min speedup: ${impl.speedup_achieved}x. Target met: ${impl.target_met}`)
if (!impl.target_met) {
  log(`WARNING: ${speedup_target}x target not met — consider running a breakthrough round manually`)
}

// ─── Phase 4: Correctness ─────────────────────────────────────────────────────

phase('Correctness')

const referenceNote = reference_impl
  ? `Reference implementation to compare against: ${reference_impl}`
  : `Reference: use the module's own test suite and compare outputs on diverse inputs not used in benchmarks.`

const correctness = await agent(
  `You are verifying that optimized Rust code produces correct outputs.

Optimized module: ${target}
${referenceNote}

Steps:
1. Run the full test suite: cargo test --all-features 2>&1
2. Compare outputs against the reference implementation using diverse inputs that DIFFER from the benchmark inputs.
3. If any output differs: investigate and fix WITHOUT causing more than a 5% speed regression.
4. Re-run benchmarks after any correctness fix: ${benchmark_cmd} -- --noplot

Rules:
- "Fast but wrong" is not acceptable — correctness fixes take priority over speed.
- The 5% regression budget is firm. If a fix costs more, it needs a different approach.
- Test with edge cases: empty inputs, max-size inputs, malformed inputs, Unicode, binary data.

Return: verdict (pass/fail/fixed), tests_run, regressions_found, speed_cost_of_fixes_pct.`,
  {
    label: `Correctness gate: ${target}`,
    phase: 'Correctness',
    schema: CORRECTNESS_SCHEMA,
    agentType: 'performance-engineer',
  }
)

if (correctness?.verdict === 'fail') {
  log('CORRECTNESS GATE FAILED — do not proceed to competitor benchmarks')
  return { ok: false, reason: 'correctness-failed', baseline, impl, correctness }
}

log(`Correctness: ${correctness?.verdict}. Speed cost of fixes: ${correctness?.speed_cost_of_fixes_pct || 0}%`)

// ─── Phase 5: Compete ─────────────────────────────────────────────────────────

phase('Compete')

const compete = await agent(
  `You are building competitor benchmarks for the optimized Rust module.

Our module: ${target}
Benchmark command: ${benchmark_cmd}

Task: Build apples-to-apples criterion benchmarks comparing our implementation against competing crates for the same functionality.

For each competing crate:
1. Add it to Cargo.toml as a dev-dependency (benchmark only)
2. Write a criterion benchmark that exercises the same operation with the same input
3. Run both: ${benchmark_cmd} -- --noplot
4. Record: our mean ns, their mean ns, speedup ratio

Target: Our implementation MUST be **at least 2.0x faster** than **ALL** competing crates in all benchmarks.

If we don't meet 2.0x on any benchmark:
- Report which crates beat us and by how much
- Suggest what technique from the competitor might close the gap

Rules (same as before):
- **NEVER** run benchmarks in parallel
- **NEVER** use target-cpu=native
- Identical inputs, identical operations

Return: per-competitor results, all_targets_met.`,
  {
    label: `Competitor benchmarks: ${target}`,
    phase: 'Compete',
    schema: COMPETE_SCHEMA,
    agentType: 'performance-engineer',
  }
)

const allCompetitorsMet = compete?.all_targets_met ?? false
if (!allCompetitorsMet) {
  log('Some competitor targets not met:')
  for (const c of (compete?.competitors || []).filter(c => !c.target_met)) {
    log(`  ${c.crate}: we=${c.our_mean_ns}ns, they=${c.their_mean_ns}ns, speedup=${c.speedup.toFixed(2)}x (target: 2.0x)`)
  }
}

// ─── Phase 6: Refactor ────────────────────────────────────────────────────────

phase('Refactor')

const refactor = await agent(
  `You are performing a refactor pass on an optimized Rust module to reduce code bloat.

Module: ${target}
Benchmark command: ${benchmark_cmd}

Goal: Reduce total SLoC (Source Lines of Code — not counting blank lines or comments) by **at least 20%** through deduplication and DRY principles.

Rules:
- **Zero criterion benchmark regressions permitted** — run benchmarks before and after every significant change.
- No single file may exceed 1,000 SLoC.
- Use SLoC, not LoC — do not game this metric by removing comments or blank lines.
- Look for: copy-pasted logic, near-identical match arms, redundant intermediate variables, helper functions that could be unified.
- After the refactor, run the full benchmark suite: ${benchmark_cmd} -- --noplot
- Note any unexpected speedups from refactoring (side-effect of removing redundant work paths).

Steps:
1. Count SLoC before: find ${target} -name "*.rs" | xargs grep -v "^[[:space:]]*$" | grep -v "^[[:space:]]*//" | wc -l
2. Apply refactors incrementally, benchmarking after each significant change.
3. Count SLoC after.
4. Report speed delta (positive = faster).

Return: sloc_before, sloc_after, sloc_reduction_pct, regressions (must be 0), speed_delta_pct.`,
  {
    label: `Refactor pass: ${target}`,
    phase: 'Refactor',
    schema: REFACTOR_SCHEMA,
    agentType: 'performance-engineer',
  }
)

if (refactor?.regressions > 0) {
  log(`WARNING: ${refactor.regressions} benchmark regression(s) introduced by refactor — must be fixed`)
}

const speedBonus = refactor?.speed_delta_pct || 0
if (speedBonus > 5) {
  log(`Bonus: refactor yielded ${speedBonus.toFixed(1)}% speed improvement as a side effect`)
}

log(`Refactor: ${refactor?.sloc_reduction_pct?.toFixed(1)}% SLoC reduction (target: ≥20%). Speed delta: ${speedBonus > 0 ? '+' : ''}${speedBonus.toFixed(1)}%`)

// ─── Summary ──────────────────────────────────────────────────────────────────

const ok = impl?.target_met && correctness?.verdict !== 'fail' && (refactor?.regressions || 0) === 0

log(ok
  ? `Optimization complete: ${impl.speedup_achieved}x speedup, correctness verified, ${refactor?.sloc_reduction_pct?.toFixed(1)}% SLoC reduction.`
  : `Optimization session done with issues — review above warnings.`
)

return {
  ok,
  target,
  baseline: baseline.benchmark_results,
  speedup_achieved: impl?.speedup_achieved,
  target_met: impl?.target_met,
  correctness: correctness?.verdict,
  competitor_targets_met: allCompetitorsMet,
  sloc_reduction_pct: refactor?.sloc_reduction_pct,
  refactor_speed_bonus_pct: speedBonus,
}
