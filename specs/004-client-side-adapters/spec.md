# Feature Specification: Client Side: Harness Adapters

**Feature Branch**: `004-client-side-adapters`

**Created**: 2026-09-28

**Status**: Draft

**Scope brief**: [specs/briefs/2026-09-28-client-side-adapters.md](../briefs/2026-09-28-client-side-adapters.md). In
clarify and plan, an answer that contradicts a confirmed row of that brief's ledger means stop and
revisit the brief.

**Input**: User description: "Client side: harness adapters. 0router gains plugins on the client side, next to the provider plugins, so that harnesses needing special handling work against 0router with only a base URL and an access key, and no middleman in between. An adapter is bound to an agent key: the operator names the key's harness when issuing or editing the key, and 0router never guesses the harness from the request. Through 0router's adapter kit, an adapter can read and edit both the request and the response. One harness is built in: hermes. It reaches full feature parity on every chosen provider, so its tools, reasoning and images work. This includes removing the reasoning hermes echoes back on assistant messages wherever the target provider rejects it, and carrying hermes's own image and attachment format. Third-party adapters are code. They ship as Rust source only, and a separate builder service running next to the core compiles them to WASM. An adapter may depend only on 0router's own adapter kit: no other crates, build scripts or procedural macros. The validation gate refuses opaque blobs and code over a size limit. The core runs only compiled adapters whose source matches the reviewed source. Adapters run in a sandbox with no network, no file access and no secrets; the core does all sending. An operator who never installs a third-party adapter runs the core alone. Installing or updating a third-party adapter puts it in a security review queue. A review agent reads a copy with comments stripped and identifiers renamed. It has no access to user data, can't run commands, and only reports how safe the adapter seems; the operator decides. The review agent runs on a model, provider and budget the operator picks. With no model set, or not enough budget, the adapter stays in quarantine. Updates are never automatic, and while an update is queued or quarantined, the previous version keeps serving. An adapter may change request content only where its harness's coupling requires it, by removing or converting parts the target provider can't accept. It never compresses, summarizes or optimizes, and every content change it makes is noted in the request record. A rule-based guardrail runs on every request; AI is used only at review time, never per request. When an adapter adds or changes a tool call, 0router drops that adapter's changes, sends the unmodified request, marks the adapter as suspect, alerts the operator, and records what the adapter tried. 0router writes the Claude Code adapter as a third-party plugin, and it goes through the full install and review pipeline, proving it on a real closed-source harness. For harnesses that are coupled to their own server or closed-source, adapters follow 9router's fixes. The slice fails if: a built-in harness needs a middleman or setup beyond a base URL and key, or one of its features breaks; a third-party adapter reaches the network, a file or a secret, or runs code other than the reviewed source; starting an update, a review or a quarantine breaks a working setup; an adapter adds or changes a tool call without the guardrail stopping it. Constraints: Constitution Principles I and IV as amended before this slice (harness adapters may be sandboxed code; an adapter's content changes are limited to what its coupling requires); provider plugins stay data only, and a provider that needs code becomes a core built-in; secrets are injected by the core only; streams are relayed without buffering, and a client disconnect cancels the upstream request; the adapter hot path carries Criterion benchmarks. Oracle: ref/9router normalizeClaudePassthrough (open-sse/translator/formats/claude.js), open-sse/translator/concerns/paramSupport.js, and open-sse/services/combo.js. Out of scope: built-in adapters for opencode, grok build and zcode → later slices (they connect as plain clients through slice 003 until then); account sign-in (OAuth), xai and grok-cli → slice 005; the routing decision and persistent request history → slice 006; code in provider plugins → never (built-ins instead). Scope brief: specs/briefs/2026-09-28-client-side-adapters.md"

## Clarifications

### Session 2026-09-28

