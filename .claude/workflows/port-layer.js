export const meta = {
  name: 'port-layer',
  description: 'Port an entire directory layer from ref/9router/ to Rust. Enumerates JS files, fans out to parallel port-module runs, then produces a layer-level completion report.',
  whenToUse: 'User wants to port a whole subdirectory at once (e.g. all of open-sse/translator/concerns/, or open-sse/config/). For a single file, use port-module instead.',
  phases: [
    { title: 'Enumerate', detail: 'List JS files in the target layer, determine port order and Rust destination paths' },
    { title: 'Port', detail: 'Fan out port-module to each file in parallel (up to concurrency limit)' },
    { title: 'Report', detail: 'Aggregate results: succeeded, failed, blocker count, total test coverage' },
  ],
}

// --- Input ---
// args: { layer: string, rust_base?: string, concurrency?: number }
// layer: JS directory under ref/9router/, e.g. "open-sse/translator/concerns"
// rust_base: Rust output directory, defaults to derived from layer
// concurrency: max parallel ports (default 4)

function resolveArgs(a) {
  if (!a) return {}
  if (typeof a === 'string') return { layer: a }
  return a
}
const { layer, rust_base, concurrency = 4 } = resolveArgs(args)

if (!layer) {
  log('No layer provided. Call Workflow({ name: "port-layer", args: { layer: "open-sse/translator/concerns" } })')
  return { ok: false, reason: 'no-input' }
}

