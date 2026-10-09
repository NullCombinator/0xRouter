# 0router Constitution

## Core Principles

### I. Plugin Safety (NON-NEGOTIABLE)

0router has plugins on both sides of the router:

- A **provider plugin** declares one provider. It is data, never code.
- A **harness adapter** handles one client harness's coupling or quirks. It may be code,
  but only sandboxed code under the rules below.

No plugin of either kind may make network requests, read or write the filesystem, or
receive secrets. The core does all sending. Secrets (API keys, OAuth client secrets) MUST
be injected by the core at request execution time only, and MUST NOT be stored in any
plugin. `inventory` (link-time registration) MUST NOT be used for any plugin extension
point; it is acceptable for built-in-only registration (same binary, no third-party
contribution) when a static table would be less ergonomic.

**Provider plugins** declare endpoints, auth schemes, model IDs, and parameter mappings in
a TOML file. The core alone acts on those declarations:

- Provider extension points are *data interpreters*, not code hooks.
- A provider plugin can never contribute a Rust function, compiled code, or `unsafe`
  block. A provider that needs code (for example, custom request signing) MUST become a
  core built-in instead.

**Bundled provider plugins** (0router's own provider TOML files) are also governed by
Plugin Safety. They are shipped with the binary but loaded through the same validation
path as third-party plugins. A bundled plugin passes the same validation gate; the only
privilege it gains is a lower install-conflict precedence (user plugins can replace
bundled ones by confirming the replace-or-decline prompt).

**Harness adapters** are either built in (core code, reviewed like the rest of the core)
or third-party. A third-party adapter:

- MUST run only inside the sandbox, never linked into the core process. The sandbox, not
  the review, is the security boundary.
- ships as source only. A builder service, separate from the core, compiles it. The core
  MUST run only a compiled adapter whose source matches the reviewed source.
- MAY depend only on 0router's adapter kit. Other crates, build scripts, and procedural
  macros MUST be refused. The validation gate MUST refuse opaque blobs (long base64 or
  hex strings) and code over the size limit.
- MUST pass review before it serves. Installing or updating it places it in a review
  queue. The review agent reads a copy with comments stripped and identifiers renamed,
  has no access to user data, cannot run commands, and only reports; the operator
  decides. The agent runs on a model, provider, and budget the operator picks; without
  them, the adapter stays in quarantine.
- MUST NOT update automatically. While an update is queued or quarantined, the previous
  version keeps serving.
- is checked on every request by a rule-based guardrail. If the adapter adds or changes a
  tool call, the core MUST discard that adapter's changes, send the unmodified request,
  mark the adapter as suspect, alert the operator, and record what the adapter tried. AI
  runs only at review time, never per request.

**Rationale**: Provider plugins extend the provider set without trust escalation, because
data cannot act. Harnesses change too often, and are too often coupled to their own
servers, for the core alone to keep up; community adapters are the only way to cover that
surface. Letting them be code is safe only because the sandbox removes network, files, and
secrets; the reviewed source is exactly what runs; the supply chain is 0router's own kit;
and nothing changes without the operator's decision.

---

### II. Routing Fidelity (NON-NEGOTIABLE)

The routing decision is the primary core responsibility. The router MUST implement all
four properties:

- **Cache-aware routing** — prefer the provider holding the caller's warm cache; switching
  costs only what the new provider lacks.
- **Per-agent isolation** — concurrent callers of the same unified model MUST have fully
  independent cache bookkeeping; no cross-caller state.
- **Windowed amortization** — over a configurable window, traffic spreads across providers
  weighted by declared parameters (rate limits, quotas, offers), corrected by what polls
  prove. Plugins declare, polls correct once the evidence is significant, and the operator
  overrides both. Precedence MUST be: operator override, then a significant fitted value,
  then the plugin's declaration. Until a correction is significant, the declared or
  overridden value MUST be used unchanged. Quota used outside 0router MUST NOT count as
  evidence against a plugin's declaration.
- **Priority order** — warm cache preference takes unconditional priority over amortization.

**Rationale**: These four properties are the entire reason 0router exists. A routing
implementation that omits or weakens any of them is not 0router. A provider's polls are
the source of truth for its quota: a plugin's wrong numbers must not fool the router for
long, but noise and outside use must never be mistaken for a wrong number.

---

### III. Unified Models and Provider Entities

Routing operates on *unified models* and *provider entities*. These are the primary
abstractions — not per-modality endpoints.

- A **provider entity** is one community plugin declaring one provider: its transport
  config, auth, and separate sections for each capability it offers (text, image, audio,
  embedding, decision, and so on). Sections are present only when the provider actually
  offers that capability.
- A **unified model** is a named target that gathers one or more provider entities
  offering the same model. Clients route to a unified model; the router selects the
  provider based on cache state, amortization, and declared weights as corrected by polls
  (II).
- **Combos** are policies over unified models (e.g. fallback chains, load-balancing
  groups). Nested combos are first-class. A combo can be tested with its own test suite,
  independent of the providers inside it.

**Implication**: 0router is a model manager, not a text-generation manager. Image,
audio, embedding, and other model types are first-class from the start.

**Rationale**: init.md defines this as the core model. Per-modality split providers
(9router's design) are a workaround for a different routing philosophy. 0router's
unified entity model is the reason plugins can deliver a complete provider in one file.

---

### IV. Scope Discipline

The router MUST forward requests as received. It MUST NOT compress, truncate, summarize,
or rewrite prompt content. Token optimization is a separate hop:

```
Agent → Optimizer → 0router → Provider
         (rtk, ponytail, caveman, headroom, …)
```

0router is a clean downstream for optimizers. It MUST expose a standard API surface that
accepts requests an optimizer has already rewritten. Where the provider speaks the
client's API style, it MUST pass through the unknown fields and headers an optimizer adds,
without rejecting or stripping them. Across styles, a field the provider's style has no
place for MAY be dropped, and every dropped field MUST be noted in the request record.
Prompt content is never dropped. At least one real optimizer chain
(agent → headroom → 0router) MUST be tested end to end.

**Harness coupling exception**: a harness adapter (Principle I) MAY change request content
only where its harness's coupling requires it: removing or converting parts the target
provider cannot accept, such as another server's tool blocks. It MUST NOT compress,
truncate, summarize, or otherwise optimize content. Every content change an adapter makes
MUST be noted in the request record.

**What is out of scope**: rtk (token compression filters), headroom, pxpipe, caveman,
ponytail. These live upstream. No code from these tools belongs in 0router.

**Rationale**: Mixing routing and optimization creates an unpredictable API surface for
upstream optimizers. The clean separation is what makes 0router a trustworthy downstream.
Some harnesses send content that only their home server accepts; removing it is what lets
them reach other providers at all. The exception stays narrow and recorded, so it can't
become optimization by the back door.

---

### V. Streaming-Native SSE

SSE responses MUST be streamed without buffering. Cancellation MUST propagate through the
full pipeline via `tokio_util::sync::CancellationToken`. The required pattern is
`axum::response::sse::Sse<impl Stream>` with `futures::StreamExt` adaptors. A buffered
SSE implementation MUST NOT be merged.

Client disconnect cancels the upstream request. Use a `CancelOnDrop` guard (or equivalent)
so that dropping the stream's sender side propagates cancellation before the upstream
response is fully consumed.

**Rationale**: Buffering defeats the latency benefit of SSE and creates unbounded memory
pressure under concurrent load. Clean cancellation is necessary to avoid leaking upstream
connections.

---

### VI. Reference-Informed Behavior

9router is a behavioral oracle for the specific behaviors 0router deliberately inherits:

- Error classification (text-rule-before-status-rule priority, specific cooldown durations)
- Retry and timeout semantics (specific defaults and env-override behavior)
- Wire translation between provider formats (translators, pivot logic, lossy pairs)
- OAuth flows

For these behaviors, 0router MUST match 9router's behavior. Parity tests using 9router's
own test suite (especially `tests/__baseline__/*.mjs`) are the verification gate.

**There is no file-order port mandate.** 0router's architecture differs from 9router's
(unified provider entities, unified models, no rtk, first-class model types). Port in the
order that builds a compiling, testable slice — not in 9router's dependency order, which
has import cycles and omits layers 0router needs.

Fail-open middleware paths (any path that recovers from errors by returning the original
value or None) MUST translate to `Result::ok()` returning `Option<T>`. They MUST NOT panic.

**Rationale**: Out-of-order mandates create unbuildable intermediate states. The behavioral
oracle rule captures what matters (correctness) without constraining architecture.

---

### VII. Trustworthy Model Tests

A model test verdict MUST come from a real, minimal call through that provider and model.
A model MUST be marked BROKEN only on a definitive rejection signal. Timeouts and
transient errors MUST produce UNKNOWN status and trigger an automatic retest. UNKNOWN
MUST NOT be promoted to BROKEN without an explicit rejection from the provider.

**Rationale**: A false BROKEN label silently removes valid capacity from the routing pool.
That is a correctness failure with direct user impact and potential revenue consequence.

---

### VIII. Latency Observability

Latency MUST be measured and surfaced — it is not a metric noticed retrospectively.
The routing layer MUST instrument time-to-first-token (TTFT) and total request duration
per provider, per unified model, from real traffic, and surface them to the operator.

Latency measurements inform the operator; they MUST NOT change the routing decision by
themselves. No provider, account, or unified-model member may be preferred, demoted,
skipped, or reweighted because it was measured slow or fast. The operator acts on what
latency shows through settings they control (priority, timeouts, and other connection
settings). A request that exceeds a configured timeout is an attempt failure, classified
and retried or failed over like any other failure (VI); that is not latency steering.

**Rationale**: init.md lists "latency becomes something measured and visible, not something
noticed" as a first-class goal. Observability is part of the routing contract, but routing
stays predictable: the four properties in II decide where a request goes, and a latency
signal that silently moved traffic would make that decision unexplainable and could undo
warm-cache preference on noise. The operator, who knows why a provider is slow, decides.

---

## Architecture Constraints

- **Language**: Rust only. No JS/TS in core routing logic. Third-party harness adapters are
  Rust compiled to WASM by the builder service.
- **Async runtime**: Tokio. Blocking operations MUST NOT run on the async executor; use
  `tokio::task::spawn_blocking` where unavoidable.
- **Provider model**: Built-in providers as bundled TOML plugins. Plugin-loaded providers
  use the same data model. `enum` dispatch for the fixed set of built-in executor/auth
  implementations; `Box<dyn Trait>` only where runtime dispatch is genuinely required:
  today, only third-party harness adapters behind the WASM sandbox.
- **No self-registration**: Side-effect registration on import is forbidden. Built-in
  translators use static tables. `inventory` is reserved for intra-binary (built-in-only)
  registration and MUST NOT be used for plugin extension.
- **Secrets isolation**: Secrets MUST NOT be passed to or stored by plugin declarations.
  They MUST be injected by the core at request execution time only.
- **Bundled provider secrets**: 9router's registry files hardcode OAuth `clientSecret` for
  four active providers (antigravity, gemini-cli, gemini, iflow). trae also carries one,
  but it is disabled in 9router and is not bundled. When these are converted to bundled
  TOML plugins, their secrets MUST be extracted into a core-side credential table, not
  placed in the TOML file.
- **Performance gate**: Any change touching routing, streaming, execution, or harness
  adapter hot paths MUST include a Criterion benchmark. Regressions block merge.

## Development Workflow

1. **Spec-driven**: All non-trivial features begin with the full speckit cycle:
   constitution → specify → clarify → plan → tasks → implement → converge.
2. **Reference-informed, not port-ordered**: 9router is the behavioral oracle. Build
   compiling, testable slices in 0router's own architecture; use 9router's `__baseline__`
   and unit test suites as parity fixtures.
3. **Parity-audit gate**: Run `/rust-parity-audit` on every module that inherits 9router
   behavior before opening a PR. A passing audit is a merge prerequisite for those modules.
4. **Benchmark gate**: Criterion benchmarks are required on routing and streaming path
   changes. Establish a baseline before any optimization work.
5. **Agent-assisted review**: Use project agents for their designated domains —
   `js-to-rust-porter` for behavior extraction from 9router, `streaming-architect` for
   SSE/async design, `plugin-system-designer` for plugin schema, `rust-engineer` for
   general Rust.

## Governance

This constitution is the authoritative source for 0router's non-negotiable invariants. It
supersedes all other development guides, README instructions, and verbal agreements. When
a conflict exists between this document and any other, this document wins.

**Amendment procedure**: Amendments MUST be proposed as a diff with explicit version bump
rationale and committed in a dedicated `docs: amend constitution` commit. MAJOR bumps
(principle removal or redefinition) require written justification in the commit body.

**Versioning policy**: Semantic versioning — MAJOR for backward-incompatible governance
changes (principle removal/redefinition), MINOR for new principles or materially expanded
guidance, PATCH for clarifications and wording fixes.

**Compliance review**: Every PR touching core routing, plugin loading, the adapter sandbox,
or SSE streaming MUST reference the relevant principle(s) in its description. Reviewers
MUST verify compliance before approving.

**Version**: 4.0.0 | **Ratified**: 2026-09-26 | **Last Amended**: 2026-10-07

**v4.0.0 changes (MAJOR)**: Redefined Latency Observability (VIII). Latency measurements
no longer feed the routing decision: they inform the operator and MUST NOT prefer, demote,
skip, or reweight anything by themselves. The operator acts through priority, timeouts,
and other connection settings. A configured timeout remains an ordinary attempt failure.
Measuring TTFT and total duration per provider and per unified model, from real traffic,
is unchanged. The four Routing Fidelity properties (II) are unchanged.

**v3.1.0 changes (MINOR)**: Expanded windowed amortization in Routing Fidelity (II). Polls
now correct a plugin's declared quota parameters once the evidence is significant; the
operator's override still wins, and until a correction is significant the declared or
overridden value is used unchanged. Outside use never counts against a declaration.
Unified Models (III) refers to the corrected weights. The four routing properties and the
warm-first priority are unchanged.

**v3.0.1 changes (PATCH)**: Clarified the optimizer pass-through in Scope Discipline (IV).
It is guaranteed where the provider speaks the client's API style. Across styles, a field
the provider's style can't hold may be dropped and is recorded; prompt content never is.

**v3.0.0 changes (MAJOR)**: Redefined Plugin Safety (I). Plugins now exist on both sides:
provider plugins stay data only, and harness adapters may be sandboxed code with no
network, file, or secret access. A third-party adapter is source only, depends only on the
adapter kit, is compiled by a separate builder service, passes an operator-decided review,
never updates automatically, and is checked per request by a rule-based tool-call
guardrail. Redefined Scope Discipline (IV): unknown optimizer fields and headers pass
through, an optimizer chain is tested, and harness adapters may change content only where
their coupling requires it, with every change recorded. Architecture Constraints follow:
adapters are Rust compiled to WASM, `Box<dyn Trait>` is allowed for them, and adapter hot
paths join the performance gate.

**v2.0.1 changes (PATCH)**: Corrected the "Bundled provider secrets" constraint to the
four active providers. trae is disabled in 9router and is not bundled. No rule changed.

**v2.0.0 changes (MAJOR)**: Added Principles III (Unified Models & Provider Entities),
VIII (Latency Observability). Replaced "Parity-First Porting" with "Reference-Informed
Behavior" (VI) — removes the fixed file-order port mandate and recognises that 0router's
architecture differs from 9router's. Replaced "sole core responsibility" framing with the
full init.md scope (unified models, model-type breadth, combos, observability, dashboard).
Removed rtk from scope (IV). Corrected `inventory` and plugin-extension-point guidance (I).
Added bundled-plugin model and secrets-extraction constraint. The four Routing Fidelity
properties (II) and Trustworthy Model Tests (VII) are unchanged.
