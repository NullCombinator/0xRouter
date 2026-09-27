# Scope brief: request pipeline (slice 003)

Shaped with `/shape-spec` on 2026-09-27, replacing the drifted draft archived in
`specs/briefs/superseded/003-request-execution/`. In `/speckit-clarify` and
`/speckit-plan`, an answer that contradicts a confirmed row below means stop and revisit
this brief. Don't accept it.

## Core map (at `dfbc3e3`)

| Capability | Status | Evidence |
|---|---|---|
| Provider entities from TOML plugins, validation gate, bundled set | shipped | slice 002, `crates/zerorouter-registry`, parity tests |
| Unified models (declare, look up, map to upstream IDs) | shipped | slice 002, `zerorouter resolve` / `model` |
| Plugin safety (data only, secrets in the core credential table) | shipped (load time) | 002 validation corpus, `credentials/` |
| Non-text model types | partial | typed in the registry; nothing executes them |
| Request execution (server, accounts, streaming) | absent | → this slice |
| Retry and fallback, error classification | absent | → this slice |
| Latency observability | absent | → this slice (in memory); persistence → 005 |
| Agent identity | absent | → this slice; per-agent isolation → 005 |
| Format translation (client API styles as data) | absent | → this slice |
| Account sign-in (OAuth) | absent | → 004 (chosen few); general → later |
| Cache-aware routing, windowed amortization | absent | → 005 |
| Combos, model tests, dashboard | absent | → later |
| Plugin-declared forwarding | absent | → this slice |

## Slice plan

- **003 (this):** request pipeline with API-key providers.
- **004:** account sign-in (OAuth) for the chosen few, plus xai and grok-cli.
- **005:** routing decision (cache-aware, per-agent isolation, amortization), plus
  persistent request history.
- **Later:** combos, model tests, dashboard, and general plugin-declared OAuth.

## Playback (confirmed)

When this slice is done, any standard client can talk to 0router in any of four API
styles: Chat Completions, Messages, Responses, or Gemini. Examples are Claude Code, Codex
CLI, Gemini CLI, an OpenAI SDK, or an optimizer in front. Each client uses its own
0router key. It can reach text, embedding, image, speech, transcription and video models
on anthropic, openrouter, opencode (Zen and Go) and ElevenLabs, using the operator's API
keys. When an account fails, 0router first retries where the agent's cache is warm, then
moves on quietly. If a stream breaks midway, it continues from the partial answer where
the provider allows it. Otherwise it does what the operator chose: an informative error
or a restart. When everything fails, the client is told what was tried and why. Every
request records latency and usage, including cache tokens, readable from the CLI. The
API styles and provider details are data files. Providers outside the chosen five become
installable community plugins.

## Ledger

