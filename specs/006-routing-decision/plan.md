# Implementation Plan: Routing Decision and Persistent Request History

**Branch**: `006-routing-decision` | **Date**: 2026-10-03 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/006-routing-decision/spec.md`

## Summary

Slice 006 replaces slice 003's ordered fallback list with 0router's routing decision, and makes
request records durable. For each request, 0router:
1. looks up the agent's longest warm prompt prefix among the target's accounts and stays there
   unless that account can't serve;
2. otherwise places the request as cold work, on the eligible subscription account with the
   largest deficit against its pace-weighted share;
3. overflows to pay-as-you-go accounts, by priority ÷ current price, only when no subscription
   can serve.

Every decision, its candidate table and every attempt go to an append-only journal on disk that
survives crashes. Warm fingerprints and deficits persist the same way.

The approach:
- **A pure decision core.** `nullrouter-engine/src/routing/` takes a state snapshot and `now`
  and returns a `Placement` (warm hit, tiered attempt order, candidate table). It does no I/O, so
  the simulated week drives it at full speed ([R2](research.md#r2-where-the-work-lives),
  [R17](research.md#r17-the-simulated-week)).
- **Salted prefix-chain fingerprints.** One 128-bit hash per message boundary over the IR, keyed
  by model, written according to the provider's declared cache mode, and expired after its
  lifetime ([R3](research.md#r3-prompt-prefix-fingerprints)).
- **Quota as present state.** Slice 005's polls, minus 0router's metered tally since the poll,
  plus plugin-declared meters (length, capacity, weights, reserve) give each window's π and r.
  Estimated accounts run on declared limits alone ([R5](research.md#r5-quota-windows-for-routing),
  [R6](research.md#r6-quota-between-polls-user-visible),
  [R7](research.md#r7-pace-rate-and-weight)).
- **Deficits per target.** Only cold work moves them. They're debited tentatively at placement,
  settled at attempt end, and reset at epoch-aligned 5-hour windows
  ([R8](research.md#r8-deficits-and-cold-placement-user-visible)).
- **One order per request.** Warm, then the subscription tier, then pay-as-you-go, then last
  resort. Retry and fallback walk that order ([R9](research.md#r9-tiers-and-the-attempt-order-fr-025-fr-039-user-visible)).
- **A journal with acked open and close.** A dedicated writer thread appends to daily JSONL
  segments. `close` is acked before the client's last byte, `fdatasync` runs every second,
  recovery marks interrupted records, and a full disk degrades to serving without recording
  ([R11](research.md#r11-persistent-request-records-user-visible),
  [R12](research.md#r12-persistent-routing-state)).
- **Operator surfaces.** Priority and overrides in `accounts.toml`, amortization in
  `config.toml`, `nullrouter routing` (view, set, window), `records` reading disk with `prune`
  and `forget`, and the unified-model limits note in `check`
  ([R13](research.md#r13-operator-settings-user-visible),
  [R14](research.md#r14-unified-model-limits-note-user-visible),
  [R15](research.md#r15-the-placement-in-the-record-user-visible)).

## Technical Context

**Language/Version**: Rust 1.93.1, edition 2024 (MSRV 1.89, unchanged).

**Primary Dependencies**: no new crates. `sha2`, `getrandom`, `ulid`, `serde_json`, `tokio`,
`arc-swap` from earlier slices ([R1](research.md#r1-dependencies)).

**Storage**: files in `$NULLROUTER_HOME`, all 0600 with 0700 directories:
- `records/YYYY-MM-DD.jsonl` and `records.lock`;
- `routing/warm.jsonl`, `routing/ledger.jsonl` and `routing/salt`;
- new keys in `accounts.toml` and `config.toml`.

Slice 005's quota history is unchanged and read for the estimate
([contracts/record-journal.md](contracts/record-journal.md)).

**Testing**:
- `cargo test` with mock providers that report cache reads and quota;
- a seeded simulated week on the decision core and the real journals;
- crash tests with a killed child process, and fault injection for a full disk;
- the secret and prompt sentinel;
- the Python SDK, Claude Code and Node harnesses;
- opt-in live check L7.

See [R17](research.md#r17-the-simulated-week) and [R18](research.md#r18-test-strategy).

**Target Platform**: Linux (and macOS); a single server process per `$NULLROUTER_HOME`.

**Project Type**: extends the existing Cargo workspace; no new crates.

**Performance Goals**: routing plus recording add ≤ 5 ms at p95 per request over slice 005
(SC-013). The decision is one lock and one sort over a few candidates, and fingerprinting is one
hash pass over the IR ([R19](research.md#r19-performance)).

**Constraints**:
- No prompt content stored; fingerprints are salted hashes.
- No secret in journals, routing files, CLI output or anything a plugin sees.
- The client's request is forwarded unchanged; fingerprinting only reads it.
- No blocking I/O on the async executor (dedicated writer thread).
- No `unsafe`.
- A 0router crash loses no record of a finished request; a power loss loses at most about 1 s.

**Scale/Scope**: 7 bundled providers; tens of accounts; targets with 2–32 candidate accounts;
tens of thousands of requests a day (about 1 KB of journal per request); 12 agents in the
simulation.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How this plan complies |
|---|---|---|
| **I. Plugin Safety** | ✅ Pass | `[routing]` is closed data: durations, numbers, enums, globs, with no URL, header or secret ([routing-schema.md](contracts/routing-schema.md)). The core does every computation and send. Plugins never see records, fingerprints or tallies |
| **II. Routing Fidelity** | ✅ Pass | All four properties ship: cache-aware (R3, R4), per-agent isolation (fingerprints keyed by agent, FR-007), windowed amortization over a configurable window (R8), and warm before amortization (R9 order). Plugins declare, the operator overrides (R13, R16) |
| **III. Unified Models & Provider Entities** | ✅ Pass | Unified and direct targets go through one decision (FR-002). Every model type uses the same rules (FR-005; non-text requests are cold). Combos stay out of scope (brief) |
| **IV. Scope Discipline** | ✅ Pass | Fingerprinting only reads the IR. No cache markers are added, moved or removed (FR-011). Forwarding is unchanged |
| **V. Streaming-Native SSE** | ✅ Pass | The relay is unchanged. Before the final event, the stream awaits the `close` ack (a channel round trip after a page-cache write). That is a bounded wait on the last event, not buffering; the bench measures it |
| **VI. Reference-Informed Behavior** | ⚠️ Pass with deviations | Classification, cooldowns and retry are unchanged. The account order deviates from 9router's fill-first and round-robin by design (R20, D-006-1 to D-006-3 in `tests/parity/deviations.toml`) |
| **VII. Trustworthy Model Tests** | ✅ N/A | No model tests in this slice |
| **VIII. Latency Observability** | ⚠️ Phased | TTFT and total per attempt are now durable and shown in `records`. Feeding latency into the decision is automatic responsiveness, out of scope by the user's decision (spec Out of Scope); the dashboard comes later |
| Arch: Tokio rules | ✅ Pass | All file I/O on the writer thread. Requests only send to a channel and await acks |
| Arch: Secrets isolation | ✅ Pass | Records keep slice 005's redaction. Fingerprints are salted hashes. The sentinel extends to the new files |
| Arch: Performance gate | ✅ Pass | New Criterion cases `route/*`, `journal/*`, `ttfb/routed` (R19) |
| Workflow: Parity-audit gate | ⚠️ Scheduled | `/rust-parity-audit` on `attempt.rs` and `plan.rs` (the retry and fallback path inherited from 9router) before the PR. `routing/` has no 9router counterpart |

**Post-design re-check**: ✅ No unjustified violations. The VI deviations are the slice's purpose
(brief row 7: no parity target for the placement rule). VIII stays phased because the user
deferred responsiveness.

## Project Structure

### Documentation (this feature)

```text
specs/006-routing-decision/
├── plan.md              # This file
├── research.md          # Phase 0: R1–R20
├── data-model.md        # Phase 1: accounts, routing declarations, windows, fingerprints, ledger, decision, journal
├── quickstart.md        # Phase 1: validation guide
├── contracts/
│   ├── routing-schema.md    # plugin [routing], account overrides
│   ├── record-journal.md    # records/*.jsonl, routing/*.jsonl, durability
│   └── operator-cli.md      # settings, commands, routing view, records, socket ops
├── checklists/
│   └── requirements.md
└── tasks.md             # Phase 2 (/speckit-tasks)
```

### Source Code (repository root)

```text
crates/
├── nullrouter-registry/src/
│   ├── schema/routing.rs     # new: [routing] cache, window meters, price schedule
│   ├── schema/config.rs      # [routing] amortization, amortization_for
│   ├── schema/style.rs       # optional key: where a cache marker's TTL sits (style data)
│   ├── validate/style_gate.rs # its rule
│   ├── validate/gate.rs      # routing rules (contracts/routing-schema.md)
│   └── load.rs               # unified-model limits note in the LoadReport
├── nullrouter-engine/src/
│   ├── routing/              # new, pure: mod.rs, fingerprint.rs, warm.rs, meter.rs, pace.rs,
│   │                         #   ledger.rs, price.rs, place.rs (Placement, tiers, candidate table)
│   ├── journal/              # new: writer.rs (thread, acks, fdatasync, health), records.rs
│   │                         #   (segments, fold, recovery, prune, forget), state.rs (warm, ledger,
│   │                         #   compaction, load)
│   ├── accounts.rs           # priority, [account.routing] overrides
│   ├── plan.rs               # candidate discovery only; ordering moves to routing::place
│   ├── attempt.rs            # walk the Placement; placement reasons; settle debits; warm writes;
│   │                         #   open and close journal lines
│   ├── records.rs            # decision, placement, interrupted; ring feeds the journal
│   ├── quota/                # estimate hook: tally since poll, recovery from the journal
│   └── state.rs              # Engine gains Router (routing state) and Journal; WarmMap removed
├── nullrouter-server/src/
│   ├── operator.rs           # routing.view, routing.health, records.forget; records ops read
│   │                         #   the journal
│   ├── relay.rs, text.rs     # hold the client's last byte until the close ack
│   └── serve.rs              # start and stop the writer; recovery before listening
└── nullrouter-cli/src/cmd/
    ├── routing.rs            # new: view, set, unset, window
    ├── accounts.rs           # priority
    ├── records.rs            # offline read, filters, prune, forget
    └── check.rs              # limits notes, routing warnings, journal health
plugins/bundled/              # [routing] for all seven (R16)
styles/bundled/anthropic-messages.toml  # cache-marker TTL location
tests/
├── parity/deviations.toml    # D-006-1..3
└── harness/                  # warm, cold and overflow scenarios for the SDK and Claude Code
crates/nullrouter-engine/tests/
    sim_week.rs, routing_warm.rs, routing_cold.rs, routing_overflow.rs, routing_state.rs,
    records_journal.rs, estimate.rs
docs/operator-config.md       # routing settings, routing view, records, live check L7
docs/plugins.md               # [routing] section for plugin authors
```

**Structure Decision**: no new crate. The decision core is a module inside the engine, kept pure
(no I/O, clock passed in), so it can be simulated and unit tested in isolation without a crate
boundary. The journal lives in the engine because the attempt loop writes it and the CLI reads it
through engine functions, the same split slice 005 used for quota history.

## Complexity Tracking

| Choice | Why | Simpler alternative rejected because |
|---|---|---|
| Acking `close` before the client's last byte | Clarifications Q1: a 0router crash loses nothing | Writing the record afterwards loses it if 0router crashes between the answer and the write; an fsync per request is far too slow |
| A dedicated writer thread for all journals | Tokio rule (no blocking I/O on the executor); one FIFO makes the `close` ack cover warm and ledger lines | `spawn_blocking` per write loses ordering between files and costs a pool hop per line |
| Fingerprint chain per message boundary, by cache mode | Longest-prefix warmth without storing content; no warmth counted where the provider caches nothing | Hashing only system and tools misses most cached tokens; raw-byte hashing breaks across client styles |
| Plugin-declared window meters (length, capacity, weights) | Polls lack window lengths and token sizes; pace and cross-provider rates need both | Inferring lengths from resets fails for `first_use` windows; requiring capacities blocks providers that don't publish them |
| Tentative debit with settlement | Concurrent cold requests must not all pick the same largest deficit (spec edge case) | Debiting only at completion lets bursts pile onto one account |
| Last-resort tier for floor-held accounts | FR-025a and FR-039: don't fail a client while an account could serve | Strict floors would turn a busy hour into client errors |
| Daily JSONL segments instead of a database | Same pattern as slice 005's history; prune is a file deletion | SQLite adds a C dependency and a second durability model |
