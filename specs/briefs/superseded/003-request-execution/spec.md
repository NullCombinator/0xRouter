# Feature Specification: Request Execution Walking Skeleton

**Feature Branch**: `003-request-execution`

**Created**: 2026-09-27

**Status**: Draft

**Input**: User description: "Request execution walking skeleton — the first end-to-end slice: a client request enters 0router, resolves through the 002 registry, executes against one provider account, and returns. An HTTP server exposes OpenAI Chat Completions (/v1/chat/completions), Anthropic Messages (/v1/messages), embeddings (/v1/embeddings), and model listing (/v1/models). The operator declares connections: one or more API-key accounts per provider. The core holds their secrets and injects them only at execution time, never into plugins. Execution is same-format passthrough only: a request runs only on a provider transport whose wire format matches the client's; a mismatch is a structured error, not a translation. Target selection is a placeholder the routing slice will replace: a unified model uses its first member with an active connection, and a direct <provider>/<model> request uses that provider. There is no retry and no fallback, and upstream errors are returned faithfully. Upstream URL and auth-header construction for API-key providers must match 9router's executors. Streaming responses are relayed chunk by chunk without buffering, and a client disconnect cancels the upstream request. Every request records an observation: agent identity, unified model, provider, connection, upstream model, status, time to first token, total duration, and token usage including cache-read and cache-write tokens, taken from both streaming and non-streaming responses. Observations can be queried per provider and per unified model through the CLI. Out of scope: the routing decision (cache-aware, per-agent, amortization), translators, OAuth flows and token refresh, error classification, cooldowns and fallback, combos, model tests, image/audio/video endpoints, dashboard, persistent usage history, token optimization."

## Clarifications

### Session 2026-09-27

- Q: Must every client request carry a 0router access key? → A: Yes, always, whatever
  address the server listens on. This matches 9router's default, and there is no switch
  to turn it off.
- Q: Where does the agent identity come from? → A: From both sources. The access key
  names the agent (one key per agent), and the client's own session id (read as 9router
  reads it) names the session within that agent. The identity is the pair; the session
  is "none" when the client supplies no id.
- Q: Are the client's own request headers (e.g. Claude Code's `anthropic-beta`)
  forwarded to the provider? → A: Only when the client harness is talking to its own
  vendor's official provider (9router's "native pair", e.g. Claude Code → `anthropic`).
  In that case every client header is forwarded for maximum compatibility, except
  credentials and connection-level headers. Otherwise no client header is forwarded.
- Q: Which provider response headers reach the client, and are they kept? → A: 0router
  records the provider's response headers in every observation, for its own use (rate
  limits and `retry-after` feed the later routing slice). They are passed through to the
  client only for native pairs; otherwise the client gets 0router's own response headers,
  as in 9router.
- Q: When the provider's stream breaks after the client has started receiving it, is
  the client told? → A: Yes, as 9router does. One closing error event is sent in the
  client's format (`event: error` for Anthropic clients; an error frame plus `[DONE]`
  for OpenAI clients), then the stream closes. While checking this, 9router's stream
  stall timeout (360 s, `STREAM_STALL_TIMEOUT_MS`) was found. It replaces the earlier
  "no time limit on the body" in FR-017.
- Q: Are upstream error bodies (including non-JSON ones such as HTML gateway pages)
  returned unchanged? → A: Unchanged for native pairs. Every other request gets
  9router's behavior, which turns every upstream error into its own
  `[<status>]: <message>` error object and every non-JSON, non-stream page into a short
  error built from the page title. The raw body is kept in the observation either way.
- Q: Does 0router answer Claude Code's `/v1/messages/count_tokens`? → A: Yes. For
  native pairs (Claude Code → `anthropic`) the request goes to the provider's own
  count_tokens endpoint for exact counts. Every other request gets 9router's local
  estimate, and nothing is sent upstream.

## User Scenarios & Testing *(mandatory)*

The "users" of this slice are (a) the **operator**, who runs 0router, declares provider
accounts, and reads latency and usage; (b) the **client agent** — Claude Code, an
OpenAI-SDK program, an optimizer hop in front of 0router — which sends standard API
requests and expects standard responses; and (c) the **routing-decision slice** that
follows, which consumes this slice's observations (cache-read tokens, time to first
token) and replaces its placeholder target selection.

### User Story 1 — A client chats with a provider through 0router (Priority: P1)

The operator declares an API key for a provider (e.g. `anthropic` or `deepseek`) and
starts 0router. A client points its base URL at 0router and sends a chat request to a
direct target such as `anthropic/claude-sonnet-4-5`, in the provider's own wire format.
The request reaches the provider with the operator's key, and the response — streamed or
not — reaches the client unchanged, as if the client had called the provider directly.

