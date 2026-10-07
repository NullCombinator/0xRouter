# Research: Latency Slice 1, Phases and Live View

9router has no per-phase timing and no live view, so User Stories 1 and 2 have no parity oracle.
9router's timeouts (`FETCH_CONNECT_TIMEOUT_MS`, `STREAM_STALL_TIMEOUT_MS` in
`open-sse/config/runtimeConfig.js`) and same-account retries (`RETRY_CONFIG`,
`resolveRetryEntry` in `open-sse/executors/base.js`) are inherited behavior (constitution VI).
Their defaults don't change here. 9router's proxies are per connection, which 0router calls an
account (`open-sse/utils/proxyFetch.js`, `chatCore.js` `connectionProxyUrl`/`connectionNoProxy`).
Its proxy pools are out of scope (spec, brief row 31).

The code facts below were read on 2026-10-07 from this branch (`9696394` plus specs). Line
numbers are approximate.

## R1. Marks, not durations: what an attempt stores

**Decision**: Each attempt stores **marks**: points in time, in ms from the request's arrival, on
the request's monotonic clock (`Inflight::arrived: Instant`, already used by
`attempt.rs::now`). It also stores two accumulated spans. The seven phases are **derived** from
these by one pure function, `phases::of(&RequestRecord)`. Records store:

| Mark | Set where | Meaning |
|---|---|---|
| `started` (exists) | `start_attempt` | the attempt begins: request built, about to send |
| `retry_wait` (span) | `walk`, around `self.pause(b.delay)` | deliberate same-account wait before this attempt |
| `refresh` (span) | `walk`, around `fresh_for_use` / `refresh_rejected` | sign-in token refresh before this attempt (R6) |
| `connected` | connector layer (R3) | connection ready; absent on a reused connection |
| `headers` | `once`, when `send()` resolves | response headers arrived |
| `first_output` | `pump`/`once`, where `self.ttft()` is called today | first model output arrived from the provider |
| `upstream_done` | end of `pump`/`read_all` | last byte from the provider |
| `blocked` (span) | `send()` on the client channel (R4) | time the engine waited for the client to take data |
| `ended` (exists) | `end_attempt` | the attempt ends |

