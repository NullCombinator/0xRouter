# Research: Request Pipeline (slice 003)

Phase 0 output of `/speckit-plan`. Every item is a technical decision Claude made, per the
user's direction (spec Clarifications, 2026-09-27). Items that change something the user can
see are marked **(user-visible)** and are reported to the user with the plan.

Sources:
- `ref/9router` at pin `39e36d3`, judged on chatCore's request path;
- the scope brief `specs/briefs/2026-09-27-request-pipeline.md` (ledger and P notes);
- vendor documentation fetched on 2026-09-27 (links inline);
- the superseded draft `specs/briefs/superseded/003-request-execution/research.md`. Only
  facts re-verified here are reused. Its native-pair framing and its "no retry, no
  translation" deviations are dead.

No NEEDS CLARIFICATION remains.

---

## R1. Toolchain and crates

**Decision**: Rust 1.93.1 (edition 2024, MSRV stays 1.85). New dependencies:

| Crate | Use |
|---|---|
| `tokio` 1 (rt-multi-thread, net, time, signal, fs, macros) | runtime |
| `tokio-util` (rt) | `CancellationToken` |
| `axum` 0.8 | client HTTP surface, `Sse` |
| `hyper` / `hyper-util` (via reqwest and axum) | — |
| `reqwest` 0.13 (rustls, stream, http2, multipart, no default features) | upstream client |
| `futures-util`, `bytes` | stream adaptors |
| `memchr` | SSE and NDJSON framing |
| `serde_json` (already, `preserve_order`, plus `raw_value`) | IR, templates, passthrough |
| `sha2`, `base64`, `getrandom` | agent-key digests, key generation |
| `aho-corasick` | secret redaction and forwarding value checks |
| `tracing`, `tracing-subscriber` | logs (redacted) |
| dev: `wiremock`-style in-process axum mocks (no extra crate), `criterion` 0.8 | tests, benches |

**Rationale**: all are mainstream, rustls avoids OpenSSL, and none needs `unsafe` in our
code (`unsafe_code = forbid` stays). crates.io is reachable from this identity; `.cargo-home`
has 135 crates cached, and the new ones are fetched once.

**Alternatives**: `hyper` directly instead of `reqwest` (more code for pooling, redirects
and multipart with no gain); `eventsource-stream` (hides raw bytes we need for native
passthrough); `ring` for SHA-256 (heavier than `sha2`).

## R2. Crate layout

**Decision**: three new crates beside the two from slice 002.

| Crate | Role | I/O |
|---|---|---|
| `zerorouter-registry` (extended) | plugin schema 2, API-style files, gate additions, fit check, security floor, community set | files only |
| `zerorouter-wire` (new) | style interpreter: intermediate representation (IR), request and response codecs, stream readers and writers, usage, token estimator, error bodies | none (pure) |
| `zerorouter-engine` (new) | accounts, agent keys, classification, attempt loop, stay-warm, break handling, upstream client, records | network |
| `zerorouter-server` (new) | axum router built from style routes, access-key check, relay, operator socket | network |
| `zerorouter-cli` (extended) | `serve`, `accounts`, `keys`, `records`, `plugins`, `behaviour` | — |

**Rationale**: `zerorouter-wire` has no I/O, so translation is unit-tested and benchmarked
in isolation, and a parity audit targets one crate. The engine has no axum types, so it is
testable without HTTP. The server is thin.

**Alternatives**: one big `zerorouter-core` (slower builds, blurred audit targets); putting
styles inside the registry crate (would pull runtime codecs into the load path).

## R3. Client API styles as data

**Decision**: adopt the plugin-system-designer's design, recorded in
[contracts/api-style-schema.md](contracts/api-style-schema.md). In short:
- four TOML files in `styles/bundled/`: `openai-chat`, `anthropic-messages`,
  `openai-responses`, `gemini`;
- they pass the same gate as plugins (unknown keys rejected, no secrets, no forwarding);
- each file is both a client front door (routes, key carriers, session carriers) and an
  upstream wire that provider endpoints name;
- the mapping language is bounded: field paths, enum maps, typed templates (no expressions),
  match rules, and layout choices from closed sets;
- every stateful algorithm is a named core primitive chosen from a closed set (stream
  framing, block model, tool-argument assembly, counters, accumulation, media codecs,
  thinking forms, async jobs, named cross-style repairs). This is the slice-002 rule for
  quirks, hooks and formats.

The core translates through one IR (`zerorouter-wire::ir`): a request IR (messages, parts,
tools, params), a stream event IR (block start/delta/stop, tool-call start/argument
fragment/stop, usage, finish, error, keepalive) and a response IR. Each style file drives
one decoder and one encoder for each direction.

**Rationale**: FR-008 and the ledger ("API styles are data, like providers"). 9router's
translators show that mappings, finish-reason tables, usage maps and endpoint choice are
data, while the Claude and Responses stream writers need ordered open/close rules, index
counters and deferred completion. Those become named primitives, so a style file cannot
express general computation (Constitution I).

