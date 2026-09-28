# Implementation Plan: Client Side, Harness Adapters

**Branch**: `004-client-side-adapters` | **Date**: 2026-09-28 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/004-client-side-adapters/spec.md`

## Summary

Slice 004 adds plugins on the client side. An operator names a harness on an agent key, and
0router runs that harness's adapter on every request and response of the key. The key then
needs nothing more than a base URL.

The approach:
- **Adapters edit, the core applies.** An adapter runs per attempt, on the client-style body,
  before slice 003 forwards or encodes it. It sees only the parts its manifest declares, and it
  returns a list of edits (remove or replace a path, with a reason code). The core applies the
  edits, records each one by path, kind and reason, and never stores the content.
  ([R2](research.md#r2-where-an-adapter-runs-and-what-it-sees))
- **hermes is built in.** It removes echoed reasoning where the target rejects it, and converts
  its images and attachments into content parts every style can carry.
  ([R4](research.md#r4-hermes-built-in))
- **Third-party adapters are sandboxed WASM.**
  - Rust source passes a syntax-level gate: only the kit, and no build scripts, procedural
    macros, `unsafe`, include macros or blobs. ([R7](research.md#r7-validation-gate-for-adapter-source))
  - A separate, optional builder binary compiles the source offline and reproducibly, and binds
    the module to the source by hash. ([R8](research.md#r8-the-builder))
  - wasmtime runs each call in a fresh instance with no imports beyond the kit's, and with
    time and memory limits. ([R3](research.md#r3-sandbox-limits))
- **A rule-based guardrail** re-decodes every changed body or event with slice 003's style
  codecs. Tool calls, definitions, results, opaque blocks and unknown fields may only shrink.
  Anything else discards the edits, marks the adapter suspect, alerts the operator and
  records the attempt. ([R6](research.md#r6-the-guardrail))
- **Review, then the operator decides.**
  - Source is scrambled: comments go and the adapter's own identifiers are renamed.
  - It is reviewed through 0router's own pipeline, on the operator's model and budget, with
    no tools declared.
  - The operator approves or rejects.
  - Versions move through a persisted state machine, and the previous approved version keeps
    serving. ([R10](research.md#r10-review-pipeline), [R11](research.md#r11-store-states-and-hot-apply))
- **Catalogue.** An index file in this repository lists adapter source by URL and SHA-256.
  0router fetches it only on an operator command. ([R12](research.md#r12-catalogue))
- **Claude Code** is the proof adapter. It is written against the kit, listed in the
  catalogue, and ports `normalizeClaudePassthrough`. ([R15](research.md#r15-claude-code-adapter))

## Technical Context

**Language/Version**: Rust 1.93.1, edition 2024. **MSRV rises from 1.85 to 1.93** for
wasmtime 45. Adapters are built with the builder's pinned 1.93.1 toolchain, for
`wasm32-unknown-unknown`. ([R1](research.md#r1-toolchain-and-crates))

**Primary Dependencies**: `wasmtime` 45 (`cranelift`, `async`, `pooling-allocator`; no WASI),
`syn` 2 (`full`, `visit`, `visit-mut`), `prettyplease` 0.2, `flate2`, `tar` and `sha2`, plus
slice 003's set. The kit has only `serde` and `serde_json`. Dev: `criterion`.

**Storage**: Files only, all mode 0600 and written atomically.
- `$ZEROROUTER_HOME/adapters/`:
  - `index.toml`;
  - per version: source tree, `module.wasm`, `build.json`, `review.json`, `decision.json`;
  - `alerts.toml`.
- `keys.toml` gains `harness`.
- `config.toml` gains `[adapters]`.
- The builder has its own home: `$ZEROROUTER_HOME/builder/`, holding the toolchain and the
  vendored kit.

([data-model.md](data-model.md), [contracts/operator-cli.md](contracts/operator-cli.md))

**Testing**: `cargo test`, split into these kinds:
- gate corpus with golden messages;
- hostile-adapter corpus, with checked-in `.wasm` fixtures and builder-built copies when the
  toolchain is present;
- guardrail matrix across request, non-stream and stream, for each client style;
- scrambler identifier audit;
- tamper tests;
- update lifecycle under load;
- hermes engine tests on scripted mocks;
- opt-in live checks: hermes on the four providers, and Claude Code through the full pipeline.

([R16](research.md#r16-test-strategy))

**Target Platform**: Linux. The builder needs a Rust toolchain on the same host. The core
does not.

**Project Type**: The Cargo workspace gains four crates:
- `adapter-kit`: the guest library;
- `sandbox`: the wasmtime host;
- `adapters`: gate, store, guardrail, review, catalogue, hermes;
- `builder`: a binary.

It also gains a non-member adapter source tree, `adapters/community/claude-code/`, and a
catalogue index, `catalogue/index.toml`.

**Performance Goals** (SC-010, starting targets):
- adapter plus guardrail ≤ 5 ms p95 per request, and ≤ 1 ms per stream event;
- no measurable cost for keys without a harness;
- sandbox instance creation around 1 µs with pooling.

([R14](research.md#r14-performance-and-benchmarks))

**Constraints**:
- Adapters never see secrets, agent keys or floor headers.
- No WASI, and no import beyond the kit's.
- A fresh instance per call.
- Epoch deadlines yield to Tokio, so they never block a worker.
- Streams stay unbuffered: the adapter runs per event.
- Records hold paths and reason codes, never content.
- The core never deserialises precompiled native code.
- No catalogue request without an operator command.
- `unsafe` stays forbidden everywhere except the kit's ABI glue (Complexity Tracking).

**Scale/Scope**:
- 1 built-in adapter (hermes) and 1 catalogue adapter (claude-code).
- 4 client styles supported by the guardrail.
- Tens of agents at once.
- Adapter source ≤ 256 KiB; module memory ≤ 64 MiB; ≤ 20 ms per request call.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How this plan complies |
|---|---|---|
| **I. Plugin Safety** | ✅ Pass | See the notes below the table |
| **II. Routing Fidelity** | ✅ N/A | Adapters don't route. They run per attempt on the target the engine picked, and fallback re-runs them for the new target. No cross-agent state: a fresh instance per call |
| **III. Unified Models & Provider Entities** | ✅ Pass | Adapters get the attempt's provider, wire style, model and capabilities. Provider plugins stay data only (schema unchanged) |
| **IV. Scope Discipline** | ✅ Pass | Edits are `remove` or `replace` of a path, with a closed set of reason codes. There is no insert operation. 9router's thinking-placeholder insertion is not ported. Every edit is recorded. hermes converts images instead of deleting them ([R4](research.md#r4-hermes-built-in), [R15](research.md#r15-claude-code-adapter)) |
| **V. Streaming-Native SSE** | ✅ Pass | The response adapter and guardrail run per event inside the relay stream, with no buffering. Epoch deadlines use async yield, and client cancellation drops the call |
| **VI. Reference-Informed Behavior** | ⚠️ Pass with deviations | `normalizeClaudePassthrough`, `dedupeTools`, `paramSupport.js` and the `combo.js` formats are the oracles. Deviations are in [R17](research.md#r17-deliberate-deviations-from-9router-summary) and asserted in `tests/parity/deviations.toml` |
| **VII. Trustworthy Model Tests** | ✅ N/A | No model-test changes |
| **VIII. Latency Observability** | ✅ Pass | Adapter time is part of recorded latency. Records gain the adapter run, so a slow adapter is visible per request |
| Arch: Rust only; adapters Rust → WASM via the builder | ✅ Pass | [R1](research.md#r1-toolchain-and-crates), [R8](research.md#r8-the-builder) |
| Arch: `Box<dyn>` only for WASM adapters | ✅ Pass | The runner is an enum `{Builtin(Hermes), Wasm(Handle)}`. No trait objects are needed |
| Arch: No self-registration | ✅ Pass | Built-ins sit in a static table. Third-party adapters come from the store |
| Arch: Secrets isolation | ✅ Pass | The adapter input is built from the body and the attempt context only. Headers are never passed, and the redactor runs over records |
| Workflow: Benchmark gate | ✅ Pass | [R14](research.md#r14-performance-and-benchmarks) benches, with a committed baseline |
| Workflow: Parity-audit gate | ⚠️ Scheduled | `/rust-parity-audit` on the hermes module and the Claude Code adapter before the PR |

**I. Plugin Safety**:
- **No plugin I/O, by construction.** A third-party adapter has no network, file, environment
  or secret access, because the module's import set is the kit's and nothing else. A module
  with any other import is refused at load.
- **Only reviewed code runs.** The core recomputes the source and module hashes before every
  load and compares them with the approved values.
- **Build and gate:**
  - The builder is a separate, optional binary.
  - Builds run offline and `--locked` against a vendored kit, with `-F unsafe_code`.
  - The gate refuses dependencies other than the kit, build scripts, procedural macros,
    include and environment macros, `unsafe`, `extern` blocks, blobs and oversized code.
- **Review and decision:** the review sees scrambled source only, with no tools. The operator
  decides. Updates never apply automatically.
- **Guardrail:** it runs on every changed request, response and event. A tool change is
  discarded and the adapter is marked suspect.
- **Content limit:** only remove and replace, with the reason recorded (IV).
- Provider plugins are untouched: still data only.

**Post-design re-check**: ✅ No unjustified violations. The kit's `unsafe` ABI glue is the one
exception to the workspace `forbid`. It runs inside the sandbox, never in the core, and is
listed below.

## Project Structure

### Documentation (this feature)

```text
specs/004-client-side-adapters/
├── plan.md              # This file
├── research.md          # Phase 0: R1–R17
├── data-model.md        # Phase 1: entities, validation, state machine
├── quickstart.md        # Phase 1: runnable validation guide
├── contracts/
│   ├── adapter-kit.md         # guest ABI, edit model, selectors, reason codes, limits
│   ├── adapter-package.md     # package layout, adapter.toml, gate rules and messages
│   ├── catalogue.md           # index format, fetch and verify rules
│   └── operator-cli.md        # commands, socket ops, files, records and alerts additions
├── bench-baseline.md    # committed Criterion summary (implement phase)
├── checklists/
│   └── requirements.md
└── tasks.md             # Phase 2 (/speckit-tasks, not created here)
```

### Source Code (repository root)

```text
Cargo.toml                          # members gain 4 crates; rust-version 1.93;
                                    #   exclude = ["adapters"]
