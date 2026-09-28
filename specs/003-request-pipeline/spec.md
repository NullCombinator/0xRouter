# Feature Specification: Request Pipeline

**Feature Branch**: `003-request-pipeline`

**Created**: 2026-09-27

**Status**: Draft

**Scope brief**: [specs/briefs/2026-09-27-request-pipeline.md](../briefs/2026-09-27-request-pipeline.md). In
clarify and plan, an answer that contradicts a confirmed row of that brief's ledger means stop and
revisit the brief.

**Input**: User description: "Request pipeline: the first slice a client can use end to end. Any standard client (Claude Code, Codex CLI, Gemini CLI, OpenAI or Anthropic SDKs, an optimizer hop in front of 0router) sends requests in one of four client API styles: OpenAI Chat Completions, Anthropic Messages, OpenAI Responses, Gemini generateContent. 0router serves them from the operator's API-key accounts on the chosen providers: anthropic, openrouter, opencode-zen, opencode-go and elevenlabs. Any client style reaches any provider, because the core translates between styles. Model types are first-class, not text-first: text, embeddings, image, text-to-speech, speech-to-text and video all run through the same pipeline, with the same retry, fallback, records and unified models. Each client can list, in its own API style, the models it can use: unified and direct, of every type. Token counting is a generic core capability; each provider declares whether and how it counts. The core is generic and the specifics are data. The four client API styles are data files the core interprets, as provider plugins are. Each provider's specifics are declared in its plugin: endpoints, headers, error placement, token-counting support, support for continuing from a partial answer, which client headers go upstream, and which provider headers or bodies come back. The core applies forwarding declarations under a security floor: credentials are never forwarded. A failure doesn't reach the client when something else can serve. On a transient failure, the same account is retried first, because it holds the agent's warm cache. Then the request moves to another account, or to another member provider of the same unified model. An agent's next request goes back to where its cache is warm. If a stream breaks after the client has received output, 0router continues the same stream from the partial answer on another account or provider, wherever the provider declares support. Otherwise it does what the operator chose, an informational error event or a restart from the beginning: a default that can be overridden per agent key. Avoiding wasted tokens is a core duty: upstream connections are kept stable and reused. When every option fails, the client gets an informational error in its own API style: a readable summary of what was tried and why each attempt failed in the standard message field, the same details in a structured extra field, and a request-record id in a response header and in the message. Every client request carries a 0router access key, one key per agent. The agent identity is the key plus the client's own session id when the client sends one. Every request is recorded with the agent, unified model, provider and account it used, what was tried and why each attempt failed, time to first token, total duration, and token usage including cache-read and cache-write tokens. Records are kept in memory and queried from the CLI per provider, per unified model, and by record id. The operator manages accounts and agent keys through the CLI. Providers outside the chosen five move out of the bundle into an installable community plugin set. Each one runs if it fits what the core handles, and is otherwise refused with a clear 'not supported by this core' message. The elevenlabs plugin gains a speech-to-text section. The slice fails if: a standard client breaks on 0router; a single 429, 5xx or timeout reaches the client when another account or provider could serve; latency or usage numbers (cache tokens included) are missing or don't match the provider; a secret appears in a log, record, error, or anything a plugin sees. Tests use more than one client harness. Constraints: plugins are data, never code, and never hold secrets; the core injects secrets only at execution; streams are relayed without buffering, and a client disconnect cancels the upstream request; requests are forwarded as received, and prompt content is never rewritten; translation, error classification and retry semantics follow 9router's behaviour; execution and streaming hot paths carry Criterion benchmarks. Out of scope: account sign-in (OAuth) and the xai and grok-cli providers → slice 004; the routing decision (cache-aware routing, per-agent isolation, windowed amortization) and persistent request history → slice 005; combos, model tests, the dashboard, and plugin-declared OAuth for any provider → later; web search → never (outside 0router's scope); token optimization → never (an upstream hop)."

## Clarifications

### Session 2026-09-27

