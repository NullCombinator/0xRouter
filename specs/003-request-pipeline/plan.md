# Implementation Plan: Request Pipeline

**Branch**: `003-request-pipeline` | **Date**: 2026-09-27 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/003-request-pipeline/spec.md`

## Summary

Slice 003 is the first slice a client can use end to end. `nullrouter serve` listens on
`127.0.0.1:20129` and accepts four client API styles: OpenAI Chat Completions, Anthropic
Messages, OpenAI Responses and Gemini generateContent. It serves them from the operator's
API-key accounts on five bundled providers: anthropic, openrouter, opencode-zen,
opencode-go and elevenlabs.

The approach:
- **Styles are data.** Each style is a TOML file (`styles/bundled/`) read by a pure
  interpreter crate, `nullrouter-wire`. The file gives routes, key and session carriers,
  codecs for each model type, stream grammar and error shape. Every stateful algorithm is a
  named core primitive chosen from a closed set. Any client style reaches any provider
  through one intermediate representation. ([R3](research.md#r3-client-api-styles-as-data))
- **Providers are data.** Plugin schema 2 declares, per model type:
  - endpoints;
  - in-band error placement;
  - token counting;
  - continuation support;
  - session headers;
  - forwarding, under a core security floor.

  ([R17](research.md#r17-provider-schema-2-and-the-chosen-five),
  [R18](research.md#r18-forwarding-and-the-security-floor))
- **Failures stay inside 0router.** `nullrouter-engine` classifies each failure with
  9router's rules. It retries the same account first (stay warm), then other accounts, then
  other members of the unified model. Once output has reached the client, a stream break
  continues where the provider declares support. Otherwise the operator's choice applies:
  restart with a visible note (default) or an error event.
  ([R6](research.md#r6-error-classification)–[R9](research.md#r9-mid-stream-breaks-continuation-restart-error-event))
- **Everything is recorded.** Each request produces an in-memory record with:
  - agent;
  - attempts and reasons;
  - TTFT and total duration;
  - usage, including cache-read and cache-write tokens.

  Records are queried from the CLI through an operator socket.
  ([R13](research.md#r13-usage-and-records), [R21](research.md#r21-records-store))
- **The bundle shrinks to five.** The other 116 providers move to an embedded community set
  that the operator can install. Each one loads whole or is refused whole with a "not
  supported by this core" message. ([R19](research.md#r19-fit-or-refuse-and-the-community-set))

## Technical Context

**Language/Version**: Rust 1.93.1, edition 2024 (MSRV 1.85). Node ≥ 22 only for the
dev-time generator and the SDK harness tests.

**Primary Dependencies**: `tokio`, `tokio-util`, `axum` 0.8, `reqwest` 0.13 (rustls,
HTTP/2, multipart), `futures-util`, `bytes`, `memchr`, `serde_json` (`raw_value`), `sha2`,
`base64`, `getrandom`, `aho-corasick` and `tracing`, plus slice 002's set. Dev: `criterion`.
([R1](research.md#r1-toolchain-and-crates))

**Storage**: Files and memory only.
- `$NULLROUTER_HOME/accounts.toml` and `keys.toml`, both mode 0600.
- `config.toml`, extended with `[server]`, `[pipeline]` and `allow_private_endpoints`.
- Installed community plugins in `plugins/`.
- Records, the stay-warm map, cooldowns and video jobs live in memory.
- The operator socket is `run/operator.sock`.

([R20](research.md#r20-operator-state-and-hot-apply),
[contracts/operator-cli.md](contracts/operator-cli.md))

**Testing**: `cargo test`, split into these kinds:
- `wire` unit and parity tests, against 9router translator fixtures plus deviation
  assertions;
- engine tests with in-process scripted mock upstreams;
- server tests end to end on loopback;
- the harness matrix: official SDKs in Python and Node, Claude Code and Codex CLI;
- gate and fit corpora;
- the secrets sentinel;
- opt-in live checks with operator keys, never in CI.

([R25](research.md#r25-test-strategy))

**Target Platform**: Linux (developer machine and small server); nothing platform-specific
beyond a Unix socket for the operator.

**Project Type**: Cargo workspace with three new crates beside slice 002's two:
- library crates: `registry`, `wire`, `engine`;
- server: `server`;
- CLI binary: `cli`.

([R2](research.md#r2-crate-layout))

**Performance Goals**:
- 0router's added time to first byte: p95 ≤ 10 ms (SC-013). Expected to be well under 1 ms
  with an instant mock.
- Recorded TTFT and total within 10 ms of the client's measurement (SC-005).
- A client disconnect stops the upstream request within 1 s (SC-010).
- One upstream connection is reused across sequential requests within the keep-alive window
  (SC-011).

([R24](research.md#r24-performance-and-benchmarks))

**Constraints**:
- Streams are relayed without buffering; `Sse<impl Stream>` is used for every SSE response.
- A `CancellationToken` and a `CancelOnDrop` guard cancel the upstream request on client
  disconnect.
- Prompt content is never rewritten (R4).
- No `unsafe`.
- No secret reaches a plugin-visible type, a log line, a record or an error body. The
  redactor ([R23](research.md#r23-secret-redaction)) enforces this.
- Blocking file I/O runs in `spawn_blocking`.

**Scale/Scope**:
- 4 styles, 5 bundled providers and 116 community plugins.
- 6 model types.
- 10,000 records in memory.
- One process for a single operator; tens of agents at once.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How this plan complies |
|---|---|---|
| **I. Plugin Safety** | ✅ Pass | See the notes below the table |
| **II. Routing Fidelity** | ✅ Phased, not blocked | The routing decision is slice 006 (the brief's 005, renumbered by user decision on 2026-09-28). This slice keeps the one property it needs, a warm-cache preference keyed per agent (key + session), in the `WarmMap` ([R8](research.md#r8-stay-warm-bookkeeping)). No cross-agent state is shared. Amortization is left for 006 |
| **III. Unified Models & Provider Entities** | ✅ Pass | One plugin is one provider, with `endpoints` for each model type. Unified models are the fallback set. All six types run through the same attempt loop, records and fallback |
| **IV. Scope Discipline** | ✅ Pass | No prompt injection, truncation or rewrite. Where 9router injects, 0router doesn't, and where a target can't carry a part, that target is skipped ([R4](research.md#r4-translation-behaviour-parity-and-deliberate-deviations)). The restart note is a new block that 0router adds to its own reply to the client; nothing is sent upstream. Optimizer pass-through (v3.0.1, amendment 2026-09-28): same-style attempts forward the body, unknown headers and non-stream responses as received; cross-style attempts drop only the fields the target style can't hold and record them ([R27](research.md#r27-optimizer-pass-through-amendment-2026-09-28)). An agent → headroom → 0router chain is tested (T152) |
| **V. Streaming-Native SSE** | ✅ Pass | `Sse<impl Stream>` with `StreamExt` adaptors. `CancellationToken` + `CancelOnDrop`. Non-SSE bodies (NDJSON, audio) use `Body::from_stream`, also without buffering. The preamble hold keeps header events only until the first content event ([R5](research.md#r5-streaming-relay-and-constitution-v)) |
| **VI. Reference-Informed Behavior** | ⚠️ Pass with deviations | Classification, backoff, retry budgets and translation shapes follow 9router. Deliberate deviations are listed in [R26](research.md#r26-deliberate-deviations-from-9router-summary) and in Complexity Tracking, and each one is asserted in `tests/parity/deviations.toml` |
| **VII. Trustworthy Model Tests** | ✅ N/A | Model tests are a later slice. Continuation is declared only for models a live check confirmed |
| **VIII. Latency Observability** | ✅ Pass | TTFT and total are recorded per request, provider, account and unified model, measured at the socket write, and queryable from the CLI (SC-005) |
| Arch: Rust only, Tokio rules | ✅ Pass | Tokio runtime. File writes and reloads run in `spawn_blocking` |
| Arch: No self-registration | ✅ Pass | Styles and plugins are data. Primitives are closed enums with static tables |
| Arch: Secrets isolation | ✅ Pass | Secrets live in `accounts.toml` and are injected at request build only. The redactor and the security floor keep them out of everything else |
| Workflow: Benchmark gate | ✅ Pass | `wire`, `engine` and `server` benches, with a committed baseline summary |
| Workflow: Parity-audit gate | ⚠️ Scheduled | `/rust-parity-audit` on the classification, retry and translation modules before the PR (tasks) |

**I. Plugin Safety**:
- Style files and schema-2 plugins pass the same gate: `deny_unknown_fields`, the secret
  checks, and closed sets for every primitive and layout choice.
- Templates have a fixed placeholder set per context. They have no expressions,
  conditionals or loops, and `{account.*}` and `{secret.*}` are rejected.
- Endpoint URLs pass SSRF checks. Redirects are never followed. Private hosts need the
  operator's `allow_private_endpoints`.
- Forwarding is declared by plugins, and the core applies it under a floor the plugins
  can't lower.
- The fit check refuses a whole plugin that needs anything outside the core's closed sets.

**Post-design re-check**: ✅ No unjustified violations. The design keeps every principle.
The VI deviations below are there because IV is the stricter invariant and wins where the
two conflict, or because 9router has a bug or gap that would break a standard client (F1).

## Project Structure

### Documentation (this feature)

```text
specs/003-request-pipeline/
├── plan.md              # This file
├── research.md          # Phase 0: R1–R26
├── data-model.md        # Phase 1: entities, validation, state machines
├── quickstart.md        # Phase 1: runnable validation guide
├── contracts/
│   ├── client-surface.md      # routes, key carriers, errors, streams, lists, counts, jobs
│   ├── api-style-schema.md    # style TOML format + gate rules
│   ├── provider-schema-v2.md  # endpoints, forwarding floor, fit check, conversion
│   └── operator-cli.md        # files, commands, socket protocol, exit codes
├── bench-baseline.md    # committed Criterion summary (implement phase)
├── checklists/
│   └── requirements.md
└── tasks.md             # Phase 2 (/speckit-tasks, not created here)
```

### Source Code (repository root)

```text
Cargo.toml                          # [workspace] gains wire, engine, server
crates/
├── nullrouter-registry/            # extended
│   └── src/
│       ├── schema/                 # + style.rs, endpoint.rs, forwarding.rs, session.rs
│       ├── validate/               # + style gate, SSRF, floor, template placeholders
│       ├── fit.rs                  # FitVerdict, refusal messages
│       ├── convert.rs              # schema 1 → 2 for community plugins
│       ├── floor.rs                # security floor names and checks
│       └── community.rs            # embedded community set, install/uninstall
├── nullrouter-wire/                # new, no I/O
│   ├── src/
│   │   ├── ir/                     # request, stream event, response IR
│   │   ├── template.rs             # typed templates: encode + reverse decode
│   │   ├── codec/                  # request/response codecs driven by style files
│   │   ├── stream/                 # framing readers/writers, block model, counters,
│   │   │                           #   ClientStreamState, restart and error-event writers
│   │   ├── primitives/             # media, thinking, embeddings, repairs, jobs
│   │   ├── usage.rs                # per-style usage extraction, input_semantics
│   │   ├── estimate.rs             # 9router estimator
│   │   └── error_body.rs           # informational error bodies per style
│   ├── tests/                      # parity vs tests/fixtures/9router/translate/*, deviations
│   └── benches/wire.rs
├── nullrouter-engine/              # new
│   ├── src/
│   │   ├── accounts.rs             # accounts.toml, SecretString, host binding
│   │   ├── keys.rs                 # keys.toml, digests, AgentId
│   │   ├── classify.rs             # 9router checkFallbackError port
│   │   ├── cooldown.rs             # backoff levels, per (provider, account, model)
│   │   ├── plan.rs                 # RequestPlan, candidate order, stay-warm
│   │   ├── attempt.rs              # attempt loop, retry budgets, timeouts, watchdog
│   │   ├── breaks.rs               # continuation, restart, error event
│   │   ├── upstream.rs             # pooled reqwest client, request build, secret injection
│   │   ├── forwarding.rs           # apply declarations under the floor
│   │   ├── jobs.rs                 # video JobMap
│   │   ├── records.rs              # RecordStore ring + indexes
│   │   └── redact.rs               # Redactor, tracing layer
│   ├── tests/                      # scripted mock upstreams: retry, fallback, breaks,
│   │                               #   usage, cancellation, reuse, secrets sentinel
│   └── benches/engine.rs
├── nullrouter-server/              # new
│   ├── src/
│   │   ├── router.rs               # axum router from style routes, discriminators
│   │   ├── auth.rs                 # key check before body read
│   │   ├── relay.rs                # Sse / Body::from_stream, CancelOnDrop, keepalive
│   │   ├── models.rs               # model lists per style
│   │   └── operator.rs             # Unix socket, NDJSON protocol
│   ├── tests/                      # end-to-end on loopback
│   └── benches/server.rs
└── nullrouter-cli/                 # extended
    └── src/cmd/                    # + serve, accounts, keys, behaviour, records, plugins
