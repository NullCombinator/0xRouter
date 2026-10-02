# Implementation Plan: Account Sign-In

**Branch**: `005-account-sign-in` | **Date**: 2026-10-02 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/005-account-sign-in/spec.md`

## Summary

Slice 005 makes subscription accounts ordinary 0router accounts. The operator signs in an
anthropic (Claude Pro/Max), xai or grok-cli account from the CLI, with or without a browser,
and it then serves through slice 003's attempt loop like an API-key account. 0router keeps the
tokens fresh, takes an account that can't be refreshed out of service and names it, and polls
provider-reported quota for every chosen account that has it, keeping each poll with a per-model
tally of 0router's own tokens for slice 006.

The approach:
- **Three generic flows, plugin data for the rest.** Device code, PKCE with loopback or
  paste-back, and refresh are core code in `nullrouter-engine/src/signin/`. Each bundled plugin
  declares its `[signin]`, `[identity]`, `[quota]` and `[models_live]` sections
  ([R3](research.md#r3-sign-in-flows-user-visible), [R16](research.md#r16-plugin-gate-and-fit-check)).
- **Tokens beside accounts.** `accounts.toml` goes to schema 2 with `kind = key | signin`;
  tokens live in `tokens.toml` (0600, locked writes, host-bound), and in a live cell on the
  engine so refreshes don't reload ([R5](research.md#r5-account-kinds-and-the-token-store-user-visible),
  [R6](research.md#r6-serving-with-a-sign-in-account)).
- **Fresh tokens three ways.** Proactive refresh before `expires_at − lead`, a check at use, and
  refresh-and-retry on rejection, all deduplicated per account
  ([R9](research.md#r9-token-freshness-fr-010fr-014)).
- **Named out-of-service states.** `refreshing`, `needs sign-in`, `refused by provider`, shown
  in the accounts list, as skipped attempts in records, and in the informational error
  ([R10](research.md#r10-refresh-failures-and-account-states-user-visible)).
- **Quota as data.** A declarative extractor (paths with `*`, alternatives, a filter, name
  templates) reads all five providers' reports; one named core decoder handles grok-cli's
  gRPC-web fallback ([R11](research.md#r11-quota-polling-user-visible),
  [R12](research.md#r12-the-quota-extractor-data-not-code)).
- **History for 006.** Append-only JSONL per account, each poll with a per-model tally in the
  quota meter's token categories, kept until the operator prunes it
  ([R15](research.md#r15-poll-history-and-traffic-tally-fr-023fr-026)).
- **One maintenance task** in `serve` drives refreshes, polls and live model lists, and stops on
  the existing shutdown channel ([R13](research.md#r13-maintenance-task)).

## Technical Context

**Language/Version**: Rust 1.93.1, edition 2024 (MSRV 1.89 for `File::lock`; was 1.85).

**Primary Dependencies**: no new crates. `reqwest`, `tokio`, `tokio-util`, `sha2`, `base64`,
`getrandom`, `url`, `serde_json`, `arc-swap`, `aho-corasick` from slice 003
([R1](research.md#r1-dependencies)).

**Storage**: files in `$NULLROUTER_HOME`: `accounts.toml` (schema 2), `tokens.toml`,
`tokens.lock`, `install-id`, `quota/<provider>/<account>.jsonl` and `.tally.json`, all 0600.
Live tokens, states and the running tally are also in memory
([contracts/operator-cli.md](contracts/operator-cli.md)).

**Testing**: `cargo test` with an in-process mock identity provider and mock providers; a
token-expiry soak; the slice 003 secret sentinel extended to tokens and codes; Python, Node and
Claude Code harnesses; opt-in live checks L1–L5 ([R17](research.md#r17-live-checks),
[R18](research.md#r18-test-strategy)).

**Target Platform**: Linux (and macOS for browser opening); headless SSH sessions first-class.

**Project Type**: extends the existing Cargo workspace; no new crates.

**Performance Goals**: a sign-in account adds no measurable time to the request path: one
atomic load for the token, a few header inserts, one tally update. The engine bench's new case
stays within noise of slice 003's baseline. Refresh and polling run off the request path.

**Constraints**: no secret in plugins, logs, records, errors, CLI output or the socket; tokens
sent only to their bound hosts; no request body change beyond declared forced non-content
parameters; no `unsafe`; blocking file I/O in `spawn_blocking`.

**Scale/Scope**: 7 bundled providers (5 + xai, grok-cli); 3 sign-in flows; 5 quota readers;
tens of accounts; one poll per account per 10 minutes by default.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-checked after Phase 1 design.*

| Principle | Status | How this plan complies |
|---|---|---|
| **I. Plugin Safety** | ✅ Pass | Sign-in, identity, quota and live-model sections are data with closed sets; the core does every send; tokens live only in core files and memory, injected at execution; URLs are SSRF-checked and host-bound; no client secret is needed for these public clients, and none can be declared. The quota protobuf decoder is a named core primitive, not plugin code |
| **II. Routing Fidelity** | ✅ Phased | Quota is displayed, not routed on (FR-022). The poll history and per-model tally are the data slice 006's windowed amortization needs |
| **III. Unified Models & Provider Entities** | ✅ Pass | xai is one plugin with text, image and video sections; sign-in is an account kind, not a separate provider (anthropic keeps one plugin) |
| **IV. Scope Discipline** | ✅ Pass | No cloaking (Q1). grok-cli's input rewrites and allowlist are not ported; only declared non-content parameters (`store`, `reasoning.*`, `include`) are forced, and each is recorded ([R8](research.md#r8-provider-forced-request-parameters)) |
| **V. Streaming-Native SSE** | ✅ Unchanged | The relay is untouched; a refresh never interrupts a running stream |
| **VI. Reference-Informed Behavior** | ⚠️ Pass with deviations | OAuth flows follow 9router's endpoints and parameters; deviations in [R19](research.md#r19-deliberate-deviations-from-9router-summary) and Complexity Tracking, each asserted in `tests/parity/deviations.toml` |
| **VII. Trustworthy Model Tests** | ✅ N/A | No model tests in this slice |
| **VIII. Latency Observability** | ✅ Unchanged | Sign-in accounts are recorded like key accounts |
| Arch: Tokio rules | ✅ Pass | Token and history writes in `spawn_blocking`; the maintenance task is async with a cancellation token |
| Arch: Secrets isolation | ✅ Pass | `tokens.toml` 0600 with locked writes; redactor rebuilt on every refresh with current and previous tokens |
| Workflow: Benchmark gate | ✅ Pass | Engine bench gains a sign-in account case |
| Workflow: Parity-audit gate | ⚠️ Scheduled | `/rust-parity-audit` on `signin/` and `quota/` before the PR |

**Post-design re-check**: ✅ No unjustified violations. The identity header placeholders
([R7](research.md#r7-client-identity-headers-user-visible-confirmed-by-the-user)) were confirmed
by the user (spec Clarifications Q5).

## Project Structure

### Documentation (this feature)

```text
specs/005-account-sign-in/
├── plan.md              # This file
├── research.md          # Phase 0: R1–R19
├── data-model.md        # Phase 1: accounts, tokens, states, quota, tally
├── quickstart.md        # Phase 1: validation guide
├── contracts/
│   ├── signin-quota-schema.md   # [signin], [identity], [quota], [models_live], force
│   └── operator-cli.md          # files, commands, output, socket ops
├── checklists/
│   └── requirements.md
└── tasks.md             # Phase 2 (/speckit-tasks)
```

### Source Code (repository root)

```text
crates/
├── nullrouter-registry/src/
│   ├── schema/          # + signin.rs, identity.rs, quota.rs, models_live.rs, force on endpoints/models
│   ├── validate/        # + gate rules for the new sections, host-set check, placeholders
│   └── fit.rs           # sign-in/quota sections accepted only in bundled plugins
├── nullrouter-engine/src/
│   ├── accounts.rs      # schema 2, kind, poll_interval; schema-1 load
│   ├── tokens.rs        # tokens.toml, lock, live token cells, states
│   ├── signin/          # device_code.rs, pkce.rs, loopback.rs, refresh.rs, classify.rs, dedup.rs
│   ├── quota/           # extract.rs, grpc_web.rs, poll.rs, history.rs, tally.rs
│   ├── maintenance.rs   # timer queue: refreshes, polls, live models
│   ├── identity.rs      # header placeholders, install id
│   ├── upstream.rs      # sign-in auth placement, identity headers, forced parameters
│   ├── plan.rs          # out-of-service accounts become recorded skips
│   ├── attempt.rs       # refresh-and-retry on rejection; tally feed in end_attempt
│   └── redact.rs        # rebuild with token generations
├── nullrouter-server/src/
│   └── operator.rs      # quota.list, quota.poll, quota.checkpoint, accounts.state extended
└── nullrouter-cli/src/cmd/
    ├── accounts.rs      # signin, list --long, enable clears refused
    └── quota.rs         # new
