# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

**0router** (null router) — a from-scratch Rust implementation of [9router](ref/9router/)'s core routing engine. The project is currently pre-code: infrastructure and tooling are in place, the first Rust crate has not yet been written. The JS reference lives in `ref/9router/`.

0router re-implements 9router's core routing engine in Rust, with a different routing decision (cache-aware, per-agent isolation, windowed amortization), a unified provider entity model (one plugin = one provider with per-modality sections), unified models as routing targets, first-class support for non-text model types, latency observability, testable combos, and a plugin model where third-party providers are declared as TOML data files. See `init.md` for the full intention and `init.md`→`constitution.md` for the non-negotiable invariants.

## Running this identity

This Claude Code instance runs under an isolated identity (`claude-0router`) with Landlock filesystem confinement. Start it with:

```bash
claude-0router      # launcher at ~/.local/bin/claude-0router
```

The gate allows rw to `~/.claude-0router`, `~/Desktop/0router`, and standard temp paths (`/tmp`, `/dev/{null,ptmx,tty,pts,shm}`). `~/Desktop` is read-only (project browsing). `/usr`, `/bin`, `/lib`, `/etc`, and other system trees are read-only. Everything else in `$HOME` is denied. See `identity/README.md` for the full ruleset and how to recompile the gate binary.

## MCP servers

All three are wired in `.mcp.json`:

| Server | Port | Purpose |
|---|---|---|
| `agentmemory` | 3211 | Main agent memory (coder-instance profile) |
| `agentmemory-team` | 3212 | Subagent memory (architect profile, `TEAM_ID=0router`, `scope=shared`) |
| `code-review-graph` | — | Structural graph of `ref/9router` (Tree-sitter parsed) |

**Which instance to use:** main agent sessions → `mcp__agentmemory__*`. Subagents spawned from workflows → `mcp__agentmemory-team__*`. Both share the same underlying data space via TEAM_ID.

## Reference codebase

`ref/9router/` is the JS source being ported. Read these two files before working in it:

- `ref/9router/CLAUDE.md` — architecture, commands, persistence layer, gotchas
- `ref/9router/open-sse/AGENTS.md` — the routing engine's conventions and "how to add a provider/executor/translator"

The request lifecycle: `src/app/api/v1/*` → `src/sse/handlers/chat.js` → `open-sse/handlers/chatCore.js` → `open-sse/translator/*` → `open-sse/executors/*` → SSE back to client.

## Spec-driven development (speckit)

The project uses speckit 1.0.12 for specification-driven development. Entry point: `/speckit-constitution`. The full SDD cycle: constitution → specify → clarify → plan → tasks → implement → converge. Quality gates: analyze (cross-artifact consistency), checklist (requirements unit tests). The `.specify/` directory holds the integration config, scripts, and workflow templates.

## Agents

Project-scoped agents in `.claude/agents/` — invokable as subagents from workflows or directly. Key agents for this project:

| Agent | When to use |
|---|---|
| `js-to-rust-porter` | File-by-file JS→Rust translation; uses `agentmemory-team` + `code-review-graph` |
| `streaming-architect` | Axum SSE design, `futures::Stream` composition, cancellation propagation |
| `plugin-system-designer` | Declarative data-not-code plugin schema, SSRF validation, built-in vs. plugin boundary |
| `perf-hypothesis-explorer` | Read-only: investigates one optimization direction, returns a hypothesis (no code changes) |
| `rust-engineer` | General Rust implementation with ownership/lifetime/async expertise |
| `llm-architect` | LLM system design, RAG, multi-model deployments |

## Model and effort policy

Defaults live in `~/.claude-0router/settings.json` (main session: Opus 5.5 @ `high`; ad-hoc subagents: Sonnet 5). Agent `model:`/`effort:` frontmatter is the per-role policy and wins over the `CLAUDE_CODE_SUBAGENT_MODEL` default — never set `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`, which silently discards every agent pin.