crates/
├── zerorouter-adapter-kit/         # new; guest library, compiled into every adapter
│   └── src/
│       ├── lib.rs                  # KIT_ABI, Adapter trait, export! macro_rules
│       ├── edit.rs                 # Edit, Kind, Reason (closed), Path
│       ├── context.rs              # AttemptContext, Direction
│       └── abi.rs                  # zr_alloc, input/output framing (the only unsafe)
├── zerorouter-sandbox/             # new; wasmtime host, no adapter logic
│   ├── src/
│   │   ├── engine.rs               # Engine config, pooling, epoch ticker
│   │   ├── module.rs               # load: hash check, import/export check, InstancePre
│   │   ├── call.rs                 # per-call instance, limits, async yield, output decode
│   │   └── abi.rs                  # host side of the kit ABI, prior-ABI shim
│   └── benches/sandbox.rs
├── zerorouter-adapters/            # new
│   ├── src/
│   │   ├── runner.rs               # AdapterRunner enum, per-attempt and per-event entry
│   │   ├── selector.rs             # selector parse, subtree extraction
│   │   ├── apply.rs                # edit checks (R5), apply to copy
│   │   ├── guard.rs                # guardrail (R6), request / response / event
│   │   ├── builtin/hermes.rs       # hermes, reject table
│   │   ├── gate.rs                 # source gate (R7)
│   │   ├── scramble.rs             # comment strip, identifier rename
│   │   ├── review.rs               # budget check, review request, report parse
│   │   ├── store.rs                # adapters/ tree, index.toml, state machine
│   │   ├── builder_client.rs       # spawn builder, JSON job/result
│   │   ├── catalogue.rs            # fetch, verify, safe unpack
│   │   └── alerts.rs               # alerts.toml
│   ├── tests/                      # gate/invalid/, hostile/ (sources + checked-in .wasm),
│   │                               #   guard/ (per style), fixtures/, scramble, tamper, store
│   └── benches/adapters.rs
├── zerorouter-builder/             # new binary; never linked into the core
│   └── src/main.rs                 # setup (embedded kit .crate + Cargo.lock → local
│                                   #   registry); build: job in, double build, result out
├── zerorouter-engine/              # extended
│   └── src/                        # keys.rs +harness; records.rs +AdapterRun;
│                                   #   attempt.rs calls the runner; state.rs +AdapterIndex
├── zerorouter-server/              # extended
│   └── src/                        # relay.rs per-event response hook; operator.rs +ops
└── zerorouter-cli/                 # extended
    └── src/cmd/                    # + adapters, alerts, catalogue; keys --harness
