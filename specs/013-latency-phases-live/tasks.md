---

description: "Task list for 013-latency-phases-live"
---

# Tasks: Latency Slice 1, Phases and Live View

**Input**: Design documents from `specs/013-latency-phases-live/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included. The spec's failure conditions and SC-001 to SC-011 are test outcomes (research
R16). Within each story, write the tests first and expect them to fail until implemented.

**No local cargo** (project rule since 2026-10-06): don't run `cargo test`, `clippy`, `check` or
`build` on this machine. Commit in groups, push once with the user's OK, and read CI with the
github MCP. "Verify" below means in CI.

**Organization**: Tasks are grouped by user story (US1–US6, in the spec's priority order).
Task IDs are in execution order. **MVP = Phases 1–4 (US1 + US2)**, as the spec says the first two
stories alone are a working result.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US6)

## Path Conventions

- `crates/nullrouter-engine/src/timing.rs`: NEW. `AttemptClock` and the `ATTEMPT` task-local
- `crates/nullrouter-engine/src/phases.rs`: NEW. `phases::of`, the one phase definition (pure)
- `crates/nullrouter-engine/src/live.rs`: NEW. The live table and its snapshot
- `crates/nullrouter-engine/src/connection/`: NEW. Effective settings, the client cache with the
  connector layer, and proxies
- `crates/nullrouter-engine/src/attempt.rs`: where the marks are taken
- `crates/nullrouter-engine/src/testkit/`: test doubles (feature `testkit`)
- `crates/nullrouter-registry/src/schema/`: plugin and `config.toml` schema
- `crates/nullrouter-server/src/operator.rs`: socket ops
- `crates/nullrouter-cli/src/cmd/`: commands

---

## Phase 1: Setup

- [X] T001 Create the module skeletons with module docs that cite their research items: `crates/nullrouter-engine/src/timing.rs` (R1–R3), `phases.rs` (R1, R14, R17), `live.rs` (R9), and `connection/mod.rs` with submodules `clients.rs` (R3, R5) and `proxy.rs` (R7, R8). Declare them in `crates/nullrouter-engine/src/lib.rs`
- [X] T002 [P] Add `tower` (layer and service traits, as the dashboard crate already uses it) to `crates/nullrouter-engine/Cargo.toml`, and the `socks` feature to the workspace `reqwest` in `Cargo.toml` (plan, Complexity Tracking)
- [X] T003 [P] Add the test target stubs `crates/nullrouter-server/tests/phases.rs`, `live.rs`, `connection.rs`, `proxy.rs` and `crates/nullrouter-engine/tests/routing_latency_blind.rs`, each with a module doc naming the SCs it covers (research R16)

---

## Phase 2: Foundational (blocking prerequisites)

**Purpose**: Verify the connect-attribution assumption, then build the clock, the phase function,
the record field and the test double. No story can start before these.

### Spike (gate)

- [X] T004 Spike the connector layer in `crates/nullrouter-engine/tests/connect_attribution.rs` against a local TLS test server (`rustls` with a self-signed certificate, trusted by the test client) and a local HTTP CONNECT proxy. A `connector_layer` whose future reads a `tokio::task_local!` at completion must show:
  - (1) the measured span includes the proxy CONNECT and the TLS handshake (delay each in the test servers and see the delay in the span);
  - (2) a connect that loses hyper-util's checkout race and finishes on a background task never sees the task-local;
  - (3) two sequential requests read new then reused, and on an HTTP/2 server, three concurrent requests read one new and two reused.

  **If any of these fails, stop and report to the user with research R3's fallback**; don't continue with US1's connect phase
- [X] T005 Record T004's result in `specs/013-latency-phases-live/research.md` R3: replace "Must verify first" with what was observed, and the reqwest/hyper-util versions

### Tests first

- [X] T006 [P] Write `phases::of` unit tests in `crates/nullrouter-engine/src/phases.rs` (`#[cfg(test)]`) on hand-built records, one per case:
  - a streamed success on a new connection: seven phases, retry wait "not applicable", sum = `total_ms` within 1 ms;
  - reused: connect "not applicable";
  - a failed then a successful attempt: `ended_in` names the failing phase, later phases "not applicable", router overhead of attempt 2 = `started₂ − ended₁ − retry_wait`;
  - a skipped attempt before the first: all "not applicable", and its time goes to the next attempt's router overhead;
  - `merged_wait`: one `waiting_for_provider` value, and no `headers`/`first_token`;
  - a whole answer: generation "not applicable";
  - client-blocked time in delivery, never generation;
  - `refresh_ms` inside connect and outside router overhead (R6);
  - a pre-slice record with no `timing`: every phase "not recorded";
  - the phases up to the first output sum to `ttft_ms` within 1 ms;
  - `slowest` picks the largest phase across attempts, with sides as in data-model § Side of a phase
- [X] T007 [P] Write `AttemptClock` tests in `crates/nullrouter-engine/src/timing.rs` (`#[cfg(test)]`): marks set from several tasks are read back exactly; `blocked` and `retry_wait` spans accumulate; turning the clock into `AttemptTiming` keeps the invariant "`connection == reused` ⇒ `connected == None`"
- [X] T008 [P] Write serde tests in `crates/nullrouter-engine/tests/records_journal.rs`: a journal line from before this slice (no `timing`) loads with `timing: None`, and a record with `timing` round-trips through the journal unchanged

