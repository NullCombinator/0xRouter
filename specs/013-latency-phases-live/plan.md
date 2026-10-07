# Implementation Plan: Latency Slice 1, Phases and Live View

**Branch**: `013-latency-phases-live` | **Date**: 2026-10-07 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `specs/013-latency-phases-live/spec.md` (clarified
2026-10-07, five answers; FR-029 corrected in planning). Scope brief:
[specs/briefs/2026-10-07-latency-phases-live.md](../briefs/2026-10-07-latency-phases-live.md).

## Summary

Every attempt records where its time went, in seven phases. A live CLI view shows requests in
flight. Per provider, account and model, the operator can set timeouts, a proxy, reuse/HTTP/2
and the retry policy. Latency never steers routing (constitution VIII v4.0.0).

The approach:

- **Marks, not durations.** An attempt stores points in time on the request's monotonic clock.
  One pure function, `phases::of`, derives the seven phases from them. Phases add up to
  `total_ms` and `ttft_ms` by construction. The records view, the list column, the live view and
  slice 010's summaries all call it ([R1](research.md#r1-marks-not-durations-what-an-attempt-stores)).
- **No extra journal writes.** The marks live in an atomic `AttemptClock` in an in-memory live
  table, and are folded into the record in the attempt's existing end-of-attempt update
  ([R2](research.md#r2-where-marks-are-written-a-live-side-table-folded-into-the-record-at-attempt-end)).
- **Connect is seen by a connector layer**, which reads a task-local clock at completion.
  hyper-util's pool race then attributes a new connection only to the request that waited for it
  ([R3](research.md#r3-connect-time-and-new-vs-reused-a-connector-layer-plus-a-task-local-clock)).
  This is the riskiest assumption, so it is verified first by a spike.
- **Delivery is client-blocked time**, measured only when the client channel is full, plus the
  tail after the provider finishes ([R4](research.md#r4-delivery-time-blocked-on-the-client)).
- **Settings resolve per attempt** from the engine snapshot, so in-flight requests keep theirs.
  There is one HTTP client per (proxy, HTTP mode, reuse), and connect timeouts travel with the
  clock ([R5](research.md#r5-connection-settings-where-they-live-and-how-they-resolve)).
- **Proxies.** Credentials go in a 0600 `proxies.toml`, and assignments sit at account, provider
  and all-providers level. An unreachable proxy (a failed probe) is paused, persisted and
  announced. Candidates behind it are skipped without cooldown, and only `proxy fixed` or a
  settings change resumes it ([R7](research.md#r7-proxies-definitions-assignments-credentials),
  [R8](research.md#r8-proxy-pause-fr-028-clarify-answers)).

## Technical Context

**Language/Version**: Rust 1.89 (workspace `rust-version`), edition 2024.

**Primary Dependencies**:
- Existing workspace crates: `reqwest` 0.13.5 (`connector_layer`, `http1_only`,
  `pool_max_idle_per_host`, `Proxy`), `hyper-util` 0.1.21 (indirect), `tokio` (`task_local!`),
  `arc-swap`, `serde`, `toml`, `tower` (layer traits; already a dashboard dependency, now on the
  engine too).
- One new feature: reqwest `socks`, for SOCKS5 proxies (Complexity Tracking).
- No new crate for the live view: it redraws with ANSI escapes when `std::io::IsTerminal` says
  stdout is a terminal (as `serve` and `accounts` already check), and quits on Ctrl-C (the
  existing `tokio::signal` handling). No raw mode, so no `crossterm`.

**Storage**:
- Request records gain `attempts[].timing` (journaled in the existing end-of-attempt line).
- New `proxies.toml` (0600) and `routing/proxies.json` (0600).
- Additions to `config.toml` and `accounts.toml`.

See [contracts/config-files.md](contracts/config-files.md) and [contracts/record.md](contracts/record.md).

**Testing**:
- `cargo test` per crate in CI, with no local cargo.
- The engine `testkit` `MockUpstream` gains `Step::Phased`, which delays accept, headers, first
  frame and per-frame gaps. Integration tests go in `crates/nullrouter-server/tests/phases.rs`,
  `live.rs`, `connection.rs` and `proxy.rs` (a local SOCKS5/HTTP proxy test double in testkit).
- Unit tests cover `phases::of` against hand-built mark sets: sums, not applicable, merged, and
  pre-slice records.

**Target Platform**: Linux and macOS operator machines (unchanged).

**Project Type**: Rust workspace: library crates, an HTTP server and a CLI.

**Performance Goals**: No measurable slowdown (FR-036, SC-004). Per request the added cost is one
map insert and one remove, about 8 atomic stores, one task-local scope, and `try_send` in place
of `send`. The criterion group `phases` in `crates/nullrouter-server/benches/server.rs` measures
it on and off ([R15](research.md#r15-performance-fr-036-sc-004)).

**Constraints**:
- Latency never steers routing (VIII, FR-035).
- Plugins stay data and can't declare proxies (I, FR-026).
- Proxy credentials are treated like account secrets (FR-027).
- Built-in timeout and retry defaults keep 9router parity (VI, FR-022/FR-030).
- Settings changes apply from the next request (FR-031).

**Scale/Scope**: Up to a few hundred requests in flight (SC-005 tests 200), 1–3 HTTP clients,
a handful of proxies. Six user stories, 36 FRs and 11 SCs.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Plugin Safety | Plugins stay data; no proxy from a plugin; no secret to a plugin | **Pass**. New plugin fields are numbers and one bool. Proxy keys are refused at validation (R10). Proxy credentials live in core-owned 0600 files and reach only `reqwest::Proxy` when a client is built |
| II. Routing Fidelity | The four properties untouched | **Pass**. The decision core and `route.rs` don't change. A paused proxy skips candidates in `outgoing()`, as unusable accounts are skipped today (R8). That is availability, not a weight |
| III. Unified Models | Settings per provider entity | **Pass**. Settings attach to provider, model and account; nothing is per modality |
| IV. Scope Discipline | No content change | **Pass**. Only timing is observed. Content is never read for timing beyond `is_output()`, as today |
| V. Streaming-Native SSE | No buffering; cancellation propagates | **Pass**. `try_send` and then `send` keep backpressure. The first-token timeout joins `pump`'s existing `select!` beside cancellation |
| VI. Reference-Informed Behavior | Timeout and retry semantics match 9router | **Pass**. Defaults and env overrides are unchanged. The header timeout still counts from the attempt's start (R12). Retry defaults stay `classify::budget`, and plugin retries are kept (clarify Q2). Proxy per account matches 9router's per-connection proxy |
| VII. Trustworthy Model Tests | — | **N/A** here. Model tests (slice 011) use the account's client and proxy, so a paused proxy makes them skip, never BROKEN (Coordination) |
| VIII. Latency Observability (v4.0.0) | Measured, shown, never steering | **Pass**. Nothing in routing reads `timing` or phases. SC-009's test proves placements are equal with one member 10× slower |
| Architecture: performance gate | Criterion bench on hot-path changes | **Pass**. The `phases` bench group (R15) |
| Architecture: secrets isolation | No secret in plugin declarations | **Pass** (as I) |

Re-check after Phase 1 design: **Pass**. The contracts add no plugin-facing surface beyond
numbers and a bool. Records and the live view carry only proxy names. The header-timeout choice
was revised in research to keep VI.

## Project Structure

### Documentation (this feature)

```text
specs/013-latency-phases-live/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── cli.md
│   ├── operator-socket.md
│   ├── record.md
│   └── config-files.md
├── checklists/requirements.md
└── tasks.md             # /speckit-tasks
```

### Source Code (repository root)

```text
crates/nullrouter-registry/src/schema/
├── endpoint.rs          # connect_timeout_ms, first_token_timeout_ms; retry cap (R11)
├── model.rs             # Model.timeouts
├── transport.rs         # http2: Option<bool>
├── config.rs            # [connection], ProviderSettings.connection/.model/.retry
└── plugin.rs            # proxy-key refusal before parse (R10); durations parse via duration.rs

crates/nullrouter-engine/src/
├── phases.rs            # NEW: AttemptPhases, phases::of, sides, slowest (R1, R14)
├── clock.rs             # AttemptClock (atomic marks); task_local ATTEMPT (R2, R3)
├── live.rs              # NEW: LiveEntry, live table, snapshot (R9)
├── connection/          # NEW
│   ├── mod.rs           # Effective settings resolution and sources (R5)
│   ├── clients.rs       # ClientKey → reqwest::Client cache; ConnectClock layer (R3)
│   └── proxy.rs         # proxies.toml load, probe, pause state, routing/proxies.json (R7, R8)
├── upstream.rs          # client() split into clients.rs; stall/first-token resolution moves to connection/
├── attempt.rs           # marks at start/refresh/retry wait/headers/first output/done; try_send; first-token timeout; skip on paused proxy
├── records.rs           # Attempt.timing: Option<AttemptTiming>
├── classify.rs          # budget(): operator and plugin precedence (R11)
├── accounts.rs          # Account.proxy
├── jobs.rs, quota/poll.rs, signin/refresh.rs   # clients.for_account(); skip when proxy paused
├── state.rs             # EngineState: clients, proxies, effective-settings tables
└── testkit/
    ├── mock_upstream.rs # Step::Phased
    └── mock_proxy.rs    # NEW: minimal HTTP CONNECT / SOCKS5 proxy test double

crates/nullrouter-server/src/
├── operator.rs          # live.snapshot, connection.view, proxy.fixed; records.* gain phases/slowest
├── text.rs              # unchanged marks (ttft_ms, total_ms); closing_ms note
└── views/               # records views render phases and slowest

crates/nullrouter-cli/src/
├── cmd/live.rs          # NEW
├── cmd/connection.rs    # NEW
├── cmd/proxy.rs         # NEW
├── cmd/records.rs       # SLOWEST column; phase table in show
├── cmd/accounts.rs      # PROXY column
├── cmd/check.rs         # paused proxies, unused settings
└── signin.rs            # clients.for_account for sign-in

crates/nullrouter-server/tests/  phases.rs, live.rs, connection.rs, proxy.rs   # NEW
crates/nullrouter-engine/tests/  routing_latency_blind.rs                       # NEW (SC-009)
crates/nullrouter-server/benches/server.rs                                      # group `phases`
docs/operator-config.md   # Connection settings, Proxies, Live view, Phases in records
```

**Structure Decision**: Timing and live state live in the engine beside the attempt loop.
Connection settings get one engine module, `connection/`, so every caller that sends upstream
(attempts, jobs, polls, refreshes, sign-in, model tests) resolves them the same way. The server
and CLI add surface following the existing op and command patterns. The dashboard is untouched
(out of scope).

## Phasing (for /speckit-tasks)

1. **Spike (gate)**: verify R3's three assumptions on a local server (proxy and TLS inside the
   layer, background completion invisible, new/reused/HTTP/2 attribution). On failure, stop and
   take the fallback to the user.
2. **Foundation**: `AttemptClock`, the task-local, the connector layer, the `phases::of` unit
   tests, `Attempt.timing` serde (pre-slice records load), and `Step::Phased`.
3. **US1 (P1)**: marks in `attempt.rs`, delivery via `try_send`, retry wait, refresh, merged
   wait, records views, the `SLOWEST` column, and the `show` phase table. Evidence: SC-001–003.
4. **US2 (P2)**: live table, `live.snapshot`, `nullrouter live`. Evidence: SC-005. **MVP =
   phases 1–4.**
5. **US3**: timeout schema and settings, resolution with sources, first-token timeout in `pump`,
   connect timeout in the layer, `connection show/set/unset`. Evidence: SC-006, SC-007.
6. **US4**: `proxies.toml`, assignments, client cache, every sender via `for_account`, probe,
   pause, resume, check, live and serve-log surfaces, plugin proxy refusal. Evidence: SC-008,
   SC-011.
7. **US5**: reuse and HTTP/2 settings, the plugin `http2 = false`, records' `http`/`connection`.
8. **US6**: retry settings and precedence, cap validation, legacy community forms (R11).
9. **Polish**: SC-009 test, the `phases` bench, docs, the secret-scan extension, the
   coordination patch for 010.

## Coordination

- **Slice 010** (`.worktrees/010`, unmerged) defines router overhead as the first non-skipped
  attempt's `started` and TTFT as `ttft_ms` (its research). 013 keeps `ttft_ms` unchanged. When
  013 rebases onto 010, `journal/summary.rs`'s router overhead calls `phases::of` (it differs
  only when a sign-in refresh preceded the first attempt, R6). SC-003's test runs against 010's
  functions once both are on one branch; until then it runs against a copy of 010's definitions
  in the test.
- **Slice 011** (model tests): its upstream calls take `clients.for_account()`, and a paused
  proxy makes a test skip with the reason (VII: never BROKEN). Whichever slice lands second makes
  the one-line change.
- **Slice 012** edits `route.rs` `candidate_of`. 013 doesn't touch `route.rs`.
- **Slices 004/012** don't touch `attempt.rs`'s send path. 011 might; re-read before editing.

## Complexity Tracking

| Choice | Why needed | Simpler alternative rejected because |
|---|---|---|
| Connector layer + task-local to attribute connects | reqwest exposes no per-request connection timing or reuse flag | Wall-clock guesses (a "long headers means new connection" heuristic) misattribute network time to the provider (spec failure signal) |
| Live side table with atomic marks | The live view needs the current phase; records journal each update | Journaling each mark adds about 6 writes per attempt (FR-036) |
| Several HTTP clients (per proxy/HTTP mode/reuse) | Proxy, HTTP/1.1-only and no-reuse are client-level in reqwest; per-account proxies must not share a pool | One client with `Proxy::custom` can't vary HTTP mode or reuse, and mixes accounts' connections |
| reqwest `socks` feature | SOCKS5 is the common proxy kind for exits (spec assumption) | HTTP/HTTPS only would leave out the most common way operators reach other regions |
| Probe before pausing a proxy | The spec's immediate second try; separates proxy failure from provider failure | Pausing on the first connect error stops all traffic on a single blip, until the operator acts |