function deriveRustBase(jsLayer) {
  return rust_base || ('src/' + jsLayer.replace(/^open-sse\//, '').replace(/-([a-z])/g, (_, c) => '_' + c))
}
const rustBase = deriveRustBase(layer)

// --- Schemas ---

const ENUMERATE_SCHEMA = {
  type: 'object',
  required: ['files', 'total', 'order_rationale'],
  properties: {
    files: {
      type: 'array',
      items: {
        type: 'object',
        required: ['js_path', 'rust_out', 'priority', 'dependencies'],
        properties: {
          js_path: { type: 'string' },
          rust_out: { type: 'string' },
          priority: { type: 'number', description: 'Port order: lower = earlier. Files with no local deps = 1.' },
          dependencies: { type: 'array', items: { type: 'string' }, description: 'Other JS files in this layer that must be ported first.' },
          skip_reason: { type: 'string', description: 'If non-empty, this file will be skipped (e.g. auto-generated, test fixture).' },
        },
      },
    },
    total: { type: 'number' },
    skipped: { type: 'number' },
    order_rationale: { type: 'string', description: 'Why files are ordered this way (e.g. schema enums before translator fns).' },
  },
}

const REPORT_SCHEMA = {
  type: 'object',
  required: ['layer', 'succeeded', 'failed', 'skipped', 'total_blockers', 'summary'],
  properties: {
    layer: { type: 'string' },
    succeeded: { type: 'number' },
    failed: { type: 'number' },
    skipped: { type: 'number' },
    total_blockers: { type: 'number' },
    total_lines: { type: 'number', description: 'Total lines of Rust written across all ported files.' },
    summary: { type: 'string', description: 'One paragraph: what was ported, what failed, what to do next.' },
    next_steps: { type: 'array', items: { type: 'string' } },
  },
}

// ─── Phase 1: Enumerate ───────────────────────────────────────────────────────

phase('Enumerate')

const enumeration = await agent(
  `You are enumerating a JavaScript directory layer for a JS-to-Rust port.

Target layer: ref/9router/${layer}
Rust destination base: ${rustBase}

Steps:
1. Use the Bash tool to list all .js files in ref/9router/${layer}/ (non-recursive first, then check subdirectories).
   Command: find ref/9router/${layer} -name "*.js" -not -path "*/node_modules/*" | sort
2. For each .js file:
   - Determine rust_out: replace the JS path prefix with the Rust base, convert kebab-case to snake_case, .js → .rs
   - Assign priority based on dependency order within the layer:
     * Pure config/constants with no imports from the same layer → priority 1
     * Files that import from priority-1 files → priority 2
     * Files that import from priority-2 files → priority 3
   - List dependencies: other .js files in THIS layer that it imports from
   - Set skip_reason for: index.js files that are auto-generated registries, test fixtures, .test.js files
3. Return the structured enumeration.

Do NOT read file contents — just list, classify by imports (scan first lines), and order.`,
  {
    label: `Enumerate ${layer}`,
    phase: 'Enumerate',
    schema: ENUMERATE_SCHEMA,
    agentType: 'js-to-rust-porter',
  }
)

if (!enumeration || !enumeration.files || enumeration.files.length === 0) {
  log('Enumeration found no files — check the layer path')
  return { ok: false, reason: 'no-files', layer }
}

const toPort = enumeration.files
  .filter(f => !f.skip_reason)
  .sort((a, b) => a.priority - b.priority)

log(`Enumeration done — ${enumeration.total} files found, ${enumeration.skipped} skipped, ${toPort.length} to port`)
log(`Port order rationale: ${enumeration.order_rationale}`)

// ─── Phase 2: Port (parallel, capped at concurrency) ─────────────────────────

phase('Port')

// Split into batches by priority level, port each batch in parallel
const byPriority = toPort.reduce((acc, f) => {
  const p = f.priority || 1
  if (!acc[p]) acc[p] = []
  acc[p].push(f)
  return acc
}, {})

const allResults = []

for (const priority of Object.keys(byPriority).sort((a, b) => Number(a) - Number(b))) {
  const batch = byPriority[priority]
  log(`Porting priority-${priority} batch (${batch.length} files) in parallel (concurrency=${concurrency})...`)

  // Cap batch size at concurrency
  for (let i = 0; i < batch.length; i += concurrency) {
    const chunk = batch.slice(i, i + concurrency)
    const results = await pipeline(chunk, (file) =>
      agent(
        `You are porting a single JavaScript file to Rust as part of a layer port.

This is a sub-task delegated from the port-layer workflow. Follow the port-module protocol exactly.

JS source: ${file.js_path}
Rust destination: ${file.rust_out}

Read these first:
1. ref/9router/open-sse/AGENTS.md
2. .claude/skills/port-js-to-rust/SKILL.md
3. .claude/skills/js-to-rust-patterns/SKILL.md
4. ${file.js_path}

Then:
1. Identify patterns (from the catalog)
2. Map data model
3. Map error contracts (especially fail-open paths)
4. Write the Rust file to ${file.rust_out}
5. Write parity tests
6. Run a quick self-audit: interface parity, fail-open parity, no .unwrap() outside tests

Return: { ok: boolean, js_path, rust_out, verdict, blockers: string[], lines_written: number, test_count: number }`,
        {
          label: `Port ${file.js_path}`,
          phase: 'Port',
          agentType: 'js-to-rust-porter',
          schema: {
            type: 'object',
            required: ['ok', 'js_path', 'rust_out'],
            properties: {
              ok: { type: 'boolean' },
              js_path: { type: 'string' },
              rust_out: { type: 'string' },
              verdict: { type: 'string' },
              blockers: { type: 'array', items: { type: 'string' } },
              lines_written: { type: 'number' },
              test_count: { type: 'number' },
            },
          },
        }
      )
    )
    allResults.push(...results)
  }
}

// ─── Phase 3: Report ─────────────────────────────────────────────────────────

phase('Report')

const succeeded = allResults.filter(r => r?.ok).length
const failed = allResults.filter(r => r && !r.ok).length
const totalBlockers = allResults.reduce((n, r) => n + (r?.blockers?.length || 0), 0)
const totalLines = allResults.reduce((n, r) => n + (r?.lines_written || 0), 0)
const failedFiles = allResults.filter(r => r && !r.ok).map(r => r.js_path).join(', ')

const report = await agent(
  `You are writing a completion report for a layer port.

Layer: ref/9router/${layer}
Rust base: ${rustBase}

Results:
- Succeeded: ${succeeded}
- Failed: ${failed} ${failedFiles ? '(' + failedFiles + ')' : ''}
- Skipped: ${enumeration.skipped} (auto-generated or test fixtures)
- Total blockers across all files: ${totalBlockers}

Per-file results:
${allResults.map(r => `  ${r?.ok ? '✓' : '✗'} ${r?.js_path || '?'} → ${r?.rust_out || '?'} (${r?.verdict || 'no verdict'})${(r?.blockers?.length) ? ' BLOCKERS: ' + r.blockers.join('; ') : ''}`).join('\n')}

Write a one-paragraph summary and a next_steps list. next_steps should include:
- Fix any blocker files
- Add the new Rust modules to Cargo.toml (list the modules)
- Run 'cargo clippy --all-targets -- -D warnings' across the new modules
- Run '/rust-parity-audit' on any files with warnings
- What to port next (the next layer that depends on this one)`,
  {
    label: `Report for ${layer}`,
    phase: 'Report',
    schema: REPORT_SCHEMA,
    agentType: 'js-to-rust-porter',
  }
)

const ok = failed === 0 && totalBlockers === 0

log(ok
  ? `Layer ${layer} fully ported: ${succeeded} files, ${totalLines} total lines.`
  : `Layer ${layer} partially ported: ${succeeded} succeeded, ${failed} failed, ${totalBlockers} blockers.`
)

if (report?.next_steps?.length) {
  log('Next steps:')
  for (const step of report.next_steps) log(`  → ${step}`)
}

return {
  ok,
  layer,
  rust_base: rustBase,
  succeeded,
  failed,
  skipped: enumeration.skipped,
  total_blockers: totalBlockers,
  total_lines: totalLines,
  summary: report?.summary,
  next_steps: report?.next_steps || [],
  results: allResults,
}