| Tag | Claim | Source |
|---|---|---|
| K | Secrets held by the core, injected only at execution | Constitution I |
| K | Stream without buffering; client disconnect cancels upstream | Constitution V |
| K | TTFT and total duration per provider and unified model | Constitution VIII |
| K | Forward as received; token optimization out of scope | Constitution IV |
| K | Translation, error classification, retry semantics follow 9router | Constitution VI |
| K | Criterion benchmark on execution and streaming hot paths | Constitution, Architecture Constraints |
| K | Plugins declare endpoints, auth, translations; the core acts | init.md, Constitution I |
| M | Retry and fallback required in the core now | agentmemory 2026-09-27 |
| M | Native pair is a bonus; forwarding is plugin-declared under a security floor | agentmemory 2026-09-27 |
| M | The core is a middleware capturing requests and responses | agentmemory 2026-09-27 |
| U | Not for Claude Code only | "Note that this is not an application for only claude code harness!" |
| U | Not text-first | "we are not going to repeat 9router mistake make 0router a text-model-first application" |
| U | Web search is outside 0router's scope | "Web search which is out of 0router scopes" |
| U | A chosen few built in, the rest community plugins | "we are not going to handle all providers plugins built-in, we must choose a few to handle" |
| U | Chosen providers | "anthropic, openrouter, opencode, grok, elevenlab" |
| U | grok must be multi-type | "it must accept other types of model rather than text too!" |
| U | Errors tell the client what happened | "Informational error to aware client about what happened, helped them manage 0router better" |
| U | Generalize in the core, specifics in plugins | "I want it to be more generalize in core, to handle these things, and shift exact specifications into plugins" |
| U | Error placement is a plugin concern | "While emphesizing on that these are plugins concern" |
| U | Avoiding token waste is a core duty | "The 0router responsibility is to avoid wasting tokens as much as possible… one of the major 0router existence reason and duties!" |
| C✓ | O1: any standard client, API-key accounts, streamed answers, failures absorbed, per-request latency and usage readable | Q: outcome |
| C✓ | F1–F4: client breaks; hiccup reaches client; numbers wrong or missing; secret leaks | Q: failure |
| C✓ | Four client API styles: Chat Completions, Messages, Responses, Gemini generateContent | Q: client APIs |
| C✓ | Format translation in this slice | Q: translation |
| C✓ | Model types in this slice: text, embeddings, image, TTS, STT, video | Q: model types |
| C✓ | Few built in, rest community (clarified) | Q: few providers |
| C✓ | 003 providers: anthropic, openrouter, opencode-zen, opencode-go, elevenlabs (+ STT section) | U5 + managed decision, confirmed in playback |
| C✓ | grok = xai sign-in + grok-cli, both → 004 | Q: grok, Q: size |
| C✓ | API styles are data, like providers | Q: API styles |
| C✓ | Declared forwarding in this slice | Q: forwarding |
| C✓ | Access key per agent, always required; agent = key + client session id | Q: access key |
| C✓ | Stay warm first: retry the same account on transient errors; the next request returns to the warm account | Premortem 2 |
| C✓ | Seamless mid-stream continuation where declared; otherwise the operator's choice, overridable per agent key | U (token waste) + Q: who chooses |
| C✓ | Error info: message field + structured extra field + record-id header | Premortem 3 |
| C✓ | Generic token counting; plugins declare support | U (generalize) |
| C✓ | Stable, reused upstream connections in this slice | U (token waste), assigned by Claude under "manage it yourself" |
| C✓ | Model listing per API style, unified and direct, every type | Q: model list |
| C✓ | Non-chosen providers move to an installable community set, fit-or-refuse | Q: other plugins |
| C✓ | Routing decision → 005 | Q: routing (renumbered at size) |
| C✓ | Persistent request history → 005 | Q: history (renumbered at size) |
| C✓ | Combos → later (fallback stays inside one unified model) | Q: combos |
| C✓ | Model tests → later | Q: model tests |
| C✓ | Dashboard → later; CLI only | Q: dashboard |
| C✓ | General plugin-declared OAuth → later | Q: OAuth |
| C✓ | Sign-in split off as 004 | Q: size |
| ✗ | "No retry and no fallback" (old draft) | rejected by M |

## Notes for research.md (P)

- URL and auth-header construction for the chosen providers follows 9router's executors.
- Error classification follows 9router's request path: `open-sse/services/accountFallback.js`
  `checkFallbackError` (an unmatched 4xx does not fall back), with
  `tests/unit/account-fallback-4xx.test.js` as the oracle. Judge parity on chatCore's call
  path.
- 9router's stream stall timeout is 360 s (`STREAM_STALL_TIMEOUT_MS`).
- Usage parsing must read nested `cached_tokens` for Responses-format providers. 9router's
  CHANGELOG records a bug where a top-level-only read billed cache hits at the full rate.
- When a provider declares no token counting, 9router answers count_tokens with a local
  estimate. Reuse that estimator as the generic fallback.
- Continuation support differs by provider. Anthropic assistant prefill has limits with
  extended thinking, and openrouter support depends on the model. Declare it per plugin;
  don't assume it.
- Informational errors must stay valid within each API style's error schema, so strict
  SDKs still parse them.
- The chosen-few plugins become 0router-owned: seeded by `tools/gen-bundled`, then
  maintained by hand. The community set stays generated. Slice 002's parity tests over all
  121 providers are re-pointed at the community set. This supersedes 002's "every 9router
  provider bundled" promise, by user decision.
- ElevenLabs STT (`/v1/speech-to-text`) is not in 9router's data. 0router authors it.
- For 004: grok-cli's live `/models` list (`open-sse/services/grokCliModels.js`) should be
  checked for non-text models. xai's OAuth (`auth.x.ai`) declares image
  (`/v1/images/generations`) and video (`/v1/videos`).

## Final command

