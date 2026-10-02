---

description: "Task list for 003-request-execution"
---

# Tasks: Request Execution Walking Skeleton

**Input**: Design documents from `specs/003-request-execution/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included, for three reasons:
- Constitution VI requires parity tests against 9router.
- Every success criterion (SC-001 to SC-011) names a test corpus or a measured bound.
- The constitution's benchmark gate requires Criterion on streaming and execution hot
  paths.

Within each story, write the tests first and confirm they fail before implementing.

**Organization**: Tasks are grouped by user story, so each story can be implemented and
tested on its own.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US5)

## Path Conventions

Cargo workspace at the repo root ([plan § Project Structure](plan.md#project-structure)):

- `crates/nullrouter-server/`: the new library (`src/`, `tests/parity/`, `tests/e2e/`, `benches/`)
- `crates/nullrouter-registry/`: two additive changes only (T004)
- `crates/nullrouter-cli/`: new subcommands `serve`, `reload`, `obs`
- `tools/gen-bundled/`: Node generator (dev only)
- `tests/fixtures/9router/`: generated parity oracle, shared across crates (never hand-edit)

**Refinement to the plan's tree, so generator tasks can run in parallel**: the execution
phase of the generator lives in `tools/gen-bundled/execution/<fixture>.mjs`, one module
per fixture. `generate.mjs` only imports and runs them.

Every command needs `export CARGO_HOME=$PWD/.cargo-home`.

**Before starting**:
- The user must confirm or override R15 D1 (504 vs 9router's 502 for a connect timeout).
- The user must decide the Constitution V amendment.

Neither blocks Phases 1–2. T101 and T102 track them.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: A server crate skeleton, new dependencies, CLI stubs, and the generator's
ability to import 9router modules.

- [ ] T001 Add the new workspace dependencies to the root `Cargo.toml` `[workspace.dependencies]`, per [research R1](research.md#r1-crates-and-workspace-layout):
  - `tokio = { version = "1.53", features = ["rt-multi-thread","macros","net","time","sync","signal","io-util"] }`
  - `tokio-util = "0.7"`
  - `axum = { version = "0.8", default-features = false, features = ["http1","http2","tokio","json"] }`
  - `reqwest = { version = "0.13", default-features = false, features = ["rustls","stream","http2"] }`
  - `futures-util = "0.3"`, `bytes = "1"`, `memchr = "2"`, `sha2 = "0.11"`, `http = "1"`
  - `tracing = "0.1"`, `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`
  - add `"raw_value"` to the existing `serde_json` features
  - `nullrouter-server = { path = "crates/nullrouter-server" }`

  Keep `rust-version = "1.85"`. If a resolved version needs a newer MSRV, pin the last
  compatible version and note it in research R1.
- [ ] T002 Create `crates/nullrouter-server/Cargo.toml` and `src/lib.rs`.
  - Dependencies: `nullrouter-registry`, the crates from T001, `serde`, `toml`, `indexmap`, `arc-swap`, `thiserror`, `url`.
  - Dev-dependencies: `criterion`, `tempfile`, `tokio` (`test-util`).
  - `[[bench]] name = "relay"`, `harness = false`.
  - `[[test]] name = "parity"`, `path = "tests/parity/main.rs"`.
  - `[[test]] name = "e2e"`, `path = "tests/e2e/main.rs"`.
  - `lints.workspace = true`.
  - `lib.rs` declares the modules from the plan's tree, each as an empty file so the crate compiles: `state`, `keys` (`schema`, `load`, `secret`, `executable`), `auth`, `session`, `client_detect`, `select`, `transport`, `outbound`, `upstream`, `relay`, `assemble`, `usage`, `errors`, `count_tokens`, `embeddings`, `models`, `observe`, `http`, `operator`, `timeouts`.
- [ ] T003 [P] Add the three subcommands `serve`, `reload`, and `obs` to `crates/nullrouter-cli/src/main.rs`.
  - Each dispatches to an empty `src/cmd/{serve,reload,obs}.rs`.
  - Flags per [contracts/operator-cli.md](contracts/operator-cli.md).
  - Exit codes: 0 ok, 1 errors, 2 usage, 3 no server or bind error.
  - Add a `nullrouter-server` dependency to `crates/nullrouter-cli/Cargo.toml`.
  - Only `serve` starts a Tokio runtime; `check`, `validate`, `resolve`, `model`, `providers`, `reload`, and `obs` stay runtime-free.
- [ ] T004 [P] Registry, additive only ([R10](research.md#r10-reload-and-snapshots)):
  - Add `pub fn load_candidate(home: &OperatorHome) -> Result<Registry, ReloadError>` on `Registry` in `crates/nullrouter-registry/src/registry.rs`. It calls `load::build(home, Mode::Reload)` without swapping.
  - Derive `serde::Serialize` on `LoadReport` and its member types in `crates/nullrouter-registry/src/load.rs`.
  - Add one unit test showing that `load_candidate` on an empty home equals `RegistryHandle::open(...).snapshot()` in provider count.
- [ ] T005 [P] Create `tools/gen-bundled/resolve-hook.mjs` ([R14](research.md#r14-parity-oracle-extension)):
  - a Node `module.register` hook that maps `@/` to `ref/9router/src/`;
  - a fixed stub list that resolves `@/lib/usageDb.js`, logging, proxy, and DB modules to no-op modules exporting the names the importers use;
  - resolution of absent bare package imports to an empty stub.

  Register it at the top of `tools/gen-bundled/generate.mjs`. Add an `execution` phase that imports every `tools/gen-bundled/execution/*.mjs` and writes each result, with the ref-SHA header, to `tests/fixtures/9router/<name>.json`.
- [ ] T006 Run `cargo build --workspace && cargo clippy --workspace --all-targets` and `node tools/gen-bundled/generate.mjs`. Confirm that both pass and that the generator's existing outputs are byte-identical (`git diff --stat plugins/ tests/fixtures/` is empty).

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**:
- secrets, the keys file, the executable-provider rule, and access-key auth;
- timeouts and error envelopes;
- the atomic `State`;
- the observation store;
- the HTTP router with auth;
- `serve`.

Every story depends on these.

**⚠️ CRITICAL**: No user story work can start until this phase is complete.

### Oracle fixtures used by the foundation

- [ ] T007 [P] Write `tools/gen-bundled/execution/executable-providers.mjs`.
  - It emits `{chat: [...], embeddings: [...]}`, sorted, from the evaluated registry, `open-sse/executors/index.js`'s specialized executor map, and `OPENAI_COMPAT_PROVIDERS` in `open-sse/handlers/embeddingProviders/index.js`.
  - It applies the R2 rule on the JS side, so the Rust rule is checked against 9router's own tables.
  - Expected at the pin: 45 chat providers and 10 embeddings providers ([R2](research.md#r2-which-providers-are-executable-fr-013)).
- [ ] T008 [P] Write `tools/gen-bundled/execution/timeouts.mjs`.
  - It evaluates `envMs` from `open-sse/config/runtimeConfig.js` over `""`, `"0"`, `"-5"`, `"120000"`, `"120000abc"`, `" 42"`, `"abc"`, `"1e3"`, `"9007199254740993"` (set `process.env` per case and re-import with a cache-busting query).
  - It also emits the defaults `FETCH_CONNECT_TIMEOUT_MS = 60000` and `STREAM_STALL_TIMEOUT_MS = 360000`.
- [ ] T009 [P] Write `tools/gen-bundled/execution/upstream-errors.mjs`.
  - It runs `parseUpstreamError` → `formatProviderError` → `buildErrorBody`, the functions `createErrorResult` uses in `open-sse/utils/error.js`.
  - Corpus: status 400, 401, 402, 403, 404, 406, 408, 413, 422, 429, 500, 502, 503, 504, 529 × body `{"error":{"message":"m"}}`, `{"message":"m"}`, `{"error":"m"}`, `{"error":{"code":1}}`, plain text, HTML page, empty.
  - Output: `{status, body_in, client_body, headers}`.

### Secrets, timeouts, and error envelopes

- [ ] T010 [P] Implement `Secret` in `crates/nullrouter-server/src/keys/secret.rs` per [data-model § Secret](data-model.md#secret):
  - inner `Box<str>`;
  - `Debug` prints `Secret(***)`;
  - **no** `Display` and **no** `Serialize`;
  - `pub(crate) fn expose(&self) -> &str`;
  - add a unit test showing `format!("{:?}")` never contains the value.
- [ ] T011 [P] Implement `crates/nullrouter-server/src/timeouts.rs` ([R7](research.md#r7-timeouts-fr-017)):
  - `env_ms(raw: Option<&str>, default) -> u64` with JS `parseInt` semantics: optional leading whitespace and sign, then leading digits. The result is used only if finite and greater than 0.
  - `Timeouts { connect_default, stall_default }` read once at `serve` start.
  - `connect_for(&EffectiveTransport)`: transport `timeout_ms` first, else the env value, else 60 000.
  - `stall_for(&EffectiveTransport)`: transport `stall_timeout_ms` first, else the env value, else 360 000.
- [ ] T012 [P] Implement `crates/nullrouter-server/src/errors.rs`:
  - `ERROR_TYPES` (status → type and code) exactly as in 9router's `open-sse/config/errorConfig.js`;
  - `DEFAULT_ERROR_MESSAGES`;
  - `ZrError { status, error_type, message }`, with a renderer for each client format per [contracts/http-api.md § Error envelopes](contracts/http-api.md#error-envelopes). OpenAI clients get `{"error":{"message","type","code"}}`; Anthropic clients get `{"type":"error","error":{"type","message"}}`, with the type mapping in that table;
  - the response header `x-nullrouter-error`;
  - `upstream_error_body(status, body_bytes) -> Bytes`, porting `parseUpstreamError` + `formatProviderError` + `buildErrorBody` (the message is `[<status>]: <msg>`; non-strings are JSON-stringified; empty values fall back to the default message);
  - no function in this module takes a `Secret`.
- [ ] T013 Parity: create the harness `crates/nullrouter-server/tests/parity/main.rs`. It holds a fixture loader that checks the ref-SHA header matches `plugins/bundled` and fails with a regenerate hint otherwise. Add these modules:
  - `parity/timeouts.rs`, checking T011 against `timeouts.json`;
  - `parity/upstream_errors.rs`, checking T012 against `upstream-errors.json`: `client_body` byte-for-byte after JSON normalization, plus `Content-Type` and `Access-Control-Allow-Origin` headers (SC-008, not native).

### Keys file and executable rule

- [ ] T014 Implement the executable rule in `crates/nullrouter-server/src/keys/executable.rs` ([R2](research.md#r2-which-providers-are-executable-fr-013)):
  - a static `SPECIALIZED: &[&str]` holding the 30 canonical ids listed in R2. Aliases are resolved by the registry first; note that `mimo-free` has no `mmf` alias in 0router;
  - `fn chat_executable(&ProviderEntity) -> Result<(), NotExecutable>` and `fn embeddings_executable(...)`;
  - `NotExecutable` reasons: `oauth`, `no-auth free provider`, `web-cookie`, `specialized executor`, `no openai or claude transport`;
  - parity module `tests/parity/executable.rs` compares both sets against `executable-providers.json`.
- [ ] T015 Implement the `keys.toml` schema in `crates/nullrouter-server/src/keys/schema.rs` per [contracts/keys-file.md](contracts/keys-file.md):
  - `deny_unknown_fields` everywhere;
  - `schema = 1` is required;
  - `SecretSource = Literal(String) | Env { env: String }`, and the `{ env }` table accepts only `env`;
  - `[[access_key]] { agent, key, active = true }`;
  - `[[connection]] { provider, name, api_key, active = true, account_id? }`.
- [ ] T016 Implement the loader in `crates/nullrouter-server/src/keys/load.rs`: `fn load(home, &Registry) -> Result<Keys, Vec<KeysError>>`, collecting **all** errors in the 002 `file:line:col path: rule` form, using `toml::de::DeTable` spans as `nullrouter-registry/src/load.rs` does. Rules:
  - A missing file gives an empty `Keys` plus a warning flag.
  - FR-007: if any secret is a literal and `mode & 0o077 != 0`, fail with `keys.toml: holds literal secrets but is readable by group/others (mode 0NNN); chmod 600 or use { env = "..." }`.
  - `agent` must match `[A-Za-z0-9._-]{1,64}` and be unique.
  - An access `key` must be at least 16 characters; resolved values must be unique across access keys.
  - Env references must be set and non-empty.
  - `provider` must resolve by id or alias to a provider that passes `chat_executable` or `embeddings_executable`, otherwise fail with `provider "<id>" is not executable in this slice (<reason>)`.
  - `name` must be unique per provider.
  - `account_id` is required when any transport URL of the provider contains `{accountId}`; it must be non-empty with no `/ ? #` or whitespace.
  - **No error string may contain a secret value.**

  Build `Keys { access_keys, access_by_digest: HashMap<[u8;32], usize>, connections: Vec<Arc<Connection>>, by_provider }` per [data-model § Keys](data-model.md#keys-snapshot).
- [ ] T017 [P] Unit tests in `crates/nullrouter-server/src/keys/load.rs` (`#[cfg(test)]`, using `tempfile`), one for each rule in T016 and each example error line in [contracts/keys-file.md § Errors](contracts/keys-file.md#errors). Also assert that no error `Display` output contains any of the literal or env secret values used.

### State, auth, and observations

- [ ] T018 Implement `crates/nullrouter-server/src/state.rs` ([R10](research.md#r10-reload-and-snapshots)):
  - `State { registry: Arc<Registry>, keys: Arc<Keys>, generation: u64 }`;
  - `Server { state: ArcSwap<State>, reload_lock: tokio::sync::Mutex<()>, … }`;
  - `async fn startup(home)`: run `Registry::load_candidate` then `keys::load` inside `spawn_blocking`. Any error is fatal: return it;
  - `async fn reload()`: the same two builds under `reload_lock`. On success, `store` generation+1 and return `(LoadReport, KeysSummary)`; on any error, keep the old state and return all errors.
- [ ] T019 [P] Implement `crates/nullrouter-server/src/auth.rs` ([R5](research.md#r5-access-keys-and-agent-identity)):
  - `extract(headers) -> Option<&str>`: `Authorization: Bearer <key>` (case-sensitive `Bearer `) first, then `x-api-key`;
  - `authenticate(&Keys, headers) -> Result<AgentName, AuthError>` via SHA-256 digest lookup;
  - `MissingKey` → 401 `missing_access_key` "Missing API key"; unknown or inactive → 401 `invalid_access_key` "Invalid API key";
  - add unit tests.
- [ ] T020 [P] Implement `crates/nullrouter-server/src/observe.rs` per [data-model § Observation](data-model.md#observation):
  - `Observation` (`Serialize`, every field listed there), `Outcome` (every variant in the table), and `Usage { input, output, cache_read, cache_write: Option<u64> }` (`None` serializes as `null`);
  - `ObservationBuilder` with a `finish(self, …)` that records exactly once, and a `Drop` impl that records `cancelled_by_client` when `finish` never ran;
  - `ObservationStore { Mutex<VecDeque>, cap }`: "`push` pops the front when full", and the cap is at least 1;
  - upstream-header capture drops `set-cookie` and hop-by-hop headers;
  - raw error body: "the first 8 KiB, lossy UTF-8";
  - unit tests for the cap, exactly-once recording, and header filtering.
- [ ] T021 Implement the router skeleton in `crates/nullrouter-server/src/http.rs`:
  - the routes from [contracts/http-api.md § Endpoints](contracts/http-api.md#endpoints), each returning 501 for now;
  - unknown paths → 404, wrong method → 405, both in the OpenAI error shape;
  - a request-scoped prelude that runs, in order: load `Arc<State>` once, start the `ObservationBuilder`, authenticate (rejection order row 1; record `rejected{error_type}` without an agent), enforce the 32 MiB body limit (413 `request_too_large`), and parse the top level as `IndexMap<String, Box<RawValue>>` (400 `invalid_json` "Invalid JSON body"; a non-object top level is also `invalid_json`), and read `model` (400 `missing_model` "Missing model");
  - the client format is fixed per endpoint: `openai` for chat and embeddings, `claude` for messages and count_tokens;
  - error bodies are rendered by T012 in the client's format;
  - **the operator channel is never mounted here** (FR-028).
- [ ] T022 Implement `nullrouter-cli serve` in `crates/nullrouter-cli/src/cmd/serve.rs`:
  - resolve `--listen`/`NULLROUTER_LISTEN` (default `127.0.0.1:20129`), `--observations-cap`/`NULLROUTER_OBSERVATIONS_CAP` (default 10 000), and `NULLROUTER_HOME`;
  - install `tracing-subscriber` with an env filter, default `info`, writing to stderr;
  - call `Server::startup` (exit 1 on errors, printing each);
  - bind (exit 3 on failure) and print the one-line banner from [contracts/operator-cli.md](contracts/operator-cli.md#nullrouter-cli-serve);
  - warn when `keys.toml` is absent;
  - on SIGINT or SIGTERM, shut down gracefully with a 10 s drain.
- [ ] T023 Create the e2e harness in `crates/nullrouter-server/tests/e2e/main.rs` and `tests/e2e/mock.rs`:
  - an in-process `Server` on `127.0.0.1:0` with a temp `NULLROUTER_HOME`;
  - a user plugin set that points chosen bundled provider ids at the mock via `plugin_decisions = "replace"` copies whose `base_url` is rewritten to the mock address. Other fields stay as bundled, so the headers stay 9router-faithful;
  - a scripted `axum` mock upstream that records every request (method, URL, headers, body bytes) and replays a script: status, headers, and a body as a sequence of `(delay, bytes)` chunks, plus `stall` and `break` steps;
  - it exposes "connection closed at" timestamps;
  - helpers for writing `keys.toml` with sentinel secrets `nr-sentinel-access-…` and `nr-sentinel-provider-…`.
- [ ] T024 E2E foundation tests in `crates/nullrouter-server/tests/e2e/foundation.rs`:
  - no key → 401 "Missing API key";
  - a wrong key or an inactive key → 401 "Invalid API key";
  - invalid JSON → 400;
  - no model → 400;
  - more than 32 MiB → 413;
  - each rejection in both client formats (the OpenAI and Anthropic envelopes);
  - **the mock received zero requests** (SC-009);
  - every rejection has exactly one observation with outcome `rejected`, and auth rejections have no agent.
- [ ] T025 Run `cargo test -p nullrouter-server && cargo clippy --workspace --all-targets`. Everything must be green.

**Checkpoint**: The server starts, authenticates, rejects malformed requests in both
client formats, records observations, and loads `keys.toml` with every validation rule.
User stories can start.

---

## Phase 3: User Story 1: A client chats with a provider through 0router (Priority: P1) 🎯 MVP

**Goal**: For a direct `<provider>/<model>` target on an executable provider:
- the outbound URL, headers, and body match 9router's generic executor;
- streamed responses are relayed event by event and byte for byte;
- non-streamed and forced-stream responses are returned correctly;
- upstream errors follow FR-020 and FR-020b;
- client disconnects cancel the upstream;
- native-pair (Claude Code → `anthropic`) headers are forwarded;
- count_tokens is answered.

**Independent Test**: `cargo test -p nullrouter-server --test parity` for the executor,
transport, client-detect, non-sse, stream-errors, sse-to-json, and count-tokens fixtures,
plus `cargo test -p nullrouter-server --test e2e us1 cancel`. Quickstart step 4 is the
live check.

### Oracle fixtures for User Story 1

- [ ] T026 [P] [US1] Write `tools/gen-bundled/execution/executor-requests.mjs`. For every chat-executable provider × each of its transports (primary + `transports[]`) × `stream ∈ {true,false}`:
  - construct `new DefaultExecutor(providerId)` from `open-sse/executors/default.js`;
  - call `buildUrl(model, stream, 0, creds)` and `buildHeaders(creds, stream)` with `creds = { apiKey: "<KEY>", providerSpecificData: { accountId: "<ACCT>" }, runtimeTransport }`, where `runtimeTransport` is `null` for the primary transport and the entry for a `transports[]` entry;
  - emit `{provider, transport_index, stream, url, headers (object), wire_headers: [...new Headers(headers)]}`.

  The `wire_headers` field captures the minimax `Anthropic-Version` + `anthropic-version` merge ([R6](research.md#r6-outbound-url-and-headers-fr-015-fr-015a)).
- [ ] T027 [P] [US1] Write `tools/gen-bundled/execution/transport-choice.mjs`. It evaluates chatCore's choice (`resolveTransport` in `open-sse/services/provider.js`, the `useTransport` rule, and the `targetFormat` fallback chain, extracted as in `open-sse/handlers/chatCore.js`) over every chat-executable provider × client format `{openai, claude}` × model variants `{none, supported_formats excluding the client format, target_format set}`. Output: `{provider, client_format, model, target_format, transport_index (-1 = primary)}`.
- [ ] T028 [P] [US1] Write `tools/gen-bundled/execution/client-detect.mjs`.
  - It runs `detectClientTool(headers, body)` and `isNativePassthrough(tool, provider)` from `open-sse/utils/clientDetector.js`.
  - Corpus: Claude Code UAs (`claude-cli/2.x`, `claude-code`), `x-app: cli`, codex UAs and `originator: codex_…`, `gemini-cli`, the copilot headers, `deepseek-tui`, a body `userAgent` antigravity, and plain curl.
  - Providers: `anthropic`, `minimax`, `deepseek`, `anthropic-compatible-x`.
- [ ] T029 [P] [US1] Write `tools/gen-bundled/execution/non-sse.mjs`, evaluating the non-SSE branch of `open-sse/handlers/chatCore/streamingHandler.js`.
  - Extract the helper by marker comments if it is not exported; see [R14](research.md#r14-parity-oracle-extension).
  - Corpus: content-type `text/html`, `text/plain`, missing, `application/json`, and `text/event-stream` × bodies with and without `<title>` (including a title containing tags and CR/LF, and a title longer than 160 characters), a short body under 200 characters, and a long body.
  - Output: `{status, content_type, body_in, handled, client_body}`.
- [ ] T030 [P] [US1] Write `tools/gen-bundled/execution/stream-errors.mjs`. It calls `buildStreamErrorBytes(504, message, fmt)` from `open-sse/utils/streamHelpers.js` for `fmt ∈ {claude, openai}` and three messages, emitting the exact bytes as a UTF-8 string.
- [ ] T031 [P] [US1] Write `tools/gen-bundled/execution/sse-to-json.mjs`. It calls `parseSSEToOpenAIResponse(rawSSE, fallbackModel)` from `open-sse/handlers/chatCore/sseToJsonHandler.js`, then applies the handler's `reasoning_content` strip.
  - Corpus: content only; content plus reasoning; reasoning only; tool-call deltas across chunks with indexes 0 and 1; `finish_reason` variants; usage in the last chunk; an in-band error chunk with `status` 403 and without a status; no chunks; malformed lines.
  - Every chunk carries `id` and `created`, so the output is deterministic.
- [ ] T032 [P] [US1] Write `tools/gen-bundled/execution/count-tokens.mjs`. It calls `estimateAnthropicInputTokens` from `src/app/api/v1/messages/count_tokens/route.js` over bodies containing: a string system; system blocks; tools with nested schemas; messages with text, `tool_use` (name and input), `tool_result` (string and blocks), thinking, image blocks, and unknown block types; nested objects (keys count); numbers; booleans; null; and an empty body (SC-011).
- [ ] T033 [US1] Run `node tools/gen-bundled/generate.mjs`. Check the seven new fixtures are written with the ref-SHA header, then spot-check three things by eye:
  - minimax's merged `anthropic-version`;
  - cloudflare-ai's `<ACCT>` in the URL;
  - anthropic's `Anthropic-Beta`.

### Tests for User Story 1 ⚠️ write first, confirm they fail

- [ ] T034 [P] [US1] Write `crates/nullrouter-server/tests/parity/executor_requests.rs`. For every fixture row, build the `EffectiveTransport` and `OutboundRequest` with a `Connection { api_key: "<KEY>", account_id: Some("<ACCT>") }`, not a native pair, and compare:
  - the URL, exactly;
  - the lowercase-merged header view against `wire_headers`, exactly (SC-001);
  - that `<KEY>` appears only in the auth header.
- [ ] T035 [P] [US1] Write `crates/nullrouter-server/tests/parity/transport_choice.rs` (FR-014) and `tests/parity/client_detect.rs` (FR-015a detection) against their fixtures.
- [ ] T036 [P] [US1] Write `crates/nullrouter-server/tests/parity/non_sse.rs` (FR-020b), `tests/parity/stream_errors.rs` (byte-exact, FR-018a), `tests/parity/sse_to_json.rs` (JSON-equal, FR-019), and `tests/parity/count_tokens.rs` (SC-011) against their fixtures.
- [ ] T037 [P] [US1] Write unit tests for the body rewrite in `crates/nullrouter-server/src/outbound.rs` ([R8](research.md#r8-outbound-body)):
  - only `model` is replaced;
  - `stream: true` is added or overwritten only for forced streaming;
  - nested bytes are kept exactly: `1.0` stays `1.0`, `"é"` stays escaped, and key order and duplicate nested whitespace are unchanged;
  - top-level key order is preserved.
- [ ] T038 [P] [US1] Write unit tests for the native-pair header overlay in `crates/nullrouter-server/src/outbound.rs`, per the formula in [contracts/outbound-parity.md § Headers (native pair)](contracts/outbound-parity.md#headers-native-pair):
  - client `anthropic-beta` and `anthropic-version` replace the declared values;
  - `authorization`, `x-api-key`, `host`, `content-length`, `accept-encoding`, the hop-by-hop headers, and names listed in `Connection` are dropped;
  - the connection's `x-api-key` is re-applied last.
- [ ] T039 [US1] Write e2e tests in `crates/nullrouter-server/tests/e2e/us1.rs`, one per US1 acceptance scenario 1–8, using the mock:
  - **(1)** OpenAI streaming to `deepseek/<model>`: the mock receives the URL and headers from T026's row, the key, and the upstream id. The client receives each event before the mock sends the next (the mock waits 200 ms between events; assert arrival timestamps).
  - **(2)** Anthropic to `anthropic/<model>`, not native: the declared `anthropic-version` and `Anthropic-Beta` are sent.
  - **(3)** Non-streaming: status and body are returned unchanged.
  - **(4)** `openai` (`force_stream`) with `stream:false`: the outbound body has `"stream":true`, and the client gets the assembled JSON.
  - **(5)** Covered in T040.
  - **(6)** A 429 with a JSON body: not native → the 9router-formatted body and only 0router's headers; native → the upstream body byte-identical plus `retry-after` and the rate-limit headers. No second request reaches the mock.
  - **(7)** The mock-received body equals the client body with only `model` changed (byte compare after the model substitution).
  - **(8)** Claude Code UA to `anthropic` forwards the client's `anthropic-beta` and `anthropic-version`. The same request to `minimax`'s claude transport sends only the declared headers.

  Also cover the edge cases:
  - a format mismatch (`/v1/messages` to an openai-only provider → 400 `format_mismatch`, Anthropic envelope, mock untouched);
  - a specialized-executor provider (`azure`) → 400 `provider_not_supported`;
  - an uncatalogued model with `allow_uncatalogued_models` on → forwarded with the id unchanged;
  - a non-SSE HTML page on a streaming request (FR-020b), both native and not;
  - a connect timeout with `FETCH_CONNECT_TIMEOUT_MS=200` → 504 `upstream_timeout` (per R15 D1);
  - an unreachable upstream (a closed port) → 502 `upstream_unreachable`, with a message that never contains the key;
  - a mid-stream break → the bytes so far, then exactly one closing frame equal to T030's bytes, for both client formats;
  - a stall with `STREAM_STALL_TIMEOUT_MS=300` → the closing frame, and the mock sees the connection closed;
  - count_tokens: not native → `{"input_tokens":N}` and the mock untouched; native to `anthropic` → forwarded to `…/v1/messages/count_tokens`; invalid JSON → 400.
- [ ] T040 [P] [US1] Write `crates/nullrouter-server/tests/e2e/cancel.rs` (SC-004, FR-021): 50 runs of starting a stream, reading two events, and dropping the client. Assert that the mock observes the connection closed within 1 s in 50 of 50 runs, and that each run's observation is `cancelled_by_client` with its TTFT set.

### Implementation for User Story 1

- [ ] T041 [US1] Implement `crates/nullrouter-server/src/transport.rs`: `EffectiveTransport` per [data-model § EffectiveTransport](data-model.md#effectivetransport), and `choose(provider, model_info, client_format) -> Result<EffectiveTransport, ZrError>` porting [R3](research.md#r3-transport-and-target-format-choice-fr-014):
  - "the first entry in `transports[]` whose `format` equals the client format";
  - the `supported_formats` gate;
  - the fallback chain for `targetFormat`;
  - a mismatch gives 400 `format_mismatch` with the message from [contracts/http-api.md](contracts/http-api.md#selection-and-rejection-order).
- [ ] T042 [US1] Implement `crates/nullrouter-server/src/client_detect.rs`: `ClientTool` and `detect(headers, body) -> Option<ClientTool>` in 9router's order ([R6](research.md#r6-outbound-url-and-headers-fr-015-fr-015a)); `NATIVE_PAIRS`; and `is_native_pair(tool, provider_id)`, which normalizes `anthropic-compatible*` to `anthropic`.
- [ ] T043 [US1] Implement `crates/nullrouter-server/src/outbound.rs` per [contracts/outbound-parity.md](contracts/outbound-parity.md):
  - the URL (base, suffix, `{accountId}`);
  - the ordered header build (Content-Type → declared headers → auth descriptor or format fallback → `anthropic-version` only when the exact lowercase key is absent, **keeping** the separately-cased declared header so the wire value merges → `Accept: text/event-stream` when streaming);
  - the native-pair overlay;
  - the body rewrite on `IndexMap<String, Box<RawValue>>`;
  - the auth `HeaderValue` is `set_sensitive(true)`, and `Secret::expose` is called only here.

  Output is `OutboundRequest` per [data-model](data-model.md#outboundrequest).
- [ ] T044 [US1] Implement the direct-target half of `crates/nullrouter-server/src/select.rs`: `select(&State, &ClientRequest) -> Result<ExecutionTarget, ZrError>`. It is pure: no I/O and no clock. It follows the rejection order rows 6–10 of [contracts/http-api.md](contracts/http-api.md#selection-and-rejection-order):
  - registry `resolve` → 404 `target_not_found`;
  - `chat_executable` → 400 `provider_not_supported`;
  - the first active connection in declaration order → 404 `no_usable_connection` "No active credentials for provider: <provider>";
  - `transport::choose`.

  Return `Rejection` for unified targets for now (US2 fills it in).
- [ ] T045 [US1] Implement `crates/nullrouter-server/src/upstream.rs`:
  - a shared `reqwest::Client` (rustls, pooled, no automatic decompression beyond reqwest's defaults, no redirects);
  - `send(OutboundRequest, &CancellationToken) -> Result<reqwest::Response, UpstreamFailure>`, which `select!`s `tokio::time::timeout(connect, send())` against `token.cancelled()`;
  - `UpstreamFailure::{Timeout, Unreachable(kind: dns|connect|tls|io), Cancelled}` mapped to 504 `upstream_timeout` / 502 `upstream_unreachable` per [contracts/http-api.md](contracts/http-api.md#error-envelopes). Messages name the provider, never the URL query or the key.
- [ ] T046 [US1] Implement `crates/nullrouter-server/src/relay.rs` ([R9](research.md#r9-relay-cancellation-and-the-sse-question-fr-018-fr-018a-fr-021)):
  - an incremental SSE event framer over `Bytes` using `memchr` (`\n\n`, `\r\n\r\n`, `\r\r`) that yields each complete event's **original bytes** zero-copy as soon as it completes, and flushes a trailing partial event at end of stream;
  - a stall `timeout` per upstream `next()`;
  - a `CancelOnDrop` guard that cancels the request's `CancellationToken` and lets the `ObservationBuilder` drop record `cancelled_by_client`;
  - on a read error or stall, emit exactly one closing frame (`buildStreamErrorBytes` port, 504) in the client format, then end; outcome `incomplete_stream` or `stream_stalled`;
  - a side channel: TTFT at the first complete event, plus an optional per-event callback for usage (a no-op until US3);
  - the response is `axum::body::Body::from_stream(relay)`, **not** `Sse`;
  - headers: non-native → the `SSE_HEADERS_CORS` equivalent (`Connection: keep-alive` on HTTP/1.1 only); native → the upstream headers minus `set-cookie`, `content-length`, and hop-by-hop headers.
- [ ] T047 [US1] Implement `crates/nullrouter-server/src/assemble.rs`: port `parseSSEToOpenAIResponse` plus the `reasoning_content` strip (FR-019):
  - an in-band error → its `status` if within 400–599, else 502, via `errors::upstream_error_body`;
  - no chunks → 502 "Invalid SSE response for non-streaming request";
  - the upstream is not `text/event-stream` → return `None`, and the caller relays as non-streaming (the 9router `null` path);
  - reads use the stall timeout.
- [ ] T048 [US1] Implement the non-streaming and error response paths in `crates/nullrouter-server/src/http.rs`:
  - **non-2xx upstream**: native → status, filtered headers, and the body unchanged; otherwise → `errors::upstream_error_body`, `Content-Type: application/json`, and `Access-Control-Allow-Origin: *`;
  - **2xx non-SSE on a streaming request (FR-020b)**: native → unchanged; otherwise → the ported non-SSE short error (`{"error":{"message":"[<status>]: <short>"}}`);
  - the observation keeps "the first 8 KiB" of every upstream error body (FR-020c) and all upstream headers except `set-cookie` and hop-by-hop headers (FR-022);
  - **2xx JSON non-streaming**: relay the body as a stream (no full buffering), with a stall timeout per chunk ([R7](research.md#r7-timeouts-fr-017)); TTFT is when the body is fully received.
- [ ] T049 [US1] Implement `crates/nullrouter-server/src/count_tokens.rs`:
  - port `estimateAnthropicInputTokens` (`countValueChars` / `countMessageChars`, `ceil(chars/4)`);
  - the handler: if Claude Code → `anthropic` is a native pair, build the outbound to `<messages base_url>/count_tokens` with the native header overlay and the model rewrite, and relay as non-streaming (no fallback on failure). Otherwise answer `200 {"input_tokens":N}` with JSON plus CORS headers, without resolving the target;
  - the observation outcome is `estimated_locally` when answered locally.
- [ ] T050 [US1] Wire `/v1/chat/completions` and `/v1/messages` in `crates/nullrouter-server/src/http.rs`, end to end:
  1. prelude (T021) → `select` → detect client and native pair;
  2. `stream_mode` per [data-model § ClientRequest](data-model.md#clientrequest), using 9router's `body.stream != false` default and the Accept rule;
  3. `outbound::build` → `upstream::send` → branch to relay (T046), assemble (T047), or non-streaming/error (T048).

  One `ObservationBuilder` spans the request, and the relay stream holds the `Arc<State>` until the end. Log one `tracing` line at completion (agent, provider, connection name, upstream model, status, TTFT, duration), never keys, bodies, or headers.
- [ ] T051 [US1] Write the Criterion bench `crates/nullrouter-server/benches/relay.rs` (SC-003, [R17](research.md#r17-performance)):
  - (a) the event framer over a 200-event recorded stream;
  - (b) `select` + `outbound::build` for `anthropic` and `minimax`;
  - (c) end-to-end TTFT through 0router against the in-process mock, compared with direct to the mock (median delta under 5 ms).

  Save the baseline with `--save-baseline slice-003`.
- [ ] T052 [US1] Run `cargo test -p nullrouter-server --test parity --test e2e` and `cargo bench -p nullrouter-server --bench relay`. All US1 tests must be green, and the TTFT delta median under 5 ms.

**Checkpoint**: A direct-target chat request works end to end in both formats, streamed,
non-streamed, and forced-stream, with 9router-faithful outbound requests and
cancellation. MVP.

---

## Phase 4: User Story 2: A client uses a unified model and discovers what is available (Priority: P1)

**Goal**: A bare unified-model target runs on its first member with an active connection.
`/v1/models` lists every usable target.

**Independent Test**: `cargo test -p nullrouter-server --test e2e us2`. Quickstart step 3
(the listing).

### Tests for User Story 2 ⚠️ write first, confirm they fail

- [ ] T053 [P] [US2] Unit tests for the unified half of `crates/nullrouter-server/src/select.rs`:
  - first member active → that member and its upstream id;
  - first member without a connection → the second member;
  - a first member whose provider is not executable is skipped as "no usable connection";
  - no member usable → 404 `no_usable_connection` "No active credentials for any member of unified model: <name>";
  - the selected member's format mismatches → 400 `format_mismatch`, and it does **not** move to the next member;
  - a bare name that is not unified → 404 `target_not_found` (002 FR-014a).
- [ ] T054 [P] [US2] Write e2e tests in `crates/nullrouter-server/tests/e2e/us2.rs` for acceptance scenarios 1–6. Two mocks back two providers of a two-member unified model declared in `config.toml`. Assert which mock received the request, and that `/v1/models` (scenario 5) contains exactly the expected ids and `kind` values, with no connection names and no keys.

### Implementation for User Story 2

- [ ] T055 [US2] Complete `crates/nullrouter-server/src/select.rs` for `Resolution::Unified`. "The member MUST be the first member, in declaration order, whose provider has at least one active connection" (FR-011); the member must also pass `chat_executable`. Then apply the transport choice once, with no fallback (FR-012). Set `ExecutionTarget.unified = Some((name, member_index))`, and record the unified name in the observation.
- [ ] T056 [US2] Implement `crates/nullrouter-server/src/models.rs` and route `GET /v1/models` per [contracts/http-api.md § Model listing](contracts/http-api.md#model-listing-fr-029):
  - unified models first, in declaration order, if at least one member has an active connection;
  - then the catalogued models of every provider with an active connection, as `<provider>/<model>`, in provider-id then catalog order;
  - `kind` is omitted when undeclared;
  - `owned_by` is `nullrouter` for unified models, else the provider id;
  - auth is required.
- [ ] T057 [US2] Run `cargo test -p nullrouter-server`. US1 and US2 must be green.

**Checkpoint**: Unified models execute end to end through the placeholder selection,
isolated in `select.rs`.

---

## Phase 5: User Story 3: The operator sees latency and cache usage per request (Priority: P1)

**Goal**:
- Token usage, including cache read and write, is extracted from streamed and
  non-streamed responses.
- Observations are queryable through the CLI with p50/p95 summaries, over an owner-only
  operator channel.

**Independent Test**: `cargo test -p nullrouter-server --test parity usage` and
`--test e2e us3`. Quickstart step 5.

### Oracle fixtures for User Story 3

- [ ] T058 [P] [US3] Write `tools/gen-bundled/execution/usage.mjs`. It feeds event sequences through `extractUsage` + `mergeUsage` from `open-sse/utils/usageTracking.js`, keeping the raw per-field presence. Sequences:
  - **Anthropic**: `message_start` with and without `cache_read_input_tokens` / `cache_creation_input_tokens`, then `message_delta` with `output_tokens`.
  - **Anthropic non-streaming** `usage`.
  - **OpenAI**: a final chunk with `usage.prompt_tokens_details.cached_tokens`; DeepSeek `prompt_cache_hit_tokens`; no usage at all.
  - **OpenAI non-streaming** `usage`.
  - **Embeddings** `usage.prompt_tokens`.

  Output: the merged usage mapped to `{input, output, cache_read, cache_write}`, using `null` where the field was absent in every event ([R11](research.md#r11-usage-extraction-fr-023)).

### Tests for User Story 3 ⚠️ write first, confirm they fail

- [ ] T059 [P] [US3] Write `crates/nullrouter-server/tests/parity/usage.rs` against `usage.json` (SC-005: 100% of counts match, 100% of absent fields are `None`, never 0).
- [ ] T060 [P] [US3] Unit tests for the percentiles and filters in `crates/nullrouter-server/src/observe.rs`:
  - nearest-rank p50/p95 over 1, 2, 20, and 101 values;
  - records without a TTFT are excluded from TTFT percentiles;
  - count_tokens records are excluded from latency summaries unless `include_count_tokens` is set;
  - each filter (`provider` by id or alias, `unified`, `agent`, `session`, `endpoint`, `since`, `until`);
  - `limit` applies to the list, never to the summary.
- [ ] T061 [P] [US3] Write e2e tests in `crates/nullrouter-server/tests/e2e/us3.rs` for acceptance scenarios 1–5:
  - the mocks report fixed usage (with and without cache fields) and delay their first event by 100 ms and 300 ms;
  - query through the operator socket and assert counts, statuses, token figures, and TTFT within ±50 ms;
  - failed, rejected, and cancelled requests have their outcome and `None` tokens;
  - an OpenAI stream without `stream_options.include_usage` records `None` tokens ([R11](research.md#r11-usage-extraction-fr-023)).
- [ ] T062 [P] [US3] Write the operator-channel test in `crates/nullrouter-server/tests/e2e/operator.rs`:
  - the socket file is mode 0600 and its directory is 0700;
  - the `status` and `observations` ops follow [contracts/operator-cli.md § protocol](contracts/operator-cli.md#operator-channel-protocol);
  - no path on the TCP listener reaches the operator ops (FR-028): try `/reload`, `/observations`, `/_operator`, and the op JSON posted to every client route;
  - a stale socket file is replaced at start.

### Implementation for User Story 3

- [ ] T063 [US3] Implement `crates/nullrouter-server/src/usage.rs`: port `extractUsage` (the Anthropic `message_start` and `message_delta` branches and the OpenAI `usage.prompt_tokens` branch, including `prompt_cache_hit_tokens`) and `mergeUsage` (per-field max, finite numbers only), keeping per-field presence. Map the result to `observe::Usage` per the [R11 table](research.md#r11-usage-extraction-fr-023). Parse from borrowed `data:` slices, with no `serde_json::Value` on the relay path.
- [ ] T064 [US3] Hook usage into the three response paths:
  - the relay side channel in `crates/nullrouter-server/src/relay.rs`: parse only events that can carry usage, and stop parsing once the stream's format rules say no more can come;
  - `assemble.rs`: the assembled `usage`;
  - the non-streaming path in `http.rs`: incrementally scan the relayed body copy, capped at 8 MiB, for the top-level `usage` object.

  Recording stays after the final write (FR-024).
- [ ] T065 [US3] Implement query and summary in `crates/nullrouter-server/src/observe.rs`: `ObservationFilter` and `ObservationSummary` per [data-model](data-model.md#observationfilter-and-observationsummary). Clone under the lock, then filter and sort outside it. Include the token sums with `reported` counts.
- [ ] T066 [US3] Implement `crates/nullrouter-server/src/operator.rs`:
  - a Tokio `UnixListener` at `$NULLROUTER_HOME/run/operator.sock`; create the directory at 0700 and set the socket to 0600;
  - at start, remove an existing socket file only if connecting to it fails; otherwise exit 3 "another server owns the socket";
  - peer-uid check via `UnixStream::peer_cred()` against the server's uid, closing on mismatch;
  - NDJSON, one request per connection;
  - ops `status` and `observations` (`reload` comes in US5);
  - start it from `serve` (T022).
- [ ] T067 [US3] Implement `nullrouter-cli obs` in `crates/nullrouter-cli/src/cmd/obs.rs`:
  - a blocking `std::os::unix::net::UnixStream` client;
  - parse `--since`/`--until` as RFC 3339 or relative (`15m`, `2h`, `1d`);
  - render the summary line, the tokens line, and the table exactly as in [contracts/operator-cli.md § obs](contracts/operator-cli.md#nullrouter-cli-obs), with `—` for "not reported";
  - `--json` prints the raw response;
  - exit 1 for an unknown provider or unified filter; exit 3 when no server is running.
- [ ] T068 [US3] Run `cargo test -p nullrouter-server && cargo test -p nullrouter-cli`. Everything must be green.

**Checkpoint**: Every request's latency and cache usage are visible per provider and per
unified model through the CLI.

---

## Phase 6: User Story 4: A client requests embeddings (Priority: P2)

**Goal**: `/v1/embeddings` executes on the OpenAI-compatible embeddings providers exactly
as 9router's adapter builds requests.

**Independent Test**: `cargo test -p nullrouter-server --test parity embeddings` and
`--test e2e us4`.

### Oracle fixtures for User Story 4

- [ ] T069 [P] [US4] Write `tools/gen-bundled/execution/embeddings.mjs`. For each embeddings-executable provider, call `createOpenAIEmbeddingAdapter(id)` from `open-sse/handlers/embeddingProviders/openai.js`, then its `buildUrl`, `buildHeaders({apiKey:"<KEY>"})`, and `buildBody` over inputs:
  - a string input;
  - an array input;
  - `encoding_format: "base64"`;
  - `dimensions` values `0`, `-1`, `"8"`, `1024`, and `NaN`;
  - an extra unknown field.

  Output: `{provider, input, url, headers, body}`.

### Tests for User Story 4 ⚠️ write first, confirm they fail

- [ ] T070 [P] [US4] Write `crates/nullrouter-server/tests/parity/embeddings.rs` against `embeddings.json`: the URL, the lowercase header view, and the body, all exact.
- [ ] T071 [P] [US4] Write e2e tests in `crates/nullrouter-server/tests/e2e/us4.rs` for acceptance scenarios 1–3:
  - an embeddings request to `openai/text-embedding-3-small` through the mock;
  - a chat model or a non-embeddings provider → 400 `not_embeddings`;
  - the observation has a duration and `usage.input` from `usage.prompt_tokens`;
  - a unified model with an embedding member works.

### Implementation for User Story 4

- [ ] T072 [US4] Implement `crates/nullrouter-server/src/embeddings.rs`:
  - the URL is `capabilities.embedding.endpoint.base_url`;
  - the headers are `Content-Type: application/json`, then `Authorization: Bearer <key>`, then the declared endpoint headers;
  - the body is `{model, input, encoding_format?, dimensions?}`, where `dimensions` is kept only when "finite, > 0" as a Number;
  - the check is: the provider passes `embeddings_executable`, and the model `kind == embedding`, or the model is uncatalogued on an allow-uncatalogued provider. Otherwise 400 `not_embeddings`.

  If the 002 registry does not expose `capabilities.embedding.endpoint` publicly, add a read-only accessor in `crates/nullrouter-registry/src/registry.rs` (additive).
- [ ] T073 [US4] Route `POST /v1/embeddings` in `crates/nullrouter-server/src/http.rs`: prelude → `select`, with the embeddings variant of row 8 → build → `upstream::send` with the connect timeout → the non-streaming path (T048) → usage (T064).
- [ ] T074 [US4] Run `cargo test -p nullrouter-server`. Everything must be green.

**Checkpoint**: A second model type executes through the same path.

---

## Phase 7: User Story 5: The operator manages accounts and agent keys without leaking them (Priority: P2)

**Goal**:
- multiple accounts per provider with active flags;
- live reload of `keys.toml` with the registry;
- per-agent and per-session identity;
- zero leakage of any key.

**Independent Test**: `cargo test -p nullrouter-server --test e2e us5 reload secrets` and
`--test parity sessions`. Quickstart steps 2 and 6.

### Oracle fixtures for User Story 5

- [ ] T075 [P] [US5] Write `tools/gen-bundled/execution/sessions.mjs`.
  - It calls `resolveSessionIdentity({headers, body, connectionId, workspaceId: null, scope})` from `open-sse/utils/sessionManager.js` **twice**, with `connectionId` `"c1"` and `"c2"`. If the two results differ, the expected value is `null` (a generated fallback). Otherwise it is the returned client value.
  - Corpus (SC-010): each carrier alone, in FR-005a order:
    - `metadata.user_id` with `_session_<uuid>`;
    - `metadata.user_id` as a JSON string containing `session_id`;
    - `x-claude-code-session-id`;
    - `x-session-id`, `session-id`, `session_id`, `x-amp-thread-id`;
    - `x-client-request-id`;
    - body `prompt_cache_key`, `session_id`, `conversation_id`, and a plain `metadata.user_id`.
  - Plus pairwise combinations, and empty, whitespace-only, 256-character, and 257-character values.
  - Plus a body with earlier assistant messages and no carrier (a hash fallback, which gives `null`).

### Tests for User Story 5 ⚠️ write first, confirm they fail

- [ ] T076 [P] [US5] Write `crates/nullrouter-server/tests/parity/sessions.rs` against `sessions.json` (SC-010: 100% match, including `null` where 9router falls back).
- [ ] T077 [P] [US5] Write e2e tests in `crates/nullrouter-server/tests/e2e/us5.rs` for acceptance scenarios 1, 2, 5, 6, and 7:
  - two active accounts → the first is used;
  - the first inactive → the second;
  - an unknown provider, a specialized executor, or `mimo-free` (no-auth) in `keys.toml` → startup fails with the named error;
  - the `ci` key via Bearer and via `x-api-key` → agent `ci`;
  - two Claude Code sessions under one key → the same agent and different sessions;
  - no carrier → session `None`;
  - the same session under two keys → two identities (FR-005b).
- [ ] T078 [P] [US5] Write `crates/nullrouter-server/tests/e2e/reload.rs` (acceptance scenario 3, FR-010, FR-027):
  1. Start a slow stream on connection A.
  2. Rewrite `keys.toml` to put connection B first, then reload through the operator socket.
  3. Assert the in-flight stream completes on A with generation N in its observation, and that new requests use B with generation N+1.
  4. Introduce an invalid `keys.toml` and reload: expect errors returned, generation unchanged, and requests still on B.
  5. Put an invalid `config.toml` together with a valid `keys.toml`: the reload is rejected as a whole.
  6. Remove a provider and its connection in the same edit: the reload succeeds.

  Also run 200 reloads concurrent with 50 streaming requests, with no torn state.
- [ ] T079 [P] [US5] Write `crates/nullrouter-server/tests/e2e/secrets.rs` (SC-006):
  1. Run a representative request mix: every outcome variant, both formats, native and not, embeddings, count_tokens, and reload with errors, using sentinel keys.
  2. Capture all observations (`--json`), the `tracing` output (a test subscriber writer), `nullrouter-cli check`/`reload`/`obs` output, every client response body and header, and every mock-received request.
  3. Assert zero occurrences of any access key anywhere.
  4. Assert zero occurrences of any provider key anywhere except the auth header of that provider's own mock-received requests.
  5. Assert `keys.toml` errors never echo values.

### Implementation for User Story 5

- [ ] T080 [US5] Implement `crates/nullrouter-server/src/session.rs`: port the client-supplied carriers of `sessionManager.js` in FR-005a order with the non-kiro rules ([R5](research.md#r5-access-keys-and-agent-identity)):
  - the Claude `metadata.user_id` `_session_<uuid>` regex, else JSON `session_id`; and `x-claude-code-session-id`. Both are recorded as `claude:<id>`;
  - the header list, then `x-client-request-id`, then the body fields;
  - `normalizeSessionId`: a string, trimmed, non-empty, **"at most 256 characters"**;
  - read-only access to the parsed body (`RawValue` lookups, never re-serialized into the outbound request).

  Set `AgentIdentity { agent, session }` in the request prelude (T021) and in the observation.
- [ ] T081 [US5] Add the `reload` op to `crates/nullrouter-server/src/operator.rs`, calling `Server::reload` (T018). The response is `{"ok":true,"report":…,"keys":{…}}` or `{"ok":false,"errors":[…]}`.
- [ ] T082 [US5] Implement `nullrouter-cli reload` in `crates/nullrouter-cli/src/cmd/reload.rs`: print the load report as `check` does, plus the keys summary; print every error and exit 1 on rejection; exit 3 when no server is running.
- [ ] T083 [US5] Extend `nullrouter-cli check` in `crates/nullrouter-cli/src/cmd/check.rs` to validate `keys.toml` against the loaded registry, when present. Print the connection and access-key counts per provider, never values; keys errors exit 1.
- [ ] T084 [US5] Run `cargo test --workspace`. Everything must be green, including the 002 suites.

**Checkpoint**: All five stories pass independently, and no key leaks anywhere.

---

## Phase 8: Polish & Cross-Cutting Concerns

- [ ] T085 [P] Update `docs/operator-config.md`: document `keys.toml` (with a link to [contracts/keys-file.md](contracts/keys-file.md)), `serve`, `reload`, `obs`, the operator socket, and the env vars from [contracts/operator-cli.md](contracts/operator-cli.md).
- [ ] T086 [P] Write `docs/clients.md`:
  - pointing Claude Code (`ANTHROPIC_BASE_URL`, `ANTHROPIC_API_KEY` = the access key, model `anthropic/<id>` or a unified name) and the OpenAI SDK (`base_url`, `api_key`) at 0router;
  - the native-pair behaviour;
  - the `stream_options.include_usage` note for OpenAI stream usage;
  - SC-002: usable by an operator with only the docs.
- [ ] T087 [P] Update the `CLAUDE.md` workspace table: add `crates/nullrouter-server`, and list the new CLI commands on the `nullrouter-cli` line.
- [ ] T088 [P] Update the `nullrouter-registry` contract doc `specs/002-provider-model-registry/contracts/registry-api.md` with an "Additions in 003" note listing `Registry::load_candidate`, `LoadReport: Serialize`, and any embeddings accessor from T072.
- [ ] T089 Run `/rust-parity-audit` on `crates/nullrouter-server/src/outbound.rs`, `relay.rs`, `session.rs`, `errors.rs`, and `usage.rs`. Judge parity on 9router's request path (chatCore → DefaultExecutor), not on helpers alone. Write the findings to `specs/003-request-execution/parity-audit.md` and fix every High and Medium finding.
- [ ] T090 Run the security review: invoke the `security-auditor` agent over `crates/nullrouter-server/src/{keys,auth,outbound,operator,http}.rs`. Cover:
  - secret flow;
  - the operator-channel boundary (FR-028);
  - that the header overlay cannot smuggle the access key;
  - SSRF: `account_id` substitution cannot change the host.

  Record the findings in `parity-audit.md` under "Security" and fix any High findings.
- [ ] T091 [P] Write a concurrency test in `crates/nullrouter-server/tests/e2e/concurrency.rs` (SC-007): 100 concurrent streaming requests through one connection. The mock releases event *k* to all streams only after it has sent event *k−1* to all of them. Assert that every stream receives every event, and that no stream's event *k* arrives after another stream's event *k+1*. That would mean a request was serialized behind another.
- [ ] T092 [P] Add the named deviation tests. There must be one test per [R15](research.md#r15-deliberate-deviations-from-9router) row D1–D12, each with a doc comment stating the 9router and 0router behaviour. Add any that are missing to `crates/nullrouter-server/tests/e2e/deviations.rs`, reusing the existing scenarios.
- [ ] T093 Run the quickstart §1 automated block and §2–§3 locally with mocks. Then run §4 against real providers, **asking the user first**, because it uses their keys and network. Record any doc corrections in `quickstart.md`.
- [ ] T094 Run `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Then run `cargo bench -p nullrouter-server --bench relay -- --baseline slice-003`: no regression beyond noise.
- [ ] T095 Regenerate and check the oracle: `node tools/gen-bundled/generate.mjs`, then `git status tests/fixtures/ plugins/` is clean, which means the generator is deterministic.
- [ ] T096 Save the slice-003 decisions to agentmemory, in both the main and team instances (`memory_save`):
  - the executable rule and its set sizes;
  - `keys.toml` and the operator socket;
  - `Body::from_stream` rather than `Sse`, and why;
  - the R15 deviation list;
  - the OpenAI stream usage gap.

  Update the `pending_items` slot: the D1 decision, the Constitution V amendment, the MIT licence placeholder, and the fact that the slice-003 criterion baseline is local only. Save a `memory_lesson_save` for anything learned the hard way.
- [ ] T097 Update `specs/003-request-execution/research.md` with an "Implementation deviations" section (as 002's R13 did) for anything that changed during implementation.
- [ ] T098 Update `.specify/memory/constitution.md` **only if** the user approved the Principle V amendment (T102), as a PATCH version bump with a sync-impact report. Otherwise leave it unchanged and keep the Complexity Tracking entry.
- [ ] T099 [P] Confirm that `CLAUDE.md`'s "SSE streaming → `axum::response::sse::Sse`" convention bullet matches the outcome of T102. Update it to "`Sse` for 0router-originated events; `Body::from_stream` for relayed upstream bytes" if the amendment was approved.
- [ ] T100 Commit per the repo rules, and **only when the user asks**:
  - generated fixtures in their own commit, naming the ref SHA;
  - then code and docs.

  First confirm the branch: the work belongs on `003-request-execution`, and the checkout was on `002-provider-model-registry` at planning time.

### Decisions tracked (ask the user; do not decide)

- [ ] T101 Ask the user to confirm R15 D1: a connect timeout returns 504 (`upstream_timeout`, per the spec) or 9router's 502. If they choose 502, change T045's mapping and the T039 assertion (a one-line change plus one test).
- [ ] T102 Ask the user whether to amend Constitution V (proposed text in [plan § Complexity Tracking](plan.md#complexity-tracking)). T098 and T099 depend on the answer.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: T001 → T002 → T006. T003, T004, and T005 run in parallel after T001.
- **Foundational (Phase 2)**: depends on Setup and blocks every story.
  - The fixture writers T007–T009 are Node and need no cargo. They run in parallel with the Rust work.
  - T010, T011, T012, T019, and T020 are independent files.
  - T014 needs T007 for its parity test.
  - T015 → T016 → T017.
  - T018 needs T004 and T016.
  - T021 needs T012, T018, T019, and T020.
  - T022 needs T021. T023 needs T022. T024 needs T023.
- **US1 (Phase 3)**: depends on Foundational.
  - The fixture writers T026–T032 run in parallel, then T033.
  - The tests T034–T040 follow.
  - Implementation T041 → T042 → T043 → T044 → T045 → T046 → T047 → T048 → T049 → T050. T041 and T042 can run in parallel, as can T045 and T047.
  - Then T051 and T052.
- **US2 (Phase 4)**: depends on US1's `select.rs` (T044) and HTTP wiring (T050).
- **US3 (Phase 5)**: depends on US1. It hooks usage into T046, T047, and T048. It is independent of US2.
- **US4 (Phase 6)**: depends on US1's `upstream`, `select`, and the non-streaming path. It uses US3's usage extraction for its observation test (T071): land T063 first, or accept a `None`-usage assertion until then.
- **US5 (Phase 7)**:
  - It depends on US3's operator socket (T066) for reload and the e2e queries.
  - Session extraction (T075, T076, T080) depends only on Foundational and can start right after Phase 2.
- **Polish (Phase 8)**: depends on every story. T101 and T102 can be asked at any time. T098 and T099 need T102.

### Story Graph

```text
Setup → Foundational ─▶ US1 (MVP) ─┬─▶ US2
                                   ├─▶ US3 ─┬─▶ US5 ─▶ Polish
                                   │        └─▶ US4 (usage)
                                   └─ (US5 sessions T075/T076/T080 may start after Foundational)
```

### Within Each Story

1. Write the oracle fixture writers (Node) and regenerate.
2. Write the parity and e2e tests, and watch them fail.
3. Implement the modules in dependency order.
4. Finish with the green-run task.

---

## Parallel Examples

```text
# Phase 2: Node fixture writers alongside the Rust foundation
T007 executable-providers.mjs   T008 timeouts.mjs   T009 upstream-errors.mjs
T010 secret.rs   T011 timeouts.rs   T012 errors.rs   T019 auth.rs   T020 observe.rs

# US1: all seven fixture writers at once (separate files under execution/)
T026 executor-requests   T027 transport-choice   T028 client-detect   T029 non-sse
T030 stream-errors       T031 sse-to-json        T032 count-tokens

# US1: tests together
T034 parity/executor_requests.rs   T035 transport_choice + client_detect
T036 non_sse + stream_errors + sse_to_json + count_tokens   T037/T038 outbound unit tests   T040 cancel.rs

# After US1: US2 and US3 in parallel (different modules; both touch http.rs only in their own routes)
US2: T053 T054 → T055 T056        US3: T058 T059 T060 T061 T062 → T063 … T067

# Sessions can start early
T075 sessions.mjs → T076 parity/sessions.rs → T080 session.rs
```

---

## Implementation Strategy

### MVP (User Story 1)

1. Phase 1, then Phase 2.
2. Phase 3. **Stop and validate**:
   - parity for executor requests, transport choice, client detection, the error builders, and assembly is green;
   - cancel is 50 of 50;
   - the relay bench's TTFT delta is under 5 ms.
3. With the user's go-ahead, run a live smoke test with Claude Code → `anthropic/<model>` (quickstart §4).

### Incremental Delivery

1. **US1**: one provider, direct targets. 0router is usable ("it works for me").
2. **US2**: unified models and the model listing. Clients use operator-declared names.
3. **US3**: observations and `obs`. This produces the data the routing-decision slice needs.
4. **US4**: embeddings. The path is proven not chat-only.
5. **US5**: multiple accounts, reload, sessions, and the secrets audit.
6. **Polish**: docs, parity and security audits, constitution follow-ups, and memory.

### Notes

- `select.rs` is the routing slice's replacement point. Keep it pure, with no I/O.
- Never hand-edit `tests/fixtures/9router/`. If a fixture looks wrong, fix the generator
  and regenerate.
- Never call `Secret::expose` outside `outbound.rs` and `embeddings.rs`. Consider a
  `#[deny]`-style grep check in T094.
