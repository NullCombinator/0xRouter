export const meta = {
  name: 'mockup-to-ui',
  description: 'Bring the rendered dashboard (crates/nullrouter-dashboard) in line with docs/dashboard/mockups/, one page per parallel agent. Shared files (frame, components, dashboard.css, tokens) change in one serial step first, so the parallel page agents never touch the same file.',
  whenToUse: 'User wants the dashboard to look like the mockups, for all pages or a few. Run it in the checkout that holds the look branch (default .worktrees/look). Not for new dashboard facts or write paths: those need a slice through /shape-spec.',
  phases: [
    { title: 'Survey', detail: 'Per page, in parallel and read-only: list the gaps between the mockup and the current code, each tagged page-local or shared' },
    { title: 'Shared', detail: 'One agent applies every shared gap (frame.rs, components.rs, dashboard.css, tokens) and returns the classes and helpers each page may use' },
    { title: 'Pages', detail: 'Per page, in parallel: apply the page-local gaps, touching only that page\'s module and its page test' },
    { title: 'Verify', detail: 'One agent runs the dashboard tests and clippy under the build budget and fixes what fails' },
    { title: 'Report', detail: 'Final survey pass: what still differs, and the departures the look reviews must record' },
  ],
}

// --- Input ---
// args: { root?: string, pages?: string[], passes?: number, concurrency?: number, implementer?: string }
// root: checkout to work in, relative to the repo root (default ".worktrees/look")
// pages: mockup names without .html (default: all eight)
// passes: survey → shared → pages → verify rounds before the final survey (default 1)
// concurrency: max parallel page agents (default 4)
// implementer: agent for the page step (default "task-implementer"; "rust-engineer" for hard pages)

function resolveArgs(a) {
  if (!a) return {}
  if (typeof a === 'string') return { pages: a.split(/[,\s]+/).filter(Boolean) }
  return a
}

// mockup file → page module and route (crates/nullrouter-dashboard/src/pages/mod.rs `Id`)
const PAGES = {
  'endpoint': { module: 'endpoint', route: '/endpoint' },
  'providers': { module: 'providers', route: '/providers' },
  'combos': { module: 'combo', route: '/combo' },
  'usage': { module: 'usage', route: '/usage' },
  'quota': { module: 'quota', route: '/quota' },
  'proxy-pools': { module: 'proxy_pools', route: '/proxy-pools' },
  'console-log': { module: 'console_log', route: '/console-log' },
  'settings': { module: 'settings', route: '/settings' },
}

const {
  root = '.worktrees/look',
  pages = Object.keys(PAGES),
  passes = 1,
  concurrency = 4,
  implementer = 'task-implementer',
} = resolveArgs(args)

const unknown = pages.filter(p => !PAGES[p])
if (unknown.length) {
  log(`Unknown page(s): ${unknown.join(', ')}. Known: ${Object.keys(PAGES).join(', ')}`)
  return { ok: false, reason: 'unknown-page', unknown }
}

const crate = `${root}/crates/nullrouter-dashboard`
const mockups = `${root}/docs/dashboard/mockups`
const checkout = root.split('/').filter(Boolean).pop() || 'main'

const RULES = `Rules that hold for every change (break one and the work is thrown away):
- The dashboard stays look-only: no <script>, no inline handlers, no forms that write, no new routes. A mockup control that needs a write path is drawn disabled with its CLI command beside it (FR-014), as the existing pages do.
- Pages only format their views (src/pages/mod.rs header comment). Never copy the mockup's sample data, its "MOCKUP NOTE" boxes or its "MOCKUP · sample data" tag; take the layout and look, not the content. A fact the views don't carry is a departure to report, not something to invent.
- style/dashboard.css may use only var(--token) values, keywords, 0 and widths; every token traces to a light-theme ref/9router line in style/tokens.toml, and tokens.css is generated from it (tests/style_guide.rs). A colour or size the mockup needs that no token has is a departure to report unless you can cite its 9router source line.
- The agreement tests (tests/agreement/) pin the dashboard's wording to the CLI. Don't change wording they check; report it as a departure.
- The mockups are generated: ${mockups}/build.sh, mockup.css and the *.py helpers are the source; the *.html files are their output. Read the part of a big file you need (endpoint.html and providers.html are ~95 KB).
- docs/dashboard/style-guide.md is the look's contract; specs/009-dashboard/look-review.md lists departures already accepted.`

