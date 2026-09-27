# Implementation Plan: Request Execution Walking Skeleton

**Branch**: `003-request-execution` | **Date**: 2026-09-27 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/003-request-execution/spec.md`

## Summary

This is the first end-to-end slice. It adds a new async crate, `zerorouter-server`, on
top of the 002 registry.

**Request path**

1. An axum listener serves `/v1/chat/completions`, `/v1/messages`,
   `/v1/messages/count_tokens`, `/v1/embeddings`, and `/v1/models`.
2. The request is authenticated with an operator access key, which names the agent.
3. The client's session id is read as 9router reads it.
4. The target is resolved through a registry snapshot.
5. A placeholder picks the first unified-model member with an active connection. It is
   pure and isolated.
6. The outbound request is built exactly as 9router's generic executor builds it:
   - the URL, headers, auth, version, and accept headers are checked against new oracle
     fixtures;
   - the body keeps the client's bytes except `model` (and `stream` for forced-stream
     providers).
7. The response is relayed one SSE event at a time, byte for byte, with 9router's
   connect and stall timeouts and `CancellationToken`-based client-disconnect
   cancellation.

**Operator side**

- Accounts and access keys live in a new operator file, `keys.toml`. It swaps
  atomically with the registry on reload.
- Every request leaves one in-memory observation: identity, target, outcome, TTFT,
  duration, usage split into cache read/write, and upstream headers.
- The operator queries observations and triggers reloads through `zerorouter-cli` over
  an owner-only Unix socket.

**Scope**: 45 chat providers and 10 embeddings providers are executable at the pin, 47
distinct providers in total
([R2](research.md#r2-which-providers-are-executable-fr-013)).

## Technical Context

**Language/Version**: Rust stable, edition 2024 (MSRV 1.85; toolchain 1.93.1 via
`CARGO_HOME=$PWD/.cargo-home`). Node ≥ 22 is used only by the dev-time generator.

**Primary Dependencies**: new ones are listed below
([R1](research.md#r1-crates-and-workspace-layout)).
- `tokio` 1.53 and `tokio-util` 0.7 (`CancellationToken`).
- `axum` 0.8, `reqwest` 0.13 (rustls, stream), `futures-util`, `bytes`, `memchr`.
- `sha2` 0.11.
- `tracing` and `tracing-subscriber`.
- `serde_json` gains the `raw_value` feature.

Existing: `zerorouter-registry`, `arc-swap`, `indexmap`, `serde`, `toml`, `clap`,
`criterion`.

**Storage**: files only. `$ZEROROUTER_HOME/keys.toml` is new operator state, 0600 when
it holds literals ([R4](research.md#r4-accounts-and-access-keys-file-secrets-validation-fr-004a-fr-006fr-010)).
Observations are in memory only (a bounded ring). The operator socket is
`$ZEROROUTER_HOME/run/operator.sock`.

**Testing**: `cargo test` ([R16](research.md#r16-test-strategy)):
- unit tests;
- `parity`: 13 new generated fixtures ([R14](research.md#r14-parity-oracle-extension));
- `e2e`: in-process server plus scripted in-process mock upstreams, covering the
  `cancel`, `concurrency`, `secrets`, and `reload` groups;
- a Criterion bench, `relay`.

**Target Platform**: Linux (the operator channel is a Unix domain socket; peer-uid check).

**Project Type**: a library crate (`zerorouter-server`) plus subcommands in the existing
CLI binary. Cargo workspace.

**Performance Goals**:
- under 5 ms median added TTFT against a local mock (SC-003); the expected overhead is
  under 50 µs of CPU;
- each event forwarded without waiting for the next;
- upstream closed within 1 s of client disconnect (SC-004);
- 100 concurrent streams on one connection, none serialized (SC-007).

([R17](research.md#r17-performance))

**Constraints**:
- No buffering beyond one SSE event.
- No `unsafe`.
- No secret in any `Debug`, `Serialize`, log, observation, or error.
- Blocking file I/O (load and reload) runs in `spawn_blocking`.
- The request body is unchanged except `model` and `stream`.
- No retries and no fallback.

**Scale/Scope**:
- a single operator;
- tens of connections and access keys;
- up to 10 000 observations in memory (configurable);
- 47 connectable providers.

**Registry changes (additive)**:
- `Registry::load_candidate(&OperatorHome) -> Result<Registry, ReloadError>`, which
  builds without swapping ([R10](research.md#r10-reload-and-snapshots));
- `Serialize` on `LoadReport` for the operator channel;
- no schema change.

**Generator changes**: an `execution` phase with a Node resolve hook for the `@/` alias
and module stubs ([R14](research.md#r14-parity-oracle-extension)).

## Constitution Check

*GATE: must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How this plan complies |
|---|---|---|
| **I. Plugin Safety** | ✅ Pass | Secrets live only in operator-owned `keys.toml` and are injected by `outbound` at build time. Plugins stay data: executability is decided by the core (static specialized-executor table plus declared category and auth), and no plugin field grants it. `Secret` has no `Display` or `Serialize`, and the auth `HeaderValue` is marked sensitive. SC-006 is scanned by test |
| **II. Routing Fidelity** | ✅ Pass (groundwork) | No routing decision is made. The placeholder is isolated in `select.rs` (FR-011). Per-agent identity (agent, session) and cache-read/write usage are recorded for the routing slice. Observations keep upstream `retry-after` and rate-limit headers |
| **III. Unified Models & Provider Entities** | ✅ Pass | Unified models are first-class targets. Embeddings are executed alongside chat, so the path is not chat-only. `/v1/models` lists model kinds |
| **IV. Scope Discipline** | ✅ Pass | Standard endpoints only. The body is byte-preserved except `model` and `stream` (stricter than 9router, R15 D8). None of 9router's prompt adjustments are applied (R15 D6) |
| **V. Streaming-Native SSE** | ⚠️ Pass with a justified deviation | No buffering, `StreamExt` composition, `CancellationToken`, and a `CancelOnDrop` guard. The response uses `Body::from_stream`, **not** `axum::response::sse::Sse`, because `Sse` re-serializes events and would break FR-018 and SC-008 byte fidelity. See Complexity Tracking |
| **VI. Reference-Informed Behavior** | ✅ Pass | URL, header, transport choice, session, client detection, errors, non-SSE handling, stream error frames, usage, embeddings, SSE-to-JSON assembly, and env timeouts are all checked against generated 9router fixtures. Twelve deliberate deviations are listed with tests (R15). D1 (504 vs 9router's 502) is flagged for the user |
| **VII. Trustworthy Model Tests** | ✅ N/A | Model tests are out of scope. No model is marked broken by this slice |
| **VIII. Latency Observability** | ✅ Pass | TTFT and total duration are recorded for every request, with p50/p95 summaries in the CLI. Recording is off the relay path (FR-024) |
| Arch: Rust only, Tokio rules | ✅ Pass | Tokio multi-thread runtime. Registry and keys builds run in `spawn_blocking`. No blocking I/O on request tasks |
| Arch: No self-registration | ✅ Pass | Static tables: the specialized-executor list, native pairs, and error types |
| Arch: Bundled provider secrets | ✅ Pass | Untouched. No provider secret is bundled; operator keys come from `keys.toml` |
| Workflow: Benchmark gate | ✅ Pass | `benches/relay.rs` sets the baseline for the streaming and execution hot path |
| Workflow: Parity-audit gate | ⚠️ Scheduled | `/rust-parity-audit` on `outbound.rs`, `relay.rs`, and `session.rs` before the PR (task) |

**Post-design re-check (after Phase 1)**: ✅ No unjustified violations.

- The data model has no field that holds a secret outside `Secret`.
- The operator channel is not reachable from the TCP listener.
- Every 9router-derived rule in the contracts names its oracle fixture.
- The single Principle V deviation is recorded below with a proposed amendment.

## Project Structure

### Documentation (this feature)

```text
specs/003-request-execution/
├── plan.md              # This file
├── research.md          # Phase 0: R1–R18, incl. R15 deviations
├── data-model.md        # Phase 1: entities, outcomes, lifecycle and reload state machines
├── quickstart.md        # Phase 1: runnable validation guide
├── contracts/
│   ├── http-api.md          # Client endpoints, auth, rejection order, error envelopes
│   ├── keys-file.md         # keys.toml format and rejection rules
│   ├── operator-cli.md      # serve / reload / obs + operator socket protocol
│   └── outbound-parity.md   # URL/header/body rules ↔ oracle fixtures
├── checklists/
│   └── requirements.md
└── tasks.md             # Phase 2 (/speckit-tasks, not created here)
```

### Source Code (repository root)

```text
Cargo.toml                              # + tokio, tokio-util, axum, reqwest, futures-util,
                                        #   bytes, memchr, sha2, tracing(-subscriber);
                                        #   serde_json +raw_value
