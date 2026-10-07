# Feature Specification: Latency Slice 1, Phases and Live View

**Feature Branch**: `013-latency-phases-live`

**Created**: 2026-10-07

**Status**: Draft

**Scope brief**: [specs/briefs/2026-10-07-latency-phases-live.md](../briefs/2026-10-07-latency-phases-live.md)
(latency slice 1 of two). In clarify, plan and analyze, an answer that contradicts a confirmed row
of that brief's ledger means stop and revisit the brief.

**Input**: User description: "Latency slice 1, phases and live view: latency becomes something the
operator sees for each request and while it happens, and can act on, without latency ever changing
routing by itself". The rest of this spec comes from that brief's final command, which the user
confirmed claim by claim on 2026-10-07.

## Clarifications

### Session 2026-10-07

- Q: What happens when an operator's proxy is unreachable? → A: "Announce user that proxy is not
  reachable, and pause for fix." 0router tells the operator, and pauses traffic through that proxy
  until it is fixed. It never sends direct, and it doesn't put the accounts behind the proxy into
  cooldown (FR-028).
- Q: What happens to the retry settings four plugins already declare (inherited from 9router)? →
  A: Plugins keep declaring same-account retries, up to a maximum that validation enforces. The
  operator's per-provider setting wins, and the core default covers what neither sets (FR-030).
  This revises brief row 19.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Phase times in every request record (Priority: P1)

The operator opens a request's record and sees where its time went, for each attempt, failed
attempts included. The six phases are router overhead, connect, response headers, first token,
generation, and delivery to the client. A phase that didn't happen shows "not applicable" and
never "0 ms", so the operator can tell what was slow and whose side it was on: 0router's, the
network's, the provider's, or the client's.

**Why this priority**: Every other story builds on these measurements. Without them the operator
can only notice that something feels slow, which is what this slice is meant to end.

**Independent Test**: Send requests through a test provider that adds a known delay to one phase
at a time, plus a slow-reading client. Then read the records with `nullrouter records`. Each
delay appears in the right phase of the right attempt. The phases add up to the request's total.
Router overhead and time to first token match what `nullrouter latency` (slice 010) reports for
the same requests.

**Acceptance Scenarios**:

1. **Given** a streamed request served on the first attempt over a new connection, **When** the
   operator shows its record, **Then** the attempt lists times for router overhead, connect,
   response headers, first token, generation and delivery, and these add up to the request's
   total.
2. **Given** a request served over a reused connection, **When** the operator shows its record,
   **Then** connect shows "not applicable" and the record says the connection was reused.
3. **Given** a request whose first attempt failed with a server error and whose second attempt
   (another account) served it, **When** the operator shows its record, **Then** the first
   attempt shows its phases up to the failure and names the phase it failed in, its later phases
   show "not applicable", the second attempt's router overhead covers the gap between the
   attempts, and all phases of both attempts add up to the total.
4. **Given** a reasoning model that streams thinking for 40 s before any visible text, **When**
   the operator shows the record, **Then** first token ends at the first thinking output, not at
   the first visible text, and keepalive pings before it don't end it.
5. **Given** a provider whose response headers arrive only together with the first output, so
   the two can't be told apart, **When** the operator shows the record, **Then** the attempt
   shows one "waiting for provider" phase in place of response headers and first token.
6. **Given** a client that reads the stream slowly, **When** the operator shows the record,
   **Then** the time 0router spent waiting for the client to take data is in delivery, not in
   generation.
7. **Given** a non-streamed answer (whole body, embedding, image, token count), **When** the
   operator shows the record, **Then** first token ends when the whole answer has arrived and
   generation shows "not applicable".
8. **Given** a record written before this slice, **When** the operator shows it, **Then** its
   phases show as "not recorded", which is different from "not applicable".

---

### User Story 2 - Live view of requests in flight (Priority: P2)

The operator runs one CLI command and sees the requests in flight right now. For each one it
shows the agent, the target, the provider and account, the attempt number, the phase it is in and
for how long, and the times of the phases already finished. The view refreshes until the operator
quits. With `--json` it prints one snapshot. It shows metadata only, never prompt or answer
content.

**Why this priority**: Records explain a request after it ends. The live view shows a slow
request while it is still slow, which is when the operator can act. Stories 1 and 2 together are
a working result.

