export const meta = {
  name: 'audit-rust-port',
  description: 'Given a Rust file (or directory), find its JS counterpart in ref/9router/ and run a structured behavioral parity audit. Returns a severity-ranked findings table.',
  whenToUse: 'After porting is complete, before merging a Rust module, or when a behavior discrepancy is suspected between 0router (Rust) and 9router (JS).',
  phases: [
    { title: 'Locate', detail: 'Find the JS counterpart for the given Rust file' },
    { title: 'Audit', detail: 'Run the rust-parity-audit protocol: interface, error contracts, state, streaming, config, translator routes, tests' },
    { title: 'Triage', detail: 'Classify findings by severity, identify blockers, suggest fixes' },
  ],
}

// --- Input ---
// args: { rust_path: string } or just a string
// rust_path: path to a Rust file, e.g. "src/executor/base.rs"

function resolveArgs(a) {
  if (!a) return {}
  if (typeof a === 'string') return { rust_path: a }
  return a
}
const { rust_path } = resolveArgs(args)

if (!rust_path) {
  log('No rust_path provided. Call Workflow({ name: "audit-rust-port", args: { rust_path: "src/executor/base.rs" } })')
  return { ok: false, reason: 'no-input' }
}

// --- Schemas ---

const LOCATE_SCHEMA = {
  type: 'object',
  required: ['rust_path', 'js_path', 'confidence', 'rationale'],
  properties: {
    rust_path: { type: 'string' },
    js_path: { type: 'string', description: 'Path to the corresponding JS file in ref/9router/.' },
    confidence: { type: 'string', enum: ['high', 'medium', 'low'], description: 'How confident the match is.' },
    rationale: { type: 'string', description: 'Why this JS file was matched to the Rust file.' },
    related_js_files: {
      type: 'array',
      items: { type: 'string' },
      description: 'Other JS files that contributed logic to this Rust module (if any).',
    },
  },
}

const AUDIT_SCHEMA = {
  type: 'object',
  required: ['verdict', 'sections', 'findings', 'blockers'],
  properties: {
    verdict: { type: 'string', enum: ['pass', 'pass-with-warnings', 'fail'] },
    sections: {
      type: 'object',
      description: 'Pass/fail per audit section.',
      properties: {
        interface_parity: { type: 'string', enum: ['pass', 'warn', 'fail'] },
        error_contracts: { type: 'string', enum: ['pass', 'warn', 'fail'] },
        state_mutation: { type: 'string', enum: ['pass', 'warn', 'fail'] },
        streaming: { type: 'string', enum: ['pass', 'warn', 'fail', 'n/a'] },
        config_surface: { type: 'string', enum: ['pass', 'warn', 'fail', 'n/a'] },
        translator_routes: { type: 'string', enum: ['pass', 'warn', 'fail', 'n/a'] },
        test_coverage: { type: 'string', enum: ['pass', 'warn', 'fail'] },
      },
    },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        required: ['id', 'category', 'finding', 'severity'],
        properties: {
          id: { type: 'number' },
          category: { type: 'string' },
          finding: { type: 'string' },
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          location: { type: 'string' },
          fix: { type: 'string', description: 'Suggested fix (one sentence).' },
        },
      },
    },
    blockers: {
      type: 'array',
      items: { type: 'string' },
      description: 'Critical and high findings — must fix before merge.',
    },
    merge_ready: { type: 'boolean' },
  },
}

const TRIAGE_SCHEMA = {
  type: 'object',
  required: ['merge_ready', 'action_items'],
  properties: {
    merge_ready: { type: 'boolean' },
    action_items: {
      type: 'array',
      items: {
        type: 'object',
        required: ['finding_id', 'action', 'priority'],
        properties: {
          finding_id: { type: 'number' },
          action: { type: 'string' },
          priority: { type: 'string', enum: ['must-fix', 'should-fix', 'nice-to-have'] },
          estimated_effort: { type: 'string', enum: ['trivial', 'small', 'medium', 'large'] },
        },
      },
    },
    summary: { type: 'string' },
  },
}

// ─── Phase 1: Locate ─────────────────────────────────────────────────────────

phase('Locate')

const location = await agent(
  `You are locating the JavaScript counterpart for a Rust file in the 0router project.

Rust file: ${rust_path}

Steps:
1. Read the Rust file: ${rust_path}
2. Infer the JS path by reverse-mapping the path convention:
   - src/executor/base.rs → ref/9router/open-sse/executors/base.js
   - src/translator/request/openai_to_claude.rs → ref/9router/open-sse/translator/request/openai-to-claude.js
   - snake_case → kebab-case, .rs → .js, src/ → ref/9router/open-sse/ (or ref/9router/src/)
3. Check if the inferred path exists with: ls <path>
4. If it doesn't exist, search for it:
   - grep for a distinctive function name or struct name from the Rust file
   - find ref/9router -name "*.js" | xargs grep -l "<term>"
5. Read the JS file once found.

Return: rust_path, js_path (confirmed exists), confidence, rationale, related_js_files (if any).`,
  {
    label: `Locate JS for ${rust_path}`,
    phase: 'Locate',
    schema: LOCATE_SCHEMA,
    agentType: 'js-to-rust-porter',
  }
)