- Q: Does the guardrail flag an adapter that adds or changes a tool definition or tool result, not only a tool call? → A: Yes. Adding or changing any of the three is stopped; removals stay allowed and recorded (FR-016).
- Q: What does a suspect adapter do next? → A: It stops serving every key until the operator clears it; bound keys work as plain clients meanwhile (FR-017).
- Q: Should the guardrail also check responses, so an adapter adding or changing a tool call in the provider's reply (streamed or not) is stopped as in a request? → A: Yes, both directions. Mid-stream, events already sent are not recalled (FR-016, US3-4).
- Q: Where can the operator install third-party adapters from in this slice? → A: From source they supply (a local folder or archive), and from a 0router-hosted catalogue they can browse and install from. Both paths go through the same gate, build and review (FR-031).
- Q: When should 0router contact the catalogue: only when the operator browses or installs, or also in the background to check for newer versions? → A: Only on operator action (browse, install, or an explicit "check for updates"). Never in the background (FR-031).
- Q: After a 0router upgrade changes the adapter kit so an installed adapter's compiled form no longer fits, what happens? → A: 0router rebuilds the same reviewed source against the new kit on its own, with no new review. If the rebuild fails, bound keys work as plain clients and the operator is alerted with the reason (FR-032).
- Q: What should this slice build on the catalogue's hosting side? → A: An index file in a 0router-owned public repository listing each adapter's harness, versions and source location. Authors are listed by pull request, merged by 0router maintainers. The slice builds the index format, the first entries (Claude Code included) and the install side; no hosted service (FR-031).

## User Scenarios & Testing *(mandatory)*

This slice builds on slice 003's request pipeline and has four kinds of user:

- The **operator** runs 0router. They issue agent keys and name each key's harness, install and
  update third-party adapters, choose the review agent's model, provider and budget, read review
  reports, and decide which adapter versions go live.
- The **client** is a harness, such as hermes or Claude Code, pointed at 0router with a base URL
  and an agent key. It must work as it does against the service it was built for.
- The **adapter author** writes a third-party adapter as source against 0router's adapter kit.
- The **review agent** reads a scrambled copy of an adapter's source and reports on its safety.
  It never decides.

### User Story 1 — hermes works fully through 0router (Priority: P1)

The operator issues an agent key with hermes as its harness. hermes, pointed at 0router with only
that key and 0router's base URL, uses tools, reasoning and images on every chosen text provider,
over several turns, streamed and not streamed. Nothing else sits between hermes and 0router.

**Why this priority**: hermes is the one built-in harness in this slice. A built-in harness that
needs extra setup, or loses a feature on some provider, is a named failure of the slice.

**Independent Test**: Issue a hermes key. Run hermes against each chosen text provider (anthropic,
openrouter, opencode-zen, opencode-go) through a multi-turn session that calls tools, carries
reasoning from one turn to the next, and sends an image and an attachment. Every turn completes
with no client or provider error.

**Acceptance Scenarios**:

1. **Given** a key with hermes as its harness, **When** hermes sends a second turn that echoes the
   first turn's reasoning on the assistant message, and the target provider rejects that field,
   **Then** the provider receives the request without it, the turn succeeds, and the record notes
   the removal.
2. **Given** the same second turn, **When** the target provider accepts the echoed reasoning,
   **Then** it goes upstream unchanged and the record notes no change.
3. **Given** a hermes request that carries an image or attachment in hermes's own format, **When**
   it goes to any chosen text provider that accepts images, **Then** the provider receives the
   image in a form it accepts, and the answer refers to it.
4. **Given** a hermes session with tool calls, **When** it runs against each chosen text provider,
   **Then** every tool call and tool result round-trips, and hermes runs each tool as it would
   against its own configured provider.
5. **Given** a key with no harness named, **When** hermes uses it, **Then** 0router treats it as a
   plain client (slice 003 behaviour) and applies no adapter, even though the request looks like
   hermes.

---

### User Story 2 — The operator installs a third-party adapter and decides when it goes live (Priority: P1)

