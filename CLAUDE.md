# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

**NullRouter** (written 0router; crates, binary and env vars use `nullrouter`, never "zero router") — a from-scratch Rust implementation of [9router](ref/9router/)'s core routing engine. The JS reference lives in `ref/9router/`.

Workspace layout:

| Path | Contents |
|---|---|
| `crates/nullrouter-registry` | Provider and unified-model registry: plugin schema, validation gate, lookup, reload |
| `crates/nullrouter-wire` | API-style interpreter: translates client bodies ↔ IR ↔ provider wire bodies; no I/O, no async runtime |
| `crates/nullrouter-engine` | Request engine: operator state snapshot, attempt loop (classification, retry, fallback, stay-warm), upstream calls, request records |
| `crates/nullrouter-server` | HTTP surface over the engine: style-built routes, access-key check, streaming relay, model lists, token counts, operator socket, read model (`views`) |
| `crates/nullrouter-dashboard` | Read-only web dashboard served by `serve`: access, pages, style tokens, assets |
| `crates/nullrouter-cli` | `nullrouter` CLI: `serve`, `accounts`, `keys`, `behaviour`, `records`, `plugins`, `check`, `validate`, `resolve`, `unified`, `model`, `providers` |
| `plugins/bundled/` | The seven bundled providers (schema 2 TOML): seeded from `ref/9router` by the generator, then maintained by hand |
| `plugins/community/` | The other 114 providers, generated from `ref/9router`; embedded, fit-checked, and installed on request |
| `styles/bundled/` | The four client API styles (Chat Completions, Messages, Responses, Gemini): data files read by `nullrouter-wire` |
| `tools/gen-bundled/` | Generator for the bundled plugins, credentials, and parity oracle |
| `tests/fixtures/9router/` | Parity oracle snapshots (generated; never hand-edit) |
| `tests/harness/` | Real SDK and harness runs (Python, Node, Claude Code, Codex CLI, headroom) against the server; `NR_HARNESS=1` |
| `docs/` | Plugin-author and operator documentation |

After updating `ref/9router`, regenerate with `node tools/gen-bundled/generate.mjs` and commit the output on its own, naming the ref SHA. Build commands need `export CARGO_HOME=$PWD/.cargo-home`.

Run the server with `cargo run -p nullrouter-cli -- serve` (listens on `127.0.0.1:20129`; state in `$NULLROUTER_HOME`, default `~/.0router`). Add an account with `nullrouter accounts add <provider> <name>` (secret on stdin) and a client key with `nullrouter keys issue <name>`. See `docs/operator-config.md`.

0router re-implements 9router's core routing engine in Rust, with a different routing decision (cache-aware, per-agent isolation, windowed amortization), a unified provider entity model (one plugin = one provider with per-modality sections), unified models as routing targets, first-class support for non-text model types, latency observability, testable combos, and a two-sided plugin model: third-party providers are declared as TOML data files, and harness adapters run as sandboxed WASM. See `init.md` for the full intention and `init.md`→`constitution.md` for the non-negotiable invariants.

## Running this identity

This Claude Code instance runs under an isolated identity (`claude-0router`) with Landlock filesystem confinement. Start it with:

```bash
claude-0router      # launcher at ~/.local/bin/claude-0router
```

The gate allows rw to `~/.claude-0router`, `~/Desktop/0router`, and standard temp paths (`/tmp`, `/dev/{null,ptmx,tty,pts,shm}`). `~/Desktop` is read-only (project browsing). `/usr`, `/bin`, `/lib`, `/etc`, and other system trees are read-only. Everything else in `$HOME` is denied. See `identity/README.md` for the full ruleset and how to recompile the gate binary.

## Cloud sessions

Plain test and clippy runs belong to GitHub Actions (`.github/workflows/ci.yml`), which is free for this public repo and runs on every push to any branch and on PRs from forks; don't spend a cloud session on them. Heavy work that needs Claude, such as fixing what CI found or a port fan-out, runs in a cloud session (`claude --cloud "<task>"`), which has 4 vCPU and 16 GB, against the pushed `main`. If the agentmemory tools are missing, you are in one. Then:

- `ref/9router` is cloned by the SessionStart hook (`tools/cloud/session-start.sh`) at the fixtures' SHA. The MCP servers, headroom, `.cargo/capped` and `.nr-live/` don't exist there; skip the session protocol below.
- Build with `CARGO_HOME=$PWD/.cargo-home`, as locally; full-workspace runs are fine.
- Work on a `claude/` branch, never `main`, and put the decisions and lessons the session protocol would have saved under a `## For memory` heading in the PR description. The local session files them in agentmemory after review.

Criterion benches stay local: cloud timings don't compare with the baselines in `target/`.

## MCP servers

All five are wired in `.mcp.json`, which is gitignored because it holds the agentmemory HMAC secrets and the GitHub token. They are enabled in `~/.claude-0router/settings.json` (`enabledMcpjsonServers`).

| Server | Port | Purpose |
|---|---|---|
| `agentmemory` | 3211 | Main-session memory |
| `agentmemory-team` | 3212 | Subagent memory (`TEAM_ID=0router`) |
| `code-review-graph` | — | Structural graph of `ref/9router` (JS oracle, static at the ref SHA) |
| `code-review-graph-0router` | — | Structural graph of this repo's tracked files (Rust). `--auto-watch` keeps it current |
| `github` | — | GitHub's remote MCP server, Actions toolset only: CI runs, job logs, reruns. Uses the repo-scoped token in `.git/nr-github-token`, which also lets `git push` work from this identity |

**The two memory instances are separate stores.** Nothing written to one is visible from the other, and `memory_team_share` does not bridge them. Main sessions use `mcp__agentmemory__*`. Subagents can reach only `mcp__agentmemory-team__*`, according to the `tools:` allowlist in their agent frontmatter. Anything a subagent must know goes to the team instance.

**Session protocol (main session):**
1. Start: run `memory_slot_get` on `project_context`, `pending_items` and `guidance`, then `memory_smart_search` on the task.
2. Before porting, auditing or changing shared code:
   - trace 9router callers with `code-review-graph` (`query_graph_tool`, `get_affected_flows_tool`);
   - check 0router callers and blast radius with `code-review-graph-0router` (`query_graph_tool`, `get_impact_radius_tool`, `detect_changes_tool`).
   Prefer these to grep for structure questions.
3. End of a slice or commit:
   - `memory_save` each decision the code doesn't show, to both instances when subagents need it;
   - update `pending_items`;
   - `memory_lesson_save` anything learned the hard way.

The file memory (`~/.claude-0router/projects/.../memory/`) holds only short pointers and user feedback. Project knowledge lives in agentmemory.

**Rebuilding graphs:** use `code-review-graph build|update|postprocess --repo <path>`. Under Landlock, SQLite's default temp directory is denied, so run these with `SQLITE_TMPDIR=/tmp`, or the full-text index silently fails to build. The MCP entries already set it. Rebuild the `ref/9router` graph only after updating the ref.

## Reference codebase

`ref/9router/` is the JS source being ported. Read these two files before working in it:

- `ref/9router/CLAUDE.md` — architecture, commands, persistence layer, gotchas
- `ref/9router/open-sse/AGENTS.md` — the routing engine's conventions and "how to add a provider/executor/translator"

The request lifecycle: `src/app/api/v1/*` → `src/sse/handlers/chat.js` → `open-sse/handlers/chatCore.js` → `open-sse/translator/*` → `open-sse/executors/*` → SSE back to client.

## Spec-driven development (speckit)

The project uses speckit 1.0.12 for specification-driven development. Entry point: `/speckit-constitution`. The full SDD cycle: constitution → specify → clarify → plan → tasks → implement → converge. Quality gates: analyze (cross-artifact consistency), checklist (requirements unit tests). The `.specify/` directory holds the integration config, scripts, and workflow templates.

**Until the core is complete, every new slice starts with `/shape-spec`, not `/speckit-specify`.** Claude builds a status snapshot of the core (shipped, partial, absent) and suggests the next slice. Then the user tests the suggestion claim by claim, and the output is a `/speckit-specify` command built only from confirmed claims. Never hand the user a raw `/speckit-specify` description: slice 003's drift came from a scope limit Claude wrote and the user pasted. Briefs are kept in `specs/briefs/`. Once the core is shipped, the user specifies freely.

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

Defaults live in `~/.claude-0router/settings.json` (main session: Opus 5.5 @ `high`; ad-hoc subagents without a pin, such as general-purpose and Plan: Haiku 5.5). Agent `model:`/`effort:` frontmatter is the per-role policy and wins over the `CLAUDE_CODE_SUBAGENT_MODEL` default — never set `CLAUDE_CODE_SUBAGENT_MODEL_FORCE`, which silently discards every agent pin.

