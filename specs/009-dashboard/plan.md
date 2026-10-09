# Implementation Plan: Dashboard

**Branch**: `009-dashboard` | **Date**: 2026-10-05 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/009-dashboard/spec.md`

**Scope brief**: [specs/briefs/2026-10-05-dashboard.md](../briefs/2026-10-05-dashboard.md) (slice 1
of two). A plan decision that contradicts a confirmed ledger row means stop and revisit the brief.

## Summary

A read-only, server-rendered dashboard served by `nullrouter serve` on its own loopback port
(`127.0.0.1:20130`, on by default), behind a dashboard token issued by `nullrouter dashboard
token` and carried in an HttpOnly, SameSite=Strict cookie. It has 9router's sidebar under
"0Router Proxy": Endpoint & Key, Providers, Combo, Usage, Quota Tracker, and under System: Proxy
Pools, Console Log, Settings. A round housekeeping button on every page opens the current
notices.

Agreement with the CLI is structural: every page renders the `views::*` values slice 008 built
for the CLI's `--json`, fetched in process (research R1). This slice adds a `subject` and the
printed line to each `check` notice (R5), the endpoint URL to `check` (R6), "last used" to
`keys list` (R9), `dashboard token` and `dashboard status`, and plugin logos checked by the core
and shipped from 9router by the generator (R10).

The look comes from a style guide taken from 9router's code: a token file where every value names
its 9router source, and CSS restricted to those tokens (R12). Inter and the icons are embedded
(R11). There are no scripts: windows and the panel have their own addresses, filters are `GET`
forms (R4). Page work is bounded and runs in its own tasks, so client requests never wait on the
dashboard (R8).

## Technical Context

**Language/Version**: Rust (workspace edition and MSRV, unchanged).

**Primary Dependencies**:
- existing: axum 0.8, tokio, serde_json, arc-swap, sha2, base64, getrandom;
  `nullrouter-server` (views, operator), `nullrouter-engine`, `nullrouter-registry`;
- new: `maud` (HTML, R3), `jiff` (time zones, R13), `subtle` (constant-time compare, R7).
- assets: Inter woff2 (OFL 1.1), about 45 Material Symbols SVGs (Apache 2.0).
- generator: ImageMagick, once, by hand, for five logo overrides (R10). Not a build dependency.

**Storage**: files in the operator home: new `dashboard.toml` (token digest, issue time), new
`config.toml [dashboard]` table, new `plugins/logos/` directory. It reads the existing files,
journal and engine state. No database.

**Testing**: `cargo test` (unit; integration with the real `serve` and CLI binaries and
`reqwest`), Criterion benches (`pages`, `keys_last_used_100k`), an opt-in isolation load test
(SC-005), and the user's side-by-side review (SC-008).

**Target Platform**: Linux and macOS, the machine `nullrouter serve` runs on, with a current
desktop browser on the same machine.

**Project Type**: a web surface inside an existing Rust service and CLI.

**Performance Goals**: each page in under 1 s on a home with 50 accounts, 20 unified models and
100,000 records (SC-009). At most 1 ms added to client p95 and 0 extra client failures while pages
load or fail (SC-005). "Last used": warm under 5 ms, cold under 1 s on 100,000 records (R9).

**Constraints**: loopback only; no scripts and no external fetches; no write route except
`POST /signin`; light theme; English; no secrets or prompt text on any page; at most 2 page
builds at once, 10 s each; logos at most 64 KiB and 256 px, PNG only.

**Scale/Scope**: 8 sidebar pages, 2 window routes, 1 panel, sign-in; 6 subjects for notices;
2 new CLI commands (`dashboard token`, `dashboard status`) and 3 changed reads (`check`,
`keys list`, plugin loading); 2 new operator ops (`server.status`, `keys.last_used`); 119 logos.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How |
|---|---|---|
| I. Plugin Safety | Pass | A plugin gains one data field, `logo`, a bare file name. The core alone reads, checks and serves the file (R10); a plugin still makes no request, reads no file and gets no secret. A bad logo can't stop a plugin from loading. Bundled and community logos pass the same check as user ones. The dashboard gives plugins nothing (FR-018). |
| II. Routing Fidelity | Pass | No routing change. Pages read `routing.view` through the same handler as the CLI. "Last used" reads the journal; it doesn't touch placement. |
| III. Unified Models and Provider Entities | Pass | Providers show every model kind their plugins declare (FR-031). Combos stay "not built yet" (brief row 7, clarify Q3); unified-model notices appear on Combo. |
| IV. Scope Discipline | Pass | No request content is touched. Records hold no prompt text, and the dashboard adds none (FR-044). |
| V. Streaming-Native SSE | Pass | Not touched. The dashboard does no streaming. |
| VI. Reference-Informed Behavior | Pass | No inherited behaviour changes. 9router is the source of the look and the logos; each style token traces to its file and line, and each logo to its path (R10, R12). |
| VII. Trustworthy Model Tests | Pass | Not built. "Test All" is disabled and says so (R15). |
| VIII. Latency Observability | Pass, partial by plan | Per-request and per-attempt TTFT and total time appear as the records carry them. Summaries and the landscape gauges are slice 2 (brief rows 20, 23), drawn as slots here. |
| Architecture: Rust only | Pass | maud templates; no JS/TS, no WASM in the browser. The generator stays Node, as it is today. |
| Architecture: no blocking on executor | Pass | Views run on the blocking pool (`run_in_process`), as do logo and journal reads (R8, R9). |
| Architecture: secrets isolation | Pass | The dashboard token is stored as a digest, 0600, compared in constant time (R7). No page shows a secret beyond the last four characters the CLI shows. |
| Performance gate | Pass | Not a routing or streaming hot path. The segment index change (R9) is on the read side; `keys_last_used_100k` and `pages` are new benches, and slice 008's `records_page` baseline must not regress. Client interference is measured by the SC-005 load test. |
| Workflow: parity-audit gate | N/A | No module inheriting 9router behaviour changes. The style-source test covers the look (SC-007). |

**Open security item**: Low **L1** (cookies are scoped by host, not port; R7) is carried from
spec 007 unjudged. It doesn't block the plan; the user decides it before implementation of the
token work.

**Post-design re-check (after Phase 1)**: no change. The design adds one crate, one config table,
one home file, one home directory, one plugin field, and two operator ops. No row moved. There
are no violations, so Complexity Tracking is empty.

## Project Structure

### Documentation (this feature)

```text
specs/009-dashboard/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── dashboard-http.md
│   ├── cli.md
│   ├── plugin-logo.md
│   └── style-guide.md
├── checklists/requirements.md
└── tasks.md             # /speckit-tasks
```

### Source Code (repository root)

```text
crates/
├── nullrouter-dashboard/                 # NEW (R2)
│   ├── Cargo.toml                        # maud, jiff, subtle, axum, tokio, nullrouter-server, nullrouter-engine
│   ├── src/
│   │   ├── lib.rs                        # spawn(engine, settings, version) -> DashboardHandle
│   │   ├── access.rs                     # token, cookie, Host, Origin, sign-in delay (R7, R8)
│   │   ├── guard.rs                      # build semaphore, timeout, panic → 500 (R8)
│   │   ├── frame.rs                      # sidebar, page header, "as of", housekeeping button and panel
│   │   ├── time.rs                       # machine-zone rendering, <time datetime> (R13)
│   │   ├── components.rs                 # card, badge, table, modal, side panel, slot, disabled control
│   │   └── pages/{endpoint,providers,combo,usage,quota,proxy_pools,console_log,settings,signin}.rs
│   ├── style/{tokens.toml,tokens.css,dashboard.css}
│   ├── assets/{inter-latin.woff2,icons/*.svg,LICENSES/}
│   ├── tests/                            # access, agreement, twins, notices, isolation, secrets, offline, style_guide, logos
│   └── benches/pages.rs
├── nullrouter-server/src/
│   ├── views/check.rs                    # + notices[{level,subject,text}], endpoint (R5, R6)
│   ├── views/keys.rs                     # + last_used (R9)
│   ├── views/dashboard.rs                # NEW: dashboard status
│   └── operator.rs                       # + server.status, keys.last_used
├── nullrouter-engine/src/
│   ├── journal/index.rs                  # + newest arrival per agent (R9)
│   ├── journal/records.rs                # + last_used read without a server (R9)
│   └── files.rs                          # dashboard.toml load/save
├── nullrouter-registry/
│   ├── build.rs                          # + embed plugins/{bundled,community}/logos/*.png
│   └── src/{schema/plugin.rs,schema/config.rs,load.rs,logo.rs}   # logo field and check; [dashboard]
└── nullrouter-cli/src/cmd/
    ├── dashboard.rs                      # NEW: token, status
    ├── check.rs                          # prints notices[].text and endpoint
    ├── keys.rs                           # last used column
    ├── plugins.rs                        # install/uninstall copy and remove the logo
    └── serve.rs                          # starts the dashboard listener
plugins/
├── bundled/logos/*.png                   # NEW (6 of 7; opencode-zen has none)
├── community/logos/*.png                 # NEW (113 of 114; ollama-search has none)
└── LOGOS.md                              # NEW: each logo's 9router source
tools/gen-bundled/
├── generate.mjs                          # + copy and check logos, write `logo =`
└── seeds/logos/{crush,nebius,reka,siliconflow,kimchi}.png   # NEW overrides (R10)
docs/
├── dashboard/style-guide.md              # revised (R12)
├── plugins.md                            # + logo field
└── operator-config.md                    # + Dashboard section, L1 note
```

**Structure Decision**: one new crate, `nullrouter-dashboard`, beside the four existing ones;
dependency order cli → dashboard → server → engine → registry. `CLAUDE.md`'s workspace table gains
the crate. The generated logos are committed on their own, naming the ref SHA, as the CLAUDE.md
rule for generator output says.

## Phases for tasks

The build order below was the plan's first cut. `tasks.md` orders the work by story priority
(token and access first, logos last as P3), and its order wins.

1. **Read-model additions** (no dashboard yet; the CLI's tests are the gate): `check` notices with
   subjects and the endpoint (R5, R6), `server.status`, `keys.last_used` and "last used" in
   `keys list` with its bench (R9).
2. **Plugin logos**: the schema field, the check and its `check` note, embedding, install and
   uninstall, the generator, the overrides, `LOGOS.md` (R10). Generated output in its own commit.
3. **Token and access**: `dashboard.toml`, `dashboard token`/`status`, `[dashboard]`, the listener
   in `serve`, sign-in, cookie, Host, Origin and CSP checks, the delay (R7, R8). User decides L1
   first.
4. **Style guide**: revise the guide, extract the new components, `tokens.toml`, generated
   `tokens.css`, the style tests, Inter and the icons (R11, R12).
5. **Frame and pages**: frame (sidebar, header, housekeeping button and panel) → Quota Tracker
   (P1) → Usage with the record window (P2) → Providers with the provider window and logos (P2) →
   Endpoint & Key and Settings (P3) → Combo, Console Log, Proxy Pools.
6. **Gates**: agreement and twin suites (SC-001 to SC-003), isolation (SC-005), secrets and
   offline (SC-006, SC-010), logos (SC-011), benches (SC-009), security review, docs, the user's
   side-by-side review (SC-008).

## Complexity Tracking

No violations.