if (!location?.js_path) {
  log(`Could not locate JS counterpart for ${rust_path}`)
  return { ok: false, reason: 'js-not-found', rust_path }
}

log(`Located: ${rust_path} ↔ ${location.js_path} (confidence: ${location.confidence})`)
if (location.confidence === 'low') {
  log(`WARNING: low-confidence match — verify manually before trusting audit results`)
}

// ─── Phase 2: Audit ──────────────────────────────────────────────────────────

phase('Audit')

const relatedNote = location.related_js_files?.length
  ? `\nAlso read these related JS files: ${location.related_js_files.join(', ')}`
  : ''

const audit = await agent(
  `You are performing a behavioral parity audit between a Rust module and its JavaScript source.

Read the full audit protocol: .claude/skills/rust-parity-audit/SKILL.md

JS source:  ${location.js_path}${relatedNote}
Rust port:  ${rust_path}

Re-read BOTH files now. Work through every section of the audit protocol:

1. **Interface parity** — for every exported JS function/method, is there a Rust equivalent with the same semantics?
2. **Error contract parity** — for every try/catch in JS:
   - If JS returns null/undefined → Rust must return None (not panic, not Err)
   - If JS throws → Rust must return Err with a typed error
   - Flag any case where Rust panics where JS would not
3. **State and mutation parity** — class instance fields → struct fields. Closures with shared state → Arc<Mutex<T>>. In-place mutations → checked for clone-on-write correctness.
4. **Streaming contract** — if the module touches SSE/streams: back-pressure, cancellation, [DONE] sentinel, no buffering.
5. **Configuration surface** — all constants/defaults from JS config/ present in Rust with matching values.
6. **Translator route coverage** — if the module is a translator: all register() calls in JS have a Rust entry; no lossy pairs using pivot when JS had a direct route.
7. **Test coverage** — fail-open paths tested, retry logic tested, streaming cancellation tested.

Assign a finding ID (sequential integer) to each issue. Severity:
- critical: behavioral difference causing data loss or errors in production
- high: correctness issue in edge cases
- medium: API surface mismatch or missing option
- low: cosmetic / naming

verdict: "pass" (0 findings), "pass-with-warnings" (only medium/low), "fail" (any critical/high).
blockers: text of critical/high findings.
merge_ready: true only when verdict is pass or pass-with-warnings.`,
  {
    label: `Audit ${rust_path}`,
    phase: 'Audit',
    schema: AUDIT_SCHEMA,
    agentType: 'js-to-rust-porter',
  }
)

if (!audit) {
  log('Audit produced no result')
  return { ok: false, reason: 'audit-failed', rust_path, js_path: location.js_path }
}

log(`Audit verdict: ${audit.verdict} — ${audit.findings.length} findings, ${audit.blockers.length} blockers`)

// ─── Phase 3: Triage ─────────────────────────────────────────────────────────

phase('Triage')

const findingsSummary = audit.findings
  .map(f => `  [${f.id}] ${f.severity.toUpperCase()} (${f.category}): ${f.finding}${f.fix ? ' → Fix: ' + f.fix : ''}`)
  .join('\n') || '  (none)'

const triage = await agent(
  `You are triaging audit findings for a Rust module port.

Rust file: ${rust_path}
JS source:  ${location.js_path}
Audit verdict: ${audit.verdict}

Findings:
${findingsSummary}

For each finding, produce an action item with:
- finding_id: the finding's numeric ID
- action: a concrete, one-sentence instruction for the developer (not "fix it", but exactly what to change)
- priority: must-fix (critical/high), should-fix (medium), nice-to-have (low)
- estimated_effort: trivial (<5 min), small (<30 min), medium (<2h), large (>2h)

Write a two-sentence summary of the overall port quality and the most important action.
merge_ready: true only if no must-fix items remain after this triage.`,
  {
    label: `Triage ${rust_path}`,
    phase: 'Triage',
    schema: TRIAGE_SCHEMA,
    agentType: 'js-to-rust-porter',
  }
)

// Print the findings table
log('\nFindings:')
for (const f of audit.findings) {
  log(`  [${f.id}] ${f.severity.padEnd(8)} ${f.category.padEnd(20)} ${f.finding}`)
}

if (triage?.action_items?.length) {
  log('\nAction items:')
  for (const a of triage.action_items.filter(a => a.priority === 'must-fix')) {
    log(`  ✗ [${a.finding_id}] ${a.action} (${a.estimated_effort})`)
  }
  for (const a of triage.action_items.filter(a => a.priority === 'should-fix')) {
    log(`  ⚠ [${a.finding_id}] ${a.action} (${a.estimated_effort})`)
  }
}

const mergeReady = triage?.merge_ready ?? audit.merge_ready ?? false
log(mergeReady ? '\n✓ Merge ready.' : '\n✗ Not merge ready — fix blockers first.')

return {
  ok: mergeReady,
  rust_path,
  js_path: location.js_path,
  verdict: audit.verdict,
  findings: audit.findings,
  blockers: audit.blockers,
  action_items: triage?.action_items || [],
  merge_ready: mergeReady,
  summary: triage?.summary,
}