The operator installs a third-party adapter from source they supply, or picks one from 0router's
catalogue. 0router checks the source against the validation gate, has the builder compile it,
and queues it for review. The review agent,
running on the model, provider and budget the operator chose, reads a scrambled copy and reports.
The operator reads the report and approves or rejects. Only an approved adapter serves, and only
for the keys whose harness names it.

**Why this priority**: The user chose to ship the whole third-party pipeline in this slice. Every
third-party adapter, the Claude Code proof included, depends on it.

**Independent Test**: Install a small test adapter. Watch it pass the gate, build, enter the
review queue, receive a report and wait. Approve it and observe that it serves the next request
from a key bound to its harness, and no request from other keys.

**Acceptance Scenarios**:

1. **Given** adapter source that depends on anything other than the adapter kit, has a build
   script or a procedural macro, holds an opaque blob, or exceeds the size limit, **When** the
   operator installs it, **Then** it is refused with a message that names each reason, and
   nothing is built.
2. **Given** adapter source that passes the gate, **When** it is installed, **Then** the builder
   compiles it and it enters the review queue. It serves no request until the operator approves.
3. **Given** a queued adapter and no review model set, or a budget too small for the review,
   **When** the queue is processed, **Then** the adapter stays in quarantine, and the operator
   sees why.
4. **Given** a review that ran, **When** the operator opens the report, **Then** it shows how safe
   the adapter seems and why. The operator approves or rejects, and the decision is recorded.
5. **Given** the review agent's input, **When** it is inspected, **Then** it holds no comment from
   the source and none of the source's own identifiers, and no user data. The review agent ran no
   command.
6. **Given** an approved adapter, **When** its compiled form no longer matches the reviewed source
   (the source or the build was altered afterwards), **Then** the core refuses to run it and
   alerts the operator.
7. **Given** an operator who never installs a third-party adapter, **When** 0router runs, **Then**
   it needs no builder, no compiler and no review model, and serves requests as in slice 003.
8. **Given** 0router's catalogue, **When** the operator browses it, **Then** they see each adapter's
   harness and published versions. **When** they install one, **Then** 0router fetches its source,
   and it goes through the same gate, build, review and decision as a local install, with no step
   skipped for being in the catalogue.

---

### User Story 3 — An adapter can't harm the operator or the agent (Priority: P1)

Whatever a third-party adapter's code tries, it can't reach the network, a file or a secret, and
it can't slip a tool call, tool definition or tool result into a request or a response. The
rule-based guardrail checks every request and response the adapter touches. When the adapter adds
or changes any of them, 0router discards that adapter's changes, sends the original, marks the
adapter as suspect, alerts the operator and records what the adapter tried. A suspect adapter
stops serving until the operator clears it.

**Why this priority**: A sandbox escape, unreviewed code running, or an unstopped tool-call edit
are named failures of the slice. Third-party code is safe to allow only because of these limits.

**Independent Test**: Install a set of hostile test adapters, one per attack: open a connection,
read a file, read the environment, read a secret, loop forever, exhaust memory, add a tool call
to a request, change a tool call's arguments, add a tool call to a streamed response. Approve
each and send requests. Every attack is contained, and every request still completes.

**Acceptance Scenarios**:

1. **Given** an adapter that tries to open a network connection, read or write a file, or read
   the environment, **When** it runs, **Then** the attempt fails inside the sandbox, nothing
   leaves the process, and the request completes.
2. **Given** any adapter, **When** it runs, **Then** it never receives a provider secret, an agent
   key, or a header on the security floor.
3. **Given** an adapter that adds a tool call to the request history, or changes a tool call's
   name or arguments, **When** the guardrail checks its output, **Then** the provider receives
   the unmodified request, the adapter is marked suspect, the operator is alerted, and the record
   shows what the adapter tried.
