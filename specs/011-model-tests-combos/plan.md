# Implementation Plan: Model Tests and Combos

**Branch**: `011-model-tests-combos` | **Date**: 2026-10-07 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/011-model-tests-combos/spec.md`. Scope brief:
`specs/briefs/2026-10-07-model-tests-combos.md`.

## Summary

A test is an ordinary engine request with two new tags: pinned to one account, and marked as a
test (research R1). It goes through plan, placement, upstream, records, quota tally and pacing
like client traffic, with the smallest body per type (R2) under a per-type deadline (R6). A new
pure judge turns the outcome into PASS, BROKEN or UNKNOWN: BROKEN only on 0router's strict
rejection list (a status plus model wording) or a plugin's `[[rejections]]` rule (R3, R4).
Verdicts live in an engine board, journalled to `routing/verdicts.jsonl` by the existing writer
thread (R5), and reset when the account's secret, sign-in or plugin digest changes (R7). The plan
step skips BROKEN pairs (R11). UNKNOWN retests, of pairs and of combos, run on their own task beside the maintenance queue,
so they never hold a token refresh back (R8). The CLI runs
tests over two new socket ops, streaming one line per pair (R10).

Combos are `[[combo]]` in `config.toml`, checked at load (R12) and flattened into an ordered
list of unified models. The attempt loop walks that list, moving on only after a member's own
walk fails with fallback allowed and before any output (R13). A combo test is one tagged request
through the combo (R14).

## Technical Context

**Language/Version**: Rust (workspace edition and toolchain unchanged)

**Primary Dependencies**: existing only: `tokio`, `tokio-util`, `serde`/`serde_json`, `sha2`,
`arc-swap`, `clap`, `criterion`. No new crate.

**Storage**: `routing/verdicts.jsonl` (new, mode 0600) through the journal writer; `config.toml`
gains `[[combo]]` and `[tests]`.

**Testing**: CI only (`cargo test` per crate in GitHub Actions); engine `testkit` mock upstreams
and simulated clock; registry load tests; server socket and model-list tests; CLI goldens;
Criterion bench extension.

**Target Platform**: Linux (Unix operator socket).

**Project Type**: Rust workspace: CLI plus server, engine and registry libraries.

**Performance Goals**: plan time with 1000 verdicts and a 3-level combo within 5 % of today's
plan time (R16, SC-009). A single-pair test returns within its timeout + 5 s (SC-004).

**Constraints**: no blocking I/O on the executor (verdict writes go to the writer thread);
client traffic never sets verdicts (FR-010); no secret, prompt or output in records, socket
answers or `verdicts.jsonl`; no model list change from verdicts (clarify Q2).

**Scale/Scope**: 3 new modules (`engine::verdict`, `engine::tests`, `registry` combos), 1 new
job kind, 4 socket ops, 3 CLI command groups, 1 plugin schema table, 2 `config.toml` tables.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle / constraint | Status |
|---|---|
| I. Plugin safety | Pass. `[[rejections]]` is data matched by the core (R4); plugins can't declare combos; the core makes every test call and injects secrets at execution only. The secret basis is a SHA-256 digest salted with the install id, never the key (R7). |
| II. Routing fidelity | Pass. Each unified model inside a combo keeps its own placement and warm state (R13); test calls don't create warm state (R1). BROKEN skips save an attempt per request. |
| III. Unified models and provider entities | Pass. Combos are policies over unified models, nested first-class, and testable on their own (R12–R14). Every model type is tested (R2). Operator-written combo test suites are deferred by the brief (row 18). |
| IV. Scope discipline | Pass. Scope is the brief's confirmed rows plus four clarify answers. Test prompts are 0router's own; client content is never rewritten. |
| V. Streaming-native SSE | Pass. A combo never switches members after output (R13); streams relay as before. |
| VI. Reference-informed behaviour | Pass. 9router's `ping.js` is the oracle for minimal bodies (R2); its fallback combo order is kept; round-robin and fusion are out (brief row 3). |
| VII. Trustworthy model tests | Pass. Verdicts come only from real minimal calls; BROKEN only from definitive rejection (R3); UNKNOWN retests and is never promoted without a rejection (R8). A combo test's UNKNOWN is kept and retested too (R14, FR-030; analyze D1). A transient failure on a pair during a combo test stores no result for that pair (clarify Q3), so there is no UNKNOWN to retest. |
| VIII. Latency observability | Pass. Test records carry TTFT and duration; the test output shows them. |
| No blocking on the executor | Pass. Writes go through the journal writer thread (R5). |
| No self-registration | Pass. Job kind, ops and judge are plain enum arms and functions. |
| Performance gate | Pass. Plan and resolve benches extended (R16). |

**Post-design re-check (after Phase 1)**: no change. The one judgement call is R3's core list:
it errs toward UNKNOWN (a bare 404 is not BROKEN), which VII requires.

## Project Structure

### Documentation (this feature)

```text
specs/011-model-tests-combos/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── cli.md
│   ├── config-and-plugins.md
│   └── operator-socket.md
├── checklists/requirements.md
└── tasks.md              # /speckit-tasks
```

### Source Code (repository root)

```text
crates/nullrouter-registry/src/
├── schema/config.rs      # ComboDecl, TestSettings ([tests])
├── schema/plugin.rs      # rejections: Vec<RejectionRule>
├── schema/rejection.rs   # RejectionRule (shape of signin::RefusedRule) — new
├── validate/gate.rs      # rejection status rules (R4)
├── combos.rs             # load checks, cycle, kinds, flattening (R12) — new
├── registry.rs           # combos, plugin_digest, test settings in RuntimeSettings
├── resolve.rs            # Resolution::Combo
crates/nullrouter-engine/src/
├── verdict/
│   ├── mod.rs            # Verdict, Pair, Board (ArcSwap), basis check (R5, R7)
│   ├── judge.rs          # core list + plugin rules (R3)
│   └── store.rs          # verdicts.jsonl lines, replay, compaction
├── tests/                # test runs: minimal bodies (R2), deadlines (R6), combo results (R14), retest task (R8) — new
├── plan.rs               # Pin, Broken skips (R11), combo flattening input
├── attempt.rs            # pin/test tags, combo outer loop (R13), record fields
├── records.rs            # TestMark, combo, Attempt.member, ErrorClass::Broken
├── journal/writer.rs     # Target::Verdicts
crates/nullrouter-server/src/
├── operator.rs           # test.plan, test.run (streamed), verdicts.list, verdicts.set
├── models.rs             # combos listed (FR-025)
├── views/                # verdicts, combos views (CLI reads go through views, spec 008)
crates/nullrouter-cli/src/cmd/
├── test.rs  verdicts.rs  combos.rs   # new
├── records.rs  resolve.rs  unified.rs  check.rs   # additions
docs/operator-config.md, docs/plugins.md
```

**Structure Decision**: no new crate. Verdicts and test runs sit in the engine next to routing
and records, which they use; combos sit in the registry next to unified models.

## Phases for tasks

1. **Registry**: `[[combo]]`, `[tests]`, `[[rejections]]` schema, load checks and errors,
   `Resolution::Combo`, plugin digests; registry tests (combos.rs) and resolve bench case.
2. **Verdict core**: `verdict::judge` with its table test; `Board`, store, replay, basis reset;
   `Target::Verdicts` in the writer.
3. **Routing**: BROKEN skips in `plan`, the all-BROKEN error, record fields; routing tests;
   plan bench.
4. **Tests (single pair)**: `pin` and `test` tags, minimal bodies per type, deadlines, video job
   completion, records marked; judge wired in.
5. **Operator surface**: socket ops, streamed `test.run`, confirmation flow, `verdicts` commands
   and settings, goldens.
6. **Retests**: the retest task, schedule, holds, restart spreading; simulated-clock tests,
   including a token refresh that still runs while retests fill the test limit.
7. **Combos at request time**: flattening, outer loop, model lists, records; combo walk tests.
8. **Combo tests**: tagged combo request, nested output, Q3 verdict rule; server test.
9. **Gates**: secrets sentinel, docs, `check` additions, bench baseline (local, user-run),
   security-auditor review (ask first), opt-in live check.

## Complexity Tracking

No violations.