**Alternatives**: hand-written Rust per pair (9router's hub, 13 formats × directions),
rejected by the ledger; a general expression language in style files (Turing-complete data
is code, rejected by Constitution I).

A style that needs an algorithm outside the closed set is "not supported by this core"
until the set grows, the same fit rule as plugins (R19).

## R4. Translation behaviour: parity and deliberate deviations

**Decision**: follow 9router's pair behaviour (Constitution VI) for shapes, finish reasons,
usage mapping, tool-call id repair and role merging. Deviate where 9router rewrites prompt
content or loses it, because Constitution IV and FR-007 forbid that and they are
non-negotiable for this project. Each deviation has a parity test that asserts the
0router behaviour.

| 9router behaviour | 0router | Why |
|---|---|---|
| openai→claude prepends the Claude Code system prompt | not done | Content injection (IV). It exists for OAuth cloaking, which is out of scope |
| `response_format` injected as system text | mapped to the provider's native structured-output field where the plugin declares one; otherwise the target doesn't fit this request (R7 "skip") | Content injection (IV) |
| opencode executors add bash/glob/grep/read fingerprint tools and a spoofed User-Agent | not done | Content injection (IV); these gate free tiers, and the operator uses API keys. A live check task confirms API-key requests succeed without them |
| cache_control markers added when translating into Messages (1 h on system and last tool, ephemeral on last assistant) | kept | Not prompt content. It prevents token waste, a core duty |
| claude→openai drops URL images and `is_error` | carried (OpenAI accepts image URLs; `is_error` becomes a readable prefix only where the target has no field, which is a shape change of a flag, not of text) | Truncation (IV) |
| Prior-turn signed thinking dropped when crossing vendors | same (parity) | A foreign signature is rejected by the target; this is a wire constraint |
| gemini client route is text-only and drops tool calls | full translation, tools included | 9router's gap would break Gemini CLI (F1) |
| non-stream Claude provider → Responses or Gemini client gets an OpenAI body | correct second hop through the IR | 9router bug |
| a content part the target cannot carry | the target doesn't fit this request; recorded as a skipped attempt | Never truncate (IV) |

**Rationale**: the brief records both "translation follows 9router" (K, VI) and "forward as
received" (K, IV). Where they conflict, IV wins because it is the stricter invariant and the
user restated it in the slice description. Listed in plan Complexity Tracking.

## R5. Streaming relay and Constitution V

**Decision**:
- Every upstream stream is framed incrementally (`memchr` SSE / NDJSON / JSON-array
  framer) into events. No response is held back beyond one event.
- **SSE to the client** uses `axum::response::sse::Sse<impl Stream>` with `StreamExt`
  adaptors, as Constitution V requires. In a native pair (client style = provider wire),
  each upstream event's `event` name and `data` payload are re-emitted unchanged; only line
  framing is normalised. Translated pairs emit encoder output. A non-stream native body is
  returned as received too (R27).
- **Non-SSE streamed bodies** (binary TTS audio, NDJSON) use `Body::from_stream`.
  Constitution V governs SSE; these are not SSE.
- A `CancelOnDrop` guard owns a `CancellationToken`. Dropping the client body cancels the
  upstream request and any pending retry, fallback or backoff sleep (`tokio::select!`).
  SC-010 (≤ 1 s) is measured in tests.
- **Preamble hold**: header events (Messages `message_start`, Responses
  `response.created`/`in_progress`, the Chat role-only chunk) are held until the first
  content event or the end of the attempt. A retry before any content then replaces them
  cleanly (US4-4). This is milliseconds and does not move TTFT, which counts from the first
  content event.
- **Keepalive**: while 0router is between attempts or backing off, the stream sends the
  style's declared keepalive (Messages `event: ping`, SSE comments for the others). Claude
  Code aborts on 300 s of silence (https://code.claude.com/docs/en/llm-gateway-protocol).

**Rationale**: the superseded draft chose `Body::from_stream` for SSE to keep byte
fidelity. Translation now rewrites events, and native pairs keep every semantic field under
`Sse`, so there is no reason to deviate from the Constitution's named pattern.

**Alternatives**: raw byte passthrough for native pairs (breaks preamble hold and index
shifting after a restart); buffering until done (forbidden).

## R6. Error classification

**Decision**: port `checkFallbackError` (`open-sse/services/accountFallback.js:23-64`) and
`ERROR_RULES` (`open-sse/config/errorConfig.js:59-76`) exactly:
- text rules first, lowercase substring, first match wins:
  `no credentials` → 2 min; `request not allowed` → 5 s; `improperly formed request` →
  2 min; `rate limit`, `too many requests`, `quota exceeded`, `capacity`, `overloaded` →
  backoff;
- then status: 401, 402, 403, 404 → 2 min; 429 → backoff;
- any other 4xx → no fallback, returned to the client (with the informational fields);
- everything else (5xx, 529, network, timeout) → fallback, 30 s transient cooldown;
- backoff = 2000 × 2^(level−1) ms, capped at 300 000, `maxLevel` 15, level per account.

The text matched is `"[<status>]: <raw upstream body>"`, as on 9router's request path
(BaseExecutor.parseError returns the raw body, so JSON keys match too:
`overloaded_error` matches, `rate_limit_error` doesn't). Oracle:
`tests/unit/account-fallback-4xx.test.js` (4 cases), ported as a fixture.

**In-band errors** (FR-024): an endpoint's `[errors]` rules declare where errors sit (a
body field in an HTTP 200, a stream error event). A matched in-band error is classified
by the same function with the declared status. 9router detects none of these for the
chosen providers; 0router's detection is additive and declared per plugin.

**Rationale**: Constitution VI names error classification as inherited behaviour.

## R7. Retry order and budgets

**Decision**: one attempt loop per request.

1. **Same account first** (0router addition, ledger "stay warm first"). Budgets:

   | Failure | Same-account retries | Delay |
   |---|---|---|
   | 502, network error, connect timeout | 3 | 3 s (9router) |
   | 503 | 3 | 2 s (9router) |
   | 504 | 2 | 3 s (9router) |
   | 429 or rate-limit / overloaded text | 1 | the provider's indicated wait (`retry-after`, reset headers) if ≤ 5 s, else 2 s; if the indicated wait is > 5 s, no retry |
   | other 5xx, 529 | 1 | 2 s |
   | 401–404, `no credentials`, `improperly formed request` | 0 | — |

   A plugin can override the table per status (9router's `transport.retry`).
2. **Other accounts of the same provider**, in operator order, skipping accounts in
   cooldown for this model. The failed account gets 9router's cooldown (R6).
3. **Other member providers of the unified model**, in declared order, each with its
   accounts. A member that isn't installed, has no account, or can't carry the request
   (R4) is recorded as a skipped attempt with its reason.
4. Direct `<provider>/<model>` requests stop after step 2.
5. When everything fails: the informational error (R11).

Cooldowns are held in memory per (account, model). 401/403 on an API-key account does not
run 9router's 3-second refresh loop (nothing to refresh).

**Rationale**: FR-015, US2, edge case "429 with a long wait". 9router's delays are reused
wherever 9router has one (VI); the 429 and other-5xx rows are 0router's additions, kept to
one retry so a truly failing account costs at most ~2 s before moving on.

**Alternatives**: no same-account retry for 429 (9router): contradicts the user's
"retry the same account first"; waiting any length for `retry-after`: stalls the client.

## R8. Stay-warm bookkeeping

**Decision**: an in-memory map `(agent, target) → (provider, account)` updated when an
attempt ends successfully (stream finished, not stream started). The next request for the
same agent and target starts at that account unless it is in cooldown; otherwise operator
order. `target` is the unified model name or the direct `<provider>/<model>`. Concurrent
requests: last success wins. Not persisted (lost on restart, rebuilt by traffic).

**Rationale**: FR-016, SC-003. 9router clears the lock when a stream starts; 0router marks
success at the end, so a stream that breaks doesn't count as warm.

## R9. Mid-stream breaks: continuation, restart, error event

**Decision**:
- "The client has received output" means at least one content event (text, thinking or
  tool-call) was sent. Before that, a break is an ordinary transient failure (R7).
- After output, the next target in R7 order is checked for continuation support:
  - the endpoint declares `[continuation]` (`assistant_prefill` or `prefix_flag`);
  - the model is in its `models` list (or not in `except_models`);
  - none of its `unless` conditions holds (`thinking_enabled`, `tool_call_in_progress`);
  - the partial answer can be expressed in the target's wire (R4).
  If so, 0router sends the original request plus the partial answer as the trailing
  assistant turn (trailing whitespace trimmed where declared). The continuation's preamble
  is suppressed, its first text block is merged into the open client block, and its usage
  is added to the first segment's. The record lists both segments.
- Otherwise the operator's choice applies (agent-key override, else the default; shipped
  default **restart**):
  - **restart**: the open text or thinking block is closed. A new text block carries the
    note `— connection lost, answer restarted —` (with blank lines around it in styles
    without blocks). The original request is sent to the next target, and its events
    follow with block indexes, output indexes and sequence numbers shifted past what the
    client has seen. Claude Code stops reading a stream that reuses an index, so shifting
    is mandatory.
  - **error event**: the style's stream error event with the informational fields (R11),
    then a clean end.
- **(user-visible) Half-sent tool call exception**: if the break happens while a tool
  call's arguments are partly sent, no style can take them back, and a restart would leave
  the client holding a broken call it might execute. In that case 0router sends the error
  event even when the choice is restart. The record says why.
- **(user-visible) Continuation availability**: Anthropic returns HTTP 400 for assistant
  prefill on Claude 4.6 and later
  (https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices#migrating-away-from-prefilled-responses).
  Anthropic's suggested alternative adds a "continue" user message, which rewrites the
  prompt (IV), so 0router doesn't use it. The chosen five declare continuation only where a
  live check confirms it: anthropic for its catalogued pre-4.6 models when thinking is off;
  openrouter and opencode per model family after the live check. Elsewhere, breaks restart.
- **Thinking**: a thinking block cut before its signature is closed as is. A harness test
  checks that Claude Code's next turn still succeeds; if not, a cut thinking block joins
  the tool-call exception.

**Rationale**: FR-017, FR-018, US4, SC-008, and the Claude Code gateway protocol
(content_block_start must precede deltas; no index reuse; a stream ending before
`message_delta` is treated as dropped and retried by Claude Code itself).

## R10. Timeouts

**Decision**:
- `FETCH_CONNECT_TIMEOUT_MS` = 60 000: time until response headers (9router). A timeout is
  retried as a 502 (9router `base.js:173`).
- `STREAM_STALL_TIMEOUT_MS` = 360 000: no upstream byte for this long is a break (R9).
  0router arms it on every streamed and chunked body, including forced-stream bodies
  collected for a non-streaming client (9router has no watchdog there: deviation).
- Both are read with 9router's `envMs` rule (integer > 0, else default) and can be set per
  endpoint (`timeout_ms`, `stall_timeout_ms`).
- Async video jobs: each poll is one bounded request (60 s); there is no total job timeout
  in 0router.

**Correction to the superseded draft**: its R7 claimed the stall watchdog on forced-stream
assembly was 9router behaviour; it isn't.

## R11. Informational errors

**Decision**: when 0router ends a request with an error (all attempts failed, non-fallback
upstream error, validation refusal), the body is the client style's own error shape from
its `[errors]` template:
- the standard message field holds: a one-line summary, the record id, then one line per
  attempt (`provider/account model: reason`);
- a structured extra field `zerorouter` holds `{ record_id, attempts: [{provider, account,
  model, status, reason, retries}] }`. Account names are shown, never keys;
- header `x-0router-request-id: <record id>` on every response, success or error;
- status: a non-fallback upstream 4xx keeps its status; all-attempts-failed returns 503
  with `retry-after` = seconds until the earliest cooldown ends (9router `unavailableResponse`);
- type/code: from the style's status-to-type map (Messages 503 → `api_error`,
  529 → `overloaded_error`; OpenAI → `server_error`; Gemini `status` = `UNAVAILABLE`).

SDK tolerance: the Anthropic Python SDK chooses the exception class by status and reads
`error.type` and `error.message` defensively; extra fields are kept on `err.body`
(verified). OpenAI and Google SDKs are verified by the SC-009 test matrix, not assumed.

**Deviation**: 9router always returns an OpenAI-shaped body with a raw upstream message
(`error.js:9-22`). FR-022/FR-023 require the client's own shape.

Claude Code matches some recovery paths on upstream error text. A non-fallback upstream
error keeps the upstream message verbatim at the start of the message field, so that
matching still works.

## R12. Access keys and agent identity

**Decision**:
- Agent keys: `0r-` + 43 characters of base64url (32 random bytes). Shown once when
  issued. Stored as SHA-256 digests in `keys.toml`; lookup is a digest map.
- Carriers, per style file: Messages `x-api-key` then `authorization: Bearer`; OpenAI
  `authorization: Bearer`; Gemini `x-goog-api-key`, then `?key=`, then Bearer (9router
  order). One key works at every style (FR-002).
- The key is checked before the body is read or parsed; a bad key is rejected in the
  style's error shape with 401. **Deviation**: 9router parses JSON first (invalid JSON →
  400 before auth). Rejecting unauthenticated callers first spends nothing on them.
- Session id carriers follow 9router's `sessionManager.js` order (verified correct), minus
  the Antigravity carrier (out of scope): Claude Code `metadata.user_id` `_session_` form,
  `x-claude-code-session-id`, Codex `session_id` header and `prompt_cache_key`, Gemini
  CLI's `x-gemini-session`-style headers as declared in the style file. Values are capped
  at 256 characters (9router `normalizeSessionId`).
- Agent = key id + session id (or key id alone).

## R13. Usage and records

**Decision**:
- Usage is read from every place a style reports it, through the style's `[usage]` map:
  Messages `message_start` and `message_delta` (`input_tokens`, `output_tokens`,
  `cache_read_input_tokens`, `cache_creation_input_tokens`); Chat `usage.prompt_tokens`,
  `prompt_tokens_details.cached_tokens` (or `prompt_cache_hit_tokens`); Responses
  `response.completed` `input_tokens_details.cached_tokens` (nested, CHANGELOG v0.5.59),
  `output_tokens_details.reasoning_tokens`; Gemini `usageMetadata` including
  `cachedContentTokenCount` and `thoughtsTokenCount`. OpenRouter adds
  `prompt_tokens_details.cache_write_tokens`.
- Merge: per field, last reported value wins (9router `mergeUsage` takes the max; equal on
  well-formed streams, but "last" is right when a provider corrects a count).
- Each record keeps usage as reported plus its `input_semantics` (`includes_cache` or
  `excludes_cache`), so cache-read and cache-write are never double counted.
- Translation to the client style converts semantics (Messages→Chat: prompt =
  input + cache_read + cache_creation, 9router `concerns/usage.js`).
- **Deviation**: usage the provider didn't report is "not reported" in the record and
  absent from the client response. 9router estimates it and adds a 2000-token buffer to
  real usage in client-visible chunks (`open-sse/utils/stream.js:96-97,211-221`). SC-004
  requires exact provider numbers.
- For Chat upstreams, 0router sets `stream_options.include_usage = true` when streaming
  (a request parameter, not prompt content) and strips the extra usage chunk if the
  client didn't ask for it.

**Corrections to the superseded draft (R11 there)**: Anthropic non-stream usage comes from
`extractUsageFromResponse`; `canonicalizeUsage` folds cache into prompt tokens for storage
too; Responses and Gemini rows were missing.

## R14. Token counting

**Decision**: count requests exist on three styles: Messages `POST
/v1/messages/count_tokens`, Responses `POST /v1/responses/input_tokens`, Gemini
`:countTokens`. Chat Completions has none (0router invents none). The target is resolved
as for generation. If the provider's text endpoint declares `[token_count]`, the request
is translated to that wire and sent (anthropic declares it). Otherwise 0router answers with
9router's `estimateAnthropicInputTokens` (ceil(chars/4) over system, tools, and message
parts) applied to the request translated into the Messages shape; for Messages clients this
is exact parity (`tests/unit/count-tokens.test.js`, 3 cases). The record marks it
`estimated`. Count requests get the same retry order as generation.