4. **Given** an adapter that adds or changes a tool call in the provider's response, streamed or
   not, **When** the guardrail checks it, **Then** the client receives the response without that
   adapter's changes, and the same marking, alert and record follow.
5. **Given** an adapter that adds a tool definition, changes one, or rewrites a tool result,
   **When** the guardrail checks it, **Then** the same discard, marking, alert and record follow.
6. **Given** an adapter marked suspect, **When** the next request arrives from any key bound to
   its harness, **Then** the adapter doesn't run, the request is served as a plain client's, and
   the record says the adapter is held as suspect. After the operator clears it, it serves again.
7. **Given** an adapter that runs too long or uses too much memory, **When** its limit is hit,
   **Then** it is stopped, the request goes on without its changes, and the record says why.
8. **Given** an adapter that removes a part the target provider can't accept (for example another
   server's tool blocks), **When** the guardrail checks it, **Then** the removal is allowed and
   noted in the record.

---

### User Story 4 — Updating an adapter never breaks a working setup (Priority: P2)

The operator starts an update of an installed adapter. The new version goes through the same
gate, build and review as a first install. Until the operator approves it, the previous version
keeps serving every key bound to that harness. Nothing updates by itself.

**Why this priority**: "Starting an update, a review or a quarantine breaks a working setup" is a
named failure. Without this story, operators would avoid updates, or updates would surprise them.

**Independent Test**: With an approved v1 serving a key, start an update to v2. Send requests while
v2 is queued, in review, quarantined, rejected, and finally approved. v1 serves every request
until the approval, and v2 serves every request after it, with no failed request in between.

**Acceptance Scenarios**:

1. **Given** an approved v1, **When** the operator starts an update to v2, **Then** v1 keeps
   serving while v2 is queued, in review or quarantined.
2. **Given** v2 is rejected, **When** requests arrive, **Then** v1 keeps serving and v2 is kept
   only for the operator's reference.
3. **Given** v2 is approved, **When** the next request arrives, **Then** v2 serves it. Requests
   already in flight finish on v1.
4. **Given** a newer version published by the adapter's author, in the catalogue or elsewhere,
   **When** the operator does nothing, **Then** 0router installs nothing and changes nothing.

---

### User Story 5 — Claude Code proves the third-party pipeline (Priority: P2)

0router writes a Claude Code adapter as a third-party plugin, lists it among the catalogue's first
entries, and puts it through the full install and review pipeline, like any community adapter.
With it approved and bound to a key, Claude Code works against 0router with only a base URL and that key, including sessions whose history holds
blocks only Anthropic's own server accepts, sent to a provider that isn't Anthropic.

**Why this priority**: The pipeline is proven only when a real, closed-source harness goes
through it. The adapter also sets the "follow 9router's fixes" pattern for coupled harnesses.

**Independent Test**: Install the Claude Code adapter from source, review it with a real model,
approve it, and bind it to a key. Run Claude Code sessions that use web search and tools against
each chosen text provider. Every session completes, and every removal shows in the records.

**Acceptance Scenarios**:

1. **Given** the Claude Code adapter's source, **When** the operator installs it, **Then** it goes
   through the gate, the builder, the review queue and the operator's decision, with no step
   skipped or special-cased.
2. **Given** a Claude Code session whose history holds another server's tool blocks, **When** it
   is sent to a provider that can't accept them, **Then** the adapter removes them as 9router
   does, the request succeeds, and the record lists each removal.
3. **Given** the same session sent to Anthropic, **When** Anthropic accepts those blocks, **Then**
   they go upstream unchanged.

---

### User Story 6 — The operator sees what each adapter did (Priority: P2)

For every request from a key with a harness, the record shows which adapter and version ran,
every content change it made, and every guardrail event. The operator lists adapters with their
state (queued, building, refused, in review, quarantined, reported, approved, rejected,
suspect, superseded), sees which approved version is active, and sees pending alerts.

