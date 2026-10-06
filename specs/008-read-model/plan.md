# Implementation Plan: Read Model

**Branch**: `008-read-model` | **Date**: 2026-10-05 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/008-read-model/spec.md`. Scope brief:
`specs/briefs/2026-10-05-read-model.md`.

## Summary

Every CLI read gets its answer from one shared read model, `nullrouter_server::views`. Each view
is a sync function of the home's files and the live answers of the operator ops it declares
(research R2). The CLI fetches those answers over the socket; the in-server route (used by the
dashboard later, and by tests now) fetches them with `operator::handle`. So the two routes differ
only in transport and agree by construction. Each view returns the exact `--json` value the CLI
prints today, plus an `extra` part for facts only the text shows (R3). A golden suite, committed
before any code moves, pins every read's output byte for byte (R4).

New: `unified [NAME]`, `behaviour show`, and `records list --before <ID>`. The record reader
reads each day segment from its end, so the newest page doesn't grow with the journal (R6), with
a Criterion bench and targets (R7).

## Technical Context

**Language/Version**: Rust (workspace edition and toolchain unchanged)

**Primary Dependencies**: existing only: `serde_json`, `tokio`, `nullrouter-registry`,
`nullrouter-engine`, `clap` (CLI), `criterion` (bench). No new crate.

**Storage**: the operator home's existing files; nothing new is stored.

**Testing**: `cargo test` per crate (`-j 2`); a new golden suite in `nullrouter-cli`, a
two-route suite in `nullrouter-server` using the engine's `testkit`, a paging test and a Criterion
bench in `nullrouter-engine`.

**Target Platform**: Linux (as the rest of 0router; the operator socket is a Unix socket).

**Project Type**: Rust workspace: CLI plus server library.

**Performance Goals**: newest page of 50 ≤ 20 ms on 100k records in either layout, within 2× of
a 1k-record journal; a page 50,000 records back ≤ 1 s (R7). Every other read stays as fast as
today.

**Constraints**: byte-for-byte output (FR-004); no blocking I/O on the async executor (views run
in `spawn_blocking` in-server); no change to the request path or the journal's write side.

**Scale/Scope**: 12 existing reads moved, 2 new reads, 1 new filter field and op parameter.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle / constraint | Status |
|---|---|
| I. Plugin safety | Pass. No plugin sees anything new; reads only show `…last4` / `env:VAR` (R9). |
| II. Routing fidelity | Pass. The request path is untouched. |
| III. Unified models and provider entities | Pass. `unified` shows them as the registry defines them. |
| IV. Scope discipline | Pass. Scope is the brief's confirmed rows; out-of-scope rows 14–21 untouched. |
| V. Streaming-native SSE | N/A. |
| VI. Reference-informed behaviour | N/A. 9router has no CLI reads to match; the dashboard slice brings the 9router oracle. |
| VII. Trustworthy model tests | N/A. |
| VIII. Latency observability | Pass. Records keep their latency; nothing is dropped from any read. |
| Async runtime: no blocking on the executor | Pass. Views are sync and run in `spawn_blocking` in-server (R2). |
| Performance gate | Pass. The journal's write path is untouched; the read change gets a Criterion bench (R7). |
| No self-registration | Pass. Views are plain functions, called by name. |

**Post-design re-check (after Phase 1)**: no change. One module, one filter field, one op
parameter, two commands.

## Project Structure

### Documentation (this feature)

```text
specs/008-read-model/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── cli.md
│   └── read-model.md
├── checklists/requirements.md
└── tasks.md              # /speckit-tasks
```

### Source Code (repository root)

```text
crates/nullrouter-server/src/
├── views/
│   ├── mod.rs            # View, ViewError, the two route helpers
│   ├── accounts.rs  quota.rs  routing.rs  records.rs  providers.rs  model.rs
│   ├── plugins.rs   keys.rs   check.rs    resolve.rs  unified.rs    behaviour.rs
├── operator.rs           # records.list gains `before`
crates/nullrouter-server/tests/
├── views_routes.rs       # both routes, every view (R5)
└── secrets.rs            # extended (R9)
crates/nullrouter-engine/src/testkit/homes.rs     # fixture homes shared by the CLI and server tests
crates/nullrouter-engine/src/journal/records.rs   # Filter.before, tail reading (R6)
crates/nullrouter-engine/tests/records_page.rs
crates/nullrouter-engine/benches/records_page.rs
crates/nullrouter-cli/src/
├── cmd/*.rs              # each read calls its view and renders text
├── cmd/unified.rs        # new
├── quota_text.rs  routing_text.rs   # text rendering only
crates/nullrouter-cli/tests/
├── read_golden.rs        # R4
└── golden/               # committed before the move
```

**Structure Decision**: one new module in the existing server crate (R1); no new crate.

## Phases for tasks

1. **Goldens** (before anything moves): fixture homes, `read_golden.rs`, generate and commit the
   goldens from unchanged code.
2. **Views move**, one read at a time, each commit keeping the goldens green: `views/mod.rs` and
   the two route helpers, then each read; `extra` cases recorded as found.
3. **Two routes**: `views_routes.rs`; extend the secrets sentinel.
4. **New reads**: `unified`, `behaviour show` (with goldens added in the same commit).
5. **Paging**: `Filter.before`, cursor lookup by ULID day, tail reading, the op parameter, the
   CLI option, `records_page.rs`, the bench and `bench-baseline.md`.
6. **Gates**: review that no read builds its own answer (SC-003); docs (`docs/operator-config.md`:
   `unified`, `behaviour show`, `--before`); clippy and fmt per crate.

## Complexity Tracking

No violations.
