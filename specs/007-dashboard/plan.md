# Implementation Plan: Dashboard

**Branch**: `007-dashboard` | **Date**: 2026-10-05 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/007-dashboard/spec.md`

**Scope brief**: [specs/briefs/2026-10-05-dashboard-v1-superseded.md](../briefs/2026-10-05-dashboard-v1-superseded.md). A
plan decision that contradicts a confirmed ledger row means stop and revisit the brief.

## Summary

> **Reconciled 2026-10-05**: the shared view layer and the new CLI reads moved to slice 008 and are merged. This plan covers what remains (token and access, style, pages, gates) and is rewritten when the dashboard is shaped again.

A read-only, server-rendered dashboard served by `nullrouter serve` on its own loopback port
(`127.0.0.1:20130`, on by default), behind a dashboard token issued by the CLI and carried in an
HttpOnly, SameSite=Strict cookie. It has four pages (accounts and quota; routing and requests;
models and providers; keys and behaviour), plus "not built yet" entries for Model tests and
Combos.

Agreement with the CLI is structural. The code that builds each CLI read command's `--json`
value moves into one shared view layer (`nullrouter_server::views`). The CLI prints those
values, and the dashboard renders the same values as HTML from the same in-process operator
handler (research R1).

The look comes from a style guide extracted from 9router's code: a TOML token file where every
value names its 9router source, and a CSS file restricted by tests to those tokens (R4). Inter
and the icons are embedded, so nothing is fetched from outside the machine. No JavaScript is
used. Times are shown in this machine's time zone. Page work is bounded and runs in its own
tasks, so client requests never wait on the dashboard (R8).

## Technical Context

**Language/Version**: Rust (workspace edition and MSRV, unchanged)

**Primary Dependencies**:
- existing: axum 0.8, tokio, serde_json, `nullrouter-server`, `nullrouter-engine`,
  `nullrouter-registry`;
- new: `maud` (HTML, R3), `jiff` (time zones, R10), `subtle` (constant-time compare, R6). `sha2`,
  `base64` and `getrandom` are already in the workspace.

**Storage**: Files in the operator home: new `dashboard.toml` (token digest), new
`config.toml [dashboard]`. It reads the existing files, journal and engine state. No database.

**Testing**: `cargo test` (unit and integration with the real `serve` and CLI binaries,
`reqwest`), a Criterion bench (page render, `records_page_100k`), and an isolation load test
(SC-003).

**Target Platform**: Linux and macOS, the machine `nullrouter serve` runs on. A current desktop
browser on the same machine.

**Project Type**: A web surface inside an existing Rust service plus CLI.

**Performance Goals**: each page in under 1 s with 50 accounts, 20 unified models and 100k
records (SC-007). At most 1 ms added to client p95 and 0 extra client failures while pages load
or fail (SC-003).

**Constraints**:
- loopback only;
- no JavaScript, no external fetches;
- no write routes beyond `POST /signin`;
- light theme only;
- English only;
- no secrets or prompt text on any page;
- at most 2 concurrent page builds, each with a 10 s timeout.

**Scale/Scope**: 4 pages, 2 placeholder pages, record and model detail pages. About 12 views
move into `nullrouter-server`. 4 new CLI commands (`unified`, `behaviour show`, `dashboard
token`, `dashboard status`) and 2 CLI changes (`check`, `records list --before`).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How |
|---|---|---|
| I. Plugin Safety | Pass | Plugins are untouched. The dashboard only reads registry data the CLI already shows, and gives nothing to plugins (FR-014). Secrets stay in the core. The dashboard shows only the last four characters the CLI already shows. |
| II. Routing Fidelity | Pass | No routing change. The dashboard reads `routing.view` through the same handler as the CLI. |
| III. Unified Models and Provider Entities | Pass | Unified models, members, kinds and limits notes get a list view (CLI and page). Combos appear as "not built yet". The combos themselves are a later slice. |
| IV. Scope Discipline | Pass | No request content is touched. Records hold no prompt text, and the dashboard adds none (FR-029). |
| V. Streaming-Native SSE | Pass | Not touched. The dashboard does no streaming. |
| VI. Reference-Informed Behavior | Pass | No inherited routing behaviour changes. 9router is used only as the source of the style guide, and each token traces to its file and line. |
| VII. Trustworthy Model Tests | Pass | Not built. The page says so and makes no claims. |
| VIII. Latency Observability | Pass, partial by plan | Per-request and per-attempt TTFT and total time appear on the dashboard as the records carry them. Summaries per provider and unified model, and trends, are the next slice (brief rows 16, 17). |
| Architecture: Rust only | Pass | maud templates, no JS/TS, no WASM (row 4). |
| Architecture: no blocking on executor | Pass | File and journal reads use `spawn_blocking` (R8). |
| Architecture: secrets isolation | Pass | The token is stored as a digest, 0600, compared in constant time (R6). |
| Performance gate | Pass | Not a routing or streaming hot path. The changed journal read gets the `records_page_100k` Criterion bench. Client-path interference is measured by the SC-003 load test against a dashboard-off baseline. |
| Workflow: parity-audit gate | N/A | No module inheriting 9router behaviour changes. The style guide's traceability test fills the "look" side (SC-005). |

**Post-design re-check (after Phase 1)**: no change. The design adds one crate, one module, one
home file and one config table. No row above moved. There are no violations, so Complexity
Tracking is empty.

## Project Structure

### Documentation (this feature)

```text
specs/007-dashboard/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── dashboard-http.md
│   ├── cli.md
│   └── style-guide.md
├── checklists/requirements.md
└── tasks.md             # /speckit-tasks
```

### Source Code (repository root)

```text
crates/
├── nullrouter-dashboard/            # NEW (R2)
│   ├── Cargo.toml                   # maud, jiff, subtle, axum, tokio, nullrouter-server, nullrouter-engine
│   ├── src/
│   │   ├── lib.rs                   # spawn(engine, settings) -> DashboardHandle; status for the op
│   │   ├── access.rs                # token check, cookie, Host check, sign-in delay (R6, R7)
│   │   ├── guard.rs                 # build semaphore, timeout, panic → 500 (R8)
│   │   ├── frame.rs                 # navigation, page header, "as of", security headers
│   │   ├── time.rs                  # machine-zone rendering, <time datetime> (R10)
│   │   ├── components.rs            # card, badge, table, empty state, from the style guide
│   │   └── pages/{accounts,routing,records,models,keys,not_built,signin}.rs
│   ├── style/{tokens.toml,tokens.css,dashboard.css}
│   ├── assets/{inter-latin.woff2,icons.rs,LICENSES/}
│   ├── tests/                       # access, isolation, cli_agreement, style_guide, secrets
│   └── benches/pages.rs
├── nullrouter-server/src/
│   ├── views/                       # exists (slice 008): --json builders + unified, behaviour (R1)
│   └── operator.rs                  # + dashboard.status (records.list before: slice 008)
├── nullrouter-engine/src/
│   ├── records.rs / journal/        # newest-first read with early stop (R9; slice 008)
│   └── files.rs                     # dashboard.toml load/save
├── nullrouter-registry/src/schema/config.rs   # [dashboard] table, loopback rule
└── nullrouter-cli/src/cmd/
    ├── unified.rs, dashboard.rs     # NEW
    ├── behaviour.rs                 # + show
    ├── check.rs, records.rs         # + dashboard lines, --before
    └── serve.rs                     # starts the dashboard listener