const BUILD = `Build budget (one cargo at a time on this machine, never the SSD target dir):
  cd ${root} && CARGO_HOME=/home/ali/Desktop/0router/.cargo-home CARGO_TARGET_DIR=/mnt/storage/0router/target/${checkout} /home/ali/Desktop/0router/.cargo/capped cargo <cmd> -p nullrouter-dashboard -j 2
Never run a workspace-wide cargo, never chain test and clippy in one command, never commit (the main session does git; never stage Cargo.lock from a worktree).`

// --- Schemas ---

const GAP = {
  type: 'object',
  required: ['id', 'scope', 'what', 'where'],
  properties: {
    id: { type: 'string', description: 'Short slug, unique within the page, e.g. "usage-recent-relative-time".' },
    scope: { type: 'string', enum: ['page', 'shared'], description: 'page: fixable inside src/pages/<module>.rs alone. shared: needs frame.rs, components.rs, page.rs, dashboard.css, tokens or another shared file.' },
    what: { type: 'string', description: 'The difference, and what the code should do instead. Name mockup selectors and values.' },
    where: { type: 'string', description: 'Mockup source (file:line or selector) and code location (file:line).' },
  },
}

const SURVEY_SCHEMA = {
  type: 'object',
  required: ['page', 'gaps', 'departures'],
  properties: {
    page: { type: 'string' },
    gaps: { type: 'array', items: GAP },
    departures: { type: 'array', items: { type: 'string' }, description: 'Differences that must stay, each with the rule that forces it (FR, test, missing fact or token).' },
  },
}

const SHARED_SCHEMA = {
  type: 'object',
  required: ['ok', 'applied', 'contract'],
  properties: {
    ok: { type: 'boolean' },
    applied: { type: 'array', items: { type: 'string' }, description: 'Gap ids applied.' },
    skipped: { type: 'array', items: { type: 'string' }, description: '"<gap id>: reason" for each gap not applied.' },
    contract: {
      type: 'object',
      description: 'Per page name: the CSS classes, component helpers and signatures that page\'s agent may now use for its page gaps.',
      additionalProperties: { type: 'string' },
    },
    files: { type: 'array', items: { type: 'string' } },
  },
}

const PAGE_SCHEMA = {
  type: 'object',
  required: ['ok', 'page', 'applied'],
  properties: {
    ok: { type: 'boolean' },
    page: { type: 'string' },
    status: { type: 'string', enum: ['DONE', 'BLOCKED', 'NEEDS_ESCALATION'] },
    applied: { type: 'array', items: { type: 'string' } },
    skipped: { type: 'array', items: { type: 'string' } },
    files: { type: 'array', items: { type: 'string' } },
  },
}

const VERIFY_SCHEMA = {
  type: 'object',
  required: ['ok', 'tests', 'clippy'],
  properties: {
    ok: { type: 'boolean' },
    tests: { type: 'string', description: 'pass, or the failing tests with one line each.' },
    clippy: { type: 'string' },
    fixes: { type: 'array', items: { type: 'string' }, description: 'file: what was fixed and why.' },
    reverted: { type: 'array', items: { type: 'string' }, description: 'Gap ids whose change was undone because it broke a rule test.' },
  },
}

// --- Steps ---