**Why this priority**: Principle IV requires every adapter content change to be recorded. Without
it, the coupling exception could turn into optimization unnoticed.

**Independent Test**: Run hermes and Claude Code sessions that trigger removals and conversions,
plus one hostile adapter. Every change and guardrail event appears in the records, and the
adapter list and alerts match what happened.

**Acceptance Scenarios**:

1. **Given** a request an adapter changed, **When** the operator shows its record, **Then** each
   change is listed with where in the request it happened, what kind of change it was, and why.
   No removed or converted content is stored.
2. **Given** a request the adapter left unchanged, **When** the operator shows its record, **Then**
   it names the adapter and version, and shows no change.
3. **Given** a guardrail event, **When** the operator lists adapters, **Then** that adapter shows as
   suspect, and the alert stays visible until the operator acknowledges it.

---

### Edge Cases

- **Fallback to another provider**: the adapter runs again for each attempt, so its changes fit
  that attempt's target. Each attempt's changes are recorded with that attempt.
- **Key bound to a harness with no approved version yet**: the key works as a plain client
  (slice 003 behaviour) until a version is approved, and each record notes that the adapter
  wasn't active and why.
- **An adapter is removed while keys are bound to it**: the operator is shown the bound keys and
  confirms. Those keys then work as plain clients, and their records note it.
- **An adapter crashes or returns output that isn't valid for the client's style**: the request
  goes on without that adapter's changes, and the record and an alert say why.
- **A guardrail event in the middle of a stream**: the rest of that response is relayed without
  the adapter's changes. Events already sent to the client are not recalled.
- **A harness name used by both a built-in and a third-party adapter**: built-in names are
  reserved. A third-party adapter can't take them.
- **A 0router upgrade changes the adapter kit**: an adapter built against an incompatible kit is
  never run. 0router rebuilds the same reviewed source against the new kit on its own, with no new
  review (a changed source still needs one). If the rebuild fails, bound keys work as plain
  clients, and the operator is alerted with the reason (FR-032).
- **The review agent fails part-way (provider error, budget exhausted)**: the adapter stays in
  quarantine, and the previous version, if any, keeps serving.
- **The operator approves despite a poor report**: allowed. The operator decides, and the
  decision is recorded with the report.
- **Client disconnects while an adapter runs**: the upstream request is cancelled as in slice
  003, and the adapter's work for that request stops.
- **An adapter adds or changes a tool definition or a tool result**: the guardrail stops it as it
  stops a tool-call edit (FR-016). Removing a tool definition, call or result the target can't
  accept stays allowed and is recorded.

## Requirements *(mandatory)*

### Functional Requirements

**Binding and the adapter kit**

- **FR-001**: The operator MUST be able to name a harness when issuing or editing an agent key,
  and to clear it. A key with a harness runs that harness's active adapter. A key without one
  is a plain client, as in slice 003.
- **FR-002**: 0router MUST NOT pick an adapter from anything in the request (user agent, headers,
  body shape). Only the key's named harness selects it.
- **FR-003**: Through the adapter kit, an adapter MUST be able to read and edit the request before
  each upstream attempt, and to read and edit the response on its way to the client, streamed or
  not. It MUST be told which provider and API style the attempt targets.
- **FR-004**: An adapter MUST never receive a provider secret, an agent key, or a header on the
  security floor (slice 003 FR-010).
- **FR-005**: Streamed responses MUST still be relayed as they arrive. An adapter works on each
  stream event without making the client wait for the whole response.

**Built-in harness: hermes**

- **FR-006**: hermes MUST be a built-in adapter, part of the core, needing no install or review.
- **FR-007**: With a hermes key, tools, reasoning across turns, and images and attachments MUST
  work on every chosen text provider, streamed and not streamed, with only a base URL and the key.
- **FR-008**: The hermes adapter MUST remove reasoning that hermes echoes on assistant messages
  only for targets that reject it, and leave it for targets that accept it.
