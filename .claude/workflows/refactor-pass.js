export const meta = {
  name: 'refactor-pass',
  description: 'Standalone SLoC-reduction refactor with a zero-regression criterion gate. Targets ≥20% SLoC reduction via deduplication and DRY. Sometimes yields unexpected speed improvements as a side effect.',
  whenToUse: 'After an optimization session has accumulated bloat, when a module has grown past 1k SLoC, or periodically as a maintenance pass. Use as a standalone pass or as the final phase of optimize-perf.',
  phases: [
    { title: 'Measure', detail: 'Count SLoC and benchmark baseline before any change' },
    { title: 'Refactor', detail: 'Reduce SLoC by ≥20% with zero criterion regressions' },
    { title: 'Verify', detail: 'Confirm no regressions, measure final SLoC and speed delta' },
  ],
}

function resolveArgs(a) {
  if (!a) return {}
  if (typeof a === 'string') return { target: a }
  return a
}
const { target, benchmark_cmd = 'cargo criterion' } = resolveArgs(args)

if (!target) {
  log('No target provided. Call Workflow({ name: "refactor-pass", args: { target: "src/executor" } })')
  return { ok: false, reason: 'no-input' }
}

const MEASURE_SCHEMA = {
  type: 'object',
  required: ['sloc', 'files_over_1k', 'benchmark_baseline', 'duplication_hotspots'],
  properties: {
    sloc: { type: 'number' },
    files_over_1k: { type: 'array', items: { type: 'string' } },
    benchmark_baseline: {
      type: 'array',
      items: {
        type: 'object',
        required: ['name', 'mean_ns'],
        properties: { name: { type: 'string' }, mean_ns: { type: 'number' } },
      },
    },
    duplication_hotspots: {
      type: 'array',
      items: {
        type: 'object',
        required: ['description', 'sloc_saveable'],
        properties: {
          description: { type: 'string' },
          sloc_saveable: { type: 'number' },
        },
      },
    },
  },
}

const REFACTOR_SCHEMA = {
  type: 'object',
  required: ['changes', 'sloc_after', 'regressions'],
  properties: {
    changes: { type: 'array', items: { type: 'string' }, description: 'List of deduplication/DRY changes applied.' },
    sloc_after: { type: 'number' },
    regressions: { type: 'number', description: 'Criterion benchmarks that got slower. Must be 0.' },
    speed_delta_pct: { type: 'number', description: 'Overall speed change. Positive = faster.' },
    notes: { type: 'string' },
  },
}

const VERIFY_SCHEMA = {
  type: 'object',
  required: ['sloc_reduction_pct', 'target_met', 'regressions', 'speed_delta_pct', 'verdict'],
  properties: {
    sloc_reduction_pct: { type: 'number' },
    target_met: { type: 'boolean' },
    regressions: { type: 'number' },
    speed_delta_pct: { type: 'number' },
    files_still_over_1k: { type: 'array', items: { type: 'string' } },
    verdict: { type: 'string', enum: ['pass', 'fail'] },
    notes: { type: 'string' },
  },
}

// ─── Phase 1: Measure ─────────────────────────────────────────────────────────

phase('Measure')

const measure = await agent(
  `You are measuring a Rust module before a refactor pass.

Target: ${target}

Steps:
1. Count SLoC (source lines, excluding blanks and comments):
   find ${target} -name "*.rs" | xargs grep -cv "^[[:space:]]*\\($\\|//\\)" | awk -F: 'NR>1{s+=$2} END{print s}'

2. List any files over 1,000 SLoC (these are primary refactor targets).

3. **Without making any code changes**, run the benchmark baseline:
   ${benchmark_cmd} -- --noplot 2>&1
   Record all benchmark names and mean times.

4. Identify duplication hotspots by reading the source:
   - Near-identical match arms with copy-pasted bodies
   - Repeated error-handling boilerplate
   - Helper functions that differ only in a parameter
   - Parallel struct definitions that could be unified with generics
   For each hotspot, estimate SLoC saveable.

Rules:
- NEVER run benchmarks in parallel.
- NEVER use target-cpu=native.

Return: sloc, files_over_1k, benchmark_baseline, duplication_hotspots sorted by sloc_saveable descending.`,
  {
    label: `Measure: ${target}`,
    phase: 'Measure',
    schema: MEASURE_SCHEMA,
    agentType: 'performance-engineer',
  }
)

if (!measure?.sloc) {
  log('Measurement failed')
  return { ok: false, reason: 'measure-failed', target }
}

const targetSloc = Math.floor(measure.sloc * 0.8)
log(`Current SLoC: ${measure.sloc}. Target after refactor: ≤${targetSloc} (20% reduction)`)
if (measure.files_over_1k?.length) log(`Files over 1k SLoC: ${measure.files_over_1k.join(', ')}`)
log(`Top duplication hotspot: ${measure.duplication_hotspots?.[0]?.description || 'none identified'}`)

// ─── Phase 2: Refactor ────────────────────────────────────────────────────────

phase('Refactor')

