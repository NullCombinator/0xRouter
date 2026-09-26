# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

**0router** (null router) — a from-scratch Rust implementation of [9router](ref/9router/)'s core routing engine. The project is currently pre-code: infrastructure and tooling are in place, the first Rust crate has not yet been written. The JS reference lives in `ref/9router/`.

The single thing 0router does differently from 9router: the routing decision. Cache-aware routing, per-agent isolation, windowed amortization across providers, and a plugin model where third-party providers are declared as data files (not code) that the core alone executes. See `init.md` for the full intention.

## Running this identity

This Claude Code instance runs under an isolated identity (`claude-0router`) with Landlock filesystem confinement. Start it with:

```bash
claude-0router      # launcher at ~/.local/bin/claude-0router
```

The gate allows rw to `~/.claude-0router` and `~/Desktop/0router` only. All other `$HOME` paths are denied at the kernel level. See `identity/README.md` for the full ruleset and how to recompile the gate binary.

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

The request lifecycle: `src/app/api/v1/*` → `src/sse/handlers/chat.js` → `open-sse/handlers/chatCore.js` → `open-sse/executors/*` → `open-sse/translator/*` → SSE back to client.

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

## Porting conventions (JS → Rust)

The 10 recurring patterns in 9router and their Rust equivalents are catalogued in `.claude/skills/port-js-to-rust/SKILL.md`. The most important:

- **Fail-open middleware** (`try { ... } catch { return null }`) → `Result::ok()` returning `Option<T>`. Never panic in these paths.
- **Class hierarchy** (BaseExecutor + subclasses) → Rust `trait` + closed `enum` for the builtin provider set; `Box<dyn Trait>` only for plugin-extensible points.
- **Side-effect self-registration** (translator `register(from, to, ...)` on import) → static table for builtins; `inventory` crate reserved for plugin translators.
- **AbortController/Signal** → `tokio_util::sync::CancellationToken` + `tokio::select!`
- **SSE streaming** → `axum::response::sse::Sse<impl Stream>` with `futures::StreamExt` adaptors; never buffer.

Port in dependency order: `config/` → `translator/schema/` → `translator/concerns/` → `translator/request|response/` → `executors/base` → `executors/default` → `rtk/` → `handlers/chatCore` → axum router.

## Plugin safety invariant

A 0router plugin **is data, not code**. It declares endpoints, auth schemes, model IDs, and parameter mappings in a TOML file. The core alone acts on those declarations. A plugin must never:
- Execute code or scripts
- Make network requests directly
- Read the filesystem
- Receive secrets

If a feature requires a plugin to run code, it belongs in the core as a built-in.