## R15. Model listing

**Decision**: `GET /v1/models` (OpenAI shape, and Anthropic shape when `anthropic-version`
is present), `GET /v1beta/models` (Gemini shape). Listed: every unified model and every
direct `<provider>/<model>` of every type, for providers with at least one account. Each
entry carries its type in the style's own field where one exists (Gemini
`supportedGenerationMethods`), else in an extra `zerorouter` field. 9router has no
Anthropic list; 0router adds it because Claude Code's gateway model discovery calls it.

## R16. Non-text model types

**Decision**: per style, only endpoints the real API defines (clarification Q1). Route
ownership:

| Type | OpenAI (openai-chat file) | Messages | Responses | Gemini |
|---|---|---|---|---|
| text | `/v1/chat/completions` | `/v1/messages` | `/v1/responses` | `:generateContent`, `:streamGenerateContent` |
| embeddings | `/v1/embeddings` | — | (shared OpenAI route) | `:embedContent`, `:batchEmbedContents` |
| image | `/v1/images/generations` | — | (shared) | `:generateContent` with image response modality |
| tts | `/v1/audio/speech` | — | (shared) | `:generateContent` with audio response modality |
| stt | `/v1/audio/transcriptions` | — | (shared) | — |
| video | `/v1/videos`, `/v1/videos/{id}`, `/v1/videos/{id}/content` | — | (shared) | `:predictLongRunning`, `operations/{id}` |