- Q: How does a client reach a model type that its API style has no endpoint for? → A: The
  styles are front doors for client compatibility and aren't tied to model types. One access key
  works at every door. A model of any type is reachable through any style that has an endpoint
  for it, and 0router invents no endpoints.
- Q: When a community plugin fits only partly, is it refused whole or loaded in part? → A:
  Refused whole. The message names every unsupported part.
- Q: What is the shipped default when a stream breaks after partial output and no target can
  continue it? → A: Restart from the beginning on the next account or provider. The operator can
  switch the default to an informational error event, and override it per agent key.
- Q: After a restart, what does the client see, given that it already holds half an answer? →
  A: A short visible note in the answer (for example "— connection lost, answer restarted —"),
  then the new answer.
- Q: Are the technical defaults in Assumptions (try order before slice 005, persistence of
  accounts and keys, record cap, latency targets) accepted? → A: The user delegates technical
  decisions to Claude. They stand as Claude's technical decisions, without user sign-off.

## User Scenarios & Testing *(mandatory)*

This slice has three kinds of user:

- The **operator** runs 0router. They add provider accounts (API keys), issue access keys to agents,
  choose failure behaviour, and read request records from the CLI.
- The **client** (agent) is any standard harness or SDK: Claude Code, Codex CLI, Gemini CLI, an
  OpenAI or Anthropic SDK, or an optimizer hop in front of 0router. It speaks one of the four
  client API styles and must not be able to tell 0router apart from the service it expects,
  except by the extra information 0router adds.
- The **plugin author** declares a provider or an API style as data.

### User Story 1 — A client gets an answer through 0router (Priority: P1)

The operator adds an API-key account for a chosen provider and issues an access key to an agent.
The agent's client, pointed at 0router with that key, sends a text request in its own API style,
for a unified model or a direct `<provider>/<model>`. It receives the answer, streamed or not,
exactly as its API style defines, whichever provider actually served it. The request is recorded.

**Why this priority**: Nothing else in the slice is usable until a real client can get an
answer end to end.

**Independent Test**: Configure one account on each chosen text provider and one agent key.
Send the same prompt from at least two different client harnesses, each in a different API
style, to each provider, streamed and not streamed. Every client completes normally and every
request appears in the CLI record listing.

**Acceptance Scenarios**:

1. **Given** an anthropic account and an agent key, **When** a Chat Completions client streams a
   request for an anthropic model, **Then** it receives a valid Chat Completions stream, and the
   usage it sees matches what anthropic reported.
2. **Given** an openrouter account, **When** a Messages client (for example Claude Code) sends a
   request for an openrouter model, **Then** it receives a valid Messages response or stream.
3. **Given** accounts on opencode-zen and opencode-go, **When** a Responses client and a Gemini
   generateContent client each send a request, **Then** each receives a valid response in its
   own style.
4. **Given** a request with no access key, or an unknown or revoked one, **When** it reaches
   0router, **Then** it is rejected in the client's own style with an authentication error, and
   nothing is sent upstream.
5. **Given** a client that sends its own session id, **When** it makes requests, **Then** the
   records show the agent as the access key plus that session id. Two sessions under one key are
   distinguishable.
6. **Given** a streamed request, **When** the client disconnects mid-stream, **Then** the
   upstream request is cancelled and the record shows the request as cancelled by the client.
7. **Given** an optimizer hop that rewrote the prompt before 0router, **When** it forwards the
   request, **Then** 0router sends the prompt content upstream unchanged.

---

### User Story 2 — A transient failure doesn't reach the client (Priority: P1)

A transient failure (429, 5xx, timeout, connection failure) on the account serving an agent is
absorbed. 0router first retries the same account, because it holds the agent's warm cache. If
that doesn't help, it moves to another account of the same provider, then to another member
provider of the same unified model. The agent's next request goes back to where its cache is
warm. Only when every option fails does the client see an error. That error tells it, in its
own API style, what was tried and why each attempt failed.

**Why this priority**: Without retry and fallback, every minor provider problem crashes the
client's session. The user named "a single 429, 5xx or timeout reaches the client when another
account or provider could serve" as a failure of this slice.