const hotspotSummary = (measure.duplication_hotspots || [])
  .slice(0, 5)
  .map(h => `  - ${h.description} (~${h.sloc_saveable} SLoC)`)
  .join('\n') || '  (none identified — look harder)'

const refactor = await agent(
  `You are performing a SLoC-reduction refactor pass on a Rust module.

Target: ${target}
Current SLoC: ${measure.sloc}
Goal: Reduce to ≤${targetSloc} SLoC (≥20% reduction)
Benchmark command: ${benchmark_cmd}

Known duplication hotspots to address:
${hotspotSummary}

Rules (strictly enforced):
- **Zero criterion benchmark regressions permitted.** Run benchmarks after each significant change: ${benchmark_cmd} -- --noplot
- No single file may exceed 1,000 SLoC.
- Use SLoC metric (not LoC) — do not remove comments or blank lines to game the count.
- Refactor techniques (DRY, deduplication only — no algorithmic changes, no feature removal):
  * Extract repeated match arm bodies into shared helper functions
  * Unify near-identical structs with a generic parameter
  * Replace copy-pasted error handling with a macro or combinator
  * Merge files with small, related concerns
  * Remove dead code (only after confirming it's truly unused: grep + cargo check)
- After each significant change, recount SLoC and check benchmarks.
- If the refactor unexpectedly makes a benchmark faster, note it — this is a free speedup.

After reaching the SLoC target with no regressions, return:
- changes: list of deduplication/DRY changes applied (one per bullet)
- sloc_after: final SLoC count
- regressions: criterion benchmarks that got slower (must be 0)
- speed_delta_pct: overall speed change vs. pre-refactor baseline (positive = faster)`,
  {
    label: `Refactor: ${target}`,
    phase: 'Refactor',
    schema: REFACTOR_SCHEMA,
    agentType: 'performance-engineer',
  }
)

if (!refactor) {
  log('Refactor phase failed')
  return { ok: false, reason: 'refactor-failed', measure }
}

if (refactor.regressions > 0) {
  log(`REGRESSION: ${refactor.regressions} benchmarks got slower — must be fixed before accepting this refactor`)
}

const reductionPct = ((measure.sloc - refactor.sloc_after) / measure.sloc * 100)
log(`Refactor done: ${measure.sloc} → ${refactor.sloc_after} SLoC (${reductionPct.toFixed(1)}% reduction). Speed delta: ${(refactor.speed_delta_pct || 0) > 0 ? '+' : ''}${(refactor.speed_delta_pct || 0).toFixed(1)}%`)

// ─── Phase 3: Verify ─────────────────────────────────────────────────────────

phase('Verify')

const verify = await agent(
  `You are verifying the results of a Rust refactor pass.

Target: ${target}
Pre-refactor SLoC: ${measure.sloc}
Post-refactor SLoC (reported): ${refactor.sloc_after}
Reported regressions: ${refactor.regressions}

Verify independently:
1. Recount SLoC: find ${target} -name "*.rs" | xargs grep -cv "^[[:space:]]*\\($\\|//\\)" | awk -F: 'NR>1{s+=$2} END{print s}'
2. Confirm no files exceed 1,000 SLoC: find ${target} -name "*.rs" -exec wc -l {} +
3. Run the full test suite: cargo test --all-features 2>&1
4. Run benchmarks one final time: ${benchmark_cmd} -- --noplot 2>&1
5. Compare against pre-refactor baseline:
${measure.benchmark_baseline.map(b => `   ${b.name}: was ${b.mean_ns}ns`).join('\n')}

Report:
- sloc_reduction_pct: (pre - post) / pre * 100
- target_met: sloc_reduction_pct >= 20
- regressions: any benchmarks slower than baseline
- speed_delta_pct: average speed change
- verdict: pass if target_met AND regressions == 0`,
  {
    label: `Verify refactor: ${target}`,
    phase: 'Verify',
    schema: VERIFY_SCHEMA,
    agentType: 'performance-engineer',
  }
)

const ok = verify?.verdict === 'pass'

if (verify?.files_still_over_1k?.length) {
  log(`Files still over 1k SLoC: ${verify.files_still_over_1k.join(', ')}`)
}

log(ok
  ? `Refactor pass complete: ${verify?.sloc_reduction_pct?.toFixed(1)}% SLoC reduction, 0 regressions, ${verify?.speed_delta_pct > 0 ? '+' : ''}${verify?.speed_delta_pct?.toFixed(1) || 0}% speed delta.`
  : `Refactor pass failed: ${verify?.regressions || 0} regression(s), ${verify?.sloc_reduction_pct?.toFixed(1)}% SLoC reduction (${verify?.target_met ? 'met' : 'missed'} 20% target).`
)

return {
  ok,
  target,
  sloc_before: measure.sloc,
  sloc_after: refactor.sloc_after,
  sloc_reduction_pct: verify?.sloc_reduction_pct,
  target_met: verify?.target_met,
  regressions: verify?.regressions,
  speed_delta_pct: verify?.speed_delta_pct,
  changes: refactor.changes,
}