### Implementation

- [X] T009 Implement `AttemptClock` in `crates/nullrouter-engine/src/timing.rs`:
  - atomic `f64` slots (bit-cast `AtomicU64`, NaN = unset) for `connected`, `headers`, `first_output`, `upstream_done`;
  - accumulating spans `blocked_ms`, `retry_wait_ms`, `refresh_ms`;
  - `merged_wait`, `http`, `proxy` name, `timeout: Option<TimeoutHit>`;
  - the attempt's effective `connect_timeout: Duration`;
  - `fn to_timing(&self) -> AttemptTiming`;
  - `tokio::task_local! { pub static ATTEMPT: Arc<AttemptClock> }`.

  Times are ms from the request's arrival `Instant` (data-model § AttemptTiming)
- [X] T010 Add `AttemptTiming`, `Connection` (`new`/`reused`/`none`), `TimeoutHit { which: connect|headers|first_token|stall, ms: u64, source: Source }` and `Source { by: operator|plugin|built_in, level: model|provider|endpoint|env|default }` to `crates/nullrouter-engine/src/records.rs`, and add `Attempt.timing: Option<AttemptTiming>` with `#[serde(default, skip_serializing_if = "Option::is_none")]` (contracts/record.md). Update every `Attempt { … }` constructor (`attempt.rs` `start_attempt`, `skip`, and tests)
- [X] T011 Implement `phases::of(&RequestRecord) -> Vec<AttemptPhases>`, `Phase`, `PhaseValue` (`Ms`, `NotApplicable`, `NotRecorded`, `InProgress`), `side(Phase)` and `slowest(&[AttemptPhases])` in `crates/nullrouter-engine/src/phases.rs`, with exactly research R1's formulas. Serialize phase names as `router_overhead`, `retry_wait`, `connect`, `headers`, `first_token`, `waiting_for_provider`, `generation`, `delivery`
- [X] T012 Implement the connector layer `ConnectClock` (a `tower::Layer` over reqwest's `BoxedConnectorService`) in `crates/nullrouter-engine/src/connection/clients.rs`, as T004 proved. At completion, inside an `ATTEMPT` scope, it sets `connected` and the connect span. It enforces the scope's `connect_timeout`, or the built-in default outside a scope, with `tokio::time::timeout`
- [X] T013 Extend `crates/nullrouter-engine/src/testkit/mock_upstream.rs` with `Step::Phased { accept_delay, headers_delay, first_frame_delay, frames, every }`, and a slow-reading client helper in `crates/nullrouter-engine/src/testkit/mod.rs` that reads a stream with a set pause between reads

  **Done without `accept_delay`**: a TCP accept can't delay the client's connect (the kernel completes it), so `Step::Phased` has `headers_delay`, `first_frame_delay`, `frames`, `every`. T014's connect case needs a delaying connector layer in the test client instead

**Checkpoint**: the clock, the phase function, the record field and the layer exist and are tested.

---

## Phase 3: User Story 1 - Phase times in every request record (Priority: P1) 🎯 MVP

**Goal**: Every attempt's record holds its marks, and views show seven phases that add up.

**Independent Test**: `Step::Phased` delays each phase in turn, plus a slow-reading client. Each
delay shows in the right phase of the right attempt, the sums hold, and router overhead and TTFT
match slice 010's definitions (spec US1).

### Tests for User Story 1

- [X] T014 [P] [US1] Write `crates/nullrouter-server/tests/phases.rs` (SC-001, SC-002). For each injected delay (0router side via a slow `outgoing` hook in testkit, retry wait, connect via `accept_delay`, headers, first frame, frame gaps, slow client), check:  ⟵ **Partly done** (CI run 65 green): connect-less phases, headers, first token, generation, delivery and the sums. The 0router-side delay is tested in the engine (`tests/phases_router_side.rs`, arrival backdated 300 ms: the test needs no hook). A slow connect is tested at the layer (`clients.rs::a_slow_connect_is_the_span_between_the_two_marks`), not end to end: the engine's own client can't take a test connector. Retry wait is checked through a 503 retry
  - the record's phases show it in the right phase of the right attempt, within `max(5 ms, 5%)`;
  - for every completed request, the phases sum to `total_ms` within 1 ms;
  - the phases up to the first output sum to `ttft_ms` within 1 ms
- [X] T015 [P] [US1] Write the US1 acceptance scenarios in `crates/nullrouter-server/tests/phases.rs`:  ⟵ **Partly done**: scenarios 2, 3, 4, 5, 6, 7, 8 and the client leaving mid-generation. A stream broken and resumed by attempt 2 is tested in the engine (`breaks.rs::a_cut_and_continued_answer_keeps_its_phases_consistent`), where the break helpers are, and the async media job in `video_jobs.rs::polling_a_job_adds_nothing_to_the_submits_phases`. The live entry being gone is tested (`a_finished_request_leaves_no_live_entry`)
  - (2) two requests in a row: the second `reused`, connect "not applicable";
  - (3) a 503 then success: attempt 1 `ended_in` and later phases "not applicable";
  - (4) a thinking-first stream, with keepalive pings before the first thinking delta: first token ends at the thinking delta;
  - (5) headers flushed with the first frame: `merged_wait`;
  - (6) a slow client: time in delivery;
  - (7) a whole JSON answer: generation "not applicable";
  - (8) a pre-slice journal record: "not recorded";
  - a client that disconnects mid-generation: the attempt ends in `generation`, later phases "not applicable", and the live entry is gone;
  - a stream broken mid-way and resumed by attempt 2: each attempt has its own first token and generation, and request TTFT is the first output the client got;
  - an async media job: the phases end when 0router answers with the job ID, and later polls add nothing
- [X] T016 [P] [US1] Write SC-003's test in `crates/nullrouter-server/tests/phases.rs`: over the same run, `phases::of`'s request-level router overhead (the first non-skipped attempt) equals the first non-skipped attempt's `started` minus any refresh span, and request TTFT equals `ttft_ms`. Include a request whose account needed a token refresh. These are slice 010's definitions as re-pointed to `phases::of` (spec Clarifications, analyze note), copied into the test with a `// 010:` comment until 010 is merged (plan, Coordination)  ⟵ **Partly done** (run 65 green): the definitions without a refresh. The request whose account needed a token refresh is tested in the engine (`refresh_dedup.rs::a_refresh_for_the_request_is_not_router_overhead`), where the sign-in kit lives.
- [X] T017 [P] [US1] Write CLI rendering tests in `crates/nullrouter-cli/src/cmd/records.rs` (`#[cfg(test)]`):
  - `list` shows the `SLOWEST` column, e.g. `first token 41.2 s (provider)`, `…` for in-flight, and `not recorded`;
  - `show` prints the phase table, with `new connection · HTTP/2 · proxy <name>`, `failed in <phase> (timeout: …)`, `(includes token refresh N ms)`, `(of which record close N ms)` and `total … = sum of phases`, as in contracts/cli.md

### Implementation for User Story 1

- [X] T018 [US1] In `crates/nullrouter-engine/src/attempt.rs`, create an `Arc<AttemptClock>` in `start_attempt` and hold it on the attempt runner. Wrap the upstream `send()` of `once`, `media_once` and `count_once` in `timing::ATTEMPT.scope(clock.clone(), …)`, so whole answers, media and token counts record connect, reuse and HTTP version too (FR-012). Set `headers` when `send()` resolves. Set `http` from `resp.version()`. In `end_attempt`, store `timing = Some(clock.to_timing())` in the same `records.update` call (research R2; no new `update` calls)
- [X] T019 [US1] Set `first_output` in `crates/nullrouter-engine/src/attempt.rs` at every place `self.ttft()` is called today (whole answers, media, count, the stream's first `is_output()` piece). Set `upstream_done` where `read_all` returns and where `pump` sees EOF. Set `merged_wait` when the first body read after headers already holds output (research R13)
- [X] T020 [US1] Replace `send()` in `crates/nullrouter-engine/src/attempt.rs` with `try_send` first. Only on `Full`, time the awaited send and add it to the clock's `blocked_ms`, keepalives included (research R4). Keep the cancellation `select!` as it is
- [X] T021 [US1] Time the same-account retry `self.pause(b.delay)` in `walk` (`crates/nullrouter-engine/src/attempt.rs`) and hand the duration to the next attempt's clock as `retry_wait_ms`. Time `fresh_for_use` and `refresh_rejected` and hand each to the next attempt as `refresh_ms` (research R6)
- [X] T022 [US1] Record `closing_ms` for the serving attempt: the time from `upstream_done` until the engine releases the record's `close` line (the FR-037 hold in `run`, `crates/nullrouter-engine/src/attempt.rs`). Store it with a `records.update` merged into the existing close update, not a new one
- [X] T023 [US1] In `crates/nullrouter-server/src/operator.rs`, make `records.get` add `phases` per attempt (from `phases::of`), and `records.list` add `slowest: {phase, ms, side, in_progress}` or `null` per record (contracts/operator-socket.md). For a request still in flight, this task returns `{in_progress: true}` with no phase yet; T031 fills in the current phase from the live table
- [X] T024 [US1] Render the `SLOWEST` column in `list` and the per-attempt phase table in `show` in `crates/nullrouter-cli/src/cmd/records.rs`, as in contracts/cli.md § `nullrouter records`. `--json` passes `timing` and `phases` through
- [X] T025 [US1] Make the records views in `crates/nullrouter-server/src/views/` carry `phases` and `slowest`, so the dashboard's request window shows the same numbers later (no dashboard change in this slice)

**Checkpoint**: US1 works alone. Push the group with the user's OK and verify SC-001–SC-003 in CI.

---

## Phase 4: User Story 2 - Live view of requests in flight (Priority: P2) 🎯 MVP

**Goal**: `nullrouter live` lists every request in flight with its current phase and the times of
its finished phases, and shows no content.

**Independent Test**: requests parked in each phase all appear in the right phase, a request is
gone one refresh after it ends, and `--json` gives one snapshot (spec US2).

### Tests for User Story 2

- [X] T026 [P] [US2] Write `crates/nullrouter-server/tests/live.rs` (SC-005):  ⟵ **Done except** the router-overhead-before-first-attempt case (engine unit test only: too short to catch from outside); the slow socket client's reply is small, so it checks the lock is not held, not a full buffer 200 concurrent requests parked in chosen phases with `Step::Phased`, and `live.snapshot` every 100 ms. Assert:
  - no request in flight is missing;
  - no request still listed one second after it ended;
  - each current phase is correct within one second of the change;
  - attempt 2 shows with attempt 1 in `finished`;
  - a request before its first attempt shows `router_overhead`;
  - a snapshot with nothing in flight gives `[]`;
  - a socket client that requests a snapshot and never reads the reply doesn't slow requests: 50 requests complete within the same time as without it (FR-020)
- [X] T027 [P] [US2] Write the content check in `crates/nullrouter-server/tests/live.rs` (FR-019): a request whose prompt, answer and headers carry marker strings, and whose account secret is a marker. The serialized snapshot contains none of them
- [X] T028 [P] [US2] Write CLI tests in `crates/nullrouter-cli/src/cmd/live.rs` (`#[cfg(test)]`): rendering of a snapshot as in contracts/cli.md (`nothing in flight`, the paused-proxy line, the finished-phases column), and `no server is running` exiting 1

### Implementation for User Story 2

- [X] T029 [US2] Implement `LiveEntry` and `Live` (`Mutex<HashMap<String, LiveEntry>>`) in `crates/nullrouter-engine/src/live.rs`, with:
  - `insert` at arrival;
  - `attempt(id, n, provider, account, model, clock)`;
  - `finish_attempt(id, AttemptPhases)`;
  - `remove`;
  - `snapshot(now) -> Vec<LiveSnapshot>`, which clones the `Arc`s under the lock and reads the clocks after unlocking (research R9).

  The entry holds no content, headers or secrets (data-model § LiveEntry)
- [X] T030 [US2] Wire the live table into `crates/nullrouter-engine/src/attempt.rs` and `crates/nullrouter-engine/src/state.rs` (`Engine.live`):
  - insert before the request's first `await`;
  - set the attempt at `start_attempt`;
  - finish at `end_attempt`;
  - remove in `end_request` and on every early return or drop path. Use a drop guard on the request runner, so a panic or cancellation can't leave an entry
- [X] T031 [US2] Add the `live.snapshot` op to `crates/nullrouter-server/src/operator.rs`. It returns `{ok, as_of, paused_proxies, in_flight}`, newest first, and lists paused proxies as an empty list until US4 (contracts/operator-socket.md). Also make `records.list` give an in-flight request's `slowest` its current phase and time from the live table, marked in progress (FR-013, research R14)
- [X] T032 [US2] Implement `nullrouter live [--json]` in `crates/nullrouter-cli/src/cmd/live.rs` and register it in `crates/nullrouter-cli/src/cmd/mod.rs`:
  - poll once a second;
  - redraw with ANSI clear when `std::io::IsTerminal` says stdout is a terminal, else print one snapshot per poll;
  - quit on Ctrl-C;
  - `--json` prints one snapshot and exits (contracts/cli.md § `nullrouter live`)

**Checkpoint**: MVP complete (US1 + US2). Push with the user's OK, verify SC-001–SC-005 in CI,
and run quickstart §1–2 against a real provider if the user wants.

---

## Phase 5: User Story 3 - Timeouts per provider and per model (Priority: P3)

**Goal**: Connect, header, first-token and stall timeouts per provider and per model, with the
precedence operator model → operator provider → plugin model → plugin endpoint → built-in.

**Independent Test**: a short header timeout fails an attempt as a header timeout that falls over
as other failures do. A reasoning stream longer than every timeout, with its gaps below stall,
is never cut (spec US3).

### Tests for User Story 3

- [X] T033 [P] [US3] Write resolution tests in `crates/nullrouter-engine/src/connection/mod.rs` (`#[cfg(test)]`): every precedence step for each of the four timeouts, and the `Source` reported. Today's env overrides (`FETCH_CONNECT_TIMEOUT_MS` for connect and header, `STREAM_STALL_TIMEOUT_MS`) remain the built-in level. First token is off unless set
- [X] T034 [P] [US3] Write `crates/nullrouter-server/tests/connection.rs` for timeouts:
  - US3 scenarios 1–4 and 6;
  - SC-006: a thinking stream of 5 min (paused tokio clock) with gaps below stall completes with defaults;
  - SC-007: change a timeout with `reload` while a request is in flight; it keeps the old one and the next request uses the new one;
  - a timeout's record carries `timeout: {which, ms, source}`;
  - the header timeout counts from the attempt's start, connect included (research R12);
  - a hand-edited invalid `config.toml` (a 0 header timeout) on `reload` is refused with the field named, and the previous settings stay in force (FR-032)
- [X] T035 [P] [US3] Write schema tests in `crates/nullrouter-registry/tests/` (the existing plugin-validation test file):
  - `connect_timeout_ms`, `first_token_timeout_ms` and `[[models]] timeouts` parse;
  - `config.toml` `[provider.P.connection]` and `[provider.P.model."M".connection]` parse;
  - "Timeouts are 1–3 600 000 ms; first token may also be 0 (off)" is enforced, with the field named in the error;
  - settings for an unknown provider load with a `check` note (FR-034)

### Implementation for User Story 3

- [X] T036 [P] [US3] Add `connect_timeout_ms` and `first_token_timeout_ms` to `Endpoint` in `crates/nullrouter-registry/src/schema/endpoint.rs`, and `timeouts: Option<ModelTimeouts { connect_ms, headers_ms, first_token_ms, stall_ms }>` to `Model` in `crates/nullrouter-registry/src/schema/model.rs`, with validation in the plugin gate (contracts/config-files.md)
- [X] T037 [P] [US3] Add `ConnectionSettings` (`connect_timeout_ms`, `header_timeout_ms`, `first_token_timeout_ms`, `stall_timeout_ms`, `reuse`, `http2`, `proxy`) to `ProviderSettings`, a per-model map `model: BTreeMap<String, ModelConnection>` (timeouts only), and the top-level `[connection]` (proxy only) in `crates/nullrouter-registry/src/schema/config.rs`. Validate "Timeouts are 1–3 600 000 ms; first token may also be 0 (off)"
- [X] T038 [US3] Implement `Effective` resolution in `crates/nullrouter-engine/src/connection/mod.rs`: `fn effective(st: &EngineState, provider, account, model, endpoint) -> Effective`, with each timeout's `Source`, as in data-model § Effective connection settings. Move `upstream::stall_timeout` and the header-timeout lookup there, leaving `upstream.rs`'s env helpers in place
- [X] T039 [US3] Use `Effective` in `crates/nullrouter-engine/src/attempt.rs`:
  - resolve once in `candidate()`, so an in-flight request keeps its settings (FR-031);
  - header timeout from the attempt's start;
  - put the connect timeout on the clock (enforced by T012's layer);
  - add the first-token deadline (from `headers` until `first_output`) to `pump`'s `select!` beside stall and cancellation, failing with `ErrorClass::Timeout` and reason `no model output within N ms`;
  - set `clock.timeout` on every timeout (research R12)
- [X] T040 [US3] Add the `connection.view` op in `crates/nullrouter-server/src/operator.rs`: per provider, every timeout with `{ms|null, source}`, the models whose timeouts differ, and (after US4–US6) proxy, reuse, http2 and retry (contracts/operator-socket.md)
- [X] T041 [US3] Implement `nullrouter connection show [<provider>] [--json]`, `set <provider> [--model M] <key> <value>` and `unset …` in `crates/nullrouter-cli/src/cmd/connection.rs` for the four timeout keys. Durations parse through `nullrouter-registry`'s `schema/duration.rs`, and `off` is allowed for first token only. The command writes `config.toml` atomically and reloads (prints `applied` / `saved; applies at next start`). A provider that isn't installed is refused with the list of known providers, and nothing is written (spec Edge Cases); test it in the module. Register it in `cmd/mod.rs`

**Checkpoint**: US3 works with the MVP.

---

## Phase 6: User Story 4 - Proxy per account, provider or all (Priority: P4)

**Goal**: The operator sets proxies at account, provider or all-providers level. Plugins can't.
Credentials are never shown. An unreachable proxy pauses until the operator marks it fixed.

**Independent Test**: proxied and unproxied traffic go where they should. A stopped proxy pauses
with every surface marked, nothing is sent direct and no account cools down. `proxy fixed`
resumes. A secret scan finds no credentials (spec US4, SC-008, SC-011).

### Tests for User Story 4

- [X] T042 [P] [US4] Add `crates/nullrouter-engine/src/testkit/mock_proxy.rs`: a minimal HTTP CONNECT and SOCKS5 proxy on 127.0.0.1, with optional basic auth, a request counter, `stop()` and `start()`. Make `MockUpstream` count direct connections separately from proxied ones (done: `stop()` is async, `start()` is `start_again()`; HTTP CONNECT, absolute-form HTTP and SOCKS5; `MockUpstream::direct_connections(&proxy)` is connections minus those the proxy carried)
- [X] T043 [P] [US4] Write `crates/nullrouter-server/tests/proxy.rs`:
  - US4 scenarios 1–3: account, provider and all levels, and `none` at each;
  - the record's `proxy` name;
  - SC-011: stop the proxy, then check that the request skips with `proxy <name> paused`, the mock upstream sees 0 direct connections, there is no cooldown on the account, `check` reports an error, `live.snapshot` lists the paused proxy, and `routing/proxies.json` holds it;
  - a `serve` restart keeps the pause;
  - `proxy.fixed` with the proxy down reports `reachable: false`; after `start()`, it resumes and the next request succeeds;
  - changing the assignment clears the pause;
  - a provider error coming back through a healthy proxy doesn't pause it (the probe succeeds)
- [X] T044 [P] [US4] Write the plugin refusal test in `crates/nullrouter-registry/tests/`: `proxy`, `proxy_url`, `https_proxy` and `no_proxy` at any depth fail validation with `plugins can't declare a proxy; proxies are operator-only` and the key's path (FR-026, research R10)
- [X] T045 [P] [US4] (new tests: `proxy_credentials_appear_nowhere` in the server secrets.rs, a literal password and a username through a served request, a failed connect and the pause; and a CLI test for an env password across the read commands. The dashboard pages are not scanned with a proxy yet) Extend the 009 secret scan (the test that scans CLI output, records, dashboard pages and logs for account secrets) with a proxy password, a username and a password from `{ env = … }`, over records, `live`, `connection show`, `proxy list`, `accounts list`, `check`, dashboard pages and the `serve` log (SC-008)

### Implementation for User Story 4

- [X] T046 [US4] (load, save and validation done; adding the passwords to the `Redactor` waits for the engine wiring in T048) Implement the `proxies.toml` load and save in `crates/nullrouter-engine/src/connection/proxy.rs`:
  - schema 1, `[[proxy]] { name, url, username?, password? }`, with the password a string or `{ env = "VAR" }`;
  - mode 0600, written atomically through `crate::files`;
  - validation: "`name` matches `[a-z0-9][a-z0-9_-]{0,31}` and is unique; `none` is reserved; `url` scheme is `http`, `https` or `socks5`, with a host and port; the URL itself has no credentials" (data-model § Proxy).

  Add each password to the `Redactor` as a secret
- [X] T047 [US4] (resolution is `connection::proxy_for`, apart from `Effective`, which stays `Copy`) Add `Account.proxy: Option<String>` (a name or `"none"`) to `crates/nullrouter-engine/src/accounts.rs` and its `accounts.toml` serde. Resolve the effective proxy (account → provider → all → none) with its level in `connection/mod.rs`'s `Effective`
- [X] T048 [US4] (per snapshot behind a `Mutex`, which a reload replaces, not an `ArcSwap`; `connection::client_for` is `for_account`; the callers move in T049) Implement the client cache in `crates/nullrouter-engine/src/connection/clients.rs`:
  - `ClientKey { proxy: Option<String>, http: Negotiate|Http1Only, reuse: bool }` → `reqwest::Client`, built lazily with today's `upstream::client` settings plus `Proxy::all(url).basic_auth(…)`, `http1_only()` and `pool_max_idle_per_host(0)` as the key says, and the `ConnectClock` layer always;
  - held in `ArcSwap` on `EngineState`, and rebuilt on reload when proxies change;
  - `fn for_account(&self, provider, account) -> (Client, Option<ProxyName>)`
- [X] T049 [US4] (`media_once` and `count_once` take the answer `once` already sent, so they have no client of their own; the CLI's interactive `signin.rs` stays direct, as an account being added has no proxy yet; `EngineState.http` stays for tests and goes in Polish) Replace every `st.http` use with `clients.for_account(…)`: `crates/nullrouter-engine/src/attempt.rs` (`once`, `media_once`, `count_once`), `jobs.rs`, `quota/poll.rs`, `signin/refresh.rs`, `signin/mod.rs`, and the CLI's `crates/nullrouter-cli/src/signin.rs` (FR-025). Set the clock's `proxy` name in `attempt.rs`
- [X] T050 [US4] Implement the probe and pause in `crates/nullrouter-engine/src/connection/proxy.rs`:
  - after a connect-class error or a 407 through a proxy, probe the proxy directly (TCP, plus TLS for `https`, plus the SOCKS5 greeting) within the connect timeout;
  - on failure, pause: `routing/proxies.json` (mode 0600, names, times and reasons only), plus one `tracing::warn!`;
  - `fn paused(&self, name) -> Option<Pause>`;
  - `fn fixed(&self, name) -> Result<bool>`, which probes and clears;
  - clear any pause whose definition or assignment changed on reload (research R8, data-model § ProxyState)
- [X] T051 [US4] Skip candidates behind a paused proxy in `outgoing()` (`crates/nullrouter-engine/src/attempt.rs`) as a `skipped` attempt with reason `proxy <name> paused`, no cooldown and the existing skip path. Quota polls, refreshes and job polls for those accounts skip with the same reason. When no candidate remains, the client error names the proxy (FR-028)
- [X] T052 [US4] Add the `proxy.fixed` op, and paused proxies in `live.snapshot` and `connection.view`, in `crates/nullrouter-server/src/operator.rs`. Make `reload` load `proxies.toml` (done: `proxy_board` lives on the engine; the redactor learns proxy passwords and usernames from `proxies.toml`; `connection.view` gains `proxy` and `accounts`; the probe uses reqwest for an `https` proxy; quota polls, token refreshes and job polls skip a paused proxy too, through `connection::paused_for`; each of the three has a test, and the final 503 names the proxy)
- [X] T053 [US4] Implement `nullrouter proxy add|list|remove|use|clear|fixed` in `crates/nullrouter-cli/src/cmd/proxy.rs`:
  - the password comes from stdin or `--password-env`, never argv;
  - `list` shows `user ✓` or `—`;
  - `remove` is refused while the proxy is assigned, naming the assignments;
  - `use` and `clear` write `config.toml` or `accounts.toml`; an unknown provider or account is refused with the list of known ones.

  Follow contracts/cli.md § `nullrouter proxy`, and register the command in `cmd/mod.rs`
- [X] T054 [US4] Add a `PROXY` column to `nullrouter accounts list` in `crates/nullrouter-cli/src/cmd/accounts.rs`. In `crates/nullrouter-cli/src/cmd/check.rs`, report paused proxies as errors, assignments that name undefined proxies, and `proxies.toml`'s file mode

**Checkpoint**: US4 works. Push with the user's OK and verify SC-008 and SC-011 in CI.

---

## Phase 7: User Story 5 - Connection reuse and HTTP/2 per provider (Priority: P5)

**Goal**: The operator turns reuse and HTTP/2 on or off per provider. A plugin can declare that
its provider has no HTTP/2. The default stays as today: reuse, and HTTP/2 negotiated.

**Independent Test**: by default the second request reuses its connection. With reuse off, every
request shows a connect time. With HTTP/2 off, records show HTTP/1.1 (spec US5).

- [X] T055 [P] [US5] Write US5 scenarios 1–3 in `crates/nullrouter-server/tests/connection.rs` against an HTTP/2-capable TLS test server (from T004). Cover the plugin's `http2 = false` → HTTP/1.1, and that operator `http2 on` doesn't force HTTP/2 against a server without it (negotiation) — done: resolution and view in `server/tests/connection.rs`; the TLS version check is in `engine/tests/http_mode.rs` (the T004 TLS code is engine dev-deps only) behind a testkit-gated `trusting_any_certificate`
- [X] T056 [US5] Add `http2: Option<bool>` to `Transport` in `crates/nullrouter-registry/src/schema/transport.rs`. Only `false` has an effect (contracts/config-files.md) — done; also on schema-2 `Endpoint`, since schema 2 rejects `[transport]`
- [X] T057 [US5] Resolve `reuse` and `http` (operator provider → plugin `http2 = false` → on/negotiate) in `crates/nullrouter-engine/src/connection/mod.rs`, and feed them into `ClientKey`. Record `connection` and `http` per attempt (T018 sets `http`; set `connection` from the clock at `end_attempt`) — done; one endpoint with `http2 = false` puts the whole provider on HTTP/1.1 (the client is per provider); `connection` and `http` were already recorded by the attempt clock
- [X] T058 [US5] Add the `reuse on|off` and `http2 on|off` keys to `crates/nullrouter-cli/src/cmd/connection.rs`, and to `connection show` and the `connection.view` op — done

---

## Phase 8: User Story 6 - Retry policy per provider (Priority: P6)

**Goal**: The operator sets same-account retries and the wait per provider, in total or per
status. Plugins keep their declarations within the cap.

**Independent Test**: with 1 retry and a 500 ms wait for 503, a 503 shows exactly one
same-account retry with a 500 ms `retry wait` before falling over (spec US6).

- [X] T059 [P] [US6] Write `classify::budget` precedence tests in `crates/nullrouter-engine/tests/retry.rs`: — done; the shipped declarations are covered by every registry start and `community.rs`, which load them through the gate
  - operator status → operator `all` → plugin status → default;
  - `retry-after` ≤ 5 s still wins on 429 (research R11);
  - the cap is enforced for operator and plugin values ("`retries` is 0–5 and `delay_ms` is 0–30 000");
  - every shipped plugin declaration (`grok-cli`, `antigravity`, `kiro`, `vercel-ai-gateway`) passes validation
- [X] T060 [P] [US6] Write US6 scenarios 1–3 in `crates/nullrouter-server/tests/connection.rs`, including the record's `retry_wait_ms` matching the configured wait within 5% — done in the order of the spec scenarios
- [X] T061 [US6] Check whether the community plugins' legacy retry forms (`{ attempts = 3 }`, `429 = 2`, `429 = 0`) parse today. If not, map them in `tools/gen-bundled/generate.mjs` (`attempts` n → `retries` n−1) and regenerate only those plugin files, with the ref SHA in the commit (research R11) — done: all three legacy forms parse today (schema 1 `RetryPolicy`), so no generator change; `convert.rs` maps `attempts` n to `retries` n unchanged
- [X] T062 [US6] Add the cap to `RetryOverride` validation in `crates/nullrouter-registry/src/schema/endpoint.rs`, and `RetrySettings { all: Option<RetryOverride>, by_status: BTreeMap<String, RetryOverride> }` to `ProviderSettings` in `crates/nullrouter-registry/src/schema/config.rs`, with the same cap and "A status key is a 3-digit HTTP status"
- [X] T063 [US6] Pass the operator's `RetrySettings` into `classify::budget` in `crates/nullrouter-engine/src/classify.rs` with research R11's precedence, and update its caller in `attempt.rs` `walk` — done; a `retry-after` ≤ 5 s on a 429 replaces a configured wait (it did not before for plugin overrides), the configured count stays
- [X] T064 [US6] Add the `retries`, `retries.<status>`, `retry-wait` and `retry-wait.<status>` keys to `crates/nullrouter-cli/src/cmd/connection.rs`, and the retry policy with sources to `connection show` and the `connection.view` op — done; a status holds one rule, so `retry-wait` needs `retries` set first and unsetting either removes the rule

---

## Phase 9: Polish & Cross-Cutting Concerns

- [X] T065 [P] Write `crates/nullrouter-engine/tests/routing_latency_blind.rs` (SC-009, FR-035): a unified model with two members, one 10× slower via `Step::Phased`. The sequence of placements equals the run where both are equally fast. Also check by search that no `routing/` or `route.rs` code reads `timing` or `phases` — done; the slow member answers after 300 ms against 30 ms, over eight requests of four agents; the source search covers `route.rs`, `plan.rs` and `routing/`
- [ ] T066 [P] Add the criterion group `phases` to `crates/nullrouter-server/benches/server.rs`: one streamed request through the in-process server against `MockUpstream`, with timing on and off (a `testkit` switch that makes the clock a no-op). **User-gated**: running it needs local cargo, which the project rule forbids by default. Ask the user before one `nice` run with 2 jobs on and off; record the result in `target/` and the pass or fail in the slice notes. Without that run, report SC-004 as unverified (research R15, SC-004)
- [X] T067 [P] Document in `docs/operator-config.md`:
  - new sections "Phases in records", "The live view", "Connection settings" (timeouts and precedence, reuse, HTTP/2, retries) and "Proxies" (files, levels, pause and `proxy fixed`);
  - `proxies.toml` and `routing/proxies.json` in the directory tree;
  - the new mutating commands in the reload list
  - Done: the sections are in the order of the file: phases, live view, connection settings, proxies, placed before "Per-provider settings"
- [X] T068 [P] Document the plugin fields `connect_timeout_ms`, `first_token_timeout_ms`, `[[models]] timeouts`, `[transport] http2 = false` and the retry cap, plus the proxy refusal, in `docs/plugins.md`
  - Done: `http2 = false` is documented on the schema-2 endpoint as well as `[transport]`
- [X] T069 Coordination note in `specs/013-latency-phases-live/plan.md` § Coordination: list the exact edits slice 010 needs on rebase (`journal/summary.rs` router overhead → `phases::of`; SC-003's test switches from the copied definitions to 010's functions) and slice 011's (`clients.for_account` in model tests; a paused proxy makes the test skip). FR-025's model-test leg is verified by whichever slice merges second, with a test that a model test goes through the account's proxy. Save the note to agentmemory (both instances)
  - Done: the note is in plan.md § Coordination; the agentmemory save follows in this session.
- [ ] T070 Run quickstart.md §1–5 against a real provider with the user (only with their OK; `serve` needs a local build, which the no-local-cargo rule forbids unless the user allows it for this run), and record the outcome in `specs/013-latency-phases-live/quickstart.md`

---

## Dependencies & Execution Order

- **Setup (T001–T003)** → **Foundational (T004–T013)** → stories. T004 is a gate: if it fails,
  stop.
- **US1 (T014–T025)**: needs Foundational.
- **US2 (T026–T032)**: needs T009–T011 and T018 (clocks exist per attempt). It can start once
  T018 lands.
- **US3 (T033–T041)**: needs T012 (layer) and T018.
- **US4 (T042–T054)**: needs T048's client cache, which needs T012. T049 touches the same
  `attempt.rs` functions as T039, so do them one after the other.
- **US5 (T055–T058)**: needs T048 (`ClientKey`).
- **US6 (T059–T064)**: needs only T021 (retry wait). It can run in parallel with US3–US5.
- **Polish (T065–T070)**: after the stories it touches. T070 is last.

```text
Setup → Foundational (T004 gate) → US1 → US2 (MVP)
                                     ├─ US3 ─┐
                                     ├─ US6  ├─ US4 → US5 → Polish
                                     └───────┘
```

## Parallel Examples

- Foundational tests: T006, T007 and T008 together.
- US1 tests: T014–T017 together, before T018–T025.
- US2 tests: T026–T028 together.
- After the MVP: US3 and US6 in parallel (different files, except `connection.rs`'s CLI, so sequence T041 and T064).
- Polish: T065–T068 together.

Per the project's pace rule, run one Opus subagent at a time unless the user asks for parallel
ones.

## Implementation Strategy

1. **T004 spike first.** It decides whether connect can be shown separately. If not, return to
   the user before US1.
2. **MVP = Phases 1–4**: phases in records and the live view. Push the group with the user's OK,
   and read CI: SC-001–SC-005.
3. **Then US3 and US6** (timeouts, retries): small, and the first actions latency calls for.
4. **Then US4** (proxies), the largest story: client cache, probe, pause, every sender.
5. **Then US5**, which reuses US4's client cache.
6. **Polish**: latency-blind routing proof, bench, docs, coordination.

Commit after each task or small group. Push only in groups, with the user's OK; each push cancels
the previous CI run.