| Tier | Agents |
|---|---|
| Opus 5.5 `high` — judgment-heavy, errors are expensive | js-to-rust-porter, streaming-architect, plugin-system-designer, architect-reviewer, security-auditor |
| Opus 5.5 `medium` — implementation, review, debugging | rust-engineer, code-reviewer, debugger, llm-architect |
| Sonnet 5 `medium` — gated or fanned-out work | performance-engineer, perf-hypothesis-explorer, error-detective, api-designer |
| Sonnet 5 `low` — mechanical | test-automator, docker-expert |

Main-session choice by work type (use `s` in `/model` or `/effort` to make it session-only):

- **Opus 5.5 `high`** (default): constitution/specify/clarify/plan/analyze, writing skills, agents, or specs, parity audits.
- **Opus 5.5 `medium`**: `/speckit-implement` and workflow orchestration — the heavy lifting is in pinned subagents.
- **Sonnet 5 `low`**: chores (git, formatting, doc typos). Not `opusplan` for anything that makes claims about `ref/9router`.
- One hard turn: add `ultrathink` to the prompt instead of raising session effort. Avoid `max`.

## Workflows

Orchestration programs in `.claude/workflows/` — chain agents in phases with structured data passing:

| Workflow | Purpose |
|---|---|
| `port-module` | 4-phase: analysis → Rust impl → parity tests → audit. Input: `{ js_path, rust_out? }` |
| `port-layer` | Enumerate a JS directory, fan out `port-module` in parallel batches, produce a layer report |
| `audit-rust-port` | Given a Rust file, locate JS counterpart, run 7-section parity audit with triage |
| `optimize-perf` | Benchmaxxing loop: baseline → 7 parallel hypothesis subagents → implement → correctness gate → competitor benchmarks → refactor pass |
| `refactor-pass` | Standalone ≥20% SLoC reduction with zero criterion-regression gate |

## Skills

The porting skill set (load before any translation work):

- `/port-js-to-rust` — 10-pattern catalog + porting workflow + pre-merge checklist
- `/js-to-rust-patterns` — quick-reference cards: JS idiom → Rust one-liner + crate table
- `/rust-parity-audit` — 7-section behavioral parity audit protocol
- `/benchmaxxing` — 8 prompting rules for criterion-driven optimization (baseline-first, anti-cheat, radical thinking, breakthrough)

## Building conventions (9router → 0router)

The 10 recurring patterns in 9router and their Rust equivalents are catalogued in `.claude/skills/port-js-to-rust/SKILL.md`. The most important:

- **Fail-open middleware** (`try { ... } catch { return null }`) → `Result::ok()` returning `Option<T>`. Never panic in these paths.
- **Class hierarchy** (BaseExecutor + subclasses) → Rust `trait` (with `async_trait` for `dyn` compatibility) + closed `enum` for the builtin provider set; `Box<dyn Trait>` only for plugin-extensible runtime dispatch.
- **Side-effect self-registration** (translator `register(from, to, ...)` on import) → static table for builtins. `inventory` is acceptable for intra-binary registration; it is **not** for plugin extension (plugins are data, not code).
- **AbortController/Signal** → `tokio_util::sync::CancellationToken` + `tokio::select!`
- **SSE streaming** → `axum::response::sse::Sse<impl Stream>` with `futures::StreamExt` adaptors; never buffer.

**9router is a behavioral oracle, not a port template.** 0router's architecture differs (unified provider entities, unified models, first-class model types beyond text, no rtk). Build compiling, testable slices using 9router's `tests/__baseline__` snapshots and unit tests as parity fixtures; do not force 9router's import-cycle dependency order onto 0router.

## Plugin safety invariant

A 0router plugin **is data, not code**. It declares endpoints, auth schemes, model IDs, and parameter mappings in a TOML file. The core alone acts on those declarations. A plugin must never:
- Execute code or scripts
- Make network requests directly
- Read the filesystem
- Receive secrets

If a feature requires a plugin to run code, it belongs in the core as a built-in.