**Why this priority**: This is the walking skeleton. Until one real request goes through,
nothing else in 0router is usable, and "it works for me" (init.md first milestone) is not
reachable.

**Independent Test**: Declare one connection for a provider, start 0router against a local
mock upstream that records what it receives, send one streaming and one non-streaming
request, and compare (a) what the mock received with what 9router's executor would send,
and (b) what the client received with what the mock sent.

**Acceptance Scenarios**:

1. **Given** a connection for an API-key provider whose transport speaks the OpenAI chat
   format, **When** a client sends a streaming request to `/v1/chat/completions` with
   target `<provider>/<model>`, **Then** the upstream receives the request at the URL and
   with the headers 9router's executor builds for that provider, carrying the operator's
   key and the resolved upstream model ID, and the client receives each upstream event as
   it arrives.
2. **Given** a connection for an API-key provider whose transport speaks the Anthropic
   Messages format, **When** a client sends a request to `/v1/messages`, **Then** the same
   holds for that format, including the provider's declared version and beta headers.
3. **Given** a client request with `stream` off, **When** the provider answers with a
   single response, **Then** the client receives that response body and status.
4. **Given** a provider that declares it must always stream (e.g. `openai`) and a client
   request with `stream` off, **When** the request runs, **Then** the upstream call is
   streamed and the client receives one complete non-streaming response in its own format,
   assembled as 9router assembles it.
5. **Given** a streaming response in progress, **When** the client disconnects, **Then**
   the upstream request is cancelled promptly rather than read to the end.
6. **Given** a provider that answers with an error status and body, **When** the request
   runs, **Then** the client receives that status, with the body unchanged for a native
   pair and as 9router formats it otherwise (FR-020); 0router neither retries nor tries
   another account or provider.
7. **Given** any request, **When** its outbound request is inspected, **Then** the body is
   the client's body with only the model field replaced by the upstream model ID — no
   prompt content is added, removed, or rewritten (Constitution IV).