styles/
└── bundled/                        # openai-chat, anthropic-messages, openai-responses, gemini
plugins/
├── bundled/                        # 5 files, schema 2, hand-maintained
└── community/                      # 116 files, schema 1, GENERATED
tools/
└── gen-bundled/
    ├── generate.mjs                # + community output, translator oracle
    └── seeds/                      # anything the generator can't evaluate
tests/
├── fixtures/9router/               # + translate/, classify/, usage/ (GENERATED)
├── parity/deviations.toml          # every deliberate deviation, asserted
├── gate/
│   ├── invalid/styles/
│   ├── invalid/providers/          # schema-2 additions
│   └── unsupported/                # fit-check corpus with golden .expected messages
└── harness/                        # SDK scripts (Python, Node), Claude Code / Codex runners
```

**Structure Decision**:
- `nullrouter-wire` has no I/O, so translation is tested and benchmarked on its own, and
  the parity audit targets one crate.
- `nullrouter-engine` has no axum types. `nullrouter-server` is a thin layer over it.
- The five bundled plugins become hand-maintained schema-2 files. The generator writes only
  the community set and the oracle fixtures.
- Style files sit at the repo root beside `plugins/`, because they are the same kind of
  data.

## Complexity Tracking

The design breaks no principle without a reason. VI deviations and new structure worth
review:

| Choice | Why | Simpler alternative rejected because |
|---|---|---|
| 9router's prompt injections dropped: the Claude Code system prompt, `response_format` as system text, the opencode fingerprint tools | IV forbids rewriting prompt content, and IV is stricter than VI | Keeping them would break the confirmed "forward as received" ledger row. They exist for OAuth cloaking and free tiers, which are out of scope |
| A part the target can't carry skips the target instead of being dropped | IV (never truncate) | Silent drop is 9router's behaviour, but it changes the answer without telling anyone |
| Gemini client route translates tools; non-stream second hop fixed | 9router's gaps would break Gemini CLI and non-stream SDK clients (SC-001) | Parity with a bug fails the slice's own fail condition |
| One same-account retry for 429 and other 5xx | Stay-warm first is a confirmed ledger row | 9router moves on at once, which loses the warm cache |
| Stall watchdog on forced-stream bodies; success marked at stream end | A stall or an early cut must count as a failure | 9router marks success at stream start, which hides later breaks |
| Missing usage is "not reported", not an estimate | SC-004: numbers must match the provider | 9router's +2000 estimate would be a wrong number shown as a real one |
| Key checked before body parse; no refresh sleep on API-key 401/403 | Spends nothing on unauthenticated callers | 9router's order exists for its OAuth flows |
| Three new crates (`wire`, `engine`, `server`) | I/O-free translation for parity and benches; HTTP-free engine tests | One core crate blurs audit targets and slows builds |
| Styles as data with named primitives instead of Rust translators | FR-008 and the ledger: styles are data like providers | 9router's hand-written pairs grow as the square of the style count and can't be refused by a fit check |