plugins/bundled/         # anthropic (+signin, quota), opencode-go/zen (+quota), xai, grok-cli (new)
plugins/community/       # xai, grok-cli removed (now bundled)
tests/
├── parity/deviations.toml   # + R19 rows
└── harness/                 # signed-in mock accounts for SDKs and Claude Code
```

**Structure Decision**: no new crate. Sign-in and quota are engine concerns that share the
engine's HTTP client, secret release and redactor; splitting them out would duplicate those.
The CLI drives the interactive flows through engine functions; the server only refreshes and
polls.

## Complexity Tracking

| Choice | Why | Simpler alternative rejected because |
|---|---|---|
| Separate `tokens.toml` with a lock file | Tokens rotate every ~40 min; two processes write them | Rewriting `accounts.toml` on every refresh churns the operator's file and races the CLI |
| Declarative quota extractor with `*`, alternatives and filters | Five providers' shapes without per-provider code (Constitution I) | Per-provider Rust readers are code paths a plugin can't extend later |
| `grpc_web_ratio` core decoder | grok-cli's paid accounts report weekly credits only over gRPC-web | A protobuf decoder can't be data; leaving it out shows wrong quota for SuperGrok (SC-006) |
| Not porting grok-cli's input rewrites | Constitution IV; harness coupling belongs to 004 adapters | Porting them changes conversation content without the coupling exception's review and record |
| Refresh-and-retry on 401 for all three providers | FR-011 | 9router skips it for xai, which would let an early expiry reach the client |
| Polling always, 10-minute default | FR-018 (also while idle) | 9router's 1-minute browser-tab cadence would poll ~6× more around the clock |
| MSRV 1.85 → 1.89 | `File::lock` for the token lock | A lock crate or a hand-rolled `flock` FFI (needs `unsafe`) |