8. **Given** Claude Code sending `/v1/messages` to `anthropic/<model>` with its own
   `anthropic-beta` and `anthropic-version` headers, **When** the request runs, **Then**
   the upstream receives those client headers (in place of the plugin's declared ones),
   the connection's `x-api-key`, and none of the client's auth headers; **given** the same
   request to a non-Anthropic provider's Anthropic-format transport, **Then** only the
   plugin's declared headers are sent.

---

### User Story 2 — A client uses a unified model and discovers what is available (Priority: P1)

The operator has declared a unified model (002) such as `sonnet`. A client asks for
`sonnet` and 0router executes it on the first member provider for which the operator has
an active connection. A client that lists models sees every target it can actually use.

**Why this priority**: Unified models are the routing target (Constitution III). Proving
that a unified-model request executes end to end — even with a placeholder choice of
member — is what lets the routing-decision slice replace only the choice, not the path.

**Independent Test**: Declare a two-member unified model where only the second member's
provider has a connection; send a request for it and check it ran on the second member;
list models and check the listing.

**Acceptance Scenarios**:

1. **Given** a unified model whose first member's provider has an active connection,
   **When** a client requests it, **Then** the request runs on that member with that
   member's upstream model ID.
2. **Given** a unified model whose first member's provider has no active connection but
   whose second member's does, **When** a client requests it, **Then** the request runs on
   the second member.
3. **Given** a unified model none of whose members has an active connection, **When** a
   client requests it, **Then** the client receives a structured error naming the unified
   model and stating that no member has a usable connection.
4. **Given** a unified model whose selected member's transport cannot serve the client's
   wire format, **When** a client requests it, **Then** the client receives a structured
   format-mismatch error; 0router does not move on to another member (no fallback in this
   slice).
5. **Given** declared unified models and connections, **When** a client lists models,
   **Then** the list contains every unified model with at least one member that has an
   active connection, and every catalogued model of every provider with an active
   connection, addressed as `<provider>/<model>`; each entry carries its model type where
   one is declared.
6. **Given** a bare target that is not a declared unified model, **When** a client sends
   it, **Then** the client receives the registry's structured not-found (002 FR-014a).

---

### User Story 3 — The operator sees latency and cache usage per request (Priority: P1)

After using 0router for a while, the operator asks the CLI how a provider or a unified
model has been performing: how long requests took to produce their first token and to
finish, which succeeded, and how many input tokens were served from the provider's prompt
cache versus written to it.

**Why this priority**: Latency observability is a constitutional requirement (VIII), and
the routing-decision slice cannot be specified concretely without real per-request cache
and latency observations to decide on.

**Independent Test**: Send a known sequence of requests to mock upstreams that report
fixed usage (including cache-read and cache-write counts) and delay their first event by
known amounts; then query observations per provider and per unified model and check
counts, statuses, token figures, and timings.

**Acceptance Scenarios**:

1. **Given** a completed streaming request, **When** observations are queried, **Then**
   one observation exists recording the agent identity, the unified model (if any),
   provider, connection, upstream model, final status, time to first token, total
   duration, and input, output, cache-read, and cache-write token counts as reported by
   the provider.
2. **Given** a completed non-streaming request, **When** observations are queried,
   **Then** the same fields are recorded, taken from the response body's usage.
3. **Given** a request that failed upstream, was rejected before execution, or was
   cancelled by client disconnect, **When** observations are queried, **Then** an
   observation exists with that outcome; token fields the provider never reported are
   recorded as "not reported", never as zero.
4. **Given** observations for several providers and unified models, **When** the
   operator queries by provider or by unified model through the CLI, **Then** only
   matching observations are shown, together with a summary: request count, success
   count, and time-to-first-token and total-duration percentiles (p50, p95).
5. **Given** a provider whose usage format reports cached input separately (Anthropic
   Messages: cache read and cache creation; OpenAI chat: cached prompt tokens), **When**
   usage is recorded, **Then** cache-read and cache-write figures are extracted per that
   format with the same meaning 9router's usage extraction gives them.

---

### User Story 4 — A client requests embeddings (Priority: P2)

A client sends an embeddings request to `/v1/embeddings` for a direct or unified target
whose provider offers embeddings through an OpenAI-compatible endpoint. The request is
executed and the embeddings are returned.

**Why this priority**: 0router is a model manager, not a text-generation manager
(Constitution III). A second, non-chat model type in the first execution slice keeps the
execution path from hard-coding "chat". It is P2 because chat is what makes the slice
usable.

**Independent Test**: Declare a connection for an embeddings provider, send an
embeddings request to a mock upstream, and check the outbound request and the returned
body.

**Acceptance Scenarios**:

1. **Given** a connection for a provider 9router serves through its OpenAI-compatible
   embeddings adapter, **When** a client requests embeddings for one of its embedding
   models, **Then** the upstream receives the URL, headers, and body 9router's adapter
   builds, and the client receives the provider's response.
2. **Given** a target whose model is not an embedding model or whose provider does not
   offer embeddings, **When** a client requests embeddings, **Then** the client receives a
   structured error saying the target does not offer embeddings.
3. **Given** a completed embeddings request, **When** observations are queried, **Then**
   an observation exists with its duration and the input tokens the provider reported.

---

### User Story 5 — The operator manages accounts and agent keys without leaking them (Priority: P2)

The operator declares one or more API-key accounts per provider, and one 0router access
key per agent. Accounts and keys can be marked active or inactive and changed while
0router runs. Neither kind of key ever appears in plugins, logs, observations, CLI
output, or error messages.

**Why this priority**: Secrets isolation is constitutional (I), and the operator needs to
rotate keys and add accounts without restarting. It is P2 because a single static account
already makes P1 work.

**Independent Test**: Declare accounts, reload with changes, and check which account runs
each request; scan all outputs for the key values.

**Acceptance Scenarios**:

1. **Given** two active accounts for one provider, **When** a request targets that
   provider, **Then** it runs on the first active account in declaration order.
2. **Given** the first account marked inactive, **When** a request targets that
   provider, **Then** it runs on the second.
3. **Given** a changed account list, **When** the operator issues a reload, **Then** new
   requests use the new list, requests already in flight finish with the account they
   started on, and an invalid account file leaves the previous accounts in service with
   the errors reported (as 002 FR-024 does for the registry).
4. **Given** any request, error, observation, log line, or CLI output, **When** it is
   scanned for the declared key values, **Then** none is found.
5. **Given** an account declared for a provider id or alias that the registry does not
   know, or for a provider that is not an API-key provider supported by this slice,
   **When** accounts load, **Then** it is rejected with an error naming the account and
   the reason.
6. **Given** two access keys declared for agents `laptop` and `ci`, **When** a request
   arrives with the `ci` key in either the `Authorization: Bearer` or the `x-api-key`
   header, **Then** it is accepted and its observation names agent `ci`; **given** a
   request with no key or an unknown or inactive key, **Then** it is rejected as
   unauthorized and nothing is sent upstream.
7. **Given** two Claude Code conversations running under the same access key, **When**
   their requests are observed, **Then** both carry the same agent and different
   sessions, taken from each conversation's own session id; **given** a client that sends
   no session id, **Then** the session is recorded as "none".

---

### Edge Cases

- A client wire format with no matching transport and no model-level target format that
  equals it (e.g. an Anthropic Messages request to a provider that only speaks OpenAI
  chat): structured format-mismatch error naming the client format and the formats the
  provider's transports speak; nothing is sent upstream.
- A provider with several transports (e.g. `minimax` with OpenAI and Anthropic
  endpoints): the transport matching the client's format is chosen, unless the model
  declares supported formats that exclude it — the same choice 9router makes; the chosen
  transport's own URL, headers, and auth scheme apply.
- A model whose declared target format differs from the client's format and whose
  provider has no matching transport: format mismatch (no translation in this slice).
- A provider whose URL needs account-specific data (e.g. `cloudflare-ai`'s account id):
  the connection supplies it; a connection without it is rejected at load with an error
  naming the missing value.
- A provider that 9router executes with a provider-specific executor rather than its
  generic one (e.g. `azure`, `vertex-partner`, `opencode-go`), or that does not
  authenticate with an API key (OAuth, web-cookie, no-auth free providers such as
  `mimo-free`): not executable in this slice; requests to it return a
  structured "provider not supported yet" error, and connections for it are rejected at
  load.
- A direct target for an uncatalogued model on a provider with "allow uncatalogued
  models" on (002 FR-017): executed with the requested model ID unchanged.
- The upstream does not send response headers within 9router's connect timeout
  (60 seconds): the client receives a structured gateway-timeout error and the
  observation records a timeout; no retry.
- The upstream is unreachable (DNS, refused connection, TLS failure): structured
  bad-gateway error naming the provider, never the key; observation records the failure.
- The upstream stream ends without a usage event: the observation records tokens as "not
  reported"; the client still receives the stream as sent.
- The upstream stream breaks mid-response: everything received so far is relayed, then
  one closing error event in the client's format ends the stream (FR-018a). The
  observation records an incomplete stream with the time to first token it reached.
- The upstream stream goes silent without closing: after the stall timeout (FR-017) the
  upstream is cancelled and the stream ends as above; the observation records a stall.
- The client disconnects mid-stream: no closing error event is written (there is no one
  to receive it); the upstream is cancelled (FR-021) and the observation records
  "cancelled by client".
- A reload while a stream is in flight: the stream finishes on the registry version,
  unified-model resolution, and account it started with.
- A request body that is not valid JSON, or lacks a model: structured bad-request error
  in the client's wire format; nothing sent upstream.
- A client that sends its own provider auth headers: they are not forwarded; the
  operator's account key is used, as 9router does. The header that carried the 0router
  access key is never forwarded either, even for a native pair.
- Claude Code counting tokens for `anthropic/<model>`: forwarded to Anthropic's own
  token-count endpoint. Claude Code counting tokens for a `minimax` target, or for a bare
  name that is not a unified model: answered locally with the estimate (FR-001a).
- Claude Code calling a non-Anthropic provider through its Anthropic-format transport
  (e.g. `minimax`): not a native pair, so only the plugin's declared headers are sent;
  its `anthropic-beta` flags are dropped.
- Claude Code calling `anthropic` with an `anthropic-beta` value that differs from the
  plugin's declared one: the client's value is sent (FR-015a).
- A 429 from `anthropic` to Claude Code: the client receives the status, body, and the
  provider's `retry-after` and rate-limit headers. The same 429 from a non-native
  provider reaches the client without those headers, and the observation records them
  in both cases.
- A request with no access key, an unknown key, or an inactive key: unauthorized error in
  the client's wire format, nothing resolved or sent; the observation records the
  rejection without an agent name.
- A client that sends several session carriers (e.g. Claude Code's body metadata and a
  `x-session-id` header): the highest-priority one wins (FR-005a).
- A session value that is empty, whitespace-only, or longer than 256 characters: treated
  as absent, as 9router treats it.
- Two clients streaming concurrently through the same account: both are relayed
  independently; neither waits for the other.
- Many observations over a long run: memory use stays bounded (oldest observations are
  dropped first once the configured cap is reached).

## Requirements *(mandatory)*

### Functional Requirements

**Client-facing surface (Constitution IV)**

- **FR-001**: The system MUST accept chat requests in OpenAI Chat Completions format at
  `/v1/chat/completions` and in Anthropic Messages format at `/v1/messages`, embeddings
  requests in OpenAI format at `/v1/embeddings`, token-count requests in Anthropic format
  at `/v1/messages/count_tokens`, and model-listing requests at `/v1/models`. The
  client's wire format MUST be determined by the endpoint it calls.
- **FR-001a**: A token-count request MUST be answered as follows:
  - If the client harness and the target's provider form a native pair (FR-015a), it
    MUST be forwarded to that provider's official token-count endpoint, with the same
    header, body, and error rules as a native-pair chat request (FR-015a, FR-016,
    FR-020, FR-020a). If this fails, the request fails; there is no fallback to the
    estimate.
  - Otherwise, it MUST be answered locally with 9router's estimate, and nothing is sent
    upstream. The estimate counts characters in the system prompt, tools, and messages,
    as 9router counts them: text, tool-use name and input, tool-result content,
    thinking text, and every other value recursively, including object keys. The result
    is `{"input_tokens": ceil(chars / 4)}`. The target does not need to resolve.
  - A body that is not valid JSON MUST be rejected as a bad request.
- **FR-002**: The target MUST be taken from the request's model field and resolved
  through the 002 registry: a name with `/` is a direct target, a bare name a unified
  model (002 FR-014a). Registry not-found results MUST be returned to the client as
  structured errors.
- **FR-003**: Errors originating in 0router (not-found, format mismatch, no usable
  connection, provider not supported, malformed request, upstream unreachable, upstream
  timeout) MUST be returned in the client's wire format with a stable machine-readable
  error type and a message naming the target, and MUST NOT contain credentials.
- **FR-004**: Every request to the client-facing surface MUST carry a valid 0router
  access key, whatever address the server listens on. The key MUST be accepted from an
  `Authorization: Bearer` header or, failing that, an `x-api-key` header, on every
  endpoint, as 9router reads it. A missing or unknown key MUST be rejected with an
  unauthorized error in the client's wire format before target resolution. There is no
  setting that turns this check off.
- **FR-004a**: The operator MUST declare access keys in their own state, each with a
  unique agent name, the key (literal or environment reference, under the same rules as
  FR-007), and an active flag. Access keys are secrets under FR-009: they MUST NOT appear
  in observations, logs, CLI output, or error responses; only the agent name does. Access
  keys take part in reload (FR-010).

**Agent identity**

- **FR-005**: Every request MUST be attributed to an agent identity made of two parts,
  both recorded in its observation, so that the routing-decision slice can keep per-agent
  state (Constitution II):
  - **agent**: the name of the access key that authenticated the request (FR-004a),
    always present;
  - **session**: the client's own conversation id, when the client supplies one, or
    "none".
- **FR-005a**: The session MUST be read from the request as 9router reads a
  client-supplied session id, in its priority order: the Claude Code session in the
  body's `metadata.user_id` or the `x-claude-code-session-id` header; then the
  `x-session-id`, `session-id`, `session_id`, `x-amp-thread-id` headers; then the
  `x-client-request-id` header; then the body's `prompt_cache_key`, `session_id`,
  `conversation_id`, or `metadata.user_id`. 9router's generated fallbacks (a hash of
  earlier assistant text, a per-connection random id, the Antigravity envelope) MUST NOT
  be used: when the client supplies none, the session is "none".
- **FR-005b**: Sessions MUST be scoped to their agent: the same session value sent under
  two different access keys MUST be two different identities. The session value MUST be
  read, never written back into the outbound request.

**Connections (Constitution I)**

- **FR-006**: The operator MUST be able to declare zero or more connections per provider,
  each with a name, the provider (by id or alias), an API key, an active flag (default
  active), and any account-specific values the provider's URL needs.
- **FR-007**: Connections MUST be declared outside plugins, in the operator's own state.
  An API key MAY be given as a literal value or as a reference to an environment variable.
  If a file holding literal keys is readable by users other than its owner, loading MUST
  fail with an error saying so.
- **FR-008**: Connections MUST be validated at load: the provider exists; it is an
  API-key provider executable in this slice (FR-013); the name is unique within the
  provider; a referenced environment variable is set; required account-specific values
  are present. Each rejection MUST name the connection, field, and rule.
- **FR-009**: An API key MUST be read by the core only when building an outbound request
  and MUST NOT appear in plugins, observations, logs, CLI output, error responses, or the
  model listing.
- **FR-010**: Connections MUST take part in the explicit reload of 002 FR-024: validated
  in full before activation, swapped atomically together with the registry, old set kept
  on any error. A request MUST keep the connection it started with for its whole lifetime.

**Placeholder target selection**

- **FR-011**: For a direct target, the provider is the one the target names. For a
  unified model, the member MUST be the first member, in declaration order, whose provider
  has at least one active connection. The connection MUST be the first active connection
  of that provider in declaration order. This selection is deliberately minimal and MUST
  be isolated so that the routing-decision slice replaces it without changing the
  execution path.
- **FR-012**: If no member (or, for a direct target, the provider) has an active
  connection, the request MUST fail with a structured "no usable connection" error. There
  MUST be no retry, no account rotation, and no fallback to another member or provider
  after a request has been sent.

**Execution (Constitution VI)**

- **FR-013**: The providers executable in this slice MUST be exactly those that
  authenticate with an operator API key (category apikey, or freeTier providers that are
  not no-auth — e.g. `openrouter`, `cloudflare-ai`), that 9router executes with its
  generic executor, and whose chosen transport speaks OpenAI chat or Anthropic Messages.
  Requests to any other provider MUST fail with a structured "provider not supported yet"
  error before anything is sent.
- **FR-014**: The transport MUST be chosen as 9router chooses it: the transport matching
  the client's wire format, if the model's declared supported formats allow it (or
  declare none); otherwise the model's declared target format, else the provider's
  default format. If the chosen target format differs from the client's wire format, the
  request MUST fail with a structured format-mismatch error; no translation is performed.
- **FR-015**: The outbound URL and headers MUST match what 9router's generic executor
  builds for the same provider, transport, model, connection, and streaming mode: base
  URL, URL suffix, account-specific substitutions, declared static headers, auth header
  name and scheme, API version header, and the streaming accept header. Client request
  headers MUST NOT be forwarded, except under FR-015a.
- **FR-015a**: When the client harness and the provider form a native pair, the client's
  request headers MUST be forwarded upstream for maximum compatibility. The client
  harness is detected from the request as 9router's client detection does it (user
  agent, `x-app`, `originator`). The native pairs are 9router's: Claude Code ↔
  `anthropic`, Codex ↔ `codex`, Gemini CLI ↔ `gemini-cli`, Antigravity ↔
  `antigravity`; of these, only Claude Code ↔ `anthropic` is executable in this slice
  (FR-013). A forwarded client header MUST take precedence over the same header declared
  by the plugin. Never forwarded: the `Authorization` and `x-api-key` headers (which
  carry the 0router access key and are replaced by the connection's auth), `Host`,
  `Content-Length`, and connection-level (hop-by-hop) headers. This is a deliberate
  deviation from 9router, which forwards no client headers even for native pairs.
- **FR-016**: The outbound body MUST be the client's body with the model field set to the
  resolved upstream model ID and, for providers that must always stream, streaming turned
  on. Nothing else in the body may change for chat requests. Embeddings requests MUST be
  built as 9router's OpenAI-compatible embeddings adapter builds them.
- **FR-017**: Timeouts MUST match 9router's, including its defaults and environment
  overrides (Constitution VI):
  - **Connect timeout**: if the upstream does not return response headers within 60
    seconds (overridable by `FETCH_CONNECT_TIMEOUT_MS`), the request MUST fail with a
    structured gateway-timeout error.
  - **Stream stall timeout**: once a stream has started, if no bytes arrive from the
    upstream for 360 seconds (overridable by `STREAM_STALL_TIMEOUT_MS`, or by the
    provider's declared stall timeout), the upstream request MUST be cancelled and the
    stream ended per FR-018a.
  - An override applies only if it parses as a positive integer, using 9router's
    leading-digits parsing (so `"120000abc"` → 120000); otherwise the default applies.

**Response relay (Constitution V)**

- **FR-018**: A streaming upstream response to a streaming client request MUST be relayed
  event by event as each event arrives, without waiting for later events and without
  altering event content.
- **FR-018a**: If a stream breaks after the client has started receiving it (upstream
  connection lost, stall timeout, upstream read error), the system MUST append one
  closing error event in the client's wire format, as 9router does, and then close the
  stream. For Anthropic Messages clients this is an `event: error` frame. For OpenAI
  clients it is an error data frame followed by `[DONE]`. The error carries a
  gateway-timeout status and a message saying why the stream ended. The system MUST NOT
  fabricate a successful finish. This applies to native pairs as well. Bytes relayed
  before the break are unchanged.
- **FR-019**: When a provider must always stream but the client asked for a non-streaming
  response, the system MUST assemble the streamed events into one non-streaming response
  in the client's wire format, matching 9router's assembly for that format.
- **FR-020**: An upstream error response (any non-success status) MUST be returned to the
  client with the upstream status. For native pairs (FR-015a) the body MUST be unchanged.
  For every other request the body MUST be the error 9router builds for it: the message
  extracted from the upstream body (its `error.message`, else `message`, else `error`,
  else the raw text), formatted as `[<status>]: <message>` inside 9router's error
  object with the type and code 9router assigns to that status.
- **FR-020b**: When a streaming request's upstream answers with a success status but a
  content type that is neither an event stream nor JSON (e.g. an HTML gateway page), a
  native-pair client MUST receive the response unchanged. Every other client MUST receive
  9router's short JSON error, with the same status: the message is the page's `<title>`
  with tags and line breaks removed, capped at 160 characters. If there is no title, it
  is the tag-stripped body when that is under 200 characters, otherwise a generic
  "non-SSE response" message.
- **FR-020c**: In every case of FR-020 and FR-020b, the observation MUST keep the
  upstream's raw error body, truncated to the first 8 KiB.
- **FR-020a**: For native pairs (FR-015a), the provider's response headers MUST be passed
  through to the client, success or error, except `Set-Cookie`, `Content-Length` where
  the body is re-framed, and connection-level (hop-by-hop) headers. For every other
  request the client MUST receive only 0router's own response headers (content type and
  streaming headers), as 9router sends them.
- **FR-021**: When the client disconnects before the response completes, the system MUST
  cancel the upstream request and stop reading from it.

**Observations (Constitution VIII)**

- **FR-022**: Every request MUST produce exactly one observation, including requests
  rejected for a missing or invalid access key (recorded without an agent). An
  observation contains: request time, agent identity (agent and session), client
  endpoint, target as
  requested, unified model (if any), provider, connection name (never its key), upstream
  model ID, streaming mode, outcome (success, upstream error with status, rejected before
  execution with error type, connect timeout, unreachable, cancelled by client, stream
  stalled, incomplete stream), time to first token (for streamed responses: first event received from
  upstream; for non-streamed: full response received), total duration, and — where the
  provider reports them — input, output, cache-read, and cache-write token counts. It
  MUST also contain the provider's response headers, whether or not they were passed to
  the client (FR-020a), excluding `Set-Cookie` and connection-level headers, so that
  rate-limit, `retry-after`, and request-id values are available to later slices.
- **FR-023**: Token counts MUST be extracted from the usage the provider reports, in the
  final usage-bearing event of a stream or in the body of a non-streaming response, using
  9router's field meanings for each wire format. Counts the provider did not report MUST
  be recorded as "not reported", distinct from zero.
- **FR-024**: Recording an observation MUST NOT delay the relay of any event to the
  client.
- **FR-025**: Observations MUST be held in memory up to an operator-configurable cap
  (default 10,000), oldest dropped first. They are lost on restart in this slice.
- **FR-026**: The operator MUST be able to query the running server's observations
  through the CLI, filtered by provider or by unified model (and optionally by agent,
  session, endpoint, and time range). Token-count requests are recorded like any other,
  with outcome "estimated locally" when no upstream was called. By default they are
  excluded from latency summaries. Queries return results as a list and as a summary:
  request count, success count, and p50/p95 time to first token and total duration.

**Operator control**

- **FR-027**: The operator MUST be able to trigger the reload of 002 FR-024 (now
  including connections) on the running server through the CLI and see the load report or
  the errors that rejected it.
- **FR-028**: Operator commands (reload, observation queries) MUST NOT be reachable by
  clients of the client-facing surface.

**Model listing**

- **FR-029**: `/v1/models` MUST list, in OpenAI list format, every unified model with at
  least one member whose provider has an active connection, and every catalogued model of
  every provider with an active connection as `<provider>/<model>`, with the declared
  model type where present.

### Key Entities

- **Connection**: One operator-declared account for one provider: name, provider, API key
  (literal or environment reference), active flag, account-specific values. Owned by the
  operator; never part of a plugin.
- **Client request**: What arrived: endpoint (and so wire format), target, streaming mode,
  body, agent identity.
- **Execution target**: The outcome of placeholder selection: provider entity, member
  (if unified), upstream model ID, chosen transport, connection.
- **Observation**: One record per request: identity, target, outcome, timings, token
  usage, provider response headers. Input to the routing-decision slice and the future dashboard.
- **Observation summary**: Aggregate over a filtered set of observations: counts and
  latency percentiles.
- **Access key**: An operator-declared 0router key naming one agent: agent name, key
  (secret), active flag. Every client request authenticates with one.
- **Agent identity**: The pair (agent, session). The agent comes from the access key and
  is always present. The session is the client-supplied conversation id, or "none". The
  routing-decision slice keeps independent cache bookkeeping per identity.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: For every provider executable in this slice (FR-013), for each of its
  transports, in both streaming and non-streaming mode, for requests from clients that do
  not form a native pair with it, the outbound URL and headers differ from 9router's
  generic executor output for the same inputs in zero fields (credentials compared by
  position, not value). For native-pair requests, the outbound headers equal that output
  overlaid with the client's headers, minus the excluded set of FR-015a.
- **SC-002**: The operator can point Claude Code (Anthropic Messages) and an OpenAI-SDK
  client at 0router and complete a streamed conversation with a real provider on the
  first attempt, using only the operator documentation.
- **SC-003**: Against a local mock upstream, 0router adds under 5 milliseconds at the
  median to time to first token compared with calling the mock directly, and each
  upstream event reaches the client without waiting for the next one.
- **SC-004**: After a client disconnects mid-stream, the upstream connection is closed
  within 1 second in 100% of test runs.
- **SC-005**: For a test corpus of upstream responses with known usage (both wire
  formats, streamed and not, with and without cache fields), 100% of recorded token counts
  match the known values, and 100% of absent fields are recorded as "not reported".
- **SC-006**: A scan of all observations, logs, CLI output, error responses, and outbound
  upstream requests produced by the full test suite finds zero occurrences of any declared
  0router access key, and zero occurrences of any provider API key outside the auth header
  of that provider's own outbound requests.
- **SC-007**: 100 concurrent streaming requests through one account all complete with
  every event delivered, with none serialized behind another.
- **SC-008**: 100% of upstream error responses in the test corpus reach the client with
  the upstream status. For native pairs the body is byte-identical. For every other
  request it equals the error 9router returns for the same upstream response, with zero
  differences.
- **SC-009**: 100% of requests without a valid access key are rejected before anything is
  sent upstream.
- **SC-010**: For a corpus of requests covering every session carrier in FR-005a, their
  combinations, and the empty and over-long cases, the recorded session matches the value
  9router's client-session extraction returns for the same request in 100% of cases.
  Where 9router would fall back to a generated id, 0router records "none".
- **SC-011**: For a corpus of token-count bodies (strings, content blocks of every kind,
  tools, nested objects), the local estimate equals 9router's for the same body in 100%
  of cases.

## Assumptions

- Target selection here is a placeholder by design. Cache-aware routing, per-agent
  isolation, windowed amortization, account rotation, cooldowns, and fallback belong to
  later slices and replace FR-011/FR-012 without changing the execution path.
- Same-format passthrough only: requests forward only when the chosen transport speaks
  the client's wire format. OpenAI Responses (`/v1/responses`) and translation between
  formats are left to the translator slice; providers whose only transport is OpenAI
  Responses are therefore "format mismatch" for both endpoints here.
- 9router's passthrough body adjustments — Claude cache re-anchoring and prompt
  normalisation, unsupported-parameter stripping, reasoning-content injection, the
  `client_metadata` drop quirk, the json-schema fallback, per-model strip lists — are not
  applied in this slice. The strictest reading of Constitution IV is taken until each is
  specified on its own; cache re-anchoring is revisited with the routing-decision slice,
  since it changes cache behaviour.
- 9router's executor-level retries (same-URL retry on configured statuses, URL fallback
  on 429) are not inherited here; "no retry" is an explicit slice boundary.
- Providers with provider-specific executors, OAuth, no-auth free, and web-cookie
  providers are not executable in this slice; OAuth flows and token refresh have their
  own slice. From the pinned bundled set, roughly 40 providers qualify under FR-013
  (the exact list is fixed during planning from the generator's output).
- User-defined OpenAI-compatible or Anthropic-compatible endpoints are added as user
  plugins (002), not as per-connection base URLs.
- Only the OpenAI-compatible embeddings adapter is in scope; Gemini and self-hosted
  embeddings are not.
- The CLI talks to the running server through an operator-only channel; how that channel
  is protected is a planning decision constrained by FR-028.
- Observations are in memory only; persistent usage history and the dashboard are out of
  scope.
- Parity of URL and header construction is checked against outputs generated from the
  pinned `ref/9router` by the existing generator (as for 002's fixtures), not against
  hand-written expectations.
- Out of scope: routing decision, translators, OAuth flows and token refresh, error
  classification, cooldowns and fallback, combos, model tests, image/audio/video
  endpoints, dashboard, persistent usage history, token optimization.
