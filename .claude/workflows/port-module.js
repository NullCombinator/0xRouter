export const meta = {
  name: 'port-module',
  description: 'Port a single JavaScript module from ref/9router/ to idiomatic Rust for 0router. Phases: analysis → Rust implementation → parity tests → audit.',
  whenToUse: 'User asks to port a specific JS file, or after identifying a single module to translate in the port-layer workflow.',
  phases: [
    { title: 'Analysis', detail: 'Read JS source, identify patterns, produce porting plan with data model and error contract maps' },
    { title: 'Implement', detail: 'Write idiomatic Rust module from the porting plan' },
    { title: 'Tests', detail: 'Write Rust parity tests covering all fail-open paths and retry scenarios' },
    { title: 'Audit', detail: 'Verify behavioral parity between JS source and Rust output; return findings table' },
  ],
}

// --- Input ---
// args: { js_path: string, rust_out: string } or just a string (js_path)
// js_path: path relative to repo root, e.g. "ref/9router/open-sse/executors/base.js"
// rust_out: optional destination, e.g. "src/executor/base.rs"

function resolveArgs(a) {
  if (!a) return {}
  if (typeof a === 'string') return { js_path: a }
  return a
}
const { js_path, rust_out } = resolveArgs(args)

if (!js_path) {
  log('No js_path provided. Call Workflow({ name: "port-module", args: { js_path: "ref/9router/open-sse/executors/base.js" } })')
  return { ok: false, reason: 'no-input' }
}

// Derive a default rust_out path from js_path if not given
function deriveRustPath(jsPath) {
  // ref/9router/open-sse/executors/base.js → src/executor/base.rs
  // ref/9router/open-sse/translator/request/openai-to-claude.js → src/translator/request/openai_to_claude.rs
  const rel = jsPath.replace(/^ref\/9router\//, '').replace(/^open-sse\//, '')
  const base = rel
    .replace(/\.js$/, '.rs')
    .replace(/-([a-z])/g, (_, c) => '_' + c)  // kebab-case → snake_case
    .replace(/\//g, '/')
  return 'src/' + base
}
const rustOut = rust_out || deriveRustPath(js_path)

// --- Schemas ---

const PLAN_SCHEMA = {
  type: 'object',
  required: ['js_path', 'module_role', 'patterns', 'data_model', 'error_contracts', 'async_boundaries', 'state_fields', 'rust_out'],
  properties: {
    js_path: { type: 'string' },
    module_role: { type: 'string', description: 'One sentence: what this module does and which layer it belongs to.' },
    patterns: {
      type: 'array',
      description: 'Which patterns from the port-js-to-rust catalog apply.',
      items: {
        type: 'object',
        required: ['pattern', 'location', 'rust_approach'],
        properties: {
          pattern: { type: 'string', enum: [
            'class-hierarchy-to-trait',
            'dynamic-dispatch-map-to-enum',
            'abortcontroller-to-cancellation-token',
            'side-effect-registration-to-table',
            'sse-streaming-to-axum-stream',
            'config-object-to-serde-struct',
            'fail-open-to-option',
            'retry-loop-with-fallback',
            'sqlite-adapter-chain',
            'openai-pivot-translator',
          ]},
          location: { type: 'string', description: 'Function/class where this pattern appears.' },
          rust_approach: { type: 'string', description: 'Specific Rust translation: e.g. "enum BuiltinProvider + match".' },
        },
      },
    },
    data_model: {
      type: 'array',
      description: 'Each JS object shape and its Rust struct/enum equivalent.',
      items: {
        type: 'object',
        required: ['js_shape', 'rust_type', 'notes'],
        properties: {
          js_shape: { type: 'string' },
          rust_type: { type: 'string' },
          notes: { type: 'string' },
        },
      },
    },
    error_contracts: {
      type: 'array',
      description: 'Each try/catch block: JS behavior → Rust translation.',
      items: {
        type: 'object',
        required: ['js_behavior', 'rust_behavior', 'is_fail_open'],
        properties: {
          js_behavior: { type: 'string' },
          rust_behavior: { type: 'string' },
          is_fail_open: { type: 'boolean' },
        },
      },
    },
    async_boundaries: {
      type: 'array',
      description: 'Each async operation and its Rust equivalent.',
      items: {
        type: 'object',
        required: ['js_op', 'rust_op'],
        properties: {
          js_op: { type: 'string' },
          rust_op: { type: 'string' },
        },
      },
    },
    state_fields: {
      type: 'array',
      description: 'Class instance fields / closure variables that become Rust struct fields.',
      items: {
        type: 'object',
        required: ['js_field', 'rust_field', 'rust_type'],
        properties: {
          js_field: { type: 'string' },
          rust_field: { type: 'string' },
          rust_type: { type: 'string' },
        },
      },
    },
    rust_out: { type: 'string', description: 'Destination path for the Rust file.' },
    crates_needed: {
      type: 'array',
      items: { type: 'string' },
      description: 'Cargo crates required (e.g. reqwest, serde_json, tokio-util).',
    },
  },
}

const IMPL_SCHEMA = {
  type: 'object',
  required: ['rust_out', 'lines_written', 'public_items'],
  properties: {
    rust_out: { type: 'string' },
    lines_written: { type: 'number' },
    public_items: {
      type: 'array',
      items: { type: 'string' },
      description: 'Public structs, enums, traits, fns exported by the module.',
    },
    unsafe_blocks: { type: 'number', description: 'Count of unsafe blocks (should be 0).' },
    unwrap_count: { type: 'number', description: 'Count of .unwrap() calls outside #[cfg(test)] (should be 0).' },
    notes: { type: 'string' },
  },
}

const TESTS_SCHEMA = {
  type: 'object',
  required: ['test_file', 'test_count', 'fail_open_tests', 'retry_tests'],
  properties: {
    test_file: { type: 'string' },
    test_count: { type: 'number' },
    fail_open_tests: { type: 'number', description: 'Tests that verify Option::None is returned on error, not a panic.' },
    retry_tests: { type: 'number', description: 'Tests that verify retry/fallback logic.' },
    notes: { type: 'string' },
  },
}

const AUDIT_SCHEMA = {
  type: 'object',
  required: ['verdict', 'findings'],
  properties: {
    verdict: { type: 'string', enum: ['pass', 'pass-with-warnings', 'fail'] },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        required: ['category', 'finding', 'severity'],
        properties: {
          category: { type: 'string', enum: ['interface', 'error-contract', 'state-mutation', 'streaming', 'config', 'translator-routes', 'tests'] },
          finding: { type: 'string' },
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          location: { type: 'string' },
        },
      },
    },
    blockers: { type: 'array', items: { type: 'string' }, description: 'Critical/high findings that must be fixed before merge.' },
  },
}