function survey(page, label) {
  const { module, route } = PAGES[page]
  return agent(
    `You compare one dashboard page with its mockup. Read only; change no file.

Page: ${page} (route ${route})
Mockup: ${mockups}/${page}.html, styled by ${mockups}/mockup.css, generated by ${mockups}/build.sh
Code: ${crate}/src/pages/${module}.rs, plus what it calls in src/components.rs, src/frame.rs, src/page.rs${page === 'endpoint' ? ', src/landscape.rs, src/topology.rs' : ''}, and style/dashboard.css

${RULES}

Steps:
1. Read docs/dashboard/style-guide.md and specs/009-dashboard/look-review.md (if present) for accepted departures.
2. Walk the mockup top to bottom: header, each card or section, tables, badges, empty states, side panels, modals, spacing. For each element, find what the code renders and compare structure, classes and the CSS values behind them.
3. Record each difference as a gap. scope=page only if src/pages/${module}.rs alone can fix it with classes and helpers that already exist; anything needing new CSS, a component change or the frame is scope=shared.
4. Record differences that must stay as departures, with the rule that forces them.
Don't list sample-data differences. Be concrete: selectors, values, file:line.`,
    { label, phase: 'Survey', schema: SURVEY_SCHEMA, agentType: 'rust-engineer' }
  )
}

async function surveyAll(phaseName) {
  phase(phaseName)
  const out = await pipeline(pages, p => survey(p, `${phaseName} ${p}`))
  return out.filter(Boolean)
}

let surveys = []
let shared = null
const pageResults = []
const verifyResults = []