**Independent Test**: Start a mix of short and long requests, some stuck in each phase on a test
provider. Run the live view. Every request in flight is listed with the phase it is really in. A
request leaves the view at the next refresh after it ends. `--json` gives the same rows as one
snapshot. No content appears.

**Acceptance Scenarios**:

1. **Given** three requests in flight, one waiting for headers, one generating and one waiting
   for 0router to start its first attempt, **When** the operator runs the live view, **Then** it
   lists all three with the phase each is in, the time spent in that phase so far, and the times
   of their finished phases.
2. **Given** the live view is open, **When** a listed request ends, **Then** it is gone from the
   next refresh, and **When** a new request arrives, **Then** it appears at the next refresh.
3. **Given** a request moves to its second attempt, **When** the view refreshes, **Then** it shows
   attempt 2 with the new provider and account, and attempt 1's phases stay visible as finished.
4. **Given** no requests are in flight, **When** the operator runs the live view, **Then** it says
   that nothing is in flight; with `--json` it prints an empty list.
5. **Given** `serve` isn't running, **When** the operator runs the live view, **Then** it says so
   and exits with an error.

---

### User Story 3 - Timeouts per provider (Priority: P3)

The operator sees that a provider often hangs in one phase and sets that provider's connect,
response-header, first-token or stall timeout. The plugin's declared value is the default. The
operator's per-provider value wins over it. A built-in default covers anything neither sets.

**Why this priority**: Timeouts are the first action latency calls for. They act on one phase and
never cut a stream that is still making progress.

**Independent Test**: Set a short header timeout for a test provider that delays its headers, and
check that the next request's attempt fails with a header timeout and falls over as other
failures do. Then stream from a reasoning model that thinks for longer than the stall timeout
while sending output, and check that the stream is never cut.

**Acceptance Scenarios**:

1. **Given** a plugin declares a 30 s header timeout and the operator sets 10 s for that
   provider, **When** a request goes to it, **Then** 10 s applies, and the effective-settings
   command shows 10 s with "operator" as the source.
2. **Given** the operator removes that override, **When** the next request goes to the provider,
   **Then** the plugin's 30 s applies again.
3. **Given** an attempt exceeds a timeout, **When** it ends, **Then** it is a failed attempt that
   is classified, retried and failed over like other transport failures, and its record names the
   timeout that fired, its value and where that value came from.
4. **Given** a reasoning model sends thinking output with gaps shorter than the stall timeout,
   **When** the total time exceeds every configured timeout, **Then** the stream is not cut.
5. **Given** a request is in flight when the operator changes a timeout, **When** it continues,
   **Then** it keeps the timeout it started with, and the next request uses the new one.

---

### User Story 4 - Proxy per provider (Priority: P4)

The operator routes one provider's traffic, or every provider's, through a proxy they control.
Only the operator can set a proxy. A plugin that declares one fails validation. Proxy credentials
are kept like account secrets and are never shown.

**Why this priority**: A proxy acts on connect and network time, and some operators need one to
reach a provider at all. It comes after timeouts because fewer operators need it.

**Independent Test**: Set a proxy for one provider and check that its requests pass through the
proxy while other providers' requests don't. Set a proxy for all providers and check that a
provider-level "no proxy" exempts that provider. Scan every CLI output, record, live view and log
for the proxy credentials and find none.

**Acceptance Scenarios**:

1. **Given** the operator sets a proxy for provider A, **When** requests go to A and B, **Then**
   only A's traffic passes through the proxy, and A's records name the proxy by its operator-given
   name.
2. **Given** a proxy for all providers and "no proxy" for provider B, **When** requests go to A
   and B, **Then** A's traffic uses the proxy and B's goes direct.
3. **Given** a plugin file that declares a proxy, **When** it is validated or installed, **Then**
   validation fails and names the proxy field as the reason.
4. **Given** a proxy with a username and password, **When** the operator lists settings, shows
   records, opens the live view or the dashboard, or reads logs and error messages, **Then** the
   credentials appear nowhere.
5. **Given** the proxy for provider A is unreachable, **When** a request goes to A, **Then**
   0router tells the operator that the proxy isn't reachable and pauses every provider that uses
   it. The request falls over to providers outside the pause or, if none remain, fails with an
   error that names the unreachable proxy. Nothing is sent without the proxy, and A's accounts
   don't go into cooldown.
6. **Given** a proxy is paused, **When** the operator fixes it and tells 0router so (or changes
   the proxy setting), **Then** 0router checks that the proxy is reachable and resumes traffic
   through it from the next request.