// ─── Phase 1: Analysis ────────────────────────────────────────────────────────

phase('Analysis')

const plan = await agent(
  `You are a JS-to-Rust porting analyst. Your job is to produce a precise porting plan for a single JavaScript module.

Read these files first (in order):
1. ref/9router/CLAUDE.md
2. ref/9router/open-sse/AGENTS.md
3. .claude/skills/port-js-to-rust/SKILL.md  ← the pattern catalog
4. .claude/skills/js-to-rust-patterns/SKILL.md  ← the quick-reference cards
5. ${js_path}  ← the file being ported

Then produce the porting plan as a structured object.

The rust_out path is: ${rustOut}

Rules:
- module_role: one sentence, include the layer (config / translator / executor / handler / rtk / storage).
- patterns: only list patterns that actually apply to this file. Do not invent.
- data_model: cover every exported type and every meaningful internal object shape.
- error_contracts: cover every try/catch. is_fail_open = true when the catch returns null/undefined.
- async_boundaries: cover every await and every AbortController/signal.
- state_fields: cover every class instance field (this.x) and every closure variable that would need to be a struct field.
- crates_needed: be specific (e.g. "reqwest" not "HTTP client").`,
  {
    label: `Analyze ${js_path}`,
    phase: 'Analysis',
    schema: PLAN_SCHEMA,
    agentType: 'js-to-rust-porter',
  }
)

if (!plan || !plan.module_role) {
  log('Analysis failed — no plan produced')
  return { ok: false, reason: 'analysis-failed', js_path }
}

log(`Analysis done — ${plan.patterns.length} patterns, ${plan.data_model.length} data shapes, ${plan.error_contracts.length} error contracts`)

// ─── Phase 2: Implement ───────────────────────────────────────────────────────

phase('Implement')

const patternSummary = plan.patterns.map(p => `- ${p.pattern} at ${p.location}: ${p.rust_approach}`).join('\n')
const dataSummary = plan.data_model.map(d => `- ${d.js_shape} → ${d.rust_type} (${d.notes})`).join('\n')
const errorSummary = plan.error_contracts.map(e => `- ${e.js_behavior} → ${e.rust_behavior}${e.is_fail_open ? ' [FAIL-OPEN → Option<T>]' : ''}`).join('\n')

const impl = await agent(
  `You are a senior Rust engineer porting a JavaScript module to Rust for the 0router project.

PORTING PLAN (from analysis phase):

Module role: ${plan.module_role}
JS source: ${js_path}
Rust destination: ${plan.rust_out}

Patterns to apply:
${patternSummary}

Data model:
${dataSummary}

Error contracts:
${errorSummary}

Async boundaries:
${plan.async_boundaries.map(a => `- ${a.js_op} → ${a.rust_op}`).join('\n')}

State fields:
${plan.state_fields.map(s => `- this.${s.js_field}: ${s.js_field} → ${s.rust_field}: ${s.rust_type}`).join('\n')}

Crates needed: ${(plan.crates_needed || []).join(', ')}

Read the JS source again before writing: ${js_path}
Also read: .claude/skills/port-js-to-rust/SKILL.md

Hard constraints:
- Zero .unwrap() on Option/Result outside #[cfg(test)]
- Zero panic!() in library code
- All public items documented with ///
- clippy::pedantic clean (add #![allow(clippy::...)] only for intentional exceptions, commented why)
- Fail-open paths (is_fail_open = true in error contracts) MUST return Option::None, never panic
- Do not invent abstractions not present in the JS source
- Streaming paths must use futures::Stream, not Vec

Write the file to ${plan.rust_out}.
Return rust_out, lines_written, public_items (pub structs/enums/traits/fns), unsafe_blocks count, unwrap_count.`,
  {
    label: `Implement ${plan.rust_out}`,
    phase: 'Implement',
    schema: IMPL_SCHEMA,
    agentType: 'rust-engineer',
  }
)