for (let pass = 1; pass <= passes; pass++) {
  log(`Pass ${pass}/${passes} in ${root}`)
  surveys = await surveyAll('Survey')

  const sharedGaps = surveys.flatMap(s => s.gaps.filter(g => g.scope === 'shared').map(g => ({ page: s.page, ...g })))
  const pageGapCount = surveys.reduce((n, s) => n + s.gaps.filter(g => g.scope === 'page').length, 0)
  log(`Survey: ${sharedGaps.length} shared gap(s), ${pageGapCount} page gap(s)`)
  if (sharedGaps.length === 0 && pageGapCount === 0) break

  // ─── Shared: one writer for every file more than one page uses ─────────────
  phase('Shared')
  shared = await agent(
    `You make the shared dashboard changes the page surveys asked for, so the page agents that run after you in parallel only need their own page module.

Checkout: ${root}. Files you own: ${crate}/src/frame.rs, src/components.rs, src/page.rs, src/landscape.rs, src/topology.rs, style/dashboard.css, style/tokens.toml and the generated style/tokens.css, assets/icons/. Don't edit src/pages/*.rs except to keep it compiling after a signature change you made.

${RULES}

Shared gaps, by page:
${JSON.stringify(sharedGaps, null, 1)}

Steps:
1. Group gaps that ask for the same thing from different pages and give them one shared rule or helper. Where two pages want one component differently, follow the mockup's mockup.css, which is shared by every page.
2. Apply each gap. New CSS goes in dashboard.css under its component's section and uses only tokens; a new token needs its ref/9router light-theme source line in tokens.toml (regenerate tokens.css the way tests/style_guide.rs expects). If a gap can't be done within the rules, skip it with the reason.
3. Before changing a signature, find its callers with query_graph_tool or grep and update them.
4. For every page (${pages.join(', ')}), write in contract the exact classes, helpers and signatures its page agent may use for its page-scope gaps and the shared gaps you applied for it (e.g. "wrap the recent list in components::card_section; rows use class .act").
Don't run cargo: the Verify phase compiles once after the pages. Read your changes back for errors instead.`,
    { label: 'Shared changes', phase: 'Shared', schema: SHARED_SCHEMA, agentType: 'rust-engineer' }
  )
  if (!shared?.ok) log(`Shared step reported failure; pages continue on what exists. Skipped: ${(shared?.skipped || []).join('; ')}`)

  // ─── Pages: one agent per page module, in parallel ──────────────────────────
  phase('Pages')
  const work = surveys.filter(s => s.gaps.some(g => g.scope === 'page') || shared?.contract?.[s.page])
  for (let i = 0; i < work.length; i += concurrency) {
    const chunk = work.slice(i, i + concurrency)
    const results = await pipeline(chunk, s => {
      const { module } = PAGES[s.page]
      return agent(
        `You bring one dashboard page in line with its mockup. Other agents are editing the other pages at the same time.

Page: ${s.page}. Mockup: ${mockups}/${s.page}.html (source: build.sh, mockup.css).
Files you may touch: ${crate}/src/pages/${module}.rs and, if it exists, ${crate}/tests/pages/${module}.rs. Nothing else: if a gap needs another file, skip it and say so.

${RULES}

Page gaps:
${JSON.stringify(s.gaps.filter(g => g.scope === 'page'), null, 1)}

What the shared step made available to this page (use it; don't re-create it):
${shared?.contract?.[s.page] || '(nothing new)'}

Shared gaps for this page that the shared step skipped (leave them): ${(shared?.skipped || []).filter(x => s.gaps.some(g => x.startsWith(g.id))).join('; ') || 'none'}

Steps:
1. Read the page module, then one neighbouring page module to match style.
2. Apply each gap and wire in the shared contract. Keep VIEWS, wants() and the facts shown unchanged; change arrangement, markup and classes.
3. If a page test asserts markup you changed on purpose, update that assertion to the new markup without weakening what it checks; never touch tests/agreement/.
4. Don't run cargo or git. Check your edits with LSP diagnostics if available, else by reading.
Return status DONE, BLOCKED (needs a file you don't own) or NEEDS_ESCALATION (needs more than markup).`,
        { label: `Page ${s.page}`, phase: 'Pages', schema: PAGE_SCHEMA, agentType: implementer }
      )
    })
    pageResults.push(...results.filter(Boolean))
  }

  // ─── Verify: the only step that compiles ────────────────────────────────────
  phase('Verify')
  const verify = await agent(
    `You compile and test the dashboard after parallel page edits and fix what broke.

${BUILD}

${RULES}

Changes this pass: shared step ${JSON.stringify(shared?.files || [])}; pages ${JSON.stringify(pageResults.map(r => ({ page: r.page, files: r.files, status: r.status })))}

Steps:
1. Run: cargo test (as above). Fix compile errors and failures. A failure in tests/style_guide.rs or tests/agreement/ means a change broke a rule: undo that change rather than the test, and list its gap id in reverted.
2. Then, as a separate command: cargo clippy --all-targets -- -D warnings (as above). Fix it.
3. Repeat until both pass or three rounds have failed; report what still fails.
Don't change the facts a page shows, and don't weaken any test.`,
    { label: `Verify pass ${pass}`, phase: 'Verify', schema: VERIFY_SCHEMA, agentType: 'rust-engineer' }
  )
  verifyResults.push(verify)
  log(verify?.ok ? `Pass ${pass}: tests and clippy pass` : `Pass ${pass}: verify failed: ${verify?.tests}`)
  if (!verify?.ok) break
}

// ─── Report: a final survey is the review ─────────────────────────────────────
const final = await surveyAll('Report')
const remaining = final.flatMap(s => s.gaps.map(g => `${s.page}/${g.id} (${g.scope}): ${g.what}`))
const departures = final.flatMap(s => s.departures.map(d => `${s.page}: ${d}`))
const lastVerify = verifyResults[verifyResults.length - 1]
const ok = (lastVerify?.ok ?? true) && remaining.length === 0

log(ok ? `All ${pages.length} page(s) match their mockups apart from recorded departures.` : `${remaining.length} gap(s) remain.`)
for (const r of remaining) log(`  gap → ${r}`)
log('Not checked here: the rendered pages on screen. Compare them in Chrome against the mockups before calling the look done.')

return {
  ok,
  root,
  pages,
  verify: lastVerify,
  shared_skipped: shared?.skipped || [],
  pages_blocked: pageResults.filter(r => r.status && r.status !== 'DONE').map(r => `${r.page}: ${r.status} ${(r.skipped || []).join('; ')}`),
  remaining,
  departures,
}