adapters/
└── community/
    ├── .cargo/config.toml          # patches the kit to the workspace path for host tests;
    │                               #   outside every package, never read by the builder
    └── claude-code/                # the proof adapter (Cargo.toml, adapter.toml, src/)
catalogue/
└── index.toml                      # the catalogue
tests/
├── parity/deviations.toml          # + slice 004 deviations
└── harness/                        # + hermes scripts, Claude Code with adapter
```

**Structure Decision**:
- **`sandbox` is split from `adapters`.** wasmtime's build cost and API surface sit in one
  crate, and the gate, guardrail and hermes logic are tested without it.
- **The builder is its own binary.** The core stays free of any compiler dependency
  (FR-013), and an operator who installs no adapter never needs it.
- **The kit is a workspace member,** so it is tested on the host. It is not published to
  crates.io in this slice: the builder embeds the packaged kit and serves it from a local
  registry ([R8](research.md#r8-the-builder), Kit source).
- **Test corpora live in the crate that tests them** (`crates/zerorouter-adapters/tests/`),
  following slices 002 and 003.
- **`adapters/` sits outside the workspace (`exclude`).** The gate refuses a `[workspace]`
  table in an adapter's `Cargo.toml`, so the Claude Code adapter must be buildable exactly as
  a third party's.

## Complexity Tracking

| Choice | Why | Simpler alternative rejected because |
|---|---|---|
| `zerorouter-adapter-kit` allows `unsafe` (overrides the workspace `forbid`) | The WASM ABI needs `#[no_mangle]` exports and raw pointer reads for input and output | No safe way exists to export a core-module function. The code runs only inside the sandbox, and adapters themselves still build with `-F unsafe_code` (the lint doesn't fire on expansions of an external `macro_rules!`; a builder test asserts it) |
| MSRV 1.85 → 1.93 | wasmtime 45 is the newest release the installed toolchain builds | wasmi is 5–20× slower and risks SC-010. Older wasmtime versions lose security fixes |
| Four new crates | The builder must be separate. The kit is a guest library. The sandbox keeps wasmtime out of the logic crate | Folding them in would link a compiler path or wasmtime into crates that don't need it |
| The previous kit ABI stays supported | Most upgrades then need no rebuild, so no plain-client window | Current ABI only would force a rebuild, and a plain-client window, on every kit change |
| Thinking placeholder not inserted (VI deviation) | IV: adapters remove or convert, never add | Inserting would need an "insert" edit, which the guardrail's shrink-only rule can't police |
| The Claude Code adapter also strips server-tool blocks for targets other than Anthropic (VI deviation) | Without this, slice 003 skips those targets, and US5-2 fails | 9router never meets the case, because it only normalizes on Anthropic passthrough |