if (!impl || !impl.rust_out) {
  log('Implementation failed')
  return { ok: false, reason: 'impl-failed', plan }
}

if (impl.unsafe_blocks > 0) log(`WARNING: ${impl.unsafe_blocks} unsafe block(s) in ${impl.rust_out}`)
if (impl.unwrap_count > 0) log(`WARNING: ${impl.unwrap_count} .unwrap() call(s) in ${impl.rust_out} — must be 0 outside tests`)

log(`Implementation done — ${impl.lines_written} lines, ${impl.public_items.length} public items`)

// ─── Phase 3: Tests ───────────────────────────────────────────────────────────

phase('Tests')

const failOpenContracts = plan.error_contracts.filter(e => e.is_fail_open)
const hasRetry = plan.patterns.some(p => p.pattern === 'retry-loop-with-fallback')

const tests = await agent(
  `You are writing Rust parity tests for a newly ported module.

JS source: ${js_path}
Rust module: ${impl.rust_out}
Public items: ${impl.public_items.join(', ')}

Fail-open contracts to test (each MUST have a test that verifies None is returned, not a panic):
${failOpenContracts.map(e => `- "${e.js_behavior}" → should return None`).join('\n') || '(none)'}

${hasRetry ? 'This module has retry/fallback logic — write at least one test per retry scenario (rate limit, network error, timeout).' : ''}

Rules:
- Write tests in a #[cfg(test)] mod at the bottom of ${impl.rust_out}, or a separate tests/ file if the module is large.
- Each fail-open test: construct a malformed/error input, call the function, assert!(result.is_none()).
- Use proptest or arbitrary inputs for translator functions if round-trip properties can be stated.
- No network calls in unit tests — mock the HTTP layer.
- Write the test file (or append to the Rust file). Return test_file path, test_count, fail_open_tests count, retry_tests count.`,
  {
    label: `Tests for ${impl.rust_out}`,
    phase: 'Tests',
    schema: TESTS_SCHEMA,
    agentType: 'rust-engineer',
  }
)

log(`Tests done — ${tests?.test_count || 0} tests (${tests?.fail_open_tests || 0} fail-open, ${tests?.retry_tests || 0} retry)`)

// ─── Phase 4: Audit ───────────────────────────────────────────────────────────

phase('Audit')

const audit = await agent(
  `You are performing a JS-to-Rust behavioral parity audit.

Read the rust-parity-audit skill: .claude/skills/rust-parity-audit/SKILL.md

JS source:  ${js_path}
Rust port:  ${impl.rust_out}
Test file:  ${tests?.test_file || '(none)'}

Re-read both files now. Then work through every section of the audit protocol:
1. Interface parity
2. Error contract parity (pay special attention to fail-open paths)
3. State and mutation parity
4. Streaming contract (if applicable)
5. Configuration surface
6. Translator route coverage (if applicable)
7. Test coverage gate

Return a structured verdict: pass / pass-with-warnings / fail.
findings: array of { category, finding, severity, location }.
blockers: critical and high findings that must be fixed before merge.`,
  {
    label: `Audit ${impl.rust_out}`,
    phase: 'Audit',
    schema: AUDIT_SCHEMA,
    agentType: 'js-to-rust-porter',
  }
)

const ok = audit?.verdict !== 'fail'

if (audit?.blockers?.length) {
  log(`BLOCKERS (must fix before merge):`)
  for (const b of audit.blockers) log(`  ✗ ${b}`)
} else {
  log(`No blockers found.`)
}

log(ok
  ? `Port complete: ${impl.rust_out} — verdict: ${audit?.verdict}`
  : `Port has blockers: ${impl.rust_out} — ${audit?.blockers?.length} issue(s) to fix`
)

return {
  ok,
  js_path,
  rust_out: impl.rust_out,
  test_file: tests?.test_file,
  verdict: audit?.verdict,
  findings: audit?.findings || [],
  blockers: audit?.blockers || [],
  stats: {
    lines: impl.lines_written,
    public_items: impl.public_items.length,
    tests: tests?.test_count || 0,
    fail_open_tests: tests?.fail_open_tests || 0,
  },
}