crates/
├── zerorouter-registry/
│   └── src/lib.rs, load.rs             # + Registry::load_candidate; LoadReport: Serialize
├── zerorouter-server/                  # NEW
│   ├── Cargo.toml
│   ├── src/
│   │   ├── lib.rs                      # Server { state: ArcSwap<State>, obs, http client }
│   │   ├── state.rs                    # State, load/reload (spawn_blocking, atomic pair swap)
│   │   ├── keys/                       # keys.toml: schema.rs, load.rs (spans, 0600 rule),
│   │   │                               #   secret.rs (Secret), executable.rs (R2 rule +
│   │   │                               #   SPECIALIZED table)
│   │   ├── auth.rs                     # access-key extraction + digest lookup
│   │   ├── session.rs                  # FR-005a carrier extraction (9router order)
│   │   ├── client_detect.rs            # detectClientTool + NATIVE_PAIRS
│   │   ├── select.rs                   # placeholder selection → ExecutionTarget (pure)
│   │   ├── transport.rs                # R3 transport/target-format choice, EffectiveTransport
│   │   ├── outbound.rs                 # URL, headers (incl. native overlay), body rewrite
│   │   ├── upstream.rs                 # reqwest send with connect timeout + cancellation
│   │   ├── relay.rs                    # SSE event framer, stall timeout, CancelOnDrop,
│   │   │                               #   closing error frame
│   │   ├── assemble.rs                 # forced-stream SSE → JSON (parseSSEToOpenAIResponse)
│   │   ├── usage.rs                    # extractUsage/mergeUsage → Usage
│   │   ├── errors.rs                   # 0router envelopes, 9router upstream-error and
│   │   │                               #   non-SSE formatting, ERROR_TYPES
│   │   ├── count_tokens.rs             # estimateAnthropicInputTokens
│   │   ├── embeddings.rs               # OpenAI-compatible adapter build
│   │   ├── models.rs                   # /v1/models listing
│   │   ├── observe.rs                  # Observation, builder, store, filter, summary
│   │   ├── http.rs                     # axum router + handlers (client surface only)
│   │   ├── operator.rs                 # Unix socket NDJSON server (peer-uid check)
│   │   └── timeouts.rs                 # envMs parsing, defaults
│   ├── tests/
│   │   ├── parity/                     # main.rs + one module per R14 fixture
│   │   └── e2e/                        # main.rs, mock.rs (scripted upstream), us1…us5.rs,
│   │                                   #   cancel.rs, concurrency.rs, secrets.rs, reload.rs
│   └── benches/
│       └── relay.rs
└── zerorouter-cli/
    └── src/cmd/                        # + serve.rs, reload.rs, obs.rs; check.rs + keys summary
