# 0router Constitution

## Core Principles

### I. Plugin Safety (NON-NEGOTIABLE)

A 0router plugin is data, not code. It MUST declare endpoints, auth schemes, model IDs,
and parameter mappings in a TOML file. It MUST NOT execute code or scripts, make network
requests directly, read the filesystem, or receive secrets. The core alone acts on plugin
declarations.

**Rationale**: Third-party plugins extend the provider set without requiring trust
escalation. Violating this invariant makes supply-chain compromise trivially easy and
breaks the security contract users depend on.

### II. Routing Fidelity (NON-NEGOTIABLE)

The routing decision is the sole core responsibility. The router MUST implement all four
properties:

- **Cache-aware routing** — prefer the provider holding the caller's warm cache; switching
  costs only what the new provider lacks.
- **Per-agent isolation** — concurrent callers of the same unified model MUST have fully
  independent cache bookkeeping; no cross-caller state.
- **Windowed amortization** — over a configurable window, traffic spreads across providers
  weighted by declared parameters (rate limits, quotas, offers).
- **Priority order** — warm cache preference takes unconditional priority over amortization.

**Rationale**: These four properties are the entire reason 0router exists. A routing
implementation that omits or weakens any of them is not 0router.

### III. Scope Discipline

The router MUST forward requests as received. It MUST NOT compress, truncate, summarize,
or rewrite prompt content. Token optimization is a separate hop (Agent → Optimizer →
0router) and MUST remain external to this codebase.

**Rationale**: Mixing routing and optimization creates an unpredictable API surface for
upstream optimizers that have already rewritten the request. The clean separation is what
makes 0router a trustworthy downstream.

### IV. Streaming-Native SSE

SSE responses MUST be streamed without buffering. Cancellation MUST propagate through the
full pipeline via `tokio_util::sync::CancellationToken`. The required pattern is
`axum::response::sse::Sse<impl Stream>` with `futures::StreamExt` adaptors. A buffered
SSE implementation MUST NOT be merged.

**Rationale**: Buffering defeats the latency benefit of SSE and creates unbounded memory
pressure under concurrent load. Clean cancellation is necessary to avoid leaking upstream
connections.

### V. Parity-First Porting

Every JS module ported from 9router MUST pass the 7-section behavioral parity audit
(`/rust-parity-audit`) before merge. Port order is fixed and MUST be followed:
`config/` → `translator/schema/` → `translator/concerns/` →
`translator/request|response/` → `executors/base` → `executors/default` → `rtk/` →
`handlers/chatCore` → axum router.

Fail-open middleware paths (JS `try { … } catch { return null }`) MUST translate to
`Result::ok()` returning `Option<T>`. They MUST NOT panic.

**Rationale**: Out-of-order porting creates unbuildable intermediate states. Parity
audits prevent silent behavior drift from being discovered in production.

### VI. Trustworthy Model Tests

A model test verdict MUST come from a real, minimal call through that provider and model.
A model MUST be marked BROKEN only on a definitive rejection signal. Timeouts and
transient errors MUST produce UNKNOWN status and trigger an automatic retest. UNKNOWN
MUST NOT be promoted to BROKEN without an explicit rejection from the provider.

**Rationale**: A false BROKEN label silently removes valid capacity from the routing pool.
That is a correctness failure with direct user impact and potential revenue consequence.

## Architecture Constraints

- **Language**: Rust only. No JS/TS in core routing logic.
- **Async runtime**: Tokio. Blocking operations MUST NOT run on the async executor; use
  `tokio::task::spawn_blocking` where unavoidable.
- **Provider model**: Closed `enum` for built-in providers; `Box<dyn Trait>` only for
  plugin-extensible points.
- **No self-registration**: Side-effect registration on import is forbidden. Built-in
  translators use static tables; the `inventory` crate is reserved for plugin translators.
- **Secrets isolation**: Secrets MUST NOT be passed to or stored by plugin declarations.
  They MUST be injected by the core at request execution time only.
- **Performance gate**: Any change touching routing, streaming, or execution hot paths MUST
  include a Criterion benchmark. Regressions block merge.

## Development Workflow

1. **Spec-driven**: All non-trivial features begin with the full speckit cycle:
   constitution → specify → clarify → plan → tasks → implement → converge.
2. **Port in dependency order**: Follow the fixed order from the Architecture Constraints
   section. Skipping layers is not permitted.
3. **Parity-audit gate**: Run `/rust-parity-audit` on every ported module before opening
   a PR. A passing audit is a merge prerequisite, not a recommendation.
4. **Benchmark gate**: Criterion benchmarks are required on routing and streaming path
   changes. Establish a baseline before any optimization work.
5. **Agent-assisted review**: Use project agents for their designated domains —
   `js-to-rust-porter` for translation, `streaming-architect` for SSE/async design,
   `plugin-system-designer` for plugin schema, `rust-engineer` for general Rust.

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

**Compliance review**: Every PR touching core routing, plugin loading, or SSE streaming
MUST reference the relevant principle(s) in its description. Reviewers MUST verify
compliance before approving.

**Version**: 1.0.0 | **Ratified**: 2026-09-26 | **Last Amended**: 2026-09-26