- **FR-009**: The hermes adapter MUST convert hermes's own image and attachment format into a
  form the target accepts.

**Third-party adapters: source, gate, builder**

- **FR-010**: A third-party adapter MUST ship as Rust source only. The gate MUST refuse anything
  else: prebuilt code, binary files, and opaque blobs (long base64 or hex strings).
- **FR-011**: The gate MUST refuse an adapter that depends on anything other than the adapter kit,
  has a build script or a procedural macro, or exceeds the code size limit. The refusal names
  every reason.
- **FR-012**: A builder service, separate from the core, MUST compile adapters to the sandbox's
  format. The core MUST run only a compiled adapter whose source matches the reviewed source, and
  MUST refuse and alert on a mismatch.
- **FR-013**: An operator who installs no third-party adapter MUST be able to run 0router without
  the builder, a compiler, or a review model.
- **FR-031**: The operator MUST be able to install a third-party adapter from source they supply
  (a local folder or archive) or from a catalogue hosted by the 0router project. The catalogue is
  an index file in a 0router-owned public repository. Each entry names the harness, its published
  versions and where each version's source lives, and authors are listed by a pull request that
  0router maintainers merge. There is no catalogue service, author account or upload. The
  catalogue lists adapter source, not compiled code. An adapter installed from the catalogue MUST
  go through the same gate, build, review and operator decision as a local one. 0router MUST
  contact the
  catalogue only when the operator browses it, installs from it, or asks to check for newer
  versions of installed adapters, and never in the background.

**Sandbox and guardrail**

- **FR-014**: Third-party adapters MUST run only inside the sandbox: no network, no file access,
  no environment, no secrets, and limits on time and memory per call. They are never linked into
  the core.
- **FR-015**: A rule-based guardrail MUST check every request and response a third-party adapter
  changed, before it goes on. No AI runs per request.
- **FR-016**: When an adapter adds or changes a tool call (its name, arguments or identity), a
  tool definition, or a tool result, in the request or in the response, 0router MUST discard
  that adapter's changes, send the unmodified request or response, mark the adapter as suspect,
  alert the operator, and record what the adapter tried. Removing a tool call, definition or
  result is not stopped; it is a content change under FR-024 and FR-025.
- **FR-017**: A suspect adapter MUST stop serving every key until the operator clears it. Until
  then, keys bound to its harness work as plain clients (slice 003 behaviour), and each record
  notes that the adapter was held as suspect.
- **FR-018**: When an adapter fails, hits a limit, or returns output that isn't valid for the
  client's style, the request MUST go on without that adapter's changes, and the record MUST say
  why.

**Review, quarantine, updates**

- **FR-019**: Installing or updating a third-party adapter MUST put it in the review queue. It
  serves no request until the operator approves it.
- **FR-020**: Before review, 0router MUST produce a copy of the source with comments stripped and
  identifiers renamed to meaningless names. The review agent MUST see only that copy: no user
  data, no records, no secrets, and no way to run commands.
- **FR-021**: The review agent MUST produce a report of how safe the adapter seems and why. It
  MUST NOT approve or reject; the operator does.
- **FR-022**: The operator MUST choose the review agent's model, provider and budget. With no model
  set, or a budget too small for the review, the adapter MUST stay in quarantine and the operator
  MUST see why.
- **FR-023**: Third-party adapters MUST NOT update automatically. While an update is queued, in
  review, quarantined or rejected, the previous approved version MUST keep serving. An approved
  update serves from the next request on.
- **FR-032**: When a 0router upgrade makes an installed adapter's compiled form incompatible with
  the adapter kit, 0router MUST NOT run the old build, and MUST rebuild the same reviewed source
  against the new kit without operator action or a new review. This rebuild is not an update: the
  source is unchanged. If it fails, bound keys MUST work as plain clients and the operator MUST be
  alerted with the reason.

**Content changes and records**