The OpenAI non-text routes live in the `openai-chat` file because OpenAI SDK clients of
either OpenAI style call them with the same key.

Provider side (chosen five):
- **openrouter**: embeddings `POST /api/v1/embeddings`; image `POST /api/v1/images`
  (sync, `b64_json`); video `POST /api/v1/videos` → 202 `{id, polling_url}`, poll
  `GET /api/v1/videos/{id}`; TTS through its speech endpoint if the live check confirms
  it, else 9router's chat-completions audio-modality method (a named primitive that
  collects `delta.audio.data`). Docs:
  https://openrouter.ai/docs/api/api-reference/embeddings/submit-an-embedding-request,
  https://openrouter.ai/docs/api/api-reference/images/generate-an-image,
  https://openrouter.ai/docs/api/api-reference/video-generation/submit-a-video-generation-request.
- **elevenlabs**: TTS `POST /v1/text-to-speech/{voice}` (and `/stream`), `xi-api-key`,
  binary audio, `output_format` query; STT `POST /v1/speech-to-text` multipart, `file` +
  `model_id` (`scribe_v2` documented; `scribe_v1` checked live), sync 200 only (0router
  never sets `webhook=true`). Docs: https://elevenlabs.io/docs/api-reference/speech-to-text/convert.
- The voice comes from the request's `voice` field (OpenAI) or speech config (Gemini).
  9router's `model/voice` string also resolves, for parity.