| Tier | Agents |
|---|---|
| Opus 5.5 `high` — judgment-heavy, errors are expensive | js-to-rust-porter, streaming-architect, plugin-system-designer, architect-reviewer, security-auditor |
| Opus 5.5 `medium` — review, debugging | code-reviewer, debugger, llm-architect |
| Sonnet 5.5 `medium` — spec'd implementation with a test to verify against | rust-engineer, performance-engineer, error-detective, api-designer |
| Haiku 5.5 `medium` — well-specified tasks, mechanical work, or fanned-out work a later gate checks | task-implementer, test-automator, docker-expert, perf-hypothesis-explorer |

The `haiku` alias (`ANTHROPIC_DEFAULT_HAIKU_MODEL`) resolves to Haiku 5.5, so the built-in Explore agent runs on it too. Keep Haiku off security work (its cyber safeguards are stricter than Haiku 4.5's) and off anything that makes claims about `ref/9router` without a later check.

Main-session choice by work type (use `s` in `/model` or `/effort` to make it session-only):

- **Opus 5.5 `high`** (default): constitution/specify/clarify/analyze, writing skills, agents, or specs, parity audits.
- **Opus 5.5 `medium`**: `/speckit-plan`, `/speckit-tasks`, workflow orchestration.
- **Sonnet 5.5 `medium`**: `/speckit-implement`. Its mandatory `before_implement` hook (`.specify/extensions.yml`) loads `implement-dispatch`, which sends each task to `task-implementer` (Haiku) by default and escalates to `rust-engineer`, then to the user.
- **Sonnet 5.5 `low`**: chores (git, formatting, doc typos). Not `opusplan` for anything that makes claims about `ref/9router`.
- One hard turn: add `ultrathink` to the prompt instead of raising session effort. Avoid `max`.
- Never switch model mid-session: each model has its own prompt cache, so the next turn re-writes the whole context. Start a new session or delegate to a pinned subagent instead.
- Main sessions auto-compact at a 300k window (`CLAUDE_CODE_AUTO_COMPACT_WINDOW`). Run `/compact` yourself between tasks, and don't let one session span a whole slice.

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
- **Class hierarchy** (BaseExecutor + subclasses) → Rust `trait` (with `async_trait` for `dyn` compatibility) + closed `enum` for the builtin provider set; `Box<dyn Trait>` only for runtime dispatch that genuinely needs it (today, only third-party harness adapters behind the WASM sandbox).
- **Side-effect self-registration** (translator `register(from, to, ...)` on import) → static table for builtins. `inventory` is acceptable for intra-binary registration; it is **not** for any plugin extension point.
- **AbortController/Signal** → `tokio_util::sync::CancellationToken` + `tokio::select!`
- **SSE streaming** → `axum::response::sse::Sse<impl Stream>` with `futures::StreamExt` adaptors; never buffer.

**9router is a behavioral oracle, not a port template.** 0router's architecture differs (unified provider entities, unified models, first-class model types beyond text, no rtk). Build compiling, testable slices using 9router's `tests/__baseline__` snapshots and unit tests as parity fixtures; do not force 9router's import-cycle dependency order onto 0router.

## Plugin safety invariant

0router has plugins on both sides of the router (constitution v3.0.0, Principle I). No plugin of either kind may make network requests, read or write the filesystem, or receive secrets. The core does all sending and injects secrets only at execution.

- A **provider plugin** is **data, not code**. It declares endpoints, auth schemes, model IDs, and parameter mappings in a TOML file, and the core alone acts on those declarations. If a provider needs code, it belongs in the core as a built-in.
- A **harness adapter** handles one client harness's quirks and may be code, but only sandboxed code. Built-in adapters (hermes) are core code. A third-party adapter:
  - ships as Rust source only, depends only on 0router's adapter kit (no other crates, build scripts or proc macros), and is compiled to WASM by a builder service separate from the core;
  - runs only inside the sandbox, and only if its source matches the reviewed source;
  - goes live only after an operator-decided review; it never updates automatically, and the previous version keeps serving meanwhile;
  - is checked on every request by a rule-based guardrail: if it adds or changes a tool call, the core drops its changes and sends the unmodified request.

An adapter may change request content only where its harness's coupling requires it, and every change is recorded (Principle IV).
