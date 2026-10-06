# 0router (null router)

Intention, written 2026-09-25 before any code. This file is the seed of the project: what it is, why it exists, and where its boundaries will come from later.

## What it is

A Rust implementation of the 9router project core, built around one thing: the routing decision.

Routing, concretely:

- Providers exposing the same model are gathered, by explicit declaration, into one **unified model** clients ask for by name — before any combos. A combo is a policy over unified models.
- **Cache-aware routing** — cached-token state is tracked per agent, per unified model, across providers. The router prefers the provider holding the caller's warm cache; switching costs only what the new provider lacks.
- **Per-agent isolation** — concurrent callers of the same unified model are fully independent, each with its own cache bookkeeping; no conflicts.
- **Windowed amortization** — over a configurable window (e.g. 24h), traffic spreads across every provider offering the model, weighted by each provider's own parameters: rate limits, quotas, special offers. Plugins declare, the user overrides. Warm cache first; amortization spreads what remains.

## Why it exists

To make 9router faster, customizable, and to enrich it:

- **Latency observability** — latency becomes something measured and visible, not something noticed.
- **Claude form of connection**
- **Nested combos** — handled more efficiently.
- **Trustworthy model tests** — a test verdict comes only from a real, minimal call through that provider and model. It fails only on definitive rejection; timeouts and transient errors return UNKNOWN, which auto-retests and never labels a working model broken.
- **Testable combos** — combos gain test capability of their own, not just the providers inside them.
- **Unified providers from plugins** — providers become community plugins users customize. One plugin delivers one provider entity — not 9router's split per-modality providers — with text, image, audio, embedding, and the rest managed as separate sections inside it, each present only when the provider actually offers it.
- **Plugin safety by design** — a third-party plugin is data, not code: it cannot install binaries, make network requests, or execute commands. Endpoints, auth schemes, and request translations are declared; the core alone acts on them.
- **Beyond text generation** — a model manager, not a text-generation manager: first-class image, audio, embedding, decision, and other model types.
- **A clean upstream for optimizers** — token-optimization tools sit before the router (Agent → Optimizer → 0router, e.g. rtk, ponytail, caveman, headroom), and the router keeps that chain working: a clean standard API surface that accepts requests an optimizer already rewrote.
- **A familiar face for migration** — the dashboard keeps 9router's look and concepts, rebuilt on 0router's own stack, so moving over feels like an upgrade, not a relearning. The look comes from a style guide taken from 9router's code: colors, type, spacing, and component styles. The sidebar and page names are 9router's, so there is nothing new to learn, and the pages follow 0router's scope: no optimizer surfaces, unified models and model types instead of per-modality pages.

## Hard constraints

Not set yet. These will be defined after the first run, with a spec-driven toolkit helper.

## Out of scope

Not set yet. These will be defined after the first run, via the spec-driven toolkit — with one boundary set now:

- **Token optimization** — compression and context-shrinking happen before the router, in a separate hop (Agent → Optimizer → 0router). 0router forwards; it does not shrink.

## First milestone

Fully implemented, and it works for me, at least.