docs/
├── operator-config.md                  # + keys.toml, serve, reload, obs
└── clients.md                          # NEW: pointing Claude Code / OpenAI SDK at 0router
tools/gen-bundled/
├── generate.mjs                        # + execution phase
└── resolve-hook.mjs                    # NEW: @/ alias + stub resolution for ref imports
tests/fixtures/9router/                 # + 13 GENERATED execution fixtures (R14)
```

**Structure Decision**

- There is one new sibling crate, as 002 anticipated. The registry stays sync and
  runtime-free, and the CLI's offline commands do not start a runtime.
- `select.rs` is the only module the routing slice replaces. `transport`, `outbound`,
  `upstream`, and `relay` are the stable execution path.
- The server does not use `RegistryHandle`. It owns the combined `ArcSwap<State>`, so
  the registry and keys swap as one unit (FR-010).
- New fixtures sit next to 002's under `tests/fixtures/9router/`, and the generator
  remains their only writer.

## Complexity Tracking

| Violation / choice | Why needed | Simpler alternative rejected because |
|---|---|---|
| **Constitution V**: response body is `Body::from_stream(impl Stream<Item = Bytes>)`, not `axum::response::sse::Sse<impl Stream>` | FR-018 requires events relayed "without altering event content", and SC-008 requires byte-identical native bodies. `Sse` re-serializes every `Event` (normalizing `data:` spacing, splitting and joining lines, field order). The principle's substance is kept: no buffering, `StreamExt` adaptors, `CancellationToken`, and a `CancelOnDrop` guard | `Sse` with re-built events alters bytes and adds an allocation per event. **Proposed PATCH amendment to V** (for the user to decide; not applied by this plan): "The required pattern is an `impl Stream` composed with `futures::StreamExt` and returned as an axum streaming body (`Sse` when 0router originates the events, `Body::from_stream` when relaying upstream bytes)." |
| New operator file `keys.toml` rather than extending `config.toml` | It keeps secrets out of the shareable config, the 0600 rule applies only where literals live, and the registry's strict schema is unchanged | Putting secrets in `config.toml` forces 0600 on all config and mixes secret and non-secret state |
| Unix-socket operator channel | FR-028 requires it to be structurally unreachable from clients, with no extra secret | An admin route on TCP is reachable in principle; SIGHUP cannot return reports or errors |
| Node resolve hook in the generator | 9router modules import via the Next `@/` alias and DB and logging modules, and `ref/9router` has no `node_modules` | `npm install` in `ref/` would mutate the read-only oracle and require network access; hand-written fixtures are forbidden by the spec |