---

### User Story 5 - Connection reuse and HTTP/2 per provider (Priority: P5)

The plugin declares whether its provider supports HTTP/2. The operator can turn connection reuse
and HTTP/2 on or off per provider, and the operator's setting wins over the plugin's declaration.

**Why this priority**: Reuse removes the connect phase from most requests. Being able to turn it
off helps with providers that break long-lived connections. Most operators never need to touch
it.

**Independent Test**: With default settings, two requests in a row to one provider show the
second as reused (connect "not applicable"). After the operator turns reuse off for that
provider, every request shows a connect time. With HTTP/2 on for a provider that supports it,
records show HTTP/2.

**Acceptance Scenarios**:

1. **Given** default settings, **When** two requests go to the same provider in quick succession,
   **Then** the second record shows a reused connection.
2. **Given** the operator turns reuse off for a provider, **When** requests go to it, **Then**
   each opens a new connection and records its connect time.
3. **Given** a plugin declares HTTP/2 support, **When** requests go to that provider, **Then**
   records show the protocol used, and **When** the operator turns HTTP/2 off for that provider,
   **Then** later requests use HTTP/1.1.

---

### User Story 6 - Retry policy per provider (Priority: P6)

The operator sets, per provider, how many times 0router retries the same account after a
transient failure and how long it waits between those retries. Retries spend the operator's
quota, so the operator has the final word.

**Why this priority**: The retry policy decides how long a failing request keeps a client
waiting before 0router falls over to another account or provider. Defaults are fine for most
operators.

**Independent Test**: For a test provider that fails with 503, set a same-account retry count of
1 and a 500 ms wait. Check that the next request's record shows exactly one same-account retry,
about 500 ms apart, before falling over.

**Acceptance Scenarios**:

1. **Given** the operator sets 0 same-account retries for a provider, **When** an attempt to it
   fails transiently, **Then** 0router moves straight to the next account or provider.
2. **Given** the operator sets a retry count and wait for a provider, **When** an attempt fails
   transiently, **Then** 0router retries that many times with that wait, and the record shows the
   wait in the next attempt's router overhead and marks it as a retry wait.
3. **Given** a plugin declares same-account retries for a failure status, **When** neither the
   operator nor anything else overrides them, **Then** they apply; **When** the operator sets that
   provider's retries, **Then** the operator's values apply; **When** a plugin declares more
   retries than the allowed maximum, **Then** it fails validation.

---

### Edge Cases

- **Attempt skipped before sending** (for example, the account is in cooldown): it shows no phase
  times, all phases are "not applicable", and the time spent deciding to skip it is in the next
  attempt's router overhead.
- **Client disconnects mid-request**: the record keeps the phases up to the disconnect, names the
  phase that was running, and marks later phases "not applicable". The live view drops the request
  at its next refresh.
- **Mid-stream break resumed by a later attempt**: each attempt has its own first token and
  generation. The request's time to first token is the first output the client got, from
  whichever attempt sent it.
- **Async media job** (the provider returns a job ID): the request's phases end when 0router
  answers with the job, and polling the job later is not part of this request's phases.
- **Clock jumps**: phases are measured with a clock that never goes backwards, so a wall-clock
  change can't produce negative or inflated phases.
- **Very many requests in flight**: the live view lists all of them. A slow or hung live-view
  client never slows or blocks client requests.
- **Invalid setting** (for example, a zero or negative timeout, a malformed proxy address, a retry
  count above the allowed maximum): the change is refused with a message that names the field, and
  the previous settings stay in force.
- **Setting for an unknown provider**: refused, with the list of known providers.
- **Plugin reload while overrides exist**: overrides stay. An override for a provider that no
  longer exists is kept and reported by `nullrouter check` as unused.
- **Proxy that alters TLS** (a proxy that intercepts encrypted traffic): out of 0router's control.
  0router still verifies the provider's certificate as it does without a proxy.

## Requirements *(mandatory)*

### Functional Requirements

**Phases in records (US1)**

- **FR-001**: Every request record MUST hold, for each attempt (failed, cancelled and skipped
  attempts included), the time spent in each of six phases: router overhead, connect, response
  headers, first token, generation and delivery.