The request-level marks already exist: `ttft_ms` (the first content handed to the client socket,
set by the server's stream writer `text.rs`, or by the engine for a whole answer) and `total_ms`.

**Phases of attempt k** (`prev_end` = the previous non-skipped attempt's `ended`, or 0):

- router overhead = `started − prev_end − retry_wait − refresh`
- retry wait = `retry_wait`, or "not applicable"
- connect = `(connected − started) + refresh` on a new connection; `refresh` alone on a reused
  connection that needed one (R6); otherwise "not applicable"
- response headers = `headers − (connected or started)`
- first token = `F − headers`, where F is the request's `ttft_ms` if this attempt produced the
  request's first output, else this attempt's `first_output`
- generation = `upstream_done − F − blocked_after_F` (streams only)
- delivery = `blocked` plus, for the serving attempt, `total_ms − upstream_done`

A phase that a mark doesn't reach is "not applicable" (FR-003). The phase the attempt was in when
it ended is the first phase whose end mark is missing (FR-007).

**Rationale**:
- **Sums hold by construction.** Every phase is a difference of adjacent marks, and the marks
  are monotonic, so the phases add up to `total_ms` (FR-009, SC-002). Ending first token at
  `ttft_ms` makes the phases up to the first output add up to TTFT exactly.
- **One definition.** FR-010 and brief row 26 need the records view, the list column, the live
  view and slice 010's summaries to agree. They all call `phases::of`.
- **Cheap.** A mark is one `f64` store. Durations derived on read cost nothing on the request
  path.

**Alternatives considered**:
- Store durations. Every writer would have to agree on boundaries, and the sums could drift.
- Store marks as wall-clock times. A clock jump would make phases negative (FR-015).

## R2. Where marks are written: a live side table, folded into the record at attempt end

**Decision**: The marks of the running attempt live in an `AttemptClock`: a small struct with
atomic `f64` slots (bit-cast `AtomicU64`), shared as `Arc`. It sits in an in-memory
`Inflight` table, `engine.live: Mutex<HashMap<RequestId, LiveEntry>>`. Each request inserts
itself on arrival and is removed when `end_request` runs. When an attempt ends, `end_attempt`
copies the clock into the attempt's `timing` in the **same** `records.update` call it makes
today. Nothing else touches the record or the journal.

**Rationale**: `records.update` writes a journal line for each change (slice 006). Writing each
mark through it would add about six journal writes per attempt, which FR-036/SC-004 forbid. The
side table takes one map insert and one remove per request, and lock-free stores for the marks.
The live view (R9) reads the same clocks, so records and the live view can't disagree.

**Alternatives considered**: (a) Journal every mark. Too costly, as above. (b) Read the live view
from records in flight. Records only learn the marks at attempt end, so the view would show the
wrong phase (SC-005).

## R3. Connect time and new-vs-reused: a connector layer plus a task-local clock

**Decision**: The HTTP client is built with
`reqwest::ClientBuilder::connector_layer(ConnectClock)` (reqwest 0.13.5, `client.rs:2475`).
The layer wraps reqwest's base connector, which does name lookup, TCP, the proxy handshake and
TLS. Its future reads a `tokio::task_local!` `ATTEMPT: Arc<AttemptClock>` **when it completes**.
If the task-local is present, it stores `connected` and the connect duration. `once` runs
`send()` inside `ATTEMPT.scope(clock, …)`.

Why completion-time works: in hyper-util 0.1.21's legacy pool (`client.rs:391–446`), a request
races `checkout` against a lazy `connect`.
- **If the connect wins,** it completed while the request's own task polled it. The task-local is
  present, so the connection is this attempt's.
- **If an idle connection wins,** the half-done connect moves to a background task
  (`exec.execute`). It completes there, where the task-local is absent, so it is never
  attributed.

The attempt is "reused" exactly when no `connected` mark arrived before `headers`.

The same layer enforces the **per-attempt connect timeout**. It reads the timeout from the
`AttemptClock` and falls back to the built-in default outside a scope. So connect timeouts
don't need one client each (R5).

The response's HTTP version comes from `Response::version()`.

**Must verify first** (the first implementation task, a spike with a local test server):
1. `connector_layer` wraps the proxy tunnel and TLS: a TLS-to-proxy test shows the handshake
   inside the measured span.
2. A completion in a background task doesn't see the task-local.
3. Sequential requests to one host show new then reused; HTTP/2 multiplexed requests show one
   new connection, and the others reused.

If (1) fails, the fallback measures only TCP connect, and reports TLS as part of response
headers, with a note in the record. The spec's "waiting for provider" merge doesn't cover
connect, so this would be recorded as a deviation and taken to the user.

**Alternatives considered**:
- `hyper`'s `Connected::extra` tags. reqwest's `Conn` is opaque to layers.
- Compare `remote_addr`. That doesn't distinguish a new connection to the same address.
- A client per request. That defeats pooling (003 FR-021).

## R4. Delivery: time blocked on the client

**Decision**: The engine's client channel (`attempt.rs::send`, capacity `CHANNEL = 64`) first
tries `try_send`. Only when the channel is full does it time the awaited `send` and add the wait
to the clock's `blocked` span. Keepalives count too. Delivery for the serving attempt adds the
tail `total_ms − upstream_done`. That tail is the server writer catching up, including the
record's `close` line it waits for (FR-037 of 006). The record notes the close wait separately as
`closing_ms`, inside delivery, so a slow disk doesn't look like a slow client.

For a whole (non-streamed) answer, delivery ends when 0router hands the body to the HTTP layer
(`end_request`). 0router can't see the socket write after that.

**Rationale**: FR-008 requires client-blocked time to be delivery, never generation. The
`try_send` path costs nothing when the client keeps up, which is the common case.

**Alternatives considered**: Time every send. That adds two `Instant::now()` calls per chunk,
which is measurable on fast streams (SC-004).

## R5. Connection settings: where they live and how they resolve

**Decision**: Settings resolve once per attempt into an `Effective` value. It is built from the
engine snapshot, so an in-flight request keeps what it started with (FR-031), as for every
existing setting.

| Setting | Plugin (data) | Operator | Built-in |
|---|---|---|---|
| connect, header, first-token, stall timeouts | endpoint `connect_timeout_ms` (new), `timeout_ms` (header, exists), `first_token_timeout_ms` (new), `stall_timeout_ms` (exists); per model `[models.timeouts]` (new, `schema/model.rs` `Model`) | `config.toml` `[provider.P.connection]`, `[provider.P.model."M".connection]` | env `FETCH_CONNECT_TIMEOUT_MS` (connect and header, as today), `STREAM_STALL_TIMEOUT_MS`; first token off |
| reuse | — | `[provider.P.connection] reuse` | on |
| HTTP/2 | `[transport] http2 = false` (new) | `[provider.P.connection] http2` | negotiate (today's ALPN `h2, http/1.1`) |
| same-account retries | endpoint `retry` (exists), capped (R11) | `[provider.P.retry]` with `all` and per-status keys | `classify::budget` (exists) |
| proxy | **never** (R10) | `proxies.toml` and assignments (R7) | none |

Timeout order (clarify Q3): operator per model, operator per provider, plugin per model, plugin
endpoint, built-in.

**Clients**: one `reqwest::Client` per distinct **(proxy, HTTP mode, reuse)** key, built lazily
and cached in `engine.clients` (`ArcSwap<HashMap<ClientKey, Client>>`; the map is rebuilt on
reload). The plain key (no proxy, negotiate, reuse) is today's client. Reuse off is
`pool_max_idle_per_host(0)`. HTTP/1.1-only is `http1_only()`. Every caller of `st.http` takes
`clients.for_account(provider, account)` instead: `attempt.rs`, `jobs.rs`, `quota/poll.rs`,
`signin/refresh.rs`, and the CLI's `signin.rs`. So token refreshes, polls and sign-ins use the
account's proxy (FR-025). Model tests (slice 011) get the same call (Coordination).

**Rationale**: Connect timeouts go through the clock (R3) and header, first-token and stall
timeouts are enforced by the attempt loop, so the client key stays small. A typical setup has
1–3 clients.

**Alternatives considered**: One client with a proxy selector closure (`Proxy::custom`). That
can't vary HTTP mode or reuse, and it mixes accounts' connections in one pool. That defeats the
per-account proxy's purpose of keeping accounts' traffic apart.

## R6. Sign-in token refresh time

**Decision**: When a sign-in token is refreshed before an attempt, the refresh time is a span in
the gap before `started`. It is reported inside **connect** (network side), and the record says
"token refreshed" with its duration. Router overhead excludes it. Slice 010's router overhead is
re-pointed to `phases::of` (Coordination), so both views agree when a refresh happened.

**Rationale**: A refresh is a network call to the provider's sign-in server. FR-008 forbids
counting provider waits as router overhead, and it isn't the client's time either.

**Alternatives considered**:
- An eighth phase. It only happens about once an hour per account, so it isn't worth widening
  every view.
- Leave it in router overhead. That breaks FR-008.

## R7. Proxies: definitions, assignments, credentials

**Decision**:
- **Definitions** go in a new `proxies.toml` (mode 0600, written by the CLI):
  `[[proxy]] name, url (http|https|socks5), username?, password?`. The password is a literal or
  `{ env = "VAR" }`, as account secrets are.
- **Assignments** live with what they apply to:
  - `config.toml` `[connection] proxy = "name"` for all providers;
  - `[provider.P.connection] proxy = "name" | "none"` per provider;
  - `accounts.toml` `[[account]] proxy = "name" | "none"` per account.
  Account wins over provider, which wins over all (clarify Q2). Every surface shows only the name.
- reqwest needs its `socks` feature for SOCKS5 (Complexity Tracking).
- Credentials are passed only to `reqwest::Proxy::basic_auth` when the client is built. The
  `Redactor` gets each proxy password as a secret, so it is masked if it ever reaches an error
  string, as account secrets are (009 T069's scans are extended, SC-008).

**Rationale**: Credentials sit with the same protection as account secrets (FR-027). Assignments
sit next to what they name, so `accounts list` can show an account's proxy.

**Alternatives considered**: Proxy URLs with inline credentials in `config.toml`. That file is
not 0600 and is shown by `check`.

## R8. Proxy pause (FR-028, clarify answers)

**Decision**: An attempt through a proxy that fails with a connect-class error (reqwest
`is_connect()`, or a proxy `407`/handshake refusal) triggers an **immediate probe**: a direct
TCP connection to the proxy's address, plus the TLS handshake for an `https` proxy and the
greeting for SOCKS5, bounded by the connect timeout.
- **If the probe fails,** the proxy is **paused**. The state is saved to
  `routing/proxies.json` (mode 0600) and survives a restart.
- **If the probe succeeds,** the failure is the provider's and is classified as today.

While a proxy is paused:
- `outgoing()` skips every candidate whose effective proxy is paused, as a `skipped` attempt
  with reason `proxy <name> paused`. No cooldown is set. The request falls over as for any skip;
  with no candidate left, the client error names the proxy.
- Quota polls, refreshes and model tests for those accounts are skipped and say why.
- Surfaces: a `check` error, a `serve` `warn!` line (once per pause), a mark in the live view
  and in `connection show`.

**Resume** happens only on the operator's action (clarify Q1):
- `nullrouter proxy fixed <name>` asks `serve` to probe. If the probe succeeds, the pause clears
  and the next request uses the proxy.
- Changing or removing the proxy's definition or assignment also clears it.

**Rationale**: The probe is the spec's "immediate second try" and separates the proxy from the
provider behind it. Skipping in `outgoing()` reuses the existing skip path, so `route.rs`
doesn't change (Coordination).

**Alternatives considered**: Pausing on the first connect error. Every brief network blip would
stop traffic until the operator acts.

## R9. Live view

**Decision**:
- **Socket op.** `{"op":"live.snapshot"}` returns every `LiveEntry` with:
  - agent (key name), target, provider, account, attempt number;
  - current phase and seconds in it;
  - finished phase times;
  - seconds since arrival.
  The engine builds it by locking the table, cloning the `Arc`s and unlocking, then reading the
  atomics. No content is in the entry (FR-019).
- **CLI.** `nullrouter live` polls the op once a second and redraws until Ctrl-C. Without
  a TTY it prints one snapshot per second. `--json` prints one snapshot and exits.
- **Requests before their first attempt** show phase `router overhead`.
- **Paused proxies** are listed above the table.

**Rationale**: A 1 s poll meets FR-017. The operator socket already requires the operator's file
access (FR-020). A slow client of the socket holds no lock: the snapshot is built before it is
written.

**Alternatives considered**: A push stream over the socket. That holds per-client state in
`serve` and needs backpressure handling, for no gain at 1 Hz.

## R10. Plugins can't declare a proxy

**Decision**: Before the schema parse, the registry's validation walks the plugin TOML for any
key named `proxy`, `proxy_url`, `https_proxy` or `no_proxy` at any depth. On a match it fails
with `plugin <id>: <path>: plugins can't declare a proxy; proxies are operator-only (see
nullrouter proxy)`. `deny_unknown_fields` would also reject the key, but with a message that
doesn't say why.

## R11. Retry policy precedence and cap

**Decision**:
- **Order.** Operator per status, then operator `all`, then plugin per status, then
  `classify::budget`'s defaults.
- **Cap.** Validation caps `retries ≤ 5` and `delay_ms ≤ 30 000` for plugins and operators alike.
  The shipped declarations (`grok-cli` 2/2000, `antigravity` 3, `kiro` 0, `vercel-ai-gateway` 2)
  all fit.
- **Legacy forms.** Three community plugins use legacy forms (`{ attempts = 3 }`, `429 = 2`).
  Whether they parse today is checked in the first registry task; if not, the generator maps
  them (`attempts` n → `retries` n−1).
- **Waits.** A provider's `retry-after` wait (`MAX_INDICATED_WAIT` 5 s) still wins over a
  configured wait on 429, as today.
- **Record.** A retry's wait is recorded as `retry_wait` (clarify Q4).

## R12. Timeouts at the right phase

**Decision**:
- **Connect**: enforced in the connector layer (R3).
- **Header**: measured from the attempt's start, connect included, as today and as in 9router
  (constitution VI: `FETCH_CONNECT_TIMEOUT_MS` bounds the whole wait for headers). The connect
  timeout separately bounds the connect part. The record still reports connect and headers as
  separate phases.
- **First token**: a deadline from `headers` until `first_output`, checked in `pump`'s select
  beside the stall timeout. Thinking output is output (spec FR-004; `ev.is_output()` covers
  thinking today).
- **Stall**: unchanged. It is reset by any bytes, keepalive pings included.

Each timeout ends the attempt as `ErrorClass::Timeout` (header, first token, connect) or `Stall`,
as today. The attempt's `timing.timeout = {which, ms, source}` is recorded (FR-024).

**Rejected**: measuring the header timeout from `connected`. That would let a new connection
wait up to connect + header, twice 9router's bound with the defaults, which breaks parity (VI).

## R13. "Waiting for provider" merge

**Decision**: When the first output is already available in the first body read after `headers`
(the provider flushed headers with its first bytes), the record marks the attempt
`merged_wait = true`. Views show one "waiting for provider" phase covering headers and first
token. 0router can always see headers separately, so this is the only case where the two can't
be told apart.

## R14. Records view and list column

**Decision**:
- `records.get` and `records.list` return `phases` (from `phases::of`) per attempt, beside the
  marks, under `timing`.
- The CLI's `show` prints a phase table per attempt. `list` adds the column `slowest`, e.g.
  `first token 41.2 s (provider)`.
- The side of each phase: router overhead → 0router; retry wait → retry; connect → network;
  headers, first token, waiting for provider, generation → provider; delivery → client.
- In-flight requests show their current phase from the live table with `…`.
- Records written before this slice have no `timing` and show `not recorded`.

## R15. Performance (FR-036, SC-004)

**Decision**: A new criterion group `phases` in `crates/nullrouter-server/benches/server.rs` runs
one full streamed request through the in-process server against a local mock provider, with
phase timing on and off (a `cfg(test)` switch on the clock). The pass rule is that on-off stays
within the group's noise (criterion's 95% interval overlaps). Baselines stay local in `target/`,
as for every bench.

The added cost per request:
- one map insert and one remove;
- about eight atomic stores;
- one task-local scope;
- one `try_send` per chunk (replacing `send`).

## R16. Evidence for the success criteria

| SC | Test |
|---|---|
| SC-001, SC-002 | `crates/nullrouter-server/tests/phases.rs`: a mock provider with injectable delays per phase (connect via a delayed accept, headers, first byte, generation) and a slow-reading client; asserts each record's phases within `max(5 ms, 5%)` and the sums |
| SC-003 | the same run's records through `phases::of` against slice 010's `summary` functions (after 010 merges; until then against the 010 definitions copied into the test) |
| SC-004 | R15 |
| SC-005 | `live.rs`: 200 concurrent requests parked in chosen phases; snapshot each 100 ms; no missing, no ended after 1 s, phase correct within 1 s |
| SC-006 | mock reasoning stream: thinking events for 5 min (paused tokio clock), gaps below stall; defaults on |
| SC-007 | settings change mid-flight with `reload`; old request keeps old timeout, next uses new |
| SC-008 | extend the 009 secret scan to proxy passwords across CLI, records, live, dashboard, logs |
| SC-009 | placement test: one member 10× slower; placements equal to the equal-speed run |
| SC-010 | the list column test, plus a quickstart walk-through |
| SC-011 | proxy test with a local proxy that the test stops; no direct sends (mock provider sees none), no cooldown, every surface marked, resume after `proxy fixed` |

## R17. What counts as the first output

**Decision**: Keep today's test, `ir::Event::is_output()` (`crates/nullrouter-wire/src/ir/event.rs:32`).
These count: block start, text delta, thinking delta, signature, tool arguments. These don't:
keepalives, `message_start`-style headers, usage, finish. A block start counts because it is the
provider opening a block of model output (text, thinking or tool use). It isn't transport
framing. Slice 010's TTFT reads the same `ttft_ms`, so both slices share this one definition
(FR-004, FR-010).