- **FR-024**: An adapter MAY change request content only where its harness's coupling requires
  it, by removing or converting parts the target provider can't accept. It MUST NOT compress,
  truncate, summarize or otherwise optimize content (Constitution IV).
- **FR-025**: Every content change an adapter makes MUST be noted in the request record, on the
  attempt it applies to, with its location, kind (removed or converted) and reason. The record
  MUST NOT hold the removed or converted content.
- **FR-026**: Every request from a key with a harness MUST record which adapter and version ran, or
  why none ran.

**Claude Code proof adapter**

- **FR-027**: 0router MUST provide a Claude Code adapter as a third-party plugin that goes through
  the full install and review pipeline, with no special path.
- **FR-028**: The Claude Code adapter MUST follow 9router's fixes for Claude Code
  (`normalizeClaudePassthrough`), removing blocks only Anthropic's server accepts when the target
  can't accept them.

**Operator surface**

- **FR-029**: The operator MUST be able, from the CLI, to browse the catalogue, check it for newer
  versions of installed adapters, install, update,
  list, show, approve, reject, remove and clear suspect third-party adapters, read review reports,
  set the review agent's model, provider and budget, and list and acknowledge alerts. Changes
  apply to the next request without a restart.

**Performance**

- **FR-030**: The adapter hot path (running an adapter and the guardrail on a request and on each
  stream event) MUST carry performance benchmarks with a recorded baseline. Regressions block
  merge.

### Key Entities

- **Harness**: a named client harness (for example hermes, claude-code). A name is either built in
  or taken by a third-party adapter.
- **Agent key** (extended from slice 003): gains an optional harness name.
- **Adapter**: the code that handles one harness's coupling. Built in (core code) or third-party.
- **Adapter version**: one submitted source of a third-party adapter, with its source fingerprint,
  its compiled form, and its state: queued, building, refused (by the gate), in review,
  quarantined, reported (review done, awaiting the operator), approved, rejected, suspect, or
  superseded (replaced by a newer approved version). At most one approved version per harness
  serves at a time.
- **Review report**: the review agent's safety assessment of one version, and the operator's
  decision on it.
- **Review settings**: the operator's chosen model, provider and budget for the review agent.
- **Catalogue**: an index file in a 0router-owned public repository, listing third-party adapters
  with their harness names, published versions and source locations. Being listed grants no trust.
- **Catalogue entry**: one adapter's listing: harness name, and per version, where its source
  lives and a fingerprint of that source.
- **Content change note**: one change an adapter made on one attempt: location, kind, reason. No
  content.
- **Guardrail event**: one blocked adapter change: the adapter and version, the request, the
  direction (request or response), and what the adapter tried.
- **Alert**: an operator notice (guardrail event, adapter failure, source mismatch) that stays
  visible until acknowledged.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: hermes, with only a base URL and an agent key, completes multi-turn sessions that use
  tools, reasoning across turns, and images, against each of the four chosen text providers,
  streamed and not streamed, with zero client or provider errors.
- **SC-002**: A hostile-adapter suite (network, files, environment, secrets, runaway time, runaway
  memory) is contained in 100% of runs: zero connections or file accesses leave the sandbox, zero
  secrets reach an adapter, and 100% of the requests complete.
- **SC-003**: In 100% of guardrail tests, adding or changing a tool call, a tool definition or a
  tool result, in requests and responses, streamed and not, is stopped: the unmodified request or
  response goes on, the suspect mark, alert and record are present, and the adapter serves no
  further request until cleared. Legitimate removals trip the guardrail in 0% of runs.
- **SC-004**: 100% of gate tests with a foreign dependency, a build script, a procedural macro, an
  opaque blob, a binary file, or oversized code are refused, and the message names each reason.
- **SC-005**: In 100% of tampering tests (source or compiled form changed after review), the core
  refuses to run the adapter and alerts the operator.