- **FR-002**: The phases MUST be defined as follows:
  - **Router overhead**: for the first attempt, from the request's arrival to the attempt's
    start; for a later attempt, from the end of the previous attempt to this attempt's start.
  - **Connect**: from the attempt's start until the connection to the provider is ready to send
    (name lookup, connection, encryption handshake and any proxy handshake).
  - **Response headers**: from the connection being ready (or from the attempt's start on a reused
    connection) until the provider's response headers arrive.
  - **First token**: from the response headers until the provider's first model output.
  - **Generation**: from the first model output until the provider's last byte of the answer.
  - **Delivery**: time 0router spends waiting for the client to accept data, during the answer or
    after the provider has finished, until the client has the last byte.
- **FR-003**: A phase that didn't happen for an attempt MUST be recorded as "not applicable" and
  MUST NOT be recorded or shown as 0 ms. A record written before this slice MUST show its phases as
  "not recorded".
- **FR-004**: First token MUST mean the first model output of any kind, thinking or reasoning
  output included. Keepalive pings, empty events and protocol framing MUST NOT count as model
  output.
- **FR-005**: Where response headers and first token can't be told apart for an attempt, the
  record MUST show them as one "waiting for provider" phase. It MUST NOT split the time between
  them by guesswork.
- **FR-006**: For an answer that isn't streamed (whole body, embeddings, images, token counts,
  async job IDs), first token MUST end when the whole answer has arrived, and generation MUST be
  "not applicable".
- **FR-007**: A failed or cancelled attempt MUST name the phase it ended in. That phase holds the
  time up to the failure or cancellation, and the later phases are "not applicable".
- **FR-008**: Time 0router spends blocked on the client MUST be counted as delivery, never as
  generation or any other provider phase. Time spent waiting for the provider MUST never be counted
  as router overhead or delivery.
- **FR-009**: For every completed request, the phases of all its attempts MUST add up to the
  request's total time, within 1 ms of rounding. The phases up to the first output the client got
  MUST add up to the request's time to first token.
- **FR-010**: A request's router overhead (its first attempt's router overhead) and its time to
  first token MUST equal the values slice 010's latency view uses for the same request (010's
  Definitions section), so `nullrouter records` and `nullrouter latency` never disagree.
- **FR-011**: When a same-account retry waits deliberately before the next attempt, that wait MUST
  be counted in the next attempt's router overhead, and the record MUST show how much of the router
  overhead was retry wait.
- **FR-012**: Each attempt's record MUST say whether its connection was new or reused, which HTTP
  version it used, and which proxy it went through (by operator-given name), if any.
- **FR-013**: `nullrouter records` MUST show the per-attempt phases in a record's detail view and
  in its `--json` output.
- **FR-014**: Phases MUST be measured from real client traffic only. This slice MUST NOT send any
  request of its own to measure latency.
- **FR-015**: Phase times MUST be taken from a clock that never goes backwards.

**Live view (US2)**

- **FR-016**: A CLI command MUST list every request in flight in the running `serve` at that
  moment, including requests that haven't started an attempt yet. For each request it MUST show:
  the agent, the target (unified or direct model), the provider and account of the current
  attempt, the attempt number, the current phase and the time spent in it so far, the times of
  the phases already finished, and the time since arrival.
- **FR-017**: The live view MUST refresh until the operator quits, at least once per second. A
  request MUST appear at the first refresh after it arrives and MUST be gone at the first refresh
  after it ends.
- **FR-018**: With `--json`, the live view MUST print one snapshot with the same fields and exit.
- **FR-019**: The live view MUST show metadata only. It MUST NOT show prompt content, answer
  content, tool arguments, headers or secrets.
- **FR-020**: The live view MUST require the same operator access as the other operator commands.
  A slow, stalled or crashed live-view client MUST NOT slow or block client requests.

**Connection settings (US3–US6)**

- **FR-021**: The operator MUST be able to set, per provider, a connect timeout, a
  response-header timeout, a first-token timeout and a stall timeout. For each timeout, the
  operator's per-provider value MUST win over the plugin's declared value, which MUST win over the
  built-in default.
- **FR-022**: Built-in default timeouts MUST keep today's behavior, including today's environment
  overrides (constitution VI). A first-token timeout MUST be off unless a plugin or the operator
  sets one.
- **FR-023**: The stall timeout MUST measure time with no data at all from the provider, so
  keepalive pings keep a stream alive. The first-token timeout MUST measure time without model
  output (FR-004), counting thinking output as model output. No timeout MAY cut a stream that is
  still sending model output.