**Independent Test**: With a controllable fake upstream, inject each transient failure kind
on the first account of a unified model that has two accounts and two member providers.
Observe from two client harnesses that none of the injected failures reaches the client, that
the retry order is same account, then other account, then other member, and that the next
request returns to the first account.

**Acceptance Scenarios**:

1. **Given** the serving account returns one 503, **When** the request is retried on the same
   account and succeeds, **Then** the client sees only the successful answer, and the record
   lists both attempts.
2. **Given** the serving account keeps failing transiently, **When** its retries are used up,
   **Then** the request moves to another account of the same provider, and then to another
   member provider of the unified model.
3. **Given** an agent was moved off its account by a transient failure, **When** that agent's
   next request arrives and the original account is available again, **Then** the request is
   sent to the original account.
4. **Given** a direct `<provider>/<model>` request, **When** all of that provider's accounts
   fail, **Then** 0router does not switch to a different provider. It returns the informational
   error.
5. **Given** an upstream error that 9router's classification does not treat as a fallback
   trigger (for example an unmatched 4xx caused by the request itself), **When** it occurs,
   **Then** 0router does not retry or fall back. The client receives the error in its own style,
   with the informational details.
6. **Given** every account and member fails, **When** the client receives the error, **Then**:
   the style's standard message field holds a readable summary of each attempt and its reason;
   a structured extra field holds the same details; the record id appears both in a response
   header and in the message; and the client's official SDK parses the error without crashing.
7. **Given** that record id, **When** the operator looks it up with the CLI, **Then** they see
   the full attempt history.

---

### User Story 3 — Every model type runs through the same pipeline (Priority: P1)

Embeddings, image generation, text-to-speech, speech-to-text and video requests go through the
same pipeline as text: access key, unified models, retry, fallback, informational errors and
records. None of them is a second-class side path.

**Why this priority**: The user rejected 9router's text-first design. A pipeline that works
only for text would have to be rebuilt later.

**Independent Test**: For each of the five non-text types, send a request for a model of that
type on a chosen provider that offers it: openrouter for embeddings, image, text-to-speech and
video; elevenlabs for text-to-speech and speech-to-text. Each succeeds and is recorded. Then
repeat with a transient failure injected on the first account, and observe the same retry and
fallback behaviour as for text.

**Acceptance Scenarios**:

1. **Given** an openrouter account, **When** a client requests embeddings, an image or a video,
   **Then** it receives the result in its API style's form for that type, and the request is
   recorded with its type and usage.
2. **Given** an elevenlabs account, **When** a client requests text-to-speech, **Then** it
   receives the audio, and the request is recorded.
3. **Given** an elevenlabs account, **When** a client sends audio for speech-to-text, **Then** it
   receives the transcription. This works through the speech-to-text section that 0router adds
   to the elevenlabs plugin.
4. **Given** a unified model of a non-text type with two member providers, **When** the first
   member fails transiently, **Then** the request falls back to the second member exactly as it
   would for text.
5. **Given** a request whose model type the target model does not have (for example
   text-to-speech sent to an embeddings model), **When** it arrives, **Then** it is refused in
   the client's style with a message naming the mismatch. Nothing is sent upstream.

---

### User Story 4 — A broken stream continues instead of wasting the answer (Priority: P2)

A stream can break after the client has already received part of the answer. When the next
account or provider declares support for continuing from a partial answer, 0router sends it
the partial answer, and the client receives the rest in the same stream. When continuation
isn't available, 0router does what the operator chose: an informational error event, or a
restart from the beginning. The operator sets a default and can override it per agent key.

**Why this priority**: Avoiding wasted tokens is one of the main reasons 0router exists. A
broken stream that throws away a half-finished answer wastes them. This story depends on
US1 and US2.

**Independent Test**: With a fake upstream that cuts the stream after N output events, test
three setups: a fallback target that declares continuation, one that doesn't with the shipped
default "restart", and one that doesn't with an agent-key override "error event". Check what
the client receives in each case, and check the record.

**Acceptance Scenarios**:

1. **Given** a stream breaks after partial output and the fallback target declares
   continuation, **When** 0router continues, **Then** the client receives one uninterrupted
   stream in its own style, with no repeated or missing output, and the record lists both
   segments and their usage.
2. **Given** a stream breaks, no fallback target declares continuation, and the operator hasn't
   changed the shipped default, **When** it happens, **Then** 0router re-sends the original
   request to the next account or provider. The client receives a short visible note marking
   the restart, then the new answer.
3. **Given** the same situation, but the agent's key overrides the default with "error event",
   **When** it happens, **Then** that client receives an informational error event in its own
   style, with the record id, and the stream ends cleanly.
4. **Given** a stream breaks before the client has received any output, **When** it happens,
   **Then** it is handled as an ordinary transient failure (US2). No continuation or restart
   choice applies.

---

### User Story 5 — The operator sees what happened to every request (Priority: P2)

Every request produces a record: the agent, the unified model (if one was used), the provider
and account that served it, every attempt with the reason it failed, time to first token,
total duration, and token usage, including cache-read and cache-write tokens. The operator
queries records from the CLI per provider, per unified model, and by record id. The operator
also manages provider accounts and agent keys from the CLI.

**Why this priority**: The user named missing or wrong latency and usage numbers as a
failure of this slice. The informational error's record id is only useful if the operator can
look it up.

**Independent Test**: Run a mixed batch of requests (successes, retried, fallen back, all
failed, client-cancelled) across providers and unified models. Query the CLI per provider,
per unified model and by id. Every request is present, and its numbers match what the
upstream reported and what the harness measured.

**Acceptance Scenarios**:

1. **Given** a completed request, **When** the operator looks up its record, **Then** it shows
   the agent, unified model, provider, account, attempts, time to first token, total duration,
   and input, output, cache-read and cache-write tokens, as the provider reported them.
