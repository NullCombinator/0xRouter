# Scope brief: client side, harness adapters (slice 004)

Shaped with `/shape-spec` on 2026-09-28. The user asked to capture, in a new spec, the design
changes made while slice 003 was being implemented ("While we were implementing spec 3, we
had some design changes. Could we fix it in new spec?"). In `/speckit-clarify` and
`/speckit-plan`, an answer that contradicts a confirmed row below means stop and revisit
this brief. Don't accept it.

## Before `/speckit-specify`

1. **Constitution amendment** (`/speckit-constitution`), covering both principles:
   - I (Plugin Safety): harness (client) adapters may be sandboxed code; provider plugins
     stay data only.
   - IV (Scope Discipline): an adapter may change content only where its harness's
     coupling requires it; never compress, summarize or optimize; every change recorded.
2. **Slice 003 amendment**: unknown fields and headers that an optimizer adds pass through
   to the provider untouched, and an agent → headroom → 0router chain is tested in 003.
   Do it before T049–T056 build the style translators. 003's plan (R5) decodes native
   pairs through the IR, which would drop those fields.

## Core map (at `d9be050` plus uncommitted 003 work)

| Capability | Status | Evidence |
|---|---|---|
| Provider plugins, validation gate, unified-model lookup | shipped | slice 002 |
| Request pipeline foundation (styles as data, keys, accounts, redactor, schema 2) | partial | 003 Phase 1–2 (41/145 tasks), `nullrouter-wire`/`-engine`/`-server`, uncommitted |
| Serving requests, translation, model listing, token counting | absent | 003 US1, US6 |
| Retry, fallback, classification | absent | 003 US2 |
| Non-text model types executing | absent | 003 US3 |
| Broken-stream handling; latency and usage records | absent | 003 US4, US5 |
| Community plugin set | absent | 003 US8 (`plugins/community/` empty) |
| Optimizer pass-through of unknown fields and headers | absent | → 003 amendment |
| Client adapter plugins, built-in harnesses, third-party pipeline | absent | → **this slice (004)** |
| Account sign-in (OAuth), xai, grok-cli | absent | → 005 |
| Routing decision (design approved 2026-09-28), persistent history | absent | → 006 |
| Built-in opencode, grok build, zcode adapters | absent | → later slices |
| Combos, model tests, dashboard | absent | → later |

## Slice plan (renumbered, confirmed)

- **003:** request pipeline (plus the pass-through amendment).
- **004 (this):** client side: harness adapters.
- **005:** account sign-in (OAuth), xai and grok-cli.
- **006:** routing decision and persistent request history.
- **Later:** built-in opencode, grok build, zcode adapters; combos, model tests, dashboard.

## Playback (confirmed)

When this slice is done, hermes works fully against 0router: its tools, reasoning and images
work on every chosen provider, with only a base URL and a key. Harnesses that need special
handling get adapters. hermes's is built in; anyone else's is a third-party adapter written
in Rust and shipped as source. You install one by binding it to an agent key. A separate
builder service compiles it using only 0router's own adapter kit. A review agent reads a
scrambled copy and reports to you, and you decide whether it goes live. Updates never switch
on by themselves: the old version keeps serving while the new one waits for review. Adapters
can't touch the network, files or secrets. They may change content only where their
harness's quirks require it, and every change is recorded. If one tries to add or alter a
tool call, 0router drops its changes, sends the original request and alerts you. Claude
Code is the first third-party adapter and proves the whole flow.

## Ledger

| Tag | Claim | Source |
|---|---|---|
| U | Capture 003's design changes in a new spec | "Could we fix it in new spec?" |
| C✓ | A new slice after 003; 003 not replaced | Q: spec shape |
| C✓ | Outcome: built-in harnesses work end to end, and the whole third-party adapter pipeline exists (user picked this over Claude's split) | Q: outcome |
| C✓ | Constitution amendment (I and IV) before this spec | Q: outcome, Q: content |
| C✓ | Failure: extra setup or broken feature; sandbox escape or unreviewed code; update breaks a setup; unflagged tool-call edit | Q: failure |
| C✓ | Built-in harness this slice: hermes only | Q: built-ins |
| C✓ | opencode, grok build, zcode: built in, later slices; plain clients via 003 until then | Q: others |
| C✓ | Claude Code: community proof adapter through the full pipeline; installing it isn't a middleman | Q: Claude Code |
| C✓ | Content changes only where the coupling requires; never compress, summarize or optimize; each recorded | Q: content |
| C✓ | Adapters are Rust compiled to WASM (user picked this over Claude's Starlark) | Q: language |
| C✓ | Only 0router's adapter kit (reads and edits request and response); no other crates, build scripts, proc macros | Q: deps |
| C✓ | Separate builder service; the core runs only WASM matching the reviewed source; core alone without third-party adapters | Q: builder |
| C✓ | 003 amended for optimizer pass-through; headroom chain tested in 003 | Q: 003 fix |
| C✓ | Order: 004 client side → 005 sign-in → 006 routing | Q: order |
| C✓ | Provider plugins stay data only; providers needing code become built-ins | Q: providers |
| C✓ | Guardrail: drop the adapter's changes, send the original, mark suspect, alert, record | Q: guardrail (premortem 1) |
| C✓ | Adapter bound to the agent key; no detection | Q: selection (premortem 2) |
| C✓ | hermes: full feature parity (echoed reasoning, attachments, plus whatever plan finds) | Q: hermes (premortem 3) |
| M | Two-sided plugins: client adapters and provider plugins | agentmemory 2026-09-27 |
| M | Source only, compiled by 0router; sandbox with no network, files or secrets; core does all sending; gate refuses opaque blobs; code size capped | agentmemory 2026-09-27 |
| M | Review agent: comments stripped, identifiers renamed, no user data, can't run commands, reports only; operator decides; operator's model, provider and budget; none or too little → quarantine | agentmemory 2026-09-27 |
| M | No automatic updates; update → review queue; previous version keeps serving | agentmemory 2026-09-27 |
| M | Guardrail is rule-based and runs per request; AI only at review time | agentmemory 2026-09-27 |
| M | Coupled or closed harnesses: follow 9router's fixes ("free ride") | agentmemory 2026-09-27 |
| K | Plugin Safety as amended; secrets injected by the core only | Constitution I |
| K | Stream without buffering; disconnect cancels upstream | Constitution V |
| K | Criterion benchmarks on hot paths | Constitution, Architecture Constraints |

## Notes for research.md (P)

- 9router picks harness behaviour by detecting the client (`chatCore.js:203`,
  `clientTool === "claude"` → `normalizeClaudePassthrough`). 0router binds the adapter to the
  agent key instead: a deliberate deviation.
- Claude Code oracle: `normalizeClaudePassthrough` (`open-sse/translator/formats/claude.js:204`)
  removes foreign `server_tool_use` blocks. Under the "free ride" policy, 9router stays an
  ongoing oracle for Claude Code; track its fixes after each ref update.
- hermes echoed reasoning: 9router drops `reasoning_content`, `reasoning`,
  `reasoning_details` per provider (groq, mistral, cerebras) in
  `open-sse/translator/concerns/paramSupport.js`. 0router moves this into the hermes adapter.
  Which chosen providers (anthropic, openrouter, opencode-zen, opencode-go) reject the
  fields needs a live check.
- hermes attachments: `images` (Ollama-style) and `attachments` /
  `experimental_attachments` (`open-sse/services/combo.js:144-155`).
- 9router's `ANTIGRAVITY_PROMPT_REWRITES` hermes identity rewrite is Antigravity-specific
  (not a chosen provider) and a prompt rewrite; it does not apply.
- WASM runtime: pick one with fuel and memory limits and no WASI imports; adapters run on
  every request of their key, so benchmark the added latency.
- Builder: a pinned toolchain; offline builds against the vendored kit; refuse manifests with
  `build.rs`, proc-macro crates or any dependency other than the kit. Match the compiled
  artifact to the reviewed source by hash.
- Review agent requests: decide in plan whether they go through 0router's own pipeline with
  an operator-chosen model and account.
- The review agent's "scrambled copy" follows the 2026-09-27 record: comments stripped,
  identifiers renamed to meaningless names.

## Final command

```
/speckit-specify Client side: harness adapters. 0router gains plugins on the client side, next to the provider plugins, so that harnesses needing special handling work against 0router with only a base URL and an access key, and no middleman in between. An adapter is bound to an agent key: the operator names the key's harness when issuing or editing the key, and 0router never guesses the harness from the request. Through 0router's adapter kit, an adapter can read and edit both the request and the response.

One harness is built in: hermes. It reaches full feature parity on every chosen provider, so its tools, reasoning and images work. This includes removing the reasoning hermes echoes back on assistant messages wherever the target provider rejects it, and carrying hermes's own image and attachment format.

Third-party adapters are code. They ship as Rust source only, and a separate builder service running next to the core compiles them to WASM. An adapter may depend only on 0router's own adapter kit: no other crates, build scripts or procedural macros. The validation gate refuses opaque blobs and code over a size limit. The core runs only compiled adapters whose source matches the reviewed source. Adapters run in a sandbox with no network, no file access and no secrets; the core does all sending. An operator who never installs a third-party adapter runs the core alone.

Installing or updating a third-party adapter puts it in a security review queue. A review agent reads a copy with comments stripped and identifiers renamed. It has no access to user data, can't run commands, and only reports how safe the adapter seems; the operator decides. The review agent runs on a model, provider and budget the operator picks. With no model set, or not enough budget, the adapter stays in quarantine. Updates are never automatic, and while an update is queued or quarantined, the previous version keeps serving.

An adapter may change request content only where its harness's coupling requires it, by removing or converting parts the target provider can't accept. It never compresses, summarizes or optimizes, and every content change it makes is noted in the request record. A rule-based guardrail runs on every request; AI is used only at review time, never per request. When an adapter adds or changes a tool call, 0router drops that adapter's changes, sends the unmodified request, marks the adapter as suspect, alerts the operator, and records what the adapter tried.

0router writes the Claude Code adapter as a third-party plugin, and it goes through the full install and review pipeline, proving it on a real closed-source harness. For harnesses that are coupled to their own server or closed-source, adapters follow 9router's fixes.

The slice fails if: a built-in harness needs a middleman or setup beyond a base URL and key, or one of its features breaks; a third-party adapter reaches the network, a file or a secret, or runs code other than the reviewed source; starting an update, a review or a quarantine breaks a working setup; an adapter adds or changes a tool call without the guardrail stopping it.

Constraints: Constitution Principles I and IV as amended before this slice (harness adapters may be sandboxed code; an adapter's content changes are limited to what its coupling requires); provider plugins stay data only, and a provider that needs code becomes a core built-in; secrets are injected by the core only; streams are relayed without buffering, and a client disconnect cancels the upstream request; the adapter hot path carries Criterion benchmarks.

Oracle: ref/9router normalizeClaudePassthrough (open-sse/translator/formats/claude.js), open-sse/translator/concerns/paramSupport.js, and open-sse/services/combo.js.

Out of scope: built-in adapters for opencode, grok build and zcode → later slices (they connect as plain clients through slice 003 until then); account sign-in (OAuth), xai and grok-cli → slice 005; the routing decision and persistent request history → slice 006; code in provider plugins → never (built-ins instead).

Scope brief: specs/briefs/2026-09-28-client-side-adapters.md
```

## Trace

| Command sentence (paraphrased) | Ledger rows |
|---|---|
| Client-side plugins; base URL and key; no middleman | M two-sided plugins, C✓ outcome, C✓ failure |
| Adapter bound to agent key; no guessing | C✓ selection |
| Kit reads and edits request and response | C✓ deps |
| hermes built in, full parity, echoed reasoning, attachments | C✓ built-ins, C✓ hermes |
| Rust source → WASM in a separate builder | C✓ language, C✓ builder, M source only |
| Kit only; no crates, build scripts or proc macros | C✓ deps |
| Gate refuses blobs and oversized code; runs only reviewed source | M 2026-09-27 |
| Sandbox; core does all sending | M 2026-09-27 |
| Core alone without third-party adapters | C✓ builder |
| Review queue, scrambled copy, reports only, operator decides | M review agent |
| Operator's model, provider, budget; quarantine | M review agent |
| No automatic updates; previous version keeps serving | M no auto-update, C✓ failure |
| Content changes only for the coupling, recorded | C✓ content |
| Rule-based guardrail; AI only at review | M guardrail |
| Block, send original, suspect, alert, record | C✓ guardrail |
| Claude Code proof adapter, full pipeline | C✓ Claude Code |
| Follow 9router's fixes for coupled or closed harnesses | M free ride |
| Failure signals | C✓ failure |
| Constraints | C✓ amendment, C✓ content, C✓ providers, K I, K V, K Architecture |
| Out of scope | C✓ others, C✓ order, C✓ providers |

## Clarify additions (2026-09-28)

Confirmed during `/speckit-specify` and `/speckit-clarify`, after the command above was run. None
contradicts a row above. The catalogue rows **add scope** the playback didn't describe: the user
picked the catalogue over Claude's recommended "local source only".

| Tag | Claim | Source | Spec |
|---|---|---|---|
| C✓ | Guardrail also stops added or changed tool definitions and tool results; removals allowed and recorded | Q1: A (specify) | FR-016, US3-5 |
| C✓ | A suspect adapter stops serving every key until cleared; bound keys work as plain clients | Q2: A (specify) | FR-017, US3-6 |
| C✓ | Guardrail checks responses too, streamed or not; events already sent aren't recalled | clarify: A | FR-016, US3-4 |
| U | Install from operator-supplied source **and** from a 0router-hosted catalogue, both in this slice; same gate, build, review and decision | clarify: C | FR-031, US2-8, SC-012 |
| C✓ | The catalogue is contacted only on operator action (browse, install, check for updates); never in the background | clarify: A | FR-031, FR-029, SC-012 |
| C✓ | A kit-changing 0router upgrade rebuilds the same reviewed source automatically, with no new review; a failed rebuild → plain client + alert | clarify: A | FR-032, SC-013 |
| C✓ | The catalogue is an index file in a 0router-owned public repository; authors listed by pull request; no hosted service; this slice builds the format, first entries (Claude Code) and install side | clarify: A | FR-031, US5 |

Note for research.md (P): each catalogue entry pins every version to a source fingerprint, and a
fetched source that doesn't match is refused before the gate (spec Assumptions, technical
decision).