docs/
├── dashboard/style-guide.md         # NEW (R4)
└── operator-config.md               # + Dashboard section, cookie-scope note (R6)
```

**Structure Decision**: one new crate, `nullrouter-dashboard`, beside the four existing ones, and
one new module, `views`, in `nullrouter-server`. The dependency direction is cli → dashboard →
server → engine → registry. `CLAUDE.md`'s workspace table gains the new crate.

## Phases for tasks

1. ~~**Views move**~~ (no behaviour change). Done in slice 008 (merged `b541985`), except the `subject` field on check items (T011).
2. ~~**New CLI reads**~~: `unified`, `behaviour show`, `records list --before`, the newest-first read
   with its bench. Done in slice 008.
3. **Token and access**: `dashboard.toml`, `dashboard token/status`, the `[dashboard]` config,
   the listener in `serve`, sign-in, cookie, Host and CSP checks, the delay.
4. **Style guide**: extraction from `ref/9router`, `tokens.toml`, the generated `tokens.css`, the
   three style tests, the embedded font and icons.
5. **Pages**: accounts and quota (P1) → routing and requests (P2) → models and providers (P2) →
   keys and behaviour (P3) → not-built pages.
6. **Gates**: the CLI-agreement suite (SC-001), the isolation load test (SC-003), the secrets
   sentinel (SC-004), offline rendering (SC-008), the user's side-by-side review (SC-006),
   security review, docs.

## Complexity Tracking

No violations.