2. **Given** a Responses-format provider that reports cached tokens nested inside the input
   token details, **When** the request completes, **Then** the record's cache-read count equals
   the nested value (9router's CHANGELOG records a bug where this was missed).
3. **Given** requests for several providers and unified models, **When** the operator lists
   records per provider or per unified model, **Then** each listing shows exactly the matching
   records, with their latency and usage.
4. **Given** the CLI, **When** the operator adds, lists or removes a provider account, or
   issues, lists or revokes an agent key, **Then** the change applies to the next request
   without a restart, and no secret is ever shown back in full.
5. **Given** any record, log line or error, **When** it is inspected, **Then** it contains no
   provider API key and no 0router access key.

---

### User Story 6 — A client lists models and counts tokens in its own style (Priority: P2)

Each client can list, in its own API style, the models it can use: unified and direct, of every
type. A client whose style defines a token-counting request can count tokens. The core counts
generically, and each provider's plugin declares whether and how that provider counts.

**Why this priority**: Harnesses such as Claude Code, Codex CLI and Gemini CLI list models and
count tokens during normal use. Without these, they break or misbehave.

**Independent Test**: From one client per style, list models and check the list against the
operator's configuration. Count tokens for the same prompt on a provider that declares counting
and on one that doesn't.

**Acceptance Scenarios**:

1. **Given** unified models and accounts on several providers, **When** a client lists models,
   **Then** it receives, in its style's list format, every unified model and every direct model
   it can reach, of every type.
2. **Given** a provider with no configured account, **When** a client lists models, **Then**
   that provider's direct models are not listed.
3. **Given** a provider whose plugin declares token counting, **When** a client counts tokens,
   **Then** the count comes from that provider.
4. **Given** a provider that declares no token counting, **When** a client counts tokens,
   **Then** 0router answers with a local estimate, using the same estimator 9router uses, and
   the record marks the count as an estimate.

---

### User Story 7 — Provider and API-style specifics are data (Priority: P3)

The four client API styles ship as data files that the core interprets, like provider plugins.
Each provider's specifics are declared in its plugin: endpoints per model type, headers, where
errors are placed, token-counting support, continuation support, which client headers go
upstream, and which provider headers or bodies come back to the client. The core applies these
forwarding declarations under a security floor: credentials are never forwarded, whatever a
plugin declares.

**Why this priority**: This is the design principle that keeps the core generic ("generalize in
the core, specify in plugins"). Its visible effects are tested through US1–US6. This story tests
the declarations themselves.

**Independent Test**: Change a chosen provider's forwarding declaration and observe that
exactly the declared headers move in each direction. Declare a credential header as forwarded
and observe that the core still refuses to forward it. Validate the four shipped API-style
files through the same validation gate as plugins.

**Acceptance Scenarios**:

1. **Given** a plugin declares that a client header is forwarded upstream, **When** a client
   sends that header, **Then** the provider receives it. Undeclared client headers are not
   forwarded.
2. **Given** a plugin declares that a provider response header or body part comes back to the
   client, **When** the provider returns it, **Then** the client receives it.
3. **Given** a plugin that declares a credential-bearing header (the client's 0router access
   key, a provider API key, cookies, authorization) as forwarded, **When** it is loaded or used,
   **Then** that header is never forwarded in either direction, and the validation gate reports
   the declaration.
4. **Given** the four shipped API-style files, **When** 0router starts, **Then** they pass the
   same validation gate as provider plugins. A malformed style file is rejected with an
   actionable error.
5. **Given** a provider whose plugin declares a different error placement (for example an
   error inside a 200 response body), **When** that error occurs, **Then** the core classifies
   it as an error by the declaration, not by the status code alone.

---

### User Story 8 — Other providers become installable community plugins (Priority: P3)

Only the chosen five stay in the bundle. Every other provider that slice 002 bundled moves into
an installable community plugin set. When the operator installs one, it runs if it fits what
the core handles. Otherwise it is refused with a clear "not supported by this core" message.

**Why this priority**: The user chose to fully support only a few providers. This story keeps
the rest usable where they fit, without the core claiming support it doesn't have.

**Independent Test**: Install every community plugin in turn. Each one either loads and appears
in model listings, or is refused with a message that names what the core does not support. No
crash and no silent partial load.

**Acceptance Scenarios**:

1. **Given** the community set, **When** the operator installs a plugin that uses only API
   styles, auth schemes and model types the core handles, **Then** it loads and can serve
   requests once an account is added.
2. **Given** a community plugin that needs something this core doesn't handle (for example
   account sign-in), **When** the operator installs it, **Then** it is refused with a
   "not supported by this core" message naming the unsupported part.
3. **Given** a plugin with some sections the core handles and some it doesn't, **When** the
   operator installs it, **Then** the whole plugin is refused. The "not supported by this core"
   message names every unsupported part, and none of the plugin's sections load.
4. **Given** slice 002's parity tests over the full 9router provider set, **When** they run,
   **Then** they check the community set and the chosen five together, and still pass.

---

### Edge Cases

- **429 with a long wait**: when the provider's indicated wait is longer than the retry budget,
  0router moves to the next account at once instead of waiting on the warm one.
- **Only one account and one provider**: transient failures are retried on that account within
  the retry budget. Then the client receives the informational error.
- **Warm account in cooldown**: the agent's next request goes elsewhere until the account's
  cooldown ends, then returns to it.
- **Stream stalls without breaking**: a stream that sends nothing for 9router's stall timeout
  (360 s) is treated as broken, and US4 applies.
- **Client disconnects during a retry or fallback**: all pending attempts are cancelled, and no
  further upstream request is started.
- **Continuation across providers with different API styles**: the partial answer is translated
  into the next provider's style before the request is sent. If the translation isn't possible
  for that pair, the target counts as not supporting continuation.
- **Restart after partial output**: the client already holds part of the first answer. It
  receives a short visible note in the answer, for example "— connection lost, answer
  restarted —", then the new answer in the same response.
- **A unified model member provider that is not installed or has no account**: that member is
  skipped, and the skip is recorded as an attempt with its reason.
- **A style with no endpoint for a model type**: for example, Anthropic Messages has no image
  or speech endpoint. The four styles exist for client compatibility, and they don't limit
  which model types an agent can use. The agent's access key works on every style's endpoints,
  so a model of any type is reachable through any style that has an endpoint for that type.
  0router doesn't invent endpoints a style doesn't define.
- **Long-running jobs**: video providers that run generation as jobs keep their job semantics.
  The record's total duration covers submission to the final result the client receives
  through 0router.
- **Two concurrent requests from the same agent**: both are served and recorded. Stay-warm
  bookkeeping keeps the most recent account that served the agent successfully.
- **Secrets in upstream error bodies**: an upstream error that echoes a credential is redacted
  before it reaches the record, log or client.
- **Provider reports no usage**: the record shows usage as "not reported", not zero.

## Requirements *(mandatory)*

### Functional Requirements

**Client surface and identity**

- **FR-001**: 0router MUST accept requests in four client API styles: OpenAI Chat Completions,
  Anthropic Messages, OpenAI Responses, and Gemini generateContent. Each style includes the
  requests that style defines for model listing, token counting and non-text model types.
- **FR-002**: Every client request MUST carry a 0router access key, in the place its API style
  normally carries an API key. Requests without a valid key MUST be rejected in the client's
  style, and nothing MUST be sent upstream. One access key MUST be accepted on the endpoints of
  all four styles.
- **FR-003**: Each access key identifies one agent. The agent identity MUST be the key plus the
  client's own session id when the client sends one.
- **FR-004**: Every response, streamed or not, MUST be valid for the client's API style, so that
  the client's official SDK and harness process it without error.
- **FR-005**: Clients MUST be able to address a unified model or a direct `<provider>/<model>`,
  with slice 002's addressing rules.

**Translation and forwarding**

- **FR-006**: Any client API style MUST reach any chosen provider, whatever style the provider
  speaks. The core translates requests, responses, stream events, errors and usage between
  styles, following 9router's translation behaviour for the pairs 9router covers.
- **FR-007**: 0router MUST forward prompt content as received and never compress, truncate,
  summarize or rewrite it. Translation changes only the wire shape.
- **FR-008**: The four client API styles MUST be data files that the core interprets and that
  pass a validation gate, like provider plugins.
- **FR-009**: Each provider plugin MUST be able to declare: endpoints per model type, headers,
  error placement, token-counting support, continuation support, which client headers go
  upstream, and which provider response headers or body parts come back to the client.
- **FR-010**: The core MUST apply forwarding declarations under a security floor. Credentials
  (0router access keys, provider API keys, authorization and cookie headers) MUST never be
  forwarded in either direction, whatever a plugin declares.

**Model types**

- **FR-011**: Text, embeddings, image, text-to-speech, speech-to-text and video requests MUST go
  through the same pipeline, with the same access control, unified models, retry, fallback,
  informational errors and records.
- **FR-012**: A request for a type the target model doesn't have MUST be refused in the client's
  style before anything is sent upstream.
- **FR-013**: The elevenlabs plugin MUST gain a speech-to-text section, authored by 0router.

**Retry, fallback, and continuation**

- **FR-014**: Error classification MUST follow 9router's request-path behaviour, which
  distinguishes transient failures that trigger retry or fallback from errors that are returned
  as they are.
- **FR-015**: On a transient failure, 0router MUST retry the same account first, within a
  bounded retry budget. Then it MUST move to another account of the same provider, then to
  another member provider of the same unified model. Direct `<provider>/<model>` requests fall
  back only across that provider's accounts.
- **FR-016**: 0router MUST remember, per agent and per unified or direct model, the account that
  last served that agent successfully. It MUST send that agent's next request there when the
  account is available.
- **FR-017**: When a stream breaks after the client has received output, and a fallback target
  declares continuation support, 0router MUST continue the same client stream from the partial
  answer on that target, with no repeated or missing output.
- **FR-018**: When continuation is not available, 0router MUST apply the operator's choice: an
  informational error event, or a restart from the beginning. The operator sets a default and
  can override it per agent key. The shipped default is restart. On a restart, the client MUST
  receive a short visible note in the answer between the partial answer and the new one.
- **FR-019**: A client disconnect MUST cancel the upstream request and any pending retry or
  fallback.
- **FR-020**: Streams MUST be relayed to the client as they arrive, without buffering the
  response.
- **FR-021**: Upstream connections MUST be kept stable and reused across requests, so that a
  request to a provider with a live connection doesn't pay a new connection setup.

**Informational errors**

- **FR-022**: When no account or provider can serve a request, the client MUST receive an error
  in its own API style that holds: a readable summary of each attempt and why it failed, in the
  style's standard message field; the same details in a structured extra field; and the request
  record id in a response header and in the message.
- **FR-023**: Informational errors MUST stay valid within the client style's error schema, so
  that strict SDKs still parse them.
- **FR-024**: Where errors are placed in a provider's responses is declared in its plugin
  (FR-009). The core MUST classify by the declaration.

**Model listing and token counting**

- **FR-025**: Each client MUST be able to list, in its own style's format, every unified model
  and every direct model it can reach, of every type, limited to providers with at least one
  configured account.
- **FR-026**: Token counting MUST be a generic core capability, offered through the counting
  request of each style that defines one. When the provider declares counting, its count MUST be
  used. Otherwise 0router MUST return a local estimate, using 9router's estimator, and mark it as
  an estimate in the record.

**Records and operator CLI**

- **FR-027**: Every request MUST produce a record with: record id, agent, model type, unified
  model (if any), serving provider and account, every attempt with its reason, outcome, time to
  first token (for streamed or chunked responses), total duration, and token usage (input,
  output, cache-read and cache-write) as the provider reported it. Usage the provider didn't
  report is marked as not reported.
- **FR-028**: Usage MUST be read from every place a provider style reports it, including cached
  tokens nested inside the input token details.
- **FR-029**: Records MUST be kept in memory and be queryable from the CLI per provider, per
  unified model, and by record id.
- **FR-030**: The operator MUST be able to add, list and remove provider accounts (API keys),
  and to issue, list and revoke agent keys, from the CLI. Changes apply to the next request
  without a restart.
- **FR-031**: The operator MUST be able to set the default mid-stream break behaviour (FR-018)
  and override it per agent key from the CLI.

**Secrets**

- **FR-032**: Provider secrets MUST be injected by the core only when a request is executed.
  Plugins and API-style files never hold or see them.
- **FR-033**: No provider API key or 0router access key may appear in any log, record, error,
  CLI output, or data a plugin can see. Secrets echoed by an upstream error are redacted.

**Provider set**

- **FR-034**: The bundle MUST contain exactly the chosen five: anthropic, openrouter,
  opencode-zen, opencode-go and elevenlabs. They serve requests with the operator's API-key
  accounts.
- **FR-035**: Every other provider from slice 002's bundle MUST move to an installable community
  plugin set. An installed community plugin MUST run if all of it fits what the core handles.
  Otherwise the whole plugin MUST be refused with a "not supported by this core" message that
  names every unsupported part. A plugin is never partly loaded.
- **FR-036**: Slice 002's parity tests over 9router's full provider set MUST keep passing
  against the chosen five plus the community set.

**Performance**

- **FR-037**: The execution and streaming hot paths MUST carry performance benchmarks with a
  recorded baseline. Regressions block merge.

### Key Entities

- **Client API style**: a data file describing one client-facing API: its requests per model
  type, its response, stream event and error shapes, where it carries the access key and session
  id, and its model-list and token-count forms.
- **Provider plugin** (extended from slice 002): adds endpoints per model type, error placement,
  token-counting support, continuation support, and forwarding declarations.
- **Provider account**: an operator-held API key for one provider. It is held by the core and
  never visible to plugins.
- **Agent key**: a 0router access key issued to one agent. It carries an optional override of
  the mid-stream break behaviour.
- **Agent**: an agent key plus the client's session id, when one is sent. Stay-warm bookkeeping
  is kept per agent.
- **Attempt**: one try against one account of one provider, with its outcome and reason.
- **Request record**: the record of one client request (FR-027). It holds its attempts, and is
  kept in memory.
- **Community plugin set**: the providers outside the chosen five, installable on demand and
  subject to fit-or-refuse.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: At least two different client harnesses, plus the official SDK of each of the four
  API styles, complete streamed and non-streamed text requests against every chosen text
  provider (anthropic, openrouter, opencode-zen, opencode-go) with zero client-side errors.
- **SC-002**: In failure-injection tests, 0% of injected 429, 5xx, timeout and connection
  failures reach the client when another account or member provider could serve. The attempt
  order is same account, then other account, then other member in 100% of runs.
- **SC-003**: In 100% of runs where an agent was moved off its account by a transient failure,
  the agent's next request is served by the original account once it is available.
- **SC-004**: For every chosen provider and client style in the test matrix, input, output,
  cache-read and cache-write token counts in records and client responses equal what the
  provider reported. No record is missing time to first token (streamed) or total duration.
- **SC-005**: Recorded time to first token and total duration are within 10 ms of what the test
  harness measures on the client side.
- **SC-006**: A secret scan across all logs, records, errors, CLI output, forwarded headers and
  plugin-visible data from the full test suite finds zero occurrences of any configured provider
  key or agent key.
- **SC-007**: Each of the six model types completes at least one request through a chosen
  provider. Each type that has two or more accounts or members shows the same fallback behaviour
  as text under failure injection.
- **SC-008**: In mid-stream break tests with a continuation-capable target, the client receives
  the complete answer in one stream with zero repeated and zero missing output events.
- **SC-009**: For every all-attempts-failed case in each of the four styles, the client's
  official SDK parses the error without raising a parse error. The error names every attempt
  and reason, and its record id resolves in the CLI.
- **SC-010**: A client disconnect stops the upstream request within 1 second in 100% of
  cancellation tests.
- **SC-011**: For a sequence of requests to the same provider within its keep-alive window,
  every request after the first reuses an existing upstream connection.
- **SC-012**: Every community plugin either loads or is refused with a "not supported by this
  core" message that names the unsupported part. Zero crashes, zero silent partial loads.
- **SC-013**: 0router's own added time before the first byte reaches the client is at most
  10 ms at the 95th percentile in the benchmark suite, and the hot-path benchmarks have a
  committed baseline.

## Assumptions

Items marked *(technical decision)* are technical decisions Claude made. Per the user's
direction, technical choices are Claude's to make. They are not user requirements, and they may
be revised in planning without asking the user, as long as nothing the user sees changes.

- The retry budget, backoff, timeouts and cooldowns follow 9router's defaults (Constitution VI).
  "Retry the same account first" is a deliberate 0router addition on top of them.
- Until slice 005's routing decision, accounts and member providers are tried in the order the
  operator configured and declared them. The only routing intelligence in this slice is "stay
  warm first". *(technical decision)*
- Provider accounts and agent keys persist in the operator's local configuration across
  restarts. Request records don't persist: they are in memory only, until slice 005.
  *(technical decision)*
- In-memory records are capped at a bounded number of recent records, and the oldest are
  evicted first. *(technical decision)*
- 0router runs as a local network service that the operator starts from the CLI.
- Chosen-five plugins become 0router-owned: seeded by the generator, then maintained by hand.
  The community set stays generated (brief, P notes).
- openrouter is used with the operator's API key, although slice 002 categorizes it as
  free-tier.
- xai and grok-cli are in the community set during this slice. Slice 004 brings them back as
  chosen providers with account sign-in.
- The 10 ms figures in SC-005 and SC-013 and the 1 s figure in SC-010 are starting targets.
  *(technical decision)*

## Out of Scope

- Account sign-in (OAuth), and the xai and grok-cli providers: slice 004.
- The routing decision (cache-aware routing, per-agent isolation, windowed amortization) and
  persistent request history: slice 005.
- Combos, model tests, the dashboard, and plugin-declared OAuth for any provider: later.
  Fallback in this slice stays inside one unified model.
- Web search: never (outside 0router's scope).
- Token optimization: never (an upstream hop).
