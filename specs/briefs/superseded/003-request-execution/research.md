# Research: Request Execution Walking Skeleton

**Feature**: [spec.md](spec.md) | **Plan**: [plan.md](plan.md) | **Date**: 2026-09-27

**Reference**: `ref/9router@39e36d3`, the same pin as the 002 bundled plugins and oracle.

Every Technical Context unknown is resolved below. Each entry records the decision, the
rationale, and the alternatives considered. Citations to 9router are relative to
`ref/9router/`.

---

## R1. Crates and workspace layout

**Decision**

- Add a library crate, `crates/nullrouter-server`. It depends on `nullrouter-registry` and
  holds everything in this slice:
  - accounts and access keys;
  - placeholder selection;
  - outbound build;
  - relay;
  - observations;
  - the HTTP surface;
  - the operator channel.
- `nullrouter-cli` gains three commands: `serve`, `reload`, and `obs`
  ([R12](#r12-operator-channel-and-cli)).
- New workspace dependencies (latest versions checked on crates.io, 2026-09-27):

| Crate | Version | Use |
|---|---|---|
| `tokio` | 1.53 (`rt-multi-thread`, `macros`, `net`, `time`, `sync`, `signal`, `io-util`) | runtime |
| `tokio-util` | 0.7 | `CancellationToken` |
| `axum` | 0.8 (default features off: `http1`, `http2`, `tokio`, `json`) | client surface |
| `reqwest` | 0.13 (default features off: `rustls`, `stream`, `http2`) | upstream client |
| `futures-util` | 0.3 | `Stream`/`StreamExt` |
| `bytes` | 1 | zero-copy chunks |
| `sha2` | 0.11 | access-key digest index ([R5](#r5-access-keys-and-agent-identity)) |
| `serde_json` | existing; add feature `raw_value` | byte-preserving body rewrite ([R8](#r8-outbound-body)) |
| `memchr` | 2 | SSE event-boundary scan ([R9](#r9-relay-cancellation-and-the-sse-question-fr-018-fr-018a-fr-021)) |
| `tracing`, `tracing-subscriber` | 0.1, 0.3 | logs ([R18](#r18-logging)) |
| hand-rolled `axum` mock upstreams | dev only | mock upstreams ([R16](#r16-test-strategy)) |

**Rationale**

- The registry stays a sync library with no runtime (002 R2). The async code lives in a
  sibling crate, as 002's Structure Decision anticipated.
- One server crate is enough for a walking skeleton. Module boundaries (`select`,
  `outbound`, `relay`, `observe`) keep the routing slice's replacement surface small
  (FR-011).
- The MSRV stays 1.85: reqwest 0.13's MSRV is 1.85, and the rest are lower.

**Alternatives considered**

- **Separate crates for execution, HTTP, and observations.** Premature: there is one
  consumer. Modules can be split out when the routing slice lands.
- **`hyper` directly instead of `axum`.** Constitution V names axum, and axum's routing
  and extractors cost nothing on the hot path.
- **`hyper-util` client instead of `reqwest`.** reqwest gives rustls, connection pooling,
  and a byte stream with no extra glue. It adds no buffering: `bytes_stream()` yields
  each frame as it arrives.

---

## R2. Which providers are executable (FR-013)

**Decision**

A provider is executable in this slice when all of the following hold:

1. Its `category` is `apikey`, or `freeTier` with no `no_auth` flag (on the auth
   declaration or in the transport's executor parameters).
2. It is not in the core's **specialized-executor table**. This is a static list of the
   ids 9router routes to its own executor classes (`open-sse/executors/index.js`):
   antigravity, azure, gemini-cli, github, iflow, qoder, qoder-cn, kiro, kimchi, codex,
   cursor, vertex, vertex-partner, opencode, opencode-go, opencode-zen, grok-web,
   grok-cli, perplexity-web, ollama-local, commandcode, xiaomi-tokenplan, xiaomi-mimo,
   mimo-free, codebuddy-cn, codebuddy-intl, trae, zed, windsurf, devin-cli. Their aliases
   (`cu`, `gcli`, `gb`, `mmf`) resolve through the registry first.
3. At least one of its transports speaks `openai` or `claude`.

For embeddings, the provider must also be in 9router's `OPENAI_COMPAT_PROVIDERS`
(`handlers/embeddingProviders/index.js`) and pass rule 1. That list is openai,
openrouter, mistral, voyage-ai, fireworks, together, nebius, nvidia, jina-ai, and
vercel-ai-gateway. github is excluded because it is OAuth.

Computed from the bundled plugins at the pin:

- **45 chat-executable providers**: alicode-intl, alicode, alims-intl, alitp-intl,
  anthropic, api-airforce, assemblyai, baidu, bazaarlink, blackbox, bluesminds, byteplus,
  cerebras, chutes, cloudflare-ai, cohere, deepgram, deepseek, featherless, fireworks,
  glm-cn, glm, groq, hyperbolic, kilo-gateway, llm7, minimax-cn, minimax, mistral, morph,
  nanobanana, nebius, nvidia, openai, openrouter, perplexity, poolside, sambanova,
  siliconflow, tencent, together, tokenrouter, venice, vercel-ai-gateway, volcengine-ark.
- **10 embeddings-executable providers**, as listed above. Two of them, voyage-ai and
  jina-ai, have no chat transport.
- A connection is accepted for any provider in either set, which makes 47 providers.

The rule is implemented in Rust. A parity test compares its result with
`executable-providers.json`, which the generator emits from the evaluated registry and
the executor map ([R14](#r14-parity-oracle-extension)). If the two disagree, the test
fails.

**Rationale**

- FR-013 defines the set by 9router's behaviour, not by a hand-kept list.
- The specialized-executor table is core knowledge about which providers need built-in
  code. That is exactly the boundary Constitution I draws: a plugin cannot declare "I
  need custom code".
- User plugins with `category = "apikey"` and an openai or claude transport are
  executable automatically. This is how operators add OpenAI-compatible endpoints
  (spec Assumptions).

**Alternatives considered**

- **A plugin field `executor = "generic"`.** This would let a plugin claim executability
  for a provider that 9router treats specially, and it adds schema surface for a
  temporary slice boundary.
- **Committing the 45-name list as the rule.** It would break for user plugins and drift
  when the ref is updated.

---

## R3. Transport and target-format choice (FR-014)

**Decision**

0router ports `chatCore.js` exactly:

1. `runtimeTransport` is the first entry in `transports[]` whose `format` equals the
   client format, else none. This mirrors `resolveTransport` in `services/provider.js`.
   The primary `transport` is not searched here, as in 9router.
2. `useTransport` is `runtimeTransport` if the model declares no `supported_formats`, or
   if they include the client format. Otherwise it is none.
3. `targetFormat` is `useTransport.format`, else the model's `target_format`, else the
   provider's primary transport format, else `openai`.
4. If `targetFormat` differs from the client format, the request fails with
   `format_mismatch`. The error lists the formats of all the provider's transports.
5. The effective transport is `useTransport` when present, else the primary transport.
   Its fields replace the primary transport's `base_url`, `url_suffix`, `headers`, and
   `auth`. This matches `DefaultExecutor`'s `rt ? rt.x : config.x`.

**Rationale**

- Constitution VI.
- The minimax case (a primary claude transport plus `[[transports]]` in openai and
  claude formats) is covered by the oracle.

**Alternatives considered**

- **Searching the primary transport by format too.** That is a deviation with no user
  benefit, and it would change which headers minimax receives.

---

## R4. Accounts and access keys: file, secrets, validation (FR-004a, FR-006–FR-010)

**Decision**

- A new operator file, `$NULLROUTER_HOME/keys.toml`, holds `schema = 1`, `[[access_key]]`,
  and `[[connection]]` (see [contracts/keys-file.md](contracts/keys-file.md)).
- A secret is either a literal string or `{ env = "VAR" }`.
- Environment references are resolved **at load**:
  - an unset or empty variable is a load error;
  - the value is held in a core-only `Secret`, and a new value takes effect on reload.
- Permission rule (FR-007): if the file contains at least one literal secret and
  `mode & 0o077 != 0`, loading fails with `keys.toml: readable by group/others (mode
  0644); chmod 600 or use { env = ... }`. A file holding only env references may be
  world-readable.
- Validation reports every error, in 002's `file:line:col path: rule` form, using
  `toml::de::DeTable` spans as the 002 loader does:
  - the provider exists (id or alias) and is executable ([R2](#r2-which-providers-are-executable-fr-013));
  - connection names are unique per provider;
  - agent names are unique across access keys, non-empty, and matching `[A-Za-z0-9._-]{1,64}`;
  - access-key values are unique;
  - access keys are at least 16 characters;
  - `account_id` is present when the effective URL contains `{accountId}` (cloudflare-ai).
- `keys.toml` is separate from `config.toml`. The registry's `OperatorConfig` stays
  `deny_unknown_fields` and secret-free.
- Reload rebuilds the registry and the keys file together. Either both swap or neither
  does ([R10](#r10-reload-and-snapshots)).

**Rationale**

- Keeping secrets out of `config.toml` means `config.toml` can be shared or checked into
  dotfiles.
- 002's `SecretString` is crate-private to the registry and bound to OAuth hosts. The
  server needs its own `Secret` newtype:
  - its `Debug` and `Display` print `***`;
  - it has no `Serialize`;
  - `expose()` is `pub(crate)` in the server crate and called only by `outbound`.
- `account_id` is not a secret. It appears in the URL, and 9router logs it.

**Alternatives considered**

- **Secrets in `config.toml`.** This mixes shareable config with secrets and forces the
  0600 rule on everything.
- **An OS keyring.** It is not available in the Landlock-confined environment and is
  over-scoped for a skeleton.
- **Resolving env vars at request time.** This hides misconfiguration until the first
  request and makes FR-008's "variable is set" check meaningless.

---

## R5. Access keys and agent identity (FR-004, FR-005, FR-005a, FR-005b)

**Decision**

- **Key extraction** follows 9router's `extractApiKey`: `Authorization: Bearer <key>`
  first, then `x-api-key`. The Bearer check is case-sensitive on the scheme, as in
  9router.
- **Key lookup**: keys are indexed by SHA-256 digest (`HashMap<[u8; 32], AgentIdx>`).
  The request key is hashed and looked up. The digest compare runs in O(1), and any
  timing difference reveals only the hash, not the key.
- **Auth failures**: a key that is inactive or unknown gives `401 Invalid API key`; a
  missing key gives `401 Missing API key` (9router `src/sse/handlers/chat.js:74,79`).
  The rejection happens before the body is parsed.
- **Session extraction** ports 9router's client-supplied carriers in `sessionManager.js`,
  in its order, with the non-kiro rules:
  1. The Claude session from body `metadata.user_id`. This is the `_session_<uuid>`
     regex, or the `session_id` field when `user_id` is a JSON string. Next comes the
     `x-claude-code-session-id` header. Both are recorded as `claude:<id>`.
  2. Headers, in order: `x-session-id`, `session-id`, `session_id`, `x-amp-thread-id`.
  3. The `x-client-request-id` header.
  4. Body fields, in order: `prompt_cache_key`, `session_id`, `conversation_id`,
     `metadata.user_id`.

  Each value goes through `normalizeSessionId`: it must be a string; it is trimmed; it
  must be non-empty and at most 256 characters. Otherwise the next carrier is tried. The
  Antigravity envelope and generated fallbacks are not ported (FR-005a).
- **Identity**: `AgentIdentity { agent: AgentName, session: Option<String> }`.
  Observations store the pair, so two agents with the same session value are distinct
  (FR-005b). The session is read only; the outbound body is built from the client's
  bytes ([R8](#r8-outbound-body)).

**Rationale**

- This matches 9router's reading order, and SC-010 checks it against a generated
  corpus.
- 9router's session extractor (`extractClientSessionId`) is not exported. The oracle
  therefore calls `resolveSessionIdentity` twice for each case, with different
  `connectionId` values. If the result changes between the two calls, 9router fell back
  to a generated id, and the expected value is "none".

**Alternatives considered**

- **Linear constant-time comparison over all keys.** This is O(n) per request for the
  same safety.
- **`subtle` crate on raw keys.** Not needed once the lookup is digest-indexed.

---

## R6. Outbound URL and headers (FR-015, FR-015a)

**Decision**

This ports `DefaultExecutor.buildUrl` and `buildHeaders` for the executable set.

**URL**

1. Start from the effective transport's `base_url`.
2. Apply `url_suffix` if present.
3. Replace `{accountId}` with the connection's `account_id`.

The openai-compatible- and anthropic-compatible- prefix rules and the gemini branch do
not apply to any executable provider. They are not ported; a debug assertion covers this.

**Headers**

The header map is built in this order:

1. `Content-Type: application/json`.
2. The effective transport's declared `headers`, in declaration order and with their
   declared casing.
3. **Auth**, from the transport's `auth` descriptor if declared. Otherwise the fallback
   applies:
   - `x-api-key: <key>` (raw), plus the version header, for format claude;
   - `Authorization: Bearer <key>` otherwise.

   A combined descriptor always sets its header.
4. **Version header**: when the descriptor has `anthropic_version`, add
   `anthropic-version: 2023-06-01`, but only if the exact lowercase key
   `anthropic-version` is absent. A declared `Anthropic-Version` with different casing
   therefore produces a **second** header. Fetch merges the two into
   `2023-06-01, 2023-06-01`, and 0router emits that same merged value. This affects
   minimax and minimax-cn; the oracle records the wire-merged form.
5. `Accept: text/event-stream` when the outbound request streams.

Auth hooks exist only on OAuth plugins, so none apply in this slice.
`selectAnthropicBeta` applies only to the `claude` OAuth provider and to
anthropic-compatible- providers, so none apply either. anthropic's `Anthropic-Beta` comes
from its declared headers.

**Header emission**

- Emission uses `http::HeaderMap`. Header names are case-insensitive on the wire, and
  HTTP/2 lowercases them.
- Parity compares the merged, lowercase-name view that fetch's `Headers` produces. It
  compares the credential by position ("the auth header carries the connection key"),
  not by value.

**Native pair (FR-015a)**

- Client detection ports `detectClientTool`, in order:
  1. body `userAgent` antigravity;
  2. `githubcopilotchat`, `openai-intent`, or `x-initiator` → github-copilot;
  3. a UA containing `claude-cli` or `claude-code`, or `x-app: cli` → claude;
  4. `gemini-cli`;
  5. `codex-tui`, `codex-cli`, `codex_cli_rs`, or `codex desktop`, or `originator`
     starting with `codex_` → codex;
  6. `deepseek-tui`.
- `isNativePassthrough` uses `NATIVE_PAIRS`: `claude` pairs with `claude` and
  `anthropic`. anthropic-compatible* ids are normalized to `anthropic`.
- When the request is a native pair:
  1. Start from the generic header map.
  2. Overlay every client header, with the client value replacing the declared value.
     Names are compared case-insensitively.
  3. Remove `authorization`, `x-api-key`, `host`, `content-length`, and the hop-by-hop
     set (`connection`, `keep-alive`, `proxy-authenticate`, `proxy-authorization`, `te`,
     `trailer`, `transfer-encoding`, `upgrade`, plus any name listed in `Connection`).
  4. Re-apply the connection's auth header last, so it cannot be overridden.
- `accept-encoding` is also removed. reqwest negotiates its own encoding, and a
  forwarded value could produce a compressed body that 0router would relay without the
  matching header context.

**Rationale**

- SC-001 needs field-for-field equality, including 9router's duplicate-header quirk.
- Emitting the merged value keeps the bytes the upstream receives identical to
  9router's.

**Alternatives considered**

- **Fixing the duplicate header (sending only `2023-06-01`).** Arguably cleaner, but a
  deviation with no user benefit. It is recorded in [R15](#r15-deliberate-deviations-from-9router)
  as a candidate once parity is established.

---

## R7. Timeouts (FR-017)

**Decision**

- **Connect timeout** is the time from sending the request until response headers
  arrive. It is implemented as `tokio::time::timeout` around `RequestBuilder::send()`.
  The value is the effective transport's `timeout_ms` if set, else
  `FETCH_CONNECT_TIMEOUT_MS`, else 60 000 ms. This follows `BaseExecutor.execute`, where
  `this.config.timeoutMs || FETCH_CONNECT_TIMEOUT_MS`. The spec text names only the env
  override; the per-provider override is 9router behaviour, so it is kept (Constitution
  VI).
- **Stall timeout** is a `tokio::time::timeout` around each `next()` of the upstream
  body stream. The value is the transport's `stall_timeout_ms`, else
  `STREAM_STALL_TIMEOUT_MS`, else 360 000 ms. It applies to streamed relays and to
  forced-stream assembly.
- **Env parsing** ports `envMs`: JS `parseInt` semantics (optional leading whitespace and
  sign, then leading digits). The value is used only if it is finite and greater than 0.
  Env values are read once at `serve` start.
- **Non-streaming bodies** (JSON responses) are read under the stall timeout, per chunk.
  9router has no body timeout on this path, but its fetch has no idle guarantee either.
  0router applies the same stall bound, so a hung body cannot pin a connection forever.
  This is recorded in [R15](#r15-deliberate-deviations-from-9router).

**Rationale**

- Constitution VI requires timeout parity, including env overrides.

**Alternatives considered**

- **reqwest's `connect_timeout`.** It covers only the TCP/TLS handshake, not time to
  headers, so it does not match 9router's semantics.

---

## R8. Outbound body (FR-016)

**Decision**

- The client body is parsed once into `IndexMap<String, Box<serde_json::value::RawValue>>`
  (top level only). Then:
  - `model` is replaced by the upstream id, as a JSON string;
  - for a forced-stream provider, `stream` is set to `true`, and inserted at the end if
    absent.
  - The map is re-serialized compactly.
- All nested values are copied byte for byte.
- Top-level key order is preserved. Top-level whitespace is normalized.
- A body whose top level is not an object is a bad request (400, `invalid_json`).
- A missing, non-string, or empty `model` gives `400 Missing model`.
- Embeddings bodies are built by the adapter rule: `{model, input, encoding_format?,
  dimensions?}`. `dimensions` is kept only when it is a finite number greater than 0;
  other client fields are dropped (`handlers/embeddingProviders/openai.js`).
- The client body limit is 32 MiB. Over the limit gives 413, error type
  `request_too_large`.

**Rationale**

- Constitution IV and FR-016 allow only the model and stream changes.
  - Preserving nested bytes is stricter than 9router, which runs `JSON.parse` and then
    `stringify` on the whole body. That changes number spelling (`1.0` becomes `1`) and
    Unicode escapes.
  - Prompt content is never re-encoded.
- The 32 MiB limit is well above a 1M-token context in JSON.

**Alternatives considered**

- **`serde_json::Value` round-trip.** It re-encodes every string and number, so the
  outbound bytes would differ from what the client sent.
- **Byte-level patching of the model string.** This is fragile with duplicate keys and
  escapes.

---

## R9. Relay, cancellation, and the `Sse` question (FR-018, FR-018a, FR-021)

**Decision**

- **The response body** is `axum::body::Body::from_stream(relay)`, and `relay` is an
  `impl Stream<Item = Result<Bytes, Infallible>>` built with `futures::StreamExt`
  adaptors over the upstream `bytes_stream()`. Streaming and native responses have no
  buffering beyond one SSE event.
- **Event framing**: an incremental splitter finds event boundaries (`\n\n`, `\r\n\r\n`,
  or `\r\r`) with `memchr`. Each complete event's original bytes are yielded as soon as
  its terminator arrives. A trailing partial event is flushed when the upstream ends.
- **Per-event yield**:
  - A chunk that holds several events yields them together, zero-copy as `Bytes`
    slices.
  - The event is never delayed waiting for a later one (FR-018, SC-003).
  - Framing at event boundaries guarantees the closing error frame of FR-018a starts on
    a clean boundary.
- **Side-channel parsing**: the same pass looks at each complete event's `data:`
  payload, from a borrowed slice, for three things:
  - time to first token, taken from the first complete event;
  - usage ([R11](#r11-usage-extraction-fr-023));
  - an in-band error for assembly.

  Parsing is skipped once it is no longer needed. It never mutates the relayed bytes.
- **Closing error (FR-018a)** is triggered by an upstream read error, end of stream after
  a read error, or a stall timeout. It emits 9router's `buildStreamErrorBytes(504,
  message, clientFormat)`:
  - Anthropic clients get `event: error\ndata: {"type":"error","error":{…}}\n\n`;
  - OpenAI clients get `data: {"error":{…}}\n\n` followed by `data: [DONE]\n\n`.

  The error object is `buildErrorBody(504, message)`.
- **Cancellation**:
  - Each request owns a `CancellationToken`.
  - The relay stream holds a `CancelOnDrop` guard. When hyper drops the body because
    the client disconnected, the guard cancels the token and records `cancelled_by_client`.
  - The upstream read `select!`s on the token, and dropping the reqwest `Response`
    closes the upstream connection.
  - Connect-phase cancellation uses the same token around `send()`.
  - SC-004's one-second bound is met by drop-propagation, not by polling.
- **Constitution V says "The required pattern is `axum::response::sse::Sse<impl Stream>`"**.
  `Sse` takes `Event` values and re-serializes them: it rewrites `data:` lines, joins
  multi-line data, and may reorder fields. That breaks FR-018's "without altering event
  content" and SC-008's byte-identical native relay. 0router satisfies the principle's
  intent without using `Sse`:
  - there is no buffering;
  - `StreamExt` composition is used throughout;
  - cancellation uses `CancellationToken` and `CancelOnDrop`.

  The departure is recorded in plan.md Complexity Tracking, with a proposed PATCH
  amendment to Constitution V.

**Rationale**

- Byte fidelity is a spec requirement, and `Sse` cannot deliver it.
- `Body::from_stream` over the same adaptors keeps every other guarantee of Principle V.

**Alternatives considered**

- **`Sse<impl Stream<Item = Event>>`, rebuilding each event.** It alters content (for
  example, `data:x` becomes `data: x`, and `retry`/`id` ordering changes) and costs an
  allocation per event.
- **Raw chunk passthrough without framing.** Simplest, but TTFT and usage would need a
  second framing pass, and a mid-event break would leave the closing error frame glued
  to a partial event.

---

## R10. Reload and snapshots (FR-010, FR-027)

**Decision**

- The server holds `ArcSwap<State>`, where `State { registry: Arc<Registry>, keys:
  Arc<Keys> }`. Each request loads the `Arc<State>` once and holds it to the end,
  including the relay stream. The request therefore keeps its registry version, its
  unified-model resolution, and its connection (spec edge case).
- Reload runs in `spawn_blocking` under a mutex:
  1. Build a candidate registry with a new public registry entry point,
     `Registry::load_candidate(&OperatorHome) -> Result<Registry, ReloadError>`.
  2. Build the candidate keys from `keys.toml` against that candidate registry.
  3. If both succeed, `store` a new `State`. Otherwise keep the old state and return
     every error.
- Startup uses the same path, with 002's startup semantics:
  - a fatal registry error aborts;
  - an invalid `keys.toml` is **fatal at startup**, because there is nothing to serve
    with;
  - on reload, an invalid `keys.toml` rejects the reload.
- The registry change is additive. `RegistryHandle` stays unchanged for the CLI's
  offline commands. The server does not use `RegistryHandle`, because two independent
  swaps could not be made atomic together.

**Rationale**

- FR-010 requires connections to swap "atomically together with the registry".
- Connections are validated against the candidate registry, so a reload that removes a
  provider and its connection in the same edit succeeds.

**Alternatives considered**

- **Two `ArcSwap`s, loaded in sequence.** A request could see a new registry with old
  keys.
- **`RwLock<State>`.** Readers would contend with reload, and the lock-free read is
  already established in 002.

---

## R11. Usage extraction (FR-023, SC-005)

**Decision**

- 0router ports `extractUsage` and `mergeUsage` from `open-sse/utils/usageTracking.js`
  for the two wire formats. For streams, every usage-bearing event is merged in order:
  `Math.max` per numeric field, and the latest wins for nested objects.
- Fields are mapped to observation fields:

| Wire format | input | output | cache read | cache write |
|---|---|---|---|---|
| Anthropic (`message_start.message.usage`, `message_delta.usage`, non-stream `usage`) | `input_tokens` | `output_tokens` | `cache_read_input_tokens` | `cache_creation_input_tokens` |
| OpenAI chat (`usage` with `prompt_tokens`) | `prompt_tokens` | `completion_tokens` | `prompt_tokens_details.cached_tokens`, else `prompt_cache_hit_tokens` (DeepSeek) | not reported by the format |
| OpenAI embeddings | `usage.prompt_tokens` | — | — | — |

- **Absence is kept.** 9router coerces a missing `input_tokens` to 0. 0router records
  "not reported" (`None`) whenever the field is absent in every usage-bearing event.
  The field meanings match 9router's; only absence is kept distinct (FR-023 requires
  it).
- **Anthropic `input` excludes cache tokens**, as the provider reports it. 9router's
  `toOpenAIUsage` folds cache into `prompt_tokens`, but only when translating.
  Observations keep the raw split, which the routing slice needs.
- **OpenAI streams carry usage only when the client sets
  `stream_options.include_usage`.** 0router does not add it, because the body is not
  changed (FR-016). Such observations record usage as "not reported". 9router injects
  the option only in its iflow executor. This observability gap is flagged to the
  routing slice.

**Rationale**

- Same meanings as 9router, and SC-005 tests them against the oracle's
  `extractUsage`/`mergeUsage` output.

**Alternatives considered**

- **Injecting `stream_options.include_usage`.** It changes the body, violating
  Constitution IV and FR-016, and some OpenAI-compatible providers reject the option.

---

## R12. Operator channel and CLI (FR-026–FR-028)

**Decision**

- **Channel**:
  - A Unix domain socket at `$NULLROUTER_HOME/run/operator.sock`.
  - The directory is created with mode 0700 and the socket is chmod 0600.
  - On accept, the peer uid (tokio `UnixStream::peer_cred`) must equal the socket
    file's owner uid, which is the server's own uid. This avoids `unsafe` and extra
    crates.
  - A stale socket file is removed at start only if connecting to it fails.
  - The channel is never bound on TCP, so clients of the client-facing surface cannot
    reach it (FR-028).
- **Protocol**:
  - One JSON request line, then one JSON response line (NDJSON).
  - Ops: `reload`, `observations`, `status`.
  - See [contracts/operator-cli.md](contracts/operator-cli.md).
- **CLI**:
  - `nullrouter-cli serve [--listen ADDR] [--observations-cap N]`.
  - `nullrouter-cli reload`.
  - `nullrouter-cli obs [--provider P] [--unified U] [--agent A] [--session S]
    [--endpoint E] [--since T] [--until T] [--include-count-tokens] [--limit N]
    [--json]`.
  - The client side uses a blocking `std::os::unix::net::UnixStream`, so the CLI's
    offline commands stay runtime-free.
- **Server settings**:
  - Listen address: default `127.0.0.1:20129`, from `--listen` or `NULLROUTER_LISTEN`.
  - Observation cap: default 10 000, from `--observations-cap` or
    `NULLROUTER_OBSERVATIONS_CAP`.
  - Both take effect at `serve` start. The default port is one above 9router's 20128,
    so both can run side by side for parity checks.

**Rationale**

- File permissions and the peer uid check give operator-only access with no token to
  manage.
- A separate transport is structurally unreachable from the client surface.

**Alternatives considered**

- **An admin route on the TCP listener with a separate token.** It is reachable by
  clients in principle, which FR-028 forbids, and adds a secret.
- **A second TCP port bound to loopback.** Any local user could reach it.
- **Signals (SIGHUP) for reload.** They cannot return the load report or errors
  (FR-027).

---

## R13. Observations store and summaries (FR-022, FR-024–FR-026)

**Decision**

- The store is a `Mutex<VecDeque<Observation>>` with a fixed capacity. A push past the
  capacity pops the front.
- One observation is recorded per request, at its terminal point:
  - after the last byte is handed to hyper;
  - when the drop guard fires;
  - or when a pre-execution rejection is returned.
- The critical section is a single push. It runs after the final relay write, so it
  never delays an event (FR-024). Contention is bounded: one push per request.
- An `ObservationBuilder` travels with the request. Its terminal handling is idempotent,
  so exactly one observation is recorded even when the stream end and the drop guard
  race.
- **Queries**:
  - The whole deque is cloned under the lock, then filtered and summarized outside it.
  - The summary gives count, success count, and p50/p95 of TTFT and total duration, by
    the nearest-rank method.
  - Observations missing a TTFT are excluded from TTFT percentiles.
  - Token-count requests are excluded from latency summaries unless
    `include_count_tokens` is set.
- **Raw upstream error body** (FR-020c): the first 8 KiB, stored as lossy UTF-8.
- **Upstream response headers**: all of them except `set-cookie` and the hop-by-hop
  set, as `Vec<(String, String)>`.

**Rationale**

- The store is simple, bounded (FR-025), and cheap at 10 000 entries.
- Lock-free structures are unjustified at one push per request.

**Alternatives considered**

- **An mpsc channel to a recorder task.** It adds a task and backpressure questions for
  no measurable gain.
- **HDR histograms.** They need per-filter state. Filtering then sorting 10 000 entries
  takes well under a millisecond.

---

## R14. Parity oracle extension

**Decision**

`tools/gen-bundled/generate.mjs` gains an `execution` phase that writes the following to
`tests/fixtures/9router/`:

| Fixture | Source (9router function) | Checks |
|---|---|---|
| `executable-providers.json` | registry and executor map | R2 set |
| `executor-requests.json` | `new DefaultExecutor(p).buildUrl/buildHeaders` over provider × transport × stream, with placeholder credentials `{apiKey:"<KEY>", providerSpecificData:{accountId:"<ACCT>"}}`; headers captured both as the object and as `new Headers(obj)` entries | FR-015, SC-001 |
| `transport-choice.json` | chatCore's choice logic over provider × client format × model | FR-014 |
| `client-detect.json` | `detectClientTool` + `isNativePassthrough` over a header/body corpus | FR-015a |
| `sessions.json` | `resolveSessionIdentity`, differential over two connectionIds | SC-010 |
| `count-tokens.json` | `estimateAnthropicInputTokens` over a body corpus | SC-011 |
| `upstream-errors.json` | `parseUpstreamError` → `formatProviderError` → `buildErrorBody`, over status × body corpus (JSON, text, HTML, empty) | FR-020, SC-008 |
| `non-sse.json` | the streamingHandler non-SSE branch (inlined helper) over content-type × body | FR-020b |
| `stream-errors.json` | `buildStreamErrorBytes` for both client formats | FR-018a |
| `usage.json` | `extractUsage` + `mergeUsage` over event sequences for both formats, with and without cache fields | SC-005 |
| `embeddings.json` | `createOpenAIEmbeddingAdapter(id)` build: URL, headers, and body | US4 |
| `sse-to-json.json` | `parseSSEToOpenAIResponse` plus the reasoning strip, over forced-stream corpora | FR-019 |
| `timeouts.json` | `envMs` over an env-string corpus | FR-017 |

- **Loading modules**: `ref/9router` has no `node_modules`, and several modules import
  through the Next `@/` alias (`@/lib/usageDb.js`). The generator registers a Node
  module resolve hook (`module.register`) with three rules:
  - `@/` maps to `ref/9router/src/`;
  - a fixed stub list replaces DB, logging, and network modules with no-op modules;
  - bare package imports that are absent resolve to stubs.
- The count_tokens route and `usage.js` have no imports. `sessionManager.js` needs only
  `node:crypto`.
- Where a helper is not exported (the non-SSE message builder inside
  `streamingHandler.js`), the generator extracts the function source by marker comments
  and evaluates it. This keeps 9router as the oracle.
- The generator stays the only writer, and the fixture header records the ref SHA.

**Rationale**

- The spec requires oracle-generated expectations, not hand-written ones.
- A resolve hook avoids installing 9router's dependency tree.

**Alternatives considered**

- **Running 9router and capturing traffic.** Heavy, needs `npm install` and network
  access, and is non-deterministic.
- **Hand-transcribed expectations.** Forbidden by the spec.

---

## R15. Deliberate deviations from 9router

Each deviation below is asserted by a named test that states the 9router behaviour and
the 0router behaviour.

| # | 9router | 0router | Source |
|---|---|---|---|
| D1 | Connect timeout aborts with `new Error("fetch connect timeout")`, which is not an AbortError. The client gets **502** `[502]: fetch connect timeout` | **504** gateway timeout, `upstream_timeout` | spec FR-017, edge case ("structured gateway-timeout error"). **Flagged for confirmation.** |
| D2 | Same-format paths never set `stream: true` for `forceStream` providers. A non-streaming client of `openai` or `api-airforce` gets whatever the upstream returns | Sets `stream: true`, then assembles the stream as JSON with `parseSSEToOpenAIResponse` | spec FR-016, FR-019, US1 scenario 4 |
| D3 | Same-format OpenAI streams are normalized: id fix, `object`/`created` injection, Azure filter-result stripping, empty `tool_calls` removal, "valueless" chunks dropped | Events relayed byte-identical | spec FR-018 |
| D4 | Every error body uses the OpenAI shape `{error:{message,type,code}}`, even for `/v1/messages` clients | Upstream errors (FR-020) keep 9router's OpenAI-shaped object. 0router-originated errors (FR-003) use the client's wire format | spec FR-003 vs FR-020 |
| D5 | No client headers are forwarded, even for native pairs | Native pairs forward client headers | spec FR-015a (stated deviation) |
| D6 | Passthrough adjustments: Claude cache re-anchoring, prompt normalization, param stripping, reasoning injection, `client_metadata` drop, json-schema fallback, strip lists | None applied | spec Assumptions |
| D7 | Executor retries and URL fallback on 429 | None | spec Assumptions |
| D8 | Body re-encoded through `JSON.parse`/`stringify` | Nested bytes preserved ([R8](#r8-outbound-body)) | Constitution IV (stricter) |
| D9 | `count_tokens` requires no auth | Requires an access key | spec FR-004 |
| D10 | Non-streaming JSON bodies have no idle timeout | Stall timeout applies per body chunk | [R7](#r7-timeouts-fr-017) |
| D11 | Session fallbacks (assistant-text hash, per-connection random id) | "none" | spec FR-005a |
| D12 | Missing usage fields coerced to 0 | "not reported" | spec FR-023 |

D1 is the only item where the spec's text may not match the user's intent: "gateway
timeout" is a reasonable reading of a timeout, but 9router returns 502. The plan follows
the spec (504). Reverting to 502 is a one-line change and one test.

---

## R16. Test strategy

**Decision**

| Kind | Where | What |
|---|---|---|
| Unit | modules | env parsing, session extraction, client detection, header build, body rewrite, SSE framing, usage merge, error formatting, percentiles |
| `parity` | `crates/nullrouter-server/tests/parity/` | every R14 fixture |
| `e2e` | `tests/e2e/` | an in-process server against in-process `axum` mock upstreams that record requests and script responses: delays, mid-stream breaks, stalls, HTML pages, and 4xx/5xx |
| `cancel` | e2e | client drop, then the mock observes connection close within 1 s (SC-004), over 50 runs |
| `concurrency` | e2e | 100 concurrent streams through one connection; the mock releases events in lockstep and asserts no serialization (SC-007) |
| `secrets` | e2e | sentinel keys (`nr-sentinel-…`) in `keys.toml`; captures observations, `tracing` output, CLI output, error bodies, and mock-received requests; asserts SC-006 |
| `reload` | e2e | reload during an in-flight stream; an invalid `keys.toml` keeps the old state |
| Bench | `benches/relay.rs` (Criterion) | relay overhead per event and TTFT delta against a direct mock (SC-003), plus a `select` + `outbound` build micro-bench |
| Live smoke | `quickstart.md`, manual | Claude Code and an OpenAI SDK against real providers (SC-002) |

**Rationale**

- In-process mocks keep tests hermetic and fast, and let tests control timing
  precisely.
- `wiremock` cannot script mid-stream stalls, so hand-rolled `axum` mocks are used.

**Alternatives considered**

- **`wiremock`.** Adequate for static responses only.
- **Docker-based mocks.** Unavailable under the Landlock gate and slower.

---

## R17. Performance

**Decision**

- **Budget**: under 5 ms median added TTFT (SC-003). Expected overhead:
  - under 50 µs for auth, parse, select, and build;
  - plus one local hop.
- **Hot-path rules**:
  - no `serde_json::Value` on relay;
  - borrowed-slice usage parsing;
  - `Bytes` slicing for events;
  - reqwest connection pooling per upstream host, so there is no TLS handshake after
    the first request.
- The Criterion bench `relay` sets the baseline. The workspace's benchmark gate then
  applies to later changes.

**Rationale**

- The constitution requires a Criterion benchmark on streaming and execution hot paths.

**Alternatives considered**

- None. The budget is generous, and the risk lies in accidental buffering, which the
  per-event test covers.

---

## R18. Logging

**Decision**

- `tracing` with `tracing-subscriber` (env filter, default `info`) writes to stderr.
- Each request logs one line at completion. The line contains agent, provider,
  connection name, upstream model, status, TTFT, and duration. It never contains keys,
  bodies, or headers.
- `Secret` has no `Display`, so a key cannot be formatted into a log line by accident.
  The secrets test scans captured log output.

**Rationale**

- Operators need a live view. The observation store holds the detail.

**Alternatives considered**

- **`println!`.** It cannot be captured in tests and has no levels.