- **FR-024**: An attempt that exceeds a timeout MUST end as a failed attempt. It is classified,
  retried and failed over like other transport failures, and its record names the timeout, its
  value and the value's source (operator, plugin or built-in).
- **FR-025**: The operator MUST be able to set a proxy for a single provider, a proxy for all
  providers, and "no proxy" for a single provider. The per-provider setting MUST win over the
  all-providers setting. Every request 0router sends to that provider (client requests, token
  refreshes, quota polls and model tests) MUST go through the proxy in effect.
- **FR-026**: A plugin that declares a proxy MUST fail validation, with a message that names the
  proxy field.
- **FR-027**: Proxy credentials MUST be stored with the same protection as account secrets. They
  MUST NOT appear in any CLI output, record, live view, dashboard page, log line or error message,
  and MUST NOT be passed to any plugin.
- **FR-028**: When 0router can't reach a proxy, can't complete its handshake, or is refused by it
  (provider errors that come back through the proxy don't count), and an immediate second try
  fails too, 0router MUST:
  - pause that proxy, which pauses all traffic through it: client requests, token refreshes,
    quota polls and model tests for every provider that uses it;
  - tell the operator: a `nullrouter check` error, a line in the `serve` log, a mark in the live
    view and the effective-settings command, and the proxy's name (never its credentials) in the
    record and the client's error;
  - route requests that would have used it as if those providers were unavailable. A request with
    no other candidate fails with an error that names the paused proxy;
  - never send that traffic without the proxy, and never put the accounts behind the proxy into
    cooldown or count the failure against them.

  The pause MUST last until the operator acts: either they tell 0router the proxy is fixed and
  0router finds it reachable, or they change or remove that proxy setting. A pause MUST survive a
  `serve` restart.
- **FR-029**: A plugin MAY declare that its provider supports HTTP/2. The operator MUST be able to
  turn connection reuse and HTTP/2 on or off per provider, and the operator's setting MUST win.
  When nothing is set, connections are reused (003 FR-021), and HTTP/2 is used only where the
  plugin declares support and the provider agrees to it.
- **FR-030**: The operator MUST be able to set, per provider, the same-account retry count and the
  wait between same-account retries, in total or per failure status. A plugin MAY declare
  same-account retries per failure status, up to a maximum that validation enforces; a plugin
  above the maximum MUST fail validation, with a message that names the field. The operator's
  per-provider setting MUST win over the plugin's declaration, which MUST win over the core
  default. The maximum MUST admit every retry declaration shipped today (`grok-cli`,
  `antigravity`, `kiro`, `vercel-ai-gateway`). The core default MUST keep today's behavior
  (003 FR-015, constitution VI). Account cooldown durations aren't part of the retry policy and
  stay as they are.
- **FR-031**: A settings change MUST apply from the next request without restarting `serve`.
  Requests already in flight MUST keep the settings they started with.
- **FR-032**: An invalid setting MUST be refused with a message that names the field, and the
  settings in force MUST stay unchanged.
- **FR-033**: A CLI command MUST show each provider's effective connection settings (every
  timeout, the proxy, reuse, HTTP/2 and the retry policy) with the source of each value (operator,
  plugin or built-in). Proxy credentials MUST be redacted there.
- **FR-034**: `nullrouter check` MUST report overrides that name an unknown provider as unused.

**Routing and cost of measuring**

- **FR-035**: No latency measurement MAY change the routing decision: no provider, account or
  unified-model member is preferred, demoted, skipped or reweighted because it was measured slow
  or fast (constitution VIII).
- **FR-036**: Measuring phases and serving the live view MUST NOT make client requests measurably
  slower. The request-path benchmark MUST show router overhead with phase timing on within the
  run-to-run noise of router overhead without it.

### Key Entities

- **Phase**: One of router overhead, connect, response headers, first token, generation, delivery,
  or the merged "waiting for provider". Its value is a duration, "not applicable" or "not
  recorded".
- **Attempt timing**: The phases of one attempt, the phase it ended in, whether its connection was
  new or reused, its HTTP version, its proxy name, its retry wait and, if a timeout fired, which
  one, with its value and source. Part of the request record.
- **In-flight entry**: A request in flight as the live view shows it: agent, target, provider,
  account, attempt number, current phase and time in it, finished phase times, time since arrival.
  It holds no content. It exists only while the request is in flight.