```
/speckit-specify Request pipeline: the first slice a client can use end to end. Any standard client (Claude Code, Codex CLI, Gemini CLI, OpenAI or Anthropic SDKs, an optimizer hop in front of 0router) sends requests in one of four client API styles: OpenAI Chat Completions, Anthropic Messages, OpenAI Responses, Gemini generateContent. 0router serves them from the operator's API-key accounts on the chosen providers: anthropic, openrouter, opencode-zen, opencode-go and elevenlabs. Any client style reaches any provider, because the core translates between styles. Model types are first-class, not text-first: text, embeddings, image, text-to-speech, speech-to-text and video all run through the same pipeline, with the same retry, fallback, records and unified models. Each client can list, in its own API style, the models it can use: unified and direct, of every type. Token counting is a generic core capability; each provider declares whether and how it counts.

The core is generic and the specifics are data. The four client API styles are data files the core interprets, as provider plugins are. Each provider's specifics are declared in its plugin: endpoints, headers, error placement, token-counting support, support for continuing from a partial answer, which client headers go upstream, and which provider headers or bodies come back. The core applies forwarding declarations under a security floor: credentials are never forwarded.

A failure doesn't reach the client when something else can serve. On a transient failure, the same account is retried first, because it holds the agent's warm cache. Then the request moves to another account, or to another member provider of the same unified model. An agent's next request goes back to where its cache is warm. If a stream breaks after the client has received output, 0router continues the same stream from the partial answer on another account or provider, wherever the provider declares support. Otherwise it does what the operator chose, an informational error event or a restart from the beginning: a default that can be overridden per agent key. Avoiding wasted tokens is a core duty: upstream connections are kept stable and reused. When every option fails, the client gets an informational error in its own API style: a readable summary of what was tried and why each attempt failed in the standard message field, the same details in a structured extra field, and a request-record id in a response header and in the message.

Every client request carries a 0router access key, one key per agent. The agent identity is the key plus the client's own session id when the client sends one. Every request is recorded with the agent, unified model, provider and account it used, what was tried and why each attempt failed, time to first token, total duration, and token usage including cache-read and cache-write tokens. Records are kept in memory and queried from the CLI per provider, per unified model, and by record id. The operator manages accounts and agent keys through the CLI.

Providers outside the chosen five move out of the bundle into an installable community plugin set. Each one runs if it fits what the core handles, and is otherwise refused with a clear "not supported by this core" message. The elevenlabs plugin gains a speech-to-text section.

The slice fails if: a standard client breaks on 0router; a single 429, 5xx or timeout reaches the client when another account or provider could serve; latency or usage numbers (cache tokens included) are missing or don't match the provider; a secret appears in a log, record, error, or anything a plugin sees. Tests use more than one client harness.

Constraints: plugins are data, never code, and never hold secrets; the core injects secrets only at execution; streams are relayed without buffering, and a client disconnect cancels the upstream request; requests are forwarded as received, and prompt content is never rewritten; translation, error classification and retry semantics follow 9router's behaviour; execution and streaming hot paths carry Criterion benchmarks.

Out of scope: account sign-in (OAuth) and the xai and grok-cli providers → slice 004; the routing decision (cache-aware routing, per-agent isolation, windowed amortization) and persistent request history → slice 005; combos, model tests, the dashboard, and plugin-declared OAuth for any provider → later; web search → never (outside 0router's scope); token optimization → never (an upstream hop).

Scope brief: specs/briefs/2026-09-27-request-pipeline.md
```

## Trace

| Command sentence (paraphrased) | Ledger rows |
|---|---|
| Any standard client, examples, four API styles | U (not Claude-only), C✓ client APIs, K init.md (optimizer hop) |
| API-key accounts on the chosen five | C✓ O1, C✓ 003 providers, U (few built in), C✓ size |
| Any style reaches any provider | C✓ translation |
| Six model types, same pipeline | U (not text-first), C✓ model types |
| Model listing per style | C✓ model list |
| Generic token counting | U (generalize), C✓ token counting |
| API styles and provider specifics are data | C✓ API styles, U (generalize), U (plugin concern), K init.md |
| Forwarding under a security floor | C✓ forwarding, M (native pair) |
| Retry same account first, then another account or member | M (retry/fallback), C✓ stay warm, C✓ combos limit |
| Next request returns to the warm cache | C✓ stay warm |
| Mid-stream continuation, else operator choice with per-agent override | U (token waste), C✓ continuation |
| Stable, reused connections | U (token waste), C✓ stable connections |
| Informational error: message, extra field, record id | U (informational), C✓ error info |
| Access key per agent; agent = key + session | C✓ access key |
| Record fields | K VIII, C✓ access key, C✓ error info, C✓ O1, C✓ F3 |
| In memory, CLI queries, CLI account and key management | C✓ history → 005, C✓ dashboard → later, C✓ error info |
| Community set, fit-or-refuse; ElevenLabs STT | C✓ community set, U (few built in), C✓ 003 providers |
| Failure signals; more than one harness | C✓ F1–F4, U (not Claude-only) |
| Constraints | K I, K V, K IV, K VI, K Architecture |
| Out of scope | C✓ size, C✓ routing/history, C✓ limits, U (web search), K IV |