- **Video jobs**: the client polls 0router; each client poll makes one upstream poll on the
  account that took the job (no fallback after submission, because the job lives there).
  Job ids returned to clients are 0router ids mapped in memory to (provider, account,
  upstream id). The record's duration runs from submission to the final result delivered.

**Alternatives**: Anthropic-style image endpoints (none exist; clarification forbids
inventing them).

## R17. Provider schema 2 and the chosen five

**Decision**: `schema = 2` for the chosen five, hand-maintained from now on (brief P note).
Schema 1 stays accepted for the generated community set through a documented conversion.
Details in [contracts/provider-schema-v2.md](contracts/provider-schema-v2.md).

Per provider:
- **anthropic**: text on `anthropic-messages`; `anthropic-version: 2023-06-01` as its only
  static header (no Claude Code beta spoofing); `anthropic-beta` forwarded from Messages
  clients (append); `[token_count]`; continuation as in R9; response headers `request-id`,
  `retry-after`, `anthropic-ratelimit-*` forwarded.
- **openrouter**: text on `openai-chat` and `anthropic-messages` (its `/api/v1/messages`
  endpoint), native pair chosen when possible; non-text as R16; `cache_write_tokens` usage
  path.
- **opencode-zen** and **opencode-go**: text endpoints per wire (`/zen/v1` and
  `/zen/go/v1`: chat/completions, messages, responses; gemini models on the chat wire),
  per-model `wires` from 9router's registry; `force_stream` where 9router forces it;
  `x-opencode-session` derived from the agent id: `ses_time_base62` for opencode-zen and
  `ses_sha256_hex32` for opencode-go, the two formats 9router's executors send.
  Bearer auth, except `/messages`, which takes `x-api-key` raw (an endpoint `auth`, as
  9router's runtime transport does). No fingerprint tools, no spoofed User-Agent (R4).
- **elevenlabs**: TTS as today plus the new STT section (FR-013).
- **(user-visible)** `systemone` / "decision" sections of openrouter and opencode-zen are
  not carried: "decision" is not one of the slice's six types. They can return with a
  later slice.

## R18. Forwarding and the security floor

**Decision**: [contracts/provider-schema-v2.md §Forwarding](contracts/provider-schema-v2.md#forwarding-and-the-security-floor).
- Never forwarded, either direction: `authorization`, `proxy-authorization`, `x-api-key`,
  `api-key`, `x-goog-api-key`, `xi-api-key`, `cookie`, `set-cookie`, `set-cookie2`,
  `www-authenticate`, `proxy-authenticate`, `x-amz-security-token`, `x-auth-token`, any
  name matching slice 002's secret-name pattern, plus every loaded style's key carriers and
  every loaded provider's auth header (computed at load, not a static list).
- Hop-by-hop and core-owned headers are never forwarded either (`host`, `content-length`,
  `transfer-encoding`, `connection`, `keep-alive`, `te`, `trailer`, `upgrade`,
  `content-encoding`, request `accept-encoding`); `x-0router-*` can't be overwritten.
- Any forwarded value containing a configured secret is dropped (Aho-Corasick over the
  secret set); values with CR/LF are rejected.
- Gate: a floor name in a forwarding list is a diagnostic and is stripped (strict mode for
  bundled plugins and CI: error). Bare `*` or a prefix shorter than 3 characters: error.
- Runtime order upstream: client headers → floor → plugin static headers → core auth last.
  Which client headers enter depends on the attempt: all of them on a same-style attempt,
  the declared list on a cross-style one (R27).
  Downstream: provider headers → allowlist → floor → core headers.
- **SSRF**: the gate rejects plugin URLs with loopback, private, link-local or metadata
  hosts and `localhost`/`*.local`/`*.internal`, unless the operator sets
  `allow_private_endpoints = true` in `config.toml` (needed for community plugins like
  ollama). The connector re-checks the resolved IP. Upstream redirects are never followed.
  `{model}` and `{voice}` are percent-encoded; `.`/`..` segments are rejected.

## R19. Fit-or-refuse and the community set

**Decision**: two verdicts. *Invalid* = malformed or unsafe (slice 002 gate). *Unsupported*
= well-formed but needs something this core lacks. Any unsupported part refuses the whole
plugin (clarification Q2) with one message listing every part:

```
<file>: not supported by this core (0router <version>, plugin schema 1-2)
  - <file>:<line>:<col> <path> = <value>: <reason>
No part of this plugin was loaded.
```

Handled: schema 1 or 2; auth `apikey` or `none`; the four wires; the six types;
retry overrides, `force_stream`, passthrough, model fetchers, registry metadata. Unsupported
examples: any OAuth or cookie auth (even alongside an API key), the nine other 9router
wires, web search/fetch and systemone sections, quirks, hooks, `executor_params`, and a
generator-emitted `requires = ["9router-executor:<id>"]` for providers with a specialised
9router executor. The core also derives needs from fields, so omitting `requires` hides
nothing.

Packaging: the 116 community plugins live in `plugins/community/`, generated, embedded in
the binary (no network). `zerorouter plugins list --community` shows fit status;
`zerorouter plugins install <id>` runs gate + fit, then copies the file to
`$ZEROROUTER_HOME/plugins/`. Fit is re-checked on every load.

Generator: `generate.mjs` writes chosen-five seeds to `tools/gen-bundled/seeds/` and
community files to `plugins/community/`; it stops emitting web search/fetch sections.

Parity (FR-036): a test-only `parity_set()` loads bundled + community with the fit check off,
so slice 002's parity tests still see 121 providers. Where the hand-maintained chosen five
now differ from 9router, the difference is listed in `tests/parity/deviations.toml`
(provider, fixture, field, reason) and asserted.

## R20. Operator state and hot apply

**Decision**:
- `$ZEROROUTER_HOME/accounts.toml`: provider accounts (`provider`, `name`, secret literal
  or `{ env = "VAR" }`, order). `$ZEROROUTER_HOME/keys.toml`: agent keys (id, name,
  digest, created, revoked, break-behaviour override). Both 0600; 0router refuses to start
  if either is group- or world-readable.
- `config.toml` (slice 002) gains `[server] listen` (default `127.0.0.1:20129`) and
  `[pipeline] break_behaviour = "restart" | "error_event"`.
- Secrets enter the CLI on stdin, never argv. They are never printed back: listings show
  the name and the last four characters.
- The CLI writes files atomically (temp + rename), then asks a running server to reload
  over the operator socket `$ZEROROUTER_HOME/run/operator.sock` (0600, NDJSON requests).
  The server swaps an `ArcSwap` snapshot; in-flight requests keep the old one. Without a
  running server the change applies at next start.
- Records are queried over the same socket (they exist only in the server's memory).

**Alternatives**: file watching (races with partial writes, no ack); HTTP admin endpoint
(reachable by clients on the same port; a Unix socket is filesystem-permissioned).

## R21. Records store

**Decision**: a bounded ring of 10 000 records (oldest evicted), behind a `Mutex<VecDeque>`
with secondary indexes by id, provider and unified model rebuilt on eviction. Record ids:
`rq_` + 26-char ULID-style (time-sortable). Writes happen once at request end; an
in-flight record is visible with state `in_progress`. TTFT = first content event written
to the client minus request arrival; total = last byte written minus arrival (SC-005,
measured on the server side at the socket write).

## R22. Connection reuse

**Decision**: one `reqwest::Client` per process with a pool keyed by host; HTTP/2 where the
upstream offers it (ALPN), keep-alive 90 s idle, TCP keepalive on. Never a client per
request. SC-011 is tested with a counting mock upstream (one accepted connection for N
sequential requests).

## R23. Secret redaction

**Decision**: a `Redactor` built from the current account secrets and agent keys
(Aho-Corasick), rebuilt on reload. Applied to every upstream error body before
classification output leaves the engine, to every log line (a tracing layer), and to
record fields. `?key=` query values are removed from logged URLs. The SC-006 test injects
sentinel secrets and scans all outputs.

## R24. Performance and benchmarks

**Decision**: Criterion benches with a committed baseline (FR-037, SC-013):
- `wire`: request translation per pair (4×4), stream event translation throughput, usage
  extraction;
- `engine`: attempt-loop overhead with an instant mock upstream (time to first byte added
  by 0router, target p95 ≤ 10 ms, expected well under 1 ms);
- `server`: access-key check, route match, end-to-end loopback request with a mock.

The slice-002 baseline is local-only in `target/`; slice 003 commits its baseline summary
under `specs/003-request-pipeline/bench-baseline.md`.

## R25. Test strategy

**Decision**:
- **Unit and parity** (`zerorouter-wire`): 9router translator fixtures regenerated through
  the `generate.mjs` resolve hook (oracle extension), plus deviation assertions (R4).
- **Engine**: in-process axum mock upstreams scripted per test (status sequences, cut
  streams after N events, stalls, in-band errors, 429 with `retry-after`), covering R6–R10.
- **Harness matrix** (F1, SC-001, SC-009): official SDKs (openai, anthropic, google-genai
  in Python and Node) against 0router with mock upstreams; Claude Code and Codex CLI in
  non-interactive mode against 0router. At least two harnesses per run (the ledger's "more
  than one harness"). Tools the sandbox can't install are run by the operator; the
  quickstart lists how.
- **Live checks** (opt-in, operator keys): one request per chosen provider and type, and
  the continuation checks of R9. Never in CI.
- **Secrets sentinel** (SC-006), **cancellation** (SC-010), **connection reuse** (SC-011),
  **community fit sweep** (SC-012).
- **Optimizer chain** (SC-014): agent SDK → `headroom proxy` → 0router → mock provider, in
  the harness runner, skipped with a message when `headroom` is absent (R27).

## R26. Deliberate deviations from 9router (summary)

| Area | 9router | 0router | Reason |
|---|---|---|---|
| Prompt injections (Claude Code prompt, response_format text, fingerprint tools) | yes | no | IV |
| Gemini client tools | dropped | translated | F1 |
| Non-stream second hop | missing | done | bug |
| Same-account retry for 429 and other 5xx | none | 1 retry (R7) | ledger: stay warm first |
| 5xx with rate-limit or overloaded text | status budget (3 retries for 502/503) | 1 retry at the indicated wait (R7 row "rate-limit / overloaded text") | text wins over status, as in classification (T141 finding 1) |
| Success mark | at stream start | at stream end | R8 |
| Mid-stream break | terminal error frame | continuation / restart / error event | FR-017/018 |
| Stall watchdog on forced-stream bodies | none | armed | edge case |
| Final error | 503, OpenAI shape, raw body | client's own shape, attempts listed, same 503 + `retry-after` | FR-022/023 |
| Missing usage | estimated, +2000 buffer | "not reported" | SC-004 |
| Auth before JSON parse | JSON first | key first | spend nothing on unauthenticated callers |
| 401/403 refresh sleep on API keys | ~3 s | none | no refresh token |
| Anthropic model list | absent | present | Claude Code discovery |
| Account locks | persisted in DB | in memory | persistence is slice 006 |
| Client headers upstream | none beyond executor-built headers | same-style: all but the floor; cross-style: declared list (R27) | Constitution IV |
## R27. Optimizer pass-through (amendment 2026-09-28)

Constitution v3.0.1 IV; spec FR-038–FR-042, SC-014, US1-8 to US1-11.

**Decision**: each attempt is **same-style** (client style id = endpoint wire id) or
**cross-style**, and the two are treated differently.

- **Same-style request body**: the decoder still builds the IR, which the engine needs for
  records, estimates, continuation and cross-style fallback. The upstream body is not
  encoded from the IR. It is the client's parsed body (`serde_json::Value`,
  `preserve_order`) with edits at named paths only: the style's model path → upstream model
  id, the stream path when the endpoint forces streaming, and `stream_options.include_usage`
  on a streamed Chat wire (R13). Opaque parts and unknown keys at any depth go through, so
  `encode` no longer refuses opaque content on a same-style wire. Repairs still run only
  across styles. Fidelity is JSON-value equality outside the edited paths, not byte
  equality.
- **Cross-style request body**: encoded from the IR, as before. The decoder records every
  key no rule consumed, at any depth (top level, message, part, tool), as a path. When the
  encoder can't place one, the attempt records `dropped { path, reason }`. The value is
  never recorded: it may hold prompt text or a secret. Opaque parts still make the target
  `cannot_carry` (R4): content is never dropped.
- **Headers**: same-style attempts send every client header except the floor (R18),
  hop-by-hop headers, `x-0router-*` and `accept-encoding`, then apply the secret-value and
  CR/LF checks. A declared `merge` rule still applies to its header. Plugin static headers
  and core auth follow, in R18's order. Cross-style attempts send only the declared list.
- **Same-style responses**: a non-stream body reaches the client as received. The response
  codec reads usage and errors from it without rebuilding it. Streams already keep event
  names and payloads (R5). The preamble hold, keepalive and removing the usage chunk 0router
  asked for (R13) still apply, because 0router caused them. Response headers stay on the
  `to_client` allowlist.
- **Continuation** (R9) to a same-style target starts from the as-received body plus the
  partial answer, so an optimizer's fields survive a continuation too.
- **Endpoint choice**: within one provider, the same-style endpoint comes first (T055 already
  does this). The stay-warm order across accounts and members is unchanged; weighing the
  style across members belongs to the routing decision (slice 006).

**headroom** (0.37.0, installed at `~/.local/bin/headroom`) is the tested optimizer. On
Anthropic routes it rewrites `messages`, `system` and `tools`, adds cache-TTL markers and
may inject tools. It sends `anthropic-beta: context-management-2025-06-27`, and the
response may carry `context_management`. On OpenAI routes it sets `store: false`,
`stream_options` and `max_completion_tokens`. Its own `x-headroom-*` headers go to its
client, not upstream. The chain test runs `headroom proxy` with 0router as its upstream,
then points a client SDK at headroom. A mock provider asserts what arrived. It is skipped
with a message when `headroom` is absent, like the Codex runner.

**Parity**: 9router already forwards same-format bodies nearly as received
(`open-sse/translator/index.js:83`). Rebuilding them through the IR was 0router's own
deviation, which this corrects. Forwarding client headers is a deliberate deviation (R26).

**Alternatives**: copying unknown fields across styles (OpenAI, Anthropic and Gemini reject
unknown fields with a 400, which isn't retried); refusing cross-style routes when unknown
fields are present (fails requests that could be served); raw byte forwarding for same-style
bodies (the model id and stream edits need a parsed body).