- **Provider connection settings**: Per provider, the four timeouts, proxy reference, reuse, HTTP/2
  and retry policy, each with its source. The effective value follows operator, then plugin (where
  allowed), then built-in.
- **Proxy**: An operator-named proxy with an address, a scheme and optional credentials, and a
  state: in use, or paused because it was unreachable, with when and why. The credentials are
  stored like account secrets.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In a test run that injects a known delay into each phase in turn (0router's side,
  connect, headers, first output, generation and a slow-reading client), 100% of records show each
  delay in the right phase of the right attempt, within 5 ms or 5%, whichever is larger.
- **SC-002**: For 100% of completed requests in the test suite, the phases add up to the total
  within 1 ms, and the phases up to the first output add up to the time to first token within 1 ms.
- **SC-003**: For 100% of requests in a shared test run, the router overhead and time to first
  token in `nullrouter records` equal those used by `nullrouter latency`.
- **SC-004**: Turning phase timing on doesn't make requests slower than the run-to-run variation of
  the request-path benchmark.
- **SC-005**: With 200 concurrent requests in mixed phases, the live view misses no request in
  flight, shows no ended request after one refresh, and shows each request's current phase within
  one second of the change.
- **SC-006**: A reasoning model that streams thinking for 5 minutes before visible text, and a
  stream with gaps shorter than its stall timeout, complete in 100% of runs with every timeout at
  its default.
- **SC-007**: After a settings change, 100% of requests that start afterwards use the new values,
  and 100% of requests already in flight keep the old ones, with no restart.
- **SC-008**: A secret scan over every CLI output, record, live-view snapshot, dashboard page and
  log produced in the test suite finds proxy credentials 0 times.
- **SC-009**: With one provider of a unified model ten times slower than the other, the placements
  0router makes are identical to a run where both are equally fast.
- **SC-010**: An operator can name the phase and side (0router, network, provider or client) that
  made a slow request slow from one record, without consulting logs.
- **SC-011**: With a proxy made unreachable, 100% of the operator's surfaces listed in FR-028 say
  so before or with the next failed request, 0 requests reach the provider without the proxy, and
  0 accounts behind it go into cooldown. Traffic resumes on the first request after the operator
  marks the proxy fixed and it is reachable.

## Assumptions

- Records keep using today's storage and retention; phase times add a small, bounded amount to
  each attempt's record.
- Connect is measured separately only for new connections; on a reused connection it is "not
  applicable" (FR-012 says why). Whether the HTTP client can tell connect apart from response
  headers on every new connection is checked in the plan; FR-005's merge covers only headers and
  first token.
- 0router's per-chunk translation work during a stream is counted in generation (or delivery when
  blocked on the client), not in router overhead, so router overhead keeps slice 010's meaning.
  It is expected to be negligible next to provider time.
- The live view refresh interval defaults to one second. The command names for the live view and
  the settings are chosen in the plan and shown to the operator for review.
- Supported proxy kinds are HTTP, HTTPS and SOCKS5, each with optional username and password.
- Settings are stored as operator files, written atomically and reloaded by the running `serve`,
  like the other operator settings (`docs/operator-config.md`).
- Retry counts have an upper bound that validation enforces, for plugins and operators alike, so
  a mistake can't spend a quota on retries. The plan sets the bound, at least high enough for the
  retry declarations shipped today.
- A proxy is paused only after a second, immediate connection try fails too, so a single dropped
  connection doesn't stop traffic. Checking whether a paused proxy is reachable again only
  connects to the proxy; it sends nothing to a provider.
- Slice 010's definitions (router overhead, time to first token, own time to first token) are
  reused, not redefined. 010 isn't merged yet, so the plan coordinates with branch
  `010-dashboard-summaries`.
- 9router has no per-phase timing or live view, so US1 and US2 have no parity oracle. Timeout and
  retry defaults are 9router-parity behaviors (constitution VI) and stay so.

## Out of Scope

- Latency trends over time → latency slice 2.
- Latency per unified model → latency slice 2.
- Judging a phase unusual against the provider's own history (the bottleneck call) → latency
  slice 2.
- Phases and the live view on the dashboard → later.
- Proxy pools (0router deploying or testing proxies for the operator) → later.
- Active latency probes; latency is measured from real traffic only (FR-014).
- Any routing change driven by latency (constitution VIII).