- **SC-006**: Across an update's full lifecycle (queued, in review, quarantined, rejected,
  approved), 0% of requests from bound keys fail, and 100% are served by the approved version
  in force at the time.
- **SC-007**: The review agent's input contains 0 comments and 0 original identifiers from the
  source, and 0 bytes of user data. The review agent runs 0 commands.
- **SC-008**: The Claude Code adapter goes through the full pipeline with no special-casing. Claude
  Code sessions using tools and web search complete against each chosen text provider with zero
  client errors.
- **SC-009**: 100% of adapter content changes in the test suite appear in their records, with
  location, kind and reason, and 0 records hold removed or converted content.
- **SC-010**: An adapter and the guardrail together add at most 5 ms at the 95th percentile to a
  request, and at most 1 ms per stream event, in the benchmark suite. The hot-path benchmarks
  have a committed baseline.
- **SC-011**: With no third-party adapter installed, 0router starts and passes slice 003's test
  suite with no builder, compiler or review model present.
- **SC-012**: In 100% of install tests, an adapter from the catalogue and the same source from a
  local folder pass through identical gate, build and review steps, and neither serves before the
  operator approves it. Over a test run with no operator catalogue command, 0router makes 0
  requests to the catalogue.
- **SC-013**: After an upgrade that changes the adapter kit, 100% of installed adapters whose
  reviewed source still compiles serve again with no operator action and no new review, and 100%
  of those that don't compile raise an alert naming the reason. No old build runs.

## Assumptions

Items marked *(technical decision)* are technical decisions Claude made. Per the user's
direction, technical choices are Claude's to make. They are not user requirements, and they may be
revised in planning without asking the user, as long as nothing the user sees changes.

- Slice 003 (request pipeline, including its optimizer pass-through amendment) is complete before
  this slice is implemented. Adapters run on slice 003's attempt path and records.
- "Every chosen provider" means slice 003's chosen text providers: anthropic, openrouter,
  opencode-zen and opencode-go. elevenlabs is outside hermes's feature set.
- hermes talks to 0router in the OpenAI Chat Completions style. Its echoed reasoning fields and its
  image and attachment formats follow 9router's handling (`paramSupport.js`, `combo.js`). Which
  chosen providers reject the echoed reasoning is checked by a live test during implementation,
  and the reject table is filled from its result. Anything else plan
  finds hermes needs is in scope (brief, C✓ hermes).
- A key bound to a harness with no approved version works as a plain client until a version is
  approved, so binding a key early never breaks it.
- The review agent's calls go through 0router's own pipeline, on an account the operator already
  holds for the chosen provider. The budget is a token limit per review, checked before the review
  starts and enforced while it runs. *(technical decision)*
- Alerts are shown in the CLI adapter listing and written to 0router's log. They stay until the
  operator acknowledges them. The dashboard is a later slice. *(technical decision)*
- The code size limit, the opaque-blob threshold, and the sandbox's time and memory limits are set
  in planning. *(technical decision)*
- The 5 ms and 1 ms figures in SC-010 are starting targets. *(technical decision)*
- Each catalogue entry pins every version to a fingerprint of its source. A fetched source that
  doesn't match its fingerprint is refused before the gate, so what the operator installs is what
  was listed, even though the source lives in the author's own repository. *(technical decision)*
- 9router selects Claude Code's handling by detecting the client. 0router binds it to the key
  instead: a deliberate deviation (brief, P notes).

## Out of Scope

- Built-in adapters for opencode, grok build and zcode: later slices. Until then, they connect as
  plain clients through slice 003.
- Account sign-in (OAuth), and the xai and grok-cli providers: slice 005.
- The routing decision and persistent request history: slice 006.
- Code in provider plugins: never. A provider that needs code becomes a core built-in.
- Per-request AI checks of adapters: never. AI runs only at review time.
- Compression, summarization or any other optimization by an adapter: never (Constitution IV).
