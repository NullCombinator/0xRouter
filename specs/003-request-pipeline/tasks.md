---

description: "Task list for 003-request-pipeline"
---

# Tasks: Request Pipeline

**Input**: Design documents from `specs/003-request-pipeline/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included. Constitution VI requires parity tests against 9router. The spec names
test-matrix success criteria (SC-001 to SC-013), and the benchmark gate requires Criterion on
the execution and streaming paths. Within each story, write the tests first and confirm they
fail before implementing.

**Organization**: Tasks are grouped by user story, so each story can be implemented and
tested on its own.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US8)

## Path Conventions

Cargo workspace at the repo root ([plan § Project Structure](plan.md#project-structure)):

- `crates/zerorouter-registry/`: schema, gate, styles, fit check, community set (sync, no tokio)
- `crates/zerorouter-wire/`: style interpreter, IR, codecs, framing (pure, no I/O, no tokio)
- `crates/zerorouter-engine/`: accounts, keys, attempt loop, upstream client, records
- `crates/zerorouter-server/`: axum surface, relay, operator socket
- `crates/zerorouter-cli/`: operator CLI
- `styles/bundled/`: the four API-style files
- `plugins/bundled/`: the chosen five (schema 2, hand-maintained after seeding)
- `plugins/community/`: the other 116 (schema 1, generated)
- `tests/fixtures/9router/`: generated parity oracle, shared across crates
- `tests/parity/deviations.toml`: every deliberate deviation, asserted
- `tests/harness/`: SDK and harness scripts (Python, Node, Claude Code, Codex CLI)

Three refinements to the plan's tree:

- The gate corpora stay where slice 002 put them: `crates/zerorouter-registry/tests/gate/`.
  New subdirectories are `invalid/styles/`, `invalid/providers/`, `strict/` and
  `unsupported/`.
- The scripted mock upstream lives in `zerorouter-engine` behind a `testkit` feature
  (`src/testkit/`), so the engine and server tests share it without a new crate.
- The template parser (the AST and its validation) lives in `zerorouter-registry`, because
  the gate needs it. `zerorouter-wire` renders and reverse-matches the parsed AST.

Build commands need `export CARGO_HOME=$PWD/.cargo-home`. `ref/` is git-ignored. Generated
artefacts record the `ref/9router` SHA in their header.

**Live checks** (tasks marked *operator-run*) need the operator's own provider keys. Ask
the user to run the given command with `! …` in the session. Never ask them for the keys.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: New crates and directories that compile.

- [X] T001 Verify the toolchain ([R1](research.md#r1-toolchain-and-crates)).
  - With `CARGO_HOME=$PWD/.cargo-home`, `cargo --version` and `rustc --version` must succeed and report 1.93.x.
  - `cargo fetch` must reach crates.io. If it can't, **stop** and tell the user. Never edit `identity/`.
- [X] T002 Extend the workspace manifest `Cargo.toml`.
  - Add to `[workspace.dependencies]`:
    - `tokio` 1 (`rt-multi-thread`, `net`, `time`, `signal`, `fs`, `macros`, `io-util`, `sync`);
    - `tokio-util` (`rt`);
    - `axum` 0.8;
    - `reqwest` 0.13 (`default-features = false`, features `rustls-tls`, `stream`, `http2`, `multipart`);
    - `futures-util`, `bytes`, `memchr`, `sha2`, `base64`, `getrandom`, `aho-corasick`, `tracing`, `tracing-subscriber` (`env-filter`);
    - `ulid` or an equivalent for time-sortable ids.
  - Add `raw_value` to the existing `serde_json` features.
  - Add path dependencies for `zerorouter-wire`, `zerorouter-engine` and `zerorouter-server`.
- [X] T003 [P] Create `crates/zerorouter-wire/Cargo.toml` and `src/lib.rs`.
  - Dependencies: `serde`, `serde_json`, `bytes`, `memchr`, `base64`, `thiserror`, `indexmap`, `zerorouter-registry`. No `tokio`, no network.
  - Modules `ir`, `template`, `codec`, `stream`, `primitives`, `usage`, `estimate`, `error_body`, each with an empty file.
  - `[[bench]] name = "wire"`, `harness = false`. `lints.workspace = true`.
- [X] T004 [P] Create `crates/zerorouter-engine/Cargo.toml` and `src/lib.rs`.
  - Dependencies: `tokio`, `tokio-util`, `reqwest`, `futures-util`, `bytes`, `sha2`, `base64`, `getrandom`, `aho-corasick`, `tracing`, `arc-swap`, `serde`, `toml`, `serde_json`, `thiserror`, `ulid`, `zerorouter-wire`, `zerorouter-registry`.
  - Feature `testkit`, which pulls in `axum` for the mock upstream.
  - Modules `accounts`, `keys`, `classify`, `cooldown`, `plan`, `attempt`, `breaks`, `upstream`, `forwarding`, `jobs`, `records`, `redact`, `state`, and `testkit` (cfg feature).
  - `[[bench]] name = "engine"`, `harness = false`.
- [X] T005 [P] Create `crates/zerorouter-server/Cargo.toml` and `src/lib.rs`.
  - Dependencies: `axum`, `tokio`, `tokio-util`, `futures-util`, `bytes`, `tracing`, `tracing-subscriber`, `serde_json`, `zerorouter-engine`, `zerorouter-wire`, `zerorouter-registry`.
  - Dev-dependency: `zerorouter-engine` with `testkit`.
  - Modules `router`, `auth`, `relay`, `models`, `count`, `operator`, `serve`.
  - `[[bench]] name = "server"`, `harness = false`.
- [X] T006 [P] Extend `crates/zerorouter-cli`.
  - Add `tokio` and `zerorouter-server` dependencies.
  - Add clap subcommands `serve`, `accounts`, `keys`, `behaviour`, `records` and `plugins`, each dispatching to an empty `src/cmd/<name>.rs`.
  - Exit codes per [contracts/operator-cli.md](contracts/operator-cli.md#commands): 0 ok, 1 invalid input or file, 2 usage, 3 plugin not supported, 4 no running server.
- [X] T007 [P] Create the empty data directories and their placeholders.
  - `styles/bundled/` and `plugins/community/`, each with a `.gitkeep`.
  - `tests/parity/deviations.toml`: header comment only. The format is `[[deviation]] provider|style, fixture, field, reason`.
  - `tests/harness/README.md`, and `tests/harness/.gitignore` for `.venv/` and `node_modules/`.
- [X] T008 [P] Check the harness tools.
  - Try `python3 -m venv tests/harness/.venv` and install `openai`, `anthropic` and `google-genai`.
  - Try `npm install --prefix tests/harness openai @anthropic-ai/sdk @google/genai`.
  - Record in `tests/harness/README.md` what installed, and whether `claude` and `codex` are on `PATH`.
  - Anything blocked by the sandbox becomes *operator-run* for T058, T082 and T108. Report it, don't stop.
- [X] T009 Run `cargo build --workspace && cargo clippy --workspace` on the skeleton and confirm both pass.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Schema 2, style files through the gate, the wire machinery, the engine state
and upstream client, and the server shell. Every story depends on these.

**⚠️ CRITICAL**: No user story work can start until this phase is complete.

### Registry: schema, templates, gate

Rule for every new struct: `#[serde(deny_unknown_fields)]`, `snake_case` keys, and
`Option` wherever absence is distinct.

- [X] T010 [P] Add the closed primitive enums in `crates/zerorouter-registry/src/schema/primitives.rs`. Each has `Display` and `pub const ALLOWED: &[&str]`.
  - Stream and layout choices:
    - `Framing`: `sse_named | sse_data | sse_data_done | ndjson | json_array`;
    - `BlockModel`: `explicit | implicit`;
    - `ToolArgumentsMode`: `fragments | whole`;
    - `SystemLayout`: `top_level_field | first_message | instructions_field | system_instruction`;
    - `ToolCallsLayout`: `content_part | message_field | output_item | function_call_part`;
    - `ToolResultsLayout`: `content_part | tool_role_message | output_item | function_response_part`;
    - `ArgumentsForm`: `json_object | json_string`;
    - `Alternation`: `merge_adjacent | as_is`;
    - `ToolResultMatch`: `id | name`.
  - Codec primitives:
    - `MediaCodec`: `data_url | anthropic_source | gemini_inline_data | url`;
    - `ThinkingForm`: `budget_tokens | effort | gemini_thinking_config`;
    - `EmbeddingVector`: `float | base64_f32le`;
    - `BodyEncoding`: `json | multipart | binary`;
    - `AsyncJob`: `poll_on_client_request`;
    - `TokenEstimator`: `estimate_9router`;
    - `SessionExtractor`: `claude_code_user_id`;
    - `Repair`: `ensure_tool_call_ids | fill_missing_tool_results | gemini_schema_sanitize | gemini_function_name_sanitize`;
    - `AudioCollector`: `chat_audio_delta_collect`.
  - Routes and types:
    - `RouteOp`: `generate | count_tokens | list_models | get_model | job_submit | job_get | job_content`;
    - `ModelType`: `text | embeddings | image | tts | stt | video`.
  - Provider declarations:
    - `ContinuationMethod`: `assistant_prefill | prefix_flag`;
    - `ContinuationUnless`: `thinking_enabled | tool_call_in_progress`;
    - `SessionDerive`: `ses_sha256_hex32 | ses_time_base62`;
    - `ForwardMerge`: `replace | append_csv`.
- [X] T011 [P] Implement the template parser in `crates/zerorouter-registry/src/template.rs` ([contracts/api-style-schema.md § Mapping vocabulary](contracts/api-style-schema.md#mapping-vocabulary-closed)), with unit tests.
  - Parse a JSON-shaped `toml::Value` into a `Template` AST, supporting:
    - a whole-string placeholder `"{p}"`, which keeps the value's type;
    - interpolation `"x{p}y"`;
    - an optional `"{p?}"`, which omits the key when the value is absent;
    - the `{{` escape.
  - `Template::check(ctx: PlaceholderSet)` rejects any placeholder outside the context's fixed set.
  - Always reject `{account.*}`, `{secret.*}`, and anything containing an operator or a space (`{a+b}` → "expressions are not allowed").
  - Field paths `a.b[0].c` and `a[*].b` are parsed to a `FieldPath`. Selectors only.
- [X] T012 [P] Implement the endpoint URL rules in `crates/zerorouter-registry/src/validate/ssrf.rs` ([R18](research.md#r18-forwarding-and-the-security-floor)), with unit tests.
  - Placeholders `{model}` and `{voice}` only, and only in the path.
  - Reject loopback, private (10/8, 172.16/12, 192.168/16, 100.64/10, fc00::/7), link-local (169.254/16, fe80::/10) and metadata hosts (169.254.169.254, fd00:ec2::254, `metadata.google.internal`), plus `localhost`, `*.local` and `*.internal`, unless `allow_private: bool` is set.
  - Export `is_private_ip(IpAddr) -> bool` for the engine's resolved-IP re-check.
- [X] T013 [P] Implement the security floor in `crates/zerorouter-registry/src/floor.rs` ([contracts/provider-schema-v2.md § Forwarding](contracts/provider-schema-v2.md#forwarding-and-the-security-floor)), with unit tests.
  - A static list: `authorization`, `proxy-authorization`, `x-api-key`, `api-key`, `x-goog-api-key`, `xi-api-key`, `cookie`, `set-cookie`, `set-cookie2`, `www-authenticate`, `proxy-authenticate`, `x-amz-security-token`, `x-auth-token`.
  - The hop-by-hop and core-owned set: `host`, `content-length`, `transfer-encoding`, `connection`, `keep-alive`, `proxy-connection`, `te`, `trailer`, `upgrade`, `content-encoding`, request `accept-encoding`, and `x-0router-*`.
  - Anything matching slice 002's secret-name check (`validate/secrets.rs::check_map_key`).
  - `Floor::computed(styles, providers)` adds every loaded style's key-carrier headers and every loaded provider's auth header.
  - `Floor::blocks(name)` is case-insensitive.
  - `Floor::pattern_risk(pattern)` returns `Error` for a bare `*` or a prefix shorter than 3 characters, `Warning` if the wildcard could match a floor name, else `Ok`.
- [X] T014 [P] Define the API-style schema in `crates/zerorouter-registry/src/schema/style.rs` per [contracts/api-style-schema.md](contracts/api-style-schema.md). Depends on T010.
  - `StyleFile`: `schema = 1`, `kind = "api-style"`, `id`, `[access_key] carriers` (`header` or `query`, with `scheme: raw | bearer`), `[session] carriers` (`header`, body `path`, or `extractor`), and `[[routes]]` (`method`, `path`, `op`, `type`, `model`, `stream`, `discriminator`).
  - Optional per-type codec sections: `text`, `embeddings`, `image`, `tts`, `stt`, `video`.
  - `[errors]`: `body`, `type_map`, `stream_event`, `keepalive`.
  - Template-valued fields are kept as `toml::Value` and parsed by T011 in the gate.
- [X] T015 [P] Define the schema-2 provider parts in `crates/zerorouter-registry/src/schema/endpoint.rs`, `forwarding.rs` and `session.rs` per [contracts/provider-schema-v2.md](contracts/provider-schema-v2.md). Depends on T010.
  - `Endpoint` fields:
    - `url`, `method`, and either `wire` or `body` + `response`;
    - `headers` (IndexMap), `encoding`;
    - `timeout_ms`, `stall_timeout_ms`, `force_stream`, `vision`;
    - `retry` (status → `{retries, delay_ms}`), `models`, `voices`;
    - `errors { body: [rule], stream: [rule] }`, `token_count { url }`;
    - `continuation { method, trim_trailing_whitespace, unless, models | except_models }`.
  - `endpoints.<type>` accepts one table or an array of tables (one entry per wire).
  - `Forwarding`: `to_upstream.headers [{name, merge, from_styles}]` and `to_client { headers, body }`.
  - `Session`: `{ header, derive }`.
- [X] T016 Extend `crates/zerorouter-registry/src/schema/plugin.rs` for schema 2. Depends on T015.
  - `schema = 2` adds `endpoints: BTreeMap<ModelType, OneOrMany<Endpoint>>`, `forwarding`, `session`, `requires: Vec<String>` and `models[].wires: Vec<StyleId>`.
  - Schema 2 rejects the schema-1 execution keys `transport`, `transports`, `capabilities.*.endpoint`, `quirks`, `executor_params` and `auth.hooks`, with the rule "schema 2 declares this under `endpoints`".
  - Schema 1 parses exactly as in slice 002.
- [X] T017 Add operator settings in `crates/zerorouter-registry/src/schema/config.rs`: `allow_private_endpoints: bool` (default `false`), `[server] listen` (default `"127.0.0.1:20129"`) and `[pipeline] break_behaviour = "restart" | "error_event"` (default `restart`). Update the config unit tests.
- [X] T018 Implement the style gate in `crates/zerorouter-registry/src/validate/style_gate.rs`. Depends on T011, T014.
  - Collect every error, not only the first, and apply slice 002's secret checks.
  - Path templates allow `{name}` and `{name*}` only.
  - Route collisions across all loaded style files: two routes may share (method, path) only when their discriminators are disjoint and exactly one of them has none (the default).
  - Every route type needs its codec section.
  - `text.finish` must be total over the style's finish reasons.
  - The `errors.body` template must contain `{error.message}` and `{error.details}`. `type_map` must cover 400, 401, 404, 429, 500 and 503.
  - Every response and stream rule must reverse unambiguously for a style used as an upstream wire (error `ambiguous-stream-rules`).
  - A style file may not declare forwarding.
- [X] T019 Extend the plugin gate in `crates/zerorouter-registry/src/validate/gate.rs` for schema 2. Depends on T012, T013, T016, T018.
  - The signature gains `ctx: GateCtx { style_ids, style_ops, strict: bool, allow_private: bool }`.
  - Endpoint rules:
    - `wire` must name a loaded style, and `wire` and `body` are mutually exclusive;
    - body placeholders come from the fixed per-type set (`{input.*}`, `{model.upstream_id}`, `{params.*}`, `{output.*}`);
    - `token_count` is allowed only if the wire style has a `count_tokens` route;
    - error-rule statuses must be 100–599;
    - `ssrf::check` applies to every URL.
  - Every model type used by `models[]` must have an endpoint (`model-type-without-endpoint`).
  - Forwarding rules:
    - a floor name is a diagnostic and the entry is stripped (an error under `strict`);
    - a risky wildcard is a warning;
    - a bare `*` or a short prefix is an error;
    - a secret-like body path is an error.
  - Keep the existing `validate()` signature as a wrapper, so slice 002 callers compile.
- [X] T020 Load the styles in `crates/zerorouter-registry/build.rs`, `src/load.rs` and `src/registry.rs`. Depends on T017, T019.
  - `build.rs` also embeds `styles/bundled/*.toml`.
  - `load` validates styles first, then plugins with `GateCtx`. Bundled plugins load under `strict`. `allow_private` comes from `config.toml`.
  - `Registry` gains `styles()`, `style(id)`, `endpoints(provider, ModelType) -> &[Endpoint]` and `floor() -> &Floor`.
  - A malformed style file is a startup error that names the file (US7-4).
- [X] T021 Derive slice 002's composed-transport view from schema-2 `endpoints` in `crates/zerorouter-registry/src/views.rs`, so `composed_transport()` still answers for schema-2 providers. Add the deviation-assertion helper in `crates/zerorouter-registry/tests/parity/deviations.rs`: it loads `tests/parity/deviations.toml` and lets a parity comparison skip exactly the listed (provider, fixture, field), failing if a listed deviation no longer differs.

### Wire: IR, templates, framing, generic codecs

- [X] T022 [P] Define the request IR in `crates/zerorouter-wire/src/ir/request.rs` ([R3](research.md#r3-client-api-styles-as-data)).
  - `Request { model, system, messages, tools, tool_choice, params, stream, extra }`.
  - `Message { role, parts }`, where `Part` is one of `Text`, `Image { media }`, `Audio { media }`, `ToolCall { id, name, arguments }`, `ToolResult { id, name, content, is_error }` or `Thinking { text, signature, vendor }`.
  - `Params` holds `max_tokens`, `temperature`, `top_p`, `stop`, `thinking`, `response_format`, `metadata`, and `extra: Map<String, RawValue>` for native passthrough.
  - Per-type IR: `EmbeddingsRequest`, `ImageRequest`, `TtsRequest { input, voice, format }`, `SttRequest { audio, language }` and `VideoRequest`.
- [X] T023 [P] Define the stream event IR and the response IR in `crates/zerorouter-wire/src/ir/event.rs` and `src/ir/response.rs`. The events are `Preamble`, `BlockStart { kind: Text | Thinking | ToolCall { id, name } }`, `TextDelta`, `ThinkingDelta`, `Signature`, `ToolArguments`, `BlockStop`, `Usage`, `Finish { reason }`, `Error`, `Keepalive` and `Done`. `Response` holds the same content as final blocks, plus usage and finish.
- [X] T024 Implement template render and reverse-match in `crates/zerorouter-wire/src/template.rs` over the T011 AST, with unit tests. Depends on T011.
  - `render(&Template, &Ctx) -> serde_json::Value` keeps types for whole-string placeholders and omits `{p?}` keys when absent.
  - `match_value(&Template, &Value) -> Option<Bindings>` matches literal parts and extracts placeholders.
  - A property test covers `match(render(t, ctx)) == ctx` for each template in `styles/bundled/`, once those exist.
- [X] T025 [P] Implement incremental framers in `crates/zerorouter-wire/src/stream/framing.rs` with `memchr`, plus unit tests.
  - Framers: named SSE (`event:` + `data:`), data-only SSE with and without `[DONE]`, NDJSON, and a streamed JSON array (Gemini non-SSE).
  - Push-based: `feed(&[u8]) -> impl Iterator<Item = Frame>`.
  - Handle CRLF, `:` comments, multi-line `data:`, and a chunk split at every byte offset (tested exhaustively on fixtures).
- [X] T026 Implement `ClientStreamState` and the style stream writer in `crates/zerorouter-wire/src/stream/writer.rs` ([data-model § ClientStreamState](data-model.md#clientstreamstate)). Depends on T023, T024.
  - Fields: `preamble_sent`, `output_seen`, `open_block` (`None | Text { index } | Thinking { index, signed } | ToolCall { index, args_started }`), `next_block_index`, `next_output_index`, `sequence_number` and `partial_text`. The counters are "carried across segments". `partial_text` is "a copy, the stream is not held".
  - Encode IR events with the style's `[[text.stream.events]]` templates under `blocks = explicit | implicit` and `tool_arguments = fragments | whole`.
  - Expose the counters `{block.index}`, `{tool.ordinal}`, `{output.index}`, `{sequence.number}` and `{response.id}`, and the accumulations `{block.full_text}`, `{block.full_arguments}` and `{response.rendered}`.
- [X] T027 Implement the generic request codec in `crates/zerorouter-wire/src/codec/request.rs`. Depends on T022, T024.
  - `decode(style, body) -> IR` and `encode(IR, wire_style) -> body`, driven by `[text.layout]`, `[text.parts]` and `[text.params]`.
  - Repairs (`ensure_tool_call_ids`, `fill_missing_tool_results`, `gemini_schema_sanitize`, `gemini_function_name_sanitize`) run only when the client style differs from the wire, and never change message text.
  - A part the wire can't carry returns `Err(CannotCarry { part, reason })`. Nothing is dropped.
- [X] T028 Implement the generic response and stream codec in `crates/zerorouter-wire/src/codec/response.rs` and `src/stream/reader.rs`. Depends on T023, T024, T025.
  - Provider frames → IR events, through the wire style's stream rules (reverse-match).
  - Provider non-stream body → `Response` IR → the client body. The second hop always goes through the IR.
  - `finish` maps both ways.
- [X] T029 [P] Add a minimal test style in `crates/zerorouter-wire/tests/fixtures/mini-style.toml` and `tests/generic.rs` that exercise T024–T028 without the real styles: a round-trip request, a stream of text + tool call, and a non-stream response.

### Engine: operator state, redaction, upstream client, records

- [X] T030 [P] Implement provider accounts in `crates/zerorouter-engine/src/accounts.rs` ([data-model § ProviderAccount](data-model.md#provideraccount-accountstoml)), with unit tests.
  - Load and save `accounts.toml`, `schema = 1`, `[[account]] provider, name, secret, order, disabled`.
  - `name`: "`[a-z0-9_-]{1,32}`", "unique per provider".
  - `secret`: a literal or `{ env = "VAR" }`, held in `SecretString` (from `zerorouter-registry::credentials`), with `Debug` printing `***`. It is "never serialised back in clear".
  - An account for a provider that isn't loaded is "reported unused, not an error".
  - Reading fails if the file is group- or world-readable (mode & 0o077 ≠ 0), naming the file and the fix `chmod 600`.
  - Writes are atomic (temp file + rename) and create the file with mode 0600.
  - Host binding: the secret is released only for the endpoint hosts the provider had when the account was added. A replacing plugin with new hosts withholds the secret until re-confirmed (slice 002 FR-012a rule).
- [X] T031 [P] Implement agent keys in `crates/zerorouter-engine/src/keys.rs` ([data-model § AgentKey](data-model.md#agentkey-keystoml), [R12](research.md#r12-access-keys-and-agent-identity)), with unit tests.
  - Generation: `0r-` + base64url (no padding) of 32 random bytes from `getrandom`, which gives 43 characters. It is returned once.
  - Stored fields:
    - `id`: "`ak_` + 8 chars";
    - `name`: unique;
    - `digest`: "SHA-256 hex of the key", written `sha256:<hex>`;
    - `last4`, `created` (RFC 3339), `revoked`, `break_behaviour`.
  - Lookup is a `HashMap<digest, KeyId>`. A revoked key never matches.
  - `AgentId = (key id, Option<session id>)`, with the session "≤ 256 chars" (longer values truncated, 9router `normalizeSessionId`).
  - Same 0600 and atomic-write rules as T030.
- [X] T032 [P] Implement the redactor in `crates/zerorouter-engine/src/redact.rs` ([R23](research.md#r23-secret-redaction)), with unit tests.
  - `Redactor::new(secrets)` builds an Aho-Corasick automaton over every account secret (≥ 8 characters) and every agent key.
  - `redact(&str) -> Cow<str>` replaces each match with `***`. `redact_url` also removes `key=` query values.
  - `RedactLayer`, a `tracing_subscriber::Layer`, formats every event through the current redactor (via `ArcSwap`).
- [X] T033 Implement the engine snapshot in `crates/zerorouter-engine/src/state.rs`. Depends on T030–T032.
  - `EngineState { registry, accounts, keys, config, redactor, generation }` behind `ArcSwap`.
  - `reload()` runs on `spawn_blocking` and swaps atomically. In-flight requests keep the old snapshot. A failed reload keeps the previous one and returns the error.
- [X] T034 Implement the upstream client in `crates/zerorouter-engine/src/upstream.rs` ([R22](research.md#r22-connection-reuse), [R18](research.md#r18-forwarding-and-the-security-floor)). Depends on T012, T033.
  - One process-wide `reqwest::Client`:
    - rustls, HTTP/2 through ALPN;
    - `pool_idle_timeout(90 s)` and `tcp_keepalive(60 s)`;
    - `redirect(Policy::none())`;
    - connect and response-header timeout from the endpoint (default 60 000 ms, 9router's `envMs` rule for `FETCH_CONNECT_TIMEOUT_MS`).
  - A custom DNS resolver re-checks every resolved IP with `ssrf::is_private_ip`, unless `allow_private_endpoints` is set.
  - `build_request(endpoint, account, body, client_headers)` assembles headers in this order: client headers → floor → plugin static headers → core auth last. The secret is injected here only, as `x-api-key`, Bearer or `xi-api-key` per the plugin's auth scheme.
  - `{model}` and `{voice}` are percent-encoded, and `.`/`..` segments are rejected.
- [X] T035 [P] Implement the record store in `crates/zerorouter-engine/src/records.rs` ([data-model § RequestRecord](data-model.md#requestrecord), [R21](research.md#r21-records-store)), with unit tests.
  - `RequestRecord` has the data-model fields; `Attempt` has `n`, `provider`, `account`, `model`, `kind`, `started`, `ended`, `outcome` and `usage`.
  - `Usage` fields are `Option<u64>`, where "`None` = not reported". It also has `input_semantics` and `estimated`.
  - Ids are "`rq_` + 26-char time-sortable id".
  - The state machine is `in_progress → succeeded | failed | refused | cancelled`.
  - The store is a "Ring of 10 000 records, oldest evicted", with indexes by id, provider and unified model rebuilt on eviction.
  - Queries: by id, by provider, by unified model, newest first, with a limit.
- [X] T036 [P] Implement the scripted mock upstream in `crates/zerorouter-engine/src/testkit/mock_upstream.rs` (feature `testkit`).
  - An in-process axum server on `127.0.0.1:0`, driven by a per-request script:
    - reply with a status and body;
    - stream N frames and then cut the connection;
    - stall;
    - `retry-after`;
    - in-band error body;
    - binary body;
    - 202 + polling.
  - It records every received request (headers and body), counts accepted TCP connections, and observes client disconnects (with a timestamp).

### Server shell

- [X] T037 Implement route matching in `crates/zerorouter-server/src/router.rs`. Depends on T020.
  - Build a route table from every loaded style's `[[routes]]`.
  - Use its own matcher (`{name}` for one segment, `{name*}` for the rest including `/`, and a literal suffix after `{name*}` such as `:generateContent`), plus `header_present` discriminators.
  - Mount a single axum fallback handler that dispatches `(style, route, captures)`.
  - An unknown path returns 404 in the OpenAI error shape.
- [X] T038 Implement the access-key check in `crates/zerorouter-server/src/auth.rs` ([contracts/client-surface.md § Access key](contracts/client-surface.md#access-key)). Depends on T031, T037.
  - Read the style's carriers in order before touching the body.
  - A missing, unknown or revoked key gets 401 in the style's error shape with `x-0router-request-id`, and a `refused` record with no agent. Nothing is sent upstream.
- [X] T039 Implement the relay in `crates/zerorouter-server/src/relay.rs` ([R5](research.md#r5-streaming-relay-and-constitution-v)). Depends on T026.
  - SSE responses use `axum::response::sse::Sse<impl Stream>` built with `StreamExt` adaptors over the engine's IR event channel.
  - Non-SSE streamed bodies use `Body::from_stream`.
  - A `CancelOnDrop` guard owns the request's `CancellationToken` and cancels it when the response body drops.
  - Every response gets `x-0router-request-id`.
- [X] T040 Implement `serve` in `crates/zerorouter-server/src/serve.rs` and `crates/zerorouter-cli/src/cmd/serve.rs`. Depends on T033, T037–T039.
  - Build the `EngineState`, refuse to start on a 0600 violation (T030/T031), bind `[server] listen` or `--listen`, install `RedactLayer` on stderr, and shut down cleanly on SIGINT/SIGTERM.
- [X] T041 Run `cargo test --workspace` and confirm slice 002's tests still pass (with deviations listed, if any) and the Phase 2 unit tests are green.

**Checkpoint**: Schema 2 and styles load through the gate. The server starts, rejects bad
keys, and has no working routes yet.

---

## Phase 3: User Story 1 — A client gets an answer through 0router (Priority: P1) 🎯 MVP

**Goal**: A text request in any of the four styles reaches anthropic, openrouter,
opencode-zen or opencode-go, and the answer comes back in the client's style, streamed or
not. The request is recorded.

**Independent Test**: [quickstart §1–2](quickstart.md#1-setup-account-key-server-us1-us5).
At least two harnesses in different styles are served by each text provider (mock upstreams
in CI), and every request produces a record.

### Tests for User Story 1 ⚠️

- [ ] T042 [US1] Extend `tools/gen-bundled/generate.mjs` with a translator oracle.
  - Import 9router's `open-sse/translator` request and response translators.
  - For each input in `tools/gen-bundled/translate-inputs/*.json`, write the translated output to `tests/fixtures/9router/translate/<from>-to-<to>/<case>.json`. The inputs cover plain text, system, multi-turn, tools with results, images, thinking and `response_format`, for every pair 9router covers among `openai`, `claude`, `openai-responses` and `gemini`.
  - Also write stream-event fixtures: upstream frames in, client frames out.
  - Header: the ref SHA. Commit the output on its own.
- [ ] T043 [P] [US1] Write the translation parity tests in `crates/zerorouter-wire/tests/parity_translate.rs`. They compare the wire codecs and the four style files against every `tests/fixtures/9router/translate/**` case, with differences allowed only through `tests/parity/deviations.toml`.
- [ ] T044 [P] [US1] Write the deviation assertions in `crates/zerorouter-wire/tests/deviations.rs`, one test per row of [R4](research.md#r4-translation-behaviour-parity-and-deliberate-deviations):
  - no Claude Code system prompt;
  - `response_format` goes to the native field, or the result is `CannotCarry`;
  - no fingerprint tools;
  - `cache_control` is kept;
  - URL images and `is_error` are carried;
  - signed thinking is dropped across vendors;
  - Gemini tools are translated;
  - the non-stream second hop is correct.

  Add the matching `[[deviation]]` rows to `tests/parity/deviations.toml`.
- [ ] T045 [P] [US1] Write end-to-end style tests in `crates/zerorouter-server/tests/styles.rs` with the mock upstream.
  - Cover each client style × each wire (native and translated), streamed and not.
  - The bodies must be valid for the client style: the stream grammar, block start before deltas, no reused index, and a terminal event.
  - `x-0router-request-id` must be present.
  - Client-visible usage must equal the mock's usage (US1-1).
- [ ] T046 [P] [US1] Write the access-key and session tests in `crates/zerorouter-server/tests/auth.rs`.
  - No key, an unknown key and a revoked key each return 401 in the style's shape, and the mock received nothing (US1-4). Run this in all four styles.
  - The Gemini `?key=` carrier works.
  - Two Claude Code sessions (`metadata.user_id` `_session_` form) under one key are distinct agents in their records (US1-5).
- [ ] T047 [P] [US1] Write the cancellation test in `crates/zerorouter-server/tests/cancel.rs`. When the client drops mid-stream, the mock sees the disconnect within 1 s (SC-010, US1-6), and the record is `cancelled`.
- [ ] T048 [P] [US1] Write the passthrough test in `crates/zerorouter-server/tests/passthrough.rs`. In a native pair and in a translated pair, the prompt text (system, messages, tool results) reaches the mock byte-for-byte equal as a string value (US1-7).

### Optimizer pass-through (amendment 2026-09-28) ⚠️ before T049

Spec FR-038–FR-042, SC-014, US1-8 to US1-11; [R27](research.md#r27-optimizer-pass-through-amendment-2026-09-28). These change Phase 2 code (T027, T028, T034, T035, T039), so they land before the style files and the attempt path are built on it.

- [X] T146 [P] [US1] Write the wire pass-through tests in `crates/zerorouter-wire/tests/passthrough.rs` with the mini style (T029).
  - Same-style: a body with unknown keys at the top level, on a message, on a part and on a tool, plus a block type no template knows, comes out JSON-equal to the input except the model path, the forced stream path and `stream_options.include_usage`.
  - Cross-style: the same body encodes without the unknown keys, and the returned drop list holds each one's path and a reason, with no values. An unknown block type is still `CannotCarry`.
  - Same-style non-stream response: unknown response fields survive, and usage is still read from it.
- [ ] T147 [P] [US1] Extend `crates/zerorouter-server/tests/passthrough.rs` (T048) with the route-level cases.
  - Same-style: the mock receives every unknown body field and every unknown client header; it never receives a floor header, a hop-by-hop header, `x-0router-*` or the access key (US1-8).
  - Cross-style: the mock receives no unknown field and no undeclared header; the record's attempt lists each dropped path (US1-9).
  - A non-stream same-style response with an unknown field reaches the client unchanged, and streamed events keep their names and payloads (US1-10).
  - Fallback crossing styles: the first, same-style attempt fails with a 503; the second, cross-style attempt records its drops and gets no unknown headers (edge case).
- [X] T148 [US1] Implement same-style forwarding and drop tracking in `crates/zerorouter-wire/src/codec/request.rs`. Depends on T027.
  - `forward(body: &Value, wire: &Style, edits: &Edits) -> Value` returns the client body with edits at named paths only: model path → upstream id, stream path when forced, `stream_options.include_usage` on a streamed Chat wire.
  - The decoder records every key no rule consumed, at any depth, as a path (`Request::unplaced`). `encode` on a cross-style wire returns the paths it couldn't place with the body. Opaque content still refuses the encode.
  - `encode` is no longer called for same-style attempts, so opaque content there is not an error.
- [X] T149 [US1] Implement the same-style response path in `crates/zerorouter-wire/src/codec/response.rs` and `crates/zerorouter-server/src/relay.rs`. Depends on T028, T039. A non-stream same-style body is returned as received; usage and in-band errors are read from it without rebuilding. The stream path keeps R5's rules.
- [X] T150 [US1] Add `dropped: Vec<Dropped { path, reason }>` to `Attempt` in `crates/zerorouter-engine/src/records.rs` ([data-model § Attempt](data-model.md#attempt)), and show it in `records get`. Depends on T035. Unit-test that no value is stored, only the path.
- [X] T151 [US1] Implement the same-style header rule in `crates/zerorouter-engine/src/forwarding.rs` and `upstream.rs::build_request`. Depends on T013, T034. Same-style attempts send every client header except the floor, hop-by-hop headers, `x-0router-*` and `accept-encoding`, then apply the secret-value and CR/LF checks and any declared `merge` rule. Cross-style attempts keep the declared list (T122).
- [ ] T152 [US1] Add the headroom chain runner to `tests/harness/` (extends T058; SC-014, US1-11).
  - Start `headroom proxy --port <free port> --anthropic-api-url http://127.0.0.1:<0router port> --openai-api-url http://127.0.0.1:<0router port>/v1`. Point the `anthropic` and `openai` Python SDKs at headroom, streamed and not streamed.
  - The mock provider asserts that headroom's added fields and headers arrived on same-style routes, and the records list the drops on cross-style routes.
  - Skipped with a message when `headroom` is not on `PATH`. It counts as a harness for SC-001 only when it ran.

### Implementation for User Story 1

- [ ] T049 [P] [US1] Write `styles/bundled/anthropic-messages.toml`.
  - Routes: `POST /v1/messages`, plus the `anthropic-version`-discriminated `GET /v1/models`, `GET /v1/models/{model*}` and `/v1/messages/count_tokens`. The last three are wired in US6; declare them here.
  - Carriers: `x-api-key`, then Bearer.
  - Session carriers: the `claude_code_user_id` extractor, then `x-claude-code-session-id`.
  - The full `[text]` codec, including stream events (`message_start`, `content_block_*`, `message_delta`, `message_stop`, `ping`).
  - `[errors]` with a top-level `zerorouter` field and the type map from the contract.
- [ ] T050 [P] [US1] Write `styles/bundled/openai-chat.toml` with its text routes and codec.
  - Routes: `/v1/chat/completions`, plus the default `GET /v1/models`.
  - Carrier: Bearer. Session carriers: Codex `session_id` header, then the `prompt_cache_key` body path.
  - Stream: `sse_data_done`, implicit blocks, tool-argument fragments.
  - Errors: `error.zerorouter`.
  - The non-text sections come in US3.
- [ ] T051 [P] [US1] Write `styles/bundled/openai-responses.toml`.
  - Route `/v1/responses`. `/v1/responses/input_tokens` is declared here and wired in US6.
  - The `output_item` layouts, and the named SSE events `response.created`, `response.in_progress`, `response.output_item.added`/`done`, `response.output_text.delta`/`done`, `response.function_call_arguments.delta`/`done`, `response.completed` and `response.failed`, with `{sequence.number}`, `{output.index}` and `{response.rendered}`.
- [ ] T052 [P] [US1] Write `styles/bundled/gemini.toml`.
  - Routes: `:generateContent` and `:streamGenerateContent` on `/v1beta/models/{model*}`.
  - Carriers: `x-goog-api-key`, then `?key=`, then Bearer.
  - Session header as the Gemini CLI sends it.
  - Codec: `system_instruction`, `function_call_part`/`function_response_part`, tool results matched by `name`, and the `gemini_thinking_config` thinking form.
  - Stream: `sse_data` with `?alt=sse`, or a JSON array otherwise.
  - Errors: `error.{code,message,status,zerorouter}`.
- [ ] T053 [US1] Add the `claude_code_user_id` session extractor and session carrier handling in `crates/zerorouter-wire/src/primitives/session.rs`, and the `ses_sha256_hex32` / `ses_time_base62` derivations (9router `opencode-session` parity, oracle `tests/unit/opencode-session.test.js` → `tests/fixtures/9router/session/*.json` via T042's generator hook).
- [ ] T054 [US1] Seed the chosen four text providers as schema 2 ([R17](research.md#r17-provider-schema-2-and-the-chosen-five)).
  - Change `generate.mjs` to write `tools/gen-bundled/seeds/{anthropic,openrouter,opencode-zen,opencode-go,elevenlabs}.json` and to stop writing those five into `plugins/bundled/`.
  - Hand-write `plugins/bundled/anthropic.toml`, `openrouter.toml`, `opencode-zen.toml` and `opencode-go.toml` as schema 2, with the text endpoints only:
    - **anthropic**: the `anthropic-messages` wire, `anthropic-version: 2023-06-01` as its only static header, and `[token_count]`;
    - **openrouter**: the `openai-chat` and `anthropic-messages` wires;
    - **opencode**: per-wire endpoints under `/zen/v1` and `/zen/go/v1`, per-model `wires` from the seed, `force_stream` where 9router forces it, and `[session] header = "x-opencode-session", derive = "ses_sha256_hex32"`.
  - No `systemone` section, no fingerprint tools, no User-Agent spoofing.
  - Add `[[deviation]]` rows for every slice 002 parity field that now differs.
- [ ] T055 [US1] Implement the happy-path attempt in `crates/zerorouter-engine/src/attempt.rs` and `src/plan.rs`. Depends on T027, T028, T034, T035, T054, T148–T151.
  - Resolve the target (slice 002 `resolve`), build a one-candidate `RequestPlan` (first account in operator order), and choose the endpoint: the native pair first, then the model's `wires` order.
  - Build the body: `forward` (T148) on a same-style endpoint, `encode` otherwise, and put the drop list on the attempt (T150). Send, and read frames into IR events on a bounded channel.
  - Honour the `CancellationToken` with `tokio::select!` on every await.
  - Finish the record as `succeeded`, `failed` or `cancelled`.
  - When streaming to a Chat wire, set `stream_options.include_usage = true`, and strip the extra usage chunk if the client didn't ask for it ([R13](research.md#r13-usage-and-records)).
  - `force_stream`: collect the stream into a non-stream client response through the `Response` IR.
- [ ] T056 [US1] Wire text generation in `crates/zerorouter-server/src/router.rs` and `relay.rs`. Depends on T055. Route `op = generate`, `type = text` → decode (T027) → engine → relay (T039) in the client style, streamed or not.
- [ ] T057 [US1] Implement `accounts add` and `keys issue` in `crates/zerorouter-cli/src/cmd/accounts.rs` and `keys.rs`.
  - `accounts add <provider> <name> [--env VAR] [--order N]` reads the secret from stdin, never argv, and warns if stdin is a TTY without hiding the input.
  - `keys issue <name> [--break restart|error_event]` prints the key once.
  - Both write through T030/T031.
- [ ] T058 [US1] Write the harness scripts in `tests/harness/` and the runner `tests/harness/run.sh`.
  - Python and Node scripts per style: `openai` chat and responses, `anthropic` messages, `google-genai` generateContent.
  - Each sends one streamed and one non-streamed text request to a running `zerorouter serve` backed by mock upstreams, and exits non-zero on any SDK error.
  - Also Claude Code (`claude -p`, with `ANTHROPIC_BASE_URL`) and Codex (`codex exec`, with `OPENAI_BASE_URL`) runners, each skipped with a message when the tool is absent.
  - `run.sh` fails unless at least two harnesses ran (SC-001).
  - Wire it as `crates/zerorouter-server/tests/harness.rs`, marked `#[ignore]` unless `ZR_HARNESS=1`.
- [ ] T059 [US1] Run `cargo test -p zerorouter-wire -p zerorouter-server` (including T146–T147) and `ZR_HARNESS=1 cargo test -p zerorouter-server --test harness` (including T152's headroom chain), then fix until green. Run slice 002's parity tests and confirm that only listed deviations differ.
- [ ] T060 [US1] *operator-run* Live smoke: ask the user to run `! ZR_LIVE=1 cargo test -p zerorouter-engine --test live -- text`. It sends one streamed and one non-streamed request per text provider with their accounts. Also check that opencode API-key requests succeed without fingerprint tools ([R4](research.md#r4-translation-behaviour-parity-and-deliberate-deviations)). Write the test in `crates/zerorouter-engine/tests/live.rs` (skipped unless `ZR_LIVE=1`).

**Checkpoint**: MVP. A standard client gets text answers in its own style from each text
provider.

---

## Phase 4: User Story 2 — A transient failure doesn't reach the client (Priority: P1)

**Goal**: 429, 5xx, timeouts and connection failures are absorbed in the order same account
→ other account → other member. Only when everything fails does the client see an
informational error in its own style.

**Independent Test**: [quickstart §3](quickstart.md#3-failures-dont-reach-the-client-us2-sc-002-sc-003-sc-009).

### Tests for User Story 2 ⚠️

- [ ] T061 [US2] Add the classification oracle to `generate.mjs`. Run `checkFallbackError` over the `tests/unit/account-fallback-4xx.test.js` cases plus a generated grid (statuses 400–599 × the ERROR_RULES texts × JSON and plain bodies), and write `tests/fixtures/9router/classify/cases.json`.
- [ ] T062 [P] [US2] Write classification parity in `crates/zerorouter-engine/tests/classify.rs` against `tests/fixtures/9router/classify/cases.json`: the verdict, the cooldown, and the backoff level progression (2000·2^(level−1), capped at 300 000, max level 15).
- [ ] T063 [P] [US2] Write `crates/zerorouter-engine/tests/retry.rs` for every row of the [R7](research.md#r7-retry-order-and-budgets) budget table:
  - 502, network error and connect timeout: 3 retries at 3 s;
  - 503: 3 × 2 s;
  - 504: 2 × 3 s;
  - 429 with `retry-after` ≤ 5 s: one retry after the indicated wait;
  - 429 with a wait > 5 s: no retry, move on at once;
  - other 5xx and 529: 1 × 2 s;
  - 401–404: no retry;
  - a plugin `retry` override.

  Use tokio paused time.
- [ ] T064 [P] [US2] Write `crates/zerorouter-engine/tests/fallback.rs`.
  - For a unified model with two accounts and two members, cover every failure kind: the order is same account → other account → other member (SC-002).
  - A direct `<provider>/<model>` target stops after its accounts (US2-4).
  - A non-fallback 4xx is returned at once with its status and the upstream message first (US2-5).
  - A member with no account or not installed is a `skipped` attempt with a reason.
  - A `CannotCarry` target is skipped.
- [ ] T065 [P] [US2] Write `crates/zerorouter-engine/tests/stay_warm.rs`.
  - After a move to the backup account, the agent's next request goes to backup. Once backup fails over and main recovers, the next request goes to main (SC-003).
  - A warm account in cooldown is skipped until the cooldown ends.
  - With two concurrent requests from one agent, the last success wins.
  - A stream that breaks isn't counted as warm.
- [ ] T066 [P] [US2] Write `crates/zerorouter-engine/tests/timeouts.rs`.
  - No response headers within `timeout_ms` counts as a 502 and is retried.
  - No byte for `stall_timeout_ms` is a break, including on a `force_stream` body collected for a non-stream client.
  - `envMs` parsing: an integer > 0, else the default.
- [ ] T067 [P] [US2] Write `crates/zerorouter-server/tests/errors.rs`.
  - For each style, when all attempts fail: status 503, `retry-after` equal to the seconds until the earliest cooldown ends, and a body per [contracts/client-surface.md § Informational error body](contracts/client-surface.md#informational-error-body). The message lists every attempt and the record id, the `zerorouter` field has the same attempts, and the header id equals the body id (US2-6).
  - While the engine retries, the stream sends keepalives and holds the preamble: no `message_start` is sent twice.
  - A client disconnect during a backoff sleep cancels it, and no further upstream request starts.

### Implementation for User Story 2

- [ ] T068 [US2] Port the classification in `crates/zerorouter-engine/src/classify.rs` ([R6](research.md#r6-error-classification)).
  - Text rules come first: lowercase substring, first match wins, against `"[<status>]: <raw body>"`.
  - The status rules come next.
  - Result: `Verdict { class, fallback: bool, cooldown: Cooldown }`, with the classes from data-model `Attempt.class`.
- [ ] T069 [US2] Implement cooldowns in `crates/zerorouter-engine/src/cooldown.rs`.
  - Keyed by "`(provider id, account name, model id) → { until: Instant, backoff_level: u8 (≤ 15) }`".
  - A success clears the model's cooldown and, when no other cooldown is active, resets the level (9router `auth.js:326-333`).
  - Provide `earliest_end()` for `retry-after`.
- [ ] T070 [US2] Extend `crates/zerorouter-engine/src/plan.rs` with the full candidate order and the `WarmMap`.
  - Order: the warm account first (if not cooling), then that provider's remaining accounts in operator order, then the other members (unified targets only), each with their accounts.
  - The `WarmMap` is keyed by "`(AgentId, Target) → (provider id, account name)`". It is "updated on successful completion only" (stream end).
- [ ] T071 [US2] Extend `crates/zerorouter-engine/src/attempt.rs` into the full attempt loop.
  - Same-account budgets per R7, with plugin overrides.
  - Parse `retry-after` (seconds or HTTP date) and the reset headers.
  - Record attempts with kind `initial | same_account_retry | next_account | next_member | skipped`.
  - Backoff sleeps sit inside `tokio::select!` with the cancellation token.
  - Apply the connect timeout and the stall watchdog to every streamed and chunked body ([R10](research.md#r10-timeouts)).
  - A break before any content event is an ordinary transient failure.
- [ ] T072 [US2] Add the preamble hold and keepalive in `crates/zerorouter-wire/src/stream/writer.rs` and `crates/zerorouter-server/src/relay.rs`.
  - Header events are held until the first content event or the end of the attempt, and dropped if the attempt is replaced.
  - Between attempts, send the style's `[errors] keepalive` (Messages `event: ping`, an SSE comment for the others).
- [ ] T073 [US2] Implement informational error bodies in `crates/zerorouter-wire/src/error_body.rs` ([R11](research.md#r11-informational-errors)) and use them in the server's error path.
  - The message is a one-line summary with the record id, then one line per attempt: `provider/account model: reason`.
  - A non-fallback upstream error keeps its upstream message verbatim first.
  - The `zerorouter` details are `{ record_id, attempts: [{provider, account, model, status, class, reason, retries}] }`.
  - The type comes from the style's `type_map`.
  - Every string passes through the redactor.
- [ ] T074 [US2] Add the SDK error checks to `tests/harness/`. For each style's SDK (Python and Node), an all-attempts-failed request must raise the SDK's normal API error type, not a parse error, with the record id readable from the message (SC-009).
- [ ] T075 [US2] Run `cargo test -p zerorouter-engine -p zerorouter-server` and the harness, then fix until green.

**Checkpoint**: US1 and US2 together satisfy the slice's first two fail conditions.

---

## Phase 5: User Story 3 — Every model type runs through the same pipeline (Priority: P1)

**Goal**: Embeddings, image, TTS, STT and video run with the same keys, unified models,
retry, fallback, errors and records as text.

**Independent Test**: [quickstart §4](quickstart.md#4-every-model-type-us3-sc-007).

### Tests for User Story 3 ⚠️

- [ ] T076 [P] [US3] Write `crates/zerorouter-server/tests/types.rs` with the mock upstream.
  - One test per type through the OpenAI routes:
    - openrouter embeddings;
    - openrouter image (`b64_json`);
    - openrouter TTS;
    - elevenlabs TTS (binary, streamed);
    - elevenlabs STT (multipart `file` + `model_id=scribe_v2`);
    - openrouter video (submit 202 → poll → content through a `vj_` id).
  - Also through the Gemini routes: `embedContent`, `batchEmbedContents`, image and TTS via response modality, and `predictLongRunning` + `operations/{id}`.
  - Each record carries its type and usage (US3-1 to US3-3).
- [ ] T077 [P] [US3] Write the non-text fallback test in `crates/zerorouter-engine/tests/fallback_types.rs`: a unified embeddings model and a unified TTS model, each with two members, fall back exactly like text when the first member fails (US3-4, SC-007).
- [ ] T078 [P] [US3] Write the type-mismatch test in `crates/zerorouter-server/tests/type_mismatch.rs`: an embeddings model on `/v1/chat/completions` and a TTS request to an embeddings model each return 400 in the style's shape, naming both types. The mock received nothing (US3-5, FR-012).

### Implementation for User Story 3

- [ ] T079 [P] [US3] Add the non-text primitives in `crates/zerorouter-wire/src/primitives/`:
  - `media.rs`: `data_url`, `anthropic_source`, `gemini_inline_data`, `url`;
  - `embeddings.rs`: `float`, `base64_f32le`;
  - `body.rs`: `json`, `multipart` (file parts from `{input.audio}`), `binary`;
  - `audio.rs`: `chat_audio_delta_collect`, which collects `delta.audio.data` from a chat stream into audio bytes (9router parity).

  Include unit tests.
- [ ] T080 [US3] Add the non-text sections to `styles/bundled/openai-chat.toml` ([R16](research.md#r16-non-text-model-types)).
  - Routes and codecs: `/v1/embeddings`, `/v1/images/generations`, `/v1/audio/speech` (binary response), `/v1/audio/transcriptions` (multipart request), and `POST /v1/videos`, `GET /v1/videos/{id}`, `GET /v1/videos/{id}/content`.
- [ ] T081 [US3] Add the Gemini non-text sections to `styles/bundled/gemini.toml`.
  - `:embedContent` and `:batchEmbedContents`.
  - Image and TTS: the `:generateContent` route selects the type by `generationConfig.responseModalities`.
  - `:predictLongRunning` and `GET /v1beta/operations/{id}`.
- [ ] T082 [US3] Add the openrouter non-text endpoints to `plugins/bundled/openrouter.toml`.
  - Embeddings: `POST /api/v1/embeddings`.
  - Image: `POST /api/v1/images`.
  - Video: `POST /api/v1/videos`, then poll `GET /api/v1/videos/{id}`.
  - TTS: its speech endpoint, pending the T087 live check. Otherwise use the chat audio modality with `chat_audio_delta_collect`.
  - Models per type come from the seed.
- [ ] T083 [US3] Rewrite `plugins/bundled/elevenlabs.toml` as schema 2 (FR-013).
  - TTS: `POST /v1/text-to-speech/{voice}` and `/stream`, `xi-api-key`, binary audio, the `output_format` query, and voices from the seed.
  - New STT section: `POST /v1/speech-to-text`, multipart, `body = { file = "{input.audio}", model_id = "{model.upstream_id}", language_code = "{input.language?}" }`, `response = { text = "text", language = "language_code" }`, and `models = ["scribe_v2"]`.
  - Never set `webhook`.
  - Add `[[deviation]]` rows for the parity fields that differ.
- [ ] T084 [US3] Generalise the engine over model types in `crates/zerorouter-engine/src/attempt.rs`.
  - The candidate's endpoint is chosen by `ModelType`.
  - Inline `body`/`response` endpoints are rendered with the wire templates.
  - Voice resolution: the request's `voice` field (OpenAI), the speech config (Gemini), or 9router's `model/voice` string for parity.
  - Check the route's type against the target model's type before any upstream call (FR-012).
- [ ] T085 [US3] Implement video jobs in `crates/zerorouter-engine/src/jobs.rs`.
  - The `JobMap` maps "`zerorouter job id → (provider, account, upstream job id, record id)`", with ids `vj_…`.
  - Retry and fallback apply at submission only. Each client poll makes one upstream poll on the owning account (60 s bound, R7 retries on that account).
  - The record stays `in_progress` until the final content is delivered or the job fails.
- [ ] T086 [US3] Relay binary and job responses in `crates/zerorouter-server/src/relay.rs`: `Body::from_stream` for audio, with `content-type` from the upstream, and the job submit, poll and content routes.
- [ ] T087 [US3] *operator-run* Live types check. Ask the user to run `! ZR_LIVE=1 cargo test -p zerorouter-engine --test live -- types`. It covers one request per type and provider, whether openrouter's speech endpoint works (T082 choice), and whether `scribe_v1` is accepted. Update the plugins from the result.
- [ ] T088 [US3] Run `cargo test -p zerorouter-server --test types --test type_mismatch` and `-p zerorouter-engine --test fallback_types`, then fix until green.

**Checkpoint**: All three P1 stories work. The slice is usable end to end for every model
type.

---

## Phase 6: User Story 4 — A broken stream continues instead of wasting the answer (Priority: P2)

**Goal**: After output has reached the client, a break continues where the target declares
support. Otherwise the operator's choice applies: restart with a visible note (the default)
or an error event.

**Independent Test**: [quickstart §5](quickstart.md#5-stream-breaks-us4-sc-008).

### Tests for User Story 4 ⚠️

- [ ] T089 [P] [US4] Write `crates/zerorouter-engine/tests/breaks.rs`, run in each client style.
  - **Continuation**: a cut after 20 text deltas with a continuation-capable next target gives one uninterrupted answer. There is no repeated or missing delta, the partial answer was sent as the trailing assistant turn (trailing whitespace trimmed), the record has two segments with usage added, and `break_handling = continued` (SC-008, US4-1).
  - **Restart**, the default: the open block closes. A new text block holds exactly `— connection lost, answer restarted —` (in Chat and Gemini, a text delta with blank lines around it). The new answer follows with shifted block indexes, output indexes and sequence numbers (US4-2).
  - **Error event**, via a key override: the style's stream error event with the record id, then a clean end (US4-3).
  - **Cut while tool-call arguments are streaming**: always an error event, and the record gives the reason.
  - **Cut before any content**: an ordinary retry with no note (US4-4).
  - **A continuation target excluded** by `unless = thinking_enabled`, by `except_models`, or by an untranslatable partial falls through to the operator's choice.
- [ ] T090 [P] [US4] Write the stream-parser replay test in `tests/harness/`. The restart and error-event streams from T089 for Messages, Chat, Responses and Gemini are fed to each SDK's stream parser (Python and Node), which must finish without an error. Run Claude Code (if present) against a mock that cuts once, and check that it completes the turn. Also check that Claude Code's next turn succeeds after a cut thinking block ([R9](research.md#r9-mid-stream-breaks-continuation-restart-error-event)).

### Implementation for User Story 4

- [ ] T091 [US4] Implement break handling in `crates/zerorouter-engine/src/breaks.rs` per the [data-model break transitions](data-model.md#clientstreamstate).
  - `open_block = ToolCall{args_started}` → error event.
  - A continuation target is available → continue: the original request plus `partial_text` as the trailing assistant turn, the continuation's preamble suppressed, its first text block merged into the open block, and its usage added.
  - Otherwise, when the behaviour is `restart`: close the block, add the note block, re-send the original request to the next target, and shift the indexes.
  - Otherwise: close the block and send the error event.
  - Eligibility: the endpoint declares `[continuation]`, the model is in `models` (or not in `except_models`), no `unless` condition holds, and the partial answer encodes into the target wire (no `CannotCarry`).
- [ ] T092 [US4] Add the restart note and the index shifting to `crates/zerorouter-wire/src/stream/writer.rs`. The exact note text is `— connection lost, answer restarted —`. Styles with explicit blocks get a new block; implicit styles get a delta surrounded by `\n\n`. Keep the counters across segments.
- [ ] T093 [US4] Resolve the break behaviour in `crates/zerorouter-engine/src/state.rs`: the agent key's `break_behaviour` if set, else `[pipeline] break_behaviour`, else `restart`.
- [ ] T094 [US4] Implement `behaviour set-break restart|error_event` in `crates/zerorouter-cli/src/cmd/behaviour.rs` (writes `config.toml`) and `keys set-break <name|id> restart|error_event|default` in `crates/zerorouter-cli/src/cmd/keys.rs` (FR-031). Both hot-apply through T104 when a server runs.
- [ ] T095 [US4] Declare continuation in `plugins/bundled/anthropic.toml`: `method = "assistant_prefill"`, `trim_trailing_whitespace = true`, `unless = ["thinking_enabled", "tool_call_in_progress"]`, and `models = ["claude-sonnet-4-20250514", "claude-opus-4-20250514", "claude-3-5-sonnet-20241022"]`. These stay subject to T096. Declare nothing for openrouter or opencode until T096 confirms a model family.
- [ ] T096 [US4] *operator-run* Live continuation check. Ask the user to run `! ZR_LIVE=1 cargo test -p zerorouter-engine --test live -- continuation`. It confirms that prefill continues on each T095 model, that Claude 4.6+ returns 400 for prefill, and whether openrouter or opencode families accept prefill. Remove any declaration the check doesn't confirm.
- [ ] T097 [US4] Run `cargo test -p zerorouter-engine --test breaks` and the replay harness, then fix until green.

**Checkpoint**: Stream breaks never waste a finished part silently, and every outcome is
visible in the record.

---

## Phase 7: User Story 5 — The operator sees what happened to every request (Priority: P2)

**Goal**: Complete records (attempts, TTFT, total, exact usage including cache tokens)
queryable from the CLI, account and key management with hot apply, and no secret anywhere.

**Independent Test**: [quickstart §6](quickstart.md#6-records-us5-sc-004-sc-005-sc-006).

### Tests for User Story 5 ⚠️

- [ ] T098 [US5] Add the usage oracle to `generate.mjs`. Run 9router's usage extraction (`extractUsage*`, `concerns/usage.js`) over the cases in `tests/unit/cached-token-usage.test.js`, `extract-usage-cache-shapes.test.js`, `openai-responses-usage-completed.test.js`, `usage-concern.test.js` and `opencode-go-usage.test.js`, and write `tests/fixtures/9router/usage/*.json`.
- [ ] T099 [P] [US5] Write `crates/zerorouter-wire/tests/usage.rs`, covering:
  - parity with `tests/fixtures/9router/usage/`;
  - Responses nested `input_tokens_details.cached_tokens` → `cache_read` (US5-2);
  - openrouter `prompt_tokens_details.cache_write_tokens` → `cache_write`;
  - Gemini `cachedContentTokenCount` and `thoughtsTokenCount`;
  - an absent field → `None` ("not reported"), never 0 and never an estimate;
  - the Messages→Chat semantics conversion.
- [ ] T100 [P] [US5] Write `crates/zerorouter-engine/tests/usage_records.rs`. For every chosen text provider × client style, the recorded input, output, cache-read and cache-write equal the mock's reported numbers, and so does the client-visible usage (SC-004).
- [ ] T101 [P] [US5] Write `crates/zerorouter-server/tests/timing.rs`. A mock with a scripted delay before the first content and before the end must give recorded `ttft_ms` and `total_ms` within 10 ms of what the test client measures (SC-005). A streamed record is never missing TTFT.
- [ ] T102 [P] [US5] Write `crates/zerorouter-server/tests/secrets.rs`.
  - Configure sentinel secrets (`SENTINEL-PROVIDER-…`, and an agent key) and have the mock echo the provider secret in an error body and a header.
  - Scan the captured logs, every record, every client response (bodies and headers), CLI output, the headers the mock received beyond the auth header, and the plugin-visible `Registry` debug output. There must be zero occurrences (SC-006, US5-5, FR-033).
- [ ] T103 [P] [US5] Write `crates/zerorouter-cli/tests/operator.rs` against a running test server.
  - `records list --provider`, `--model` and `records show` return exactly the matching records (US5-3).
  - `accounts add`, `list`, `remove`, `disable` and `enable`, and `keys issue`, `list` and `revoke`, apply to the next request without a restart (US5-4).
  - A revoked key is rejected at once.
  - `accounts list` and `keys list` show only `…last4`.
  - With no server running, `records` exits with code 4 and mutating commands print `saved; applies at next start`.

### Implementation for User Story 5

- [ ] T104 [US5] Implement the operator socket in `crates/zerorouter-server/src/operator.rs` ([contracts/operator-cli.md § Operator socket protocol](contracts/operator-cli.md#operator-socket-protocol)).
  - A Unix socket at `$ZEROROUTER_HOME/run/operator.sock`, mode 0600, carrying NDJSON: `reload`, `records.list`, `records.get` and `accounts.state`.
  - A failed reload keeps the previous snapshot and returns the error.
  - Remove a stale socket at start.
- [ ] T105 [US5] Complete record filling in `crates/zerorouter-engine/src/attempt.rs` and `src/records.rs`.
  - Usage is merged per field (last value wins) and summed across segments.
  - `ttft_ms` is taken at the first content event written to the client, and `total_ms` at the last byte written. Both are measured from request arrival at the server's socket write, so the server reports write times back to the engine.
  - Carry `break_handling`, `served_by`, `job` and `model_type`.
  - Every string field passes through the redactor.
- [ ] T106 [US5] Implement `records list [--provider P] [--model UNIFIED] [--limit N] [--json]` and `records show <rq_id> [--json]` in `crates/zerorouter-cli/src/cmd/records.rs`. The text layout follows [contracts/operator-cli.md § records show](contracts/operator-cli.md#records-show-output-text), with `not reported` for `None` usage fields. Each attempt lists its `dropped` paths and reasons (T150).
- [ ] T107 [US5] Complete `accounts list|remove|disable|enable` in `crates/zerorouter-cli/src/cmd/accounts.rs` and `keys list|revoke` in `crates/zerorouter-cli/src/cmd/keys.rs`. Each mutating command writes atomically and then sends `reload` over the socket, printing `applied` or `saved; applies at next start`.
- [ ] T108 [US5] Run `cargo test -p zerorouter-wire --test usage`, `-p zerorouter-engine --test usage_records`, `-p zerorouter-server --test timing --test secrets` and `-p zerorouter-cli --test operator`, then fix until green.

**Checkpoint**: Every request is traceable from its error message to its full record.

---

## Phase 8: User Story 6 — A client lists models and counts tokens in its own style (Priority: P2)

**Goal**: Model lists in all three list shapes, and token counting through the Messages,
Responses and Gemini count routes, using the provider's count or 9router's estimate.

**Independent Test**: [quickstart §7](quickstart.md#7-model-lists-and-token-counts-us6).

### Tests for User Story 6 ⚠️

- [ ] T109 [US6] Add the estimator oracle to `generate.mjs`. Run `estimateAnthropicInputTokens` over the 3 cases of `tests/unit/count-tokens.test.js` plus generated requests (system, tools, images, multi-turn), and write `tests/fixtures/9router/count/*.json`.
- [ ] T110 [P] [US6] Write `crates/zerorouter-server/tests/models.rs`.
  - OpenAI, Anthropic (with `anthropic-version`) and Gemini lists have the right shape.
  - They include every unified model and every direct model of every type, with the type in `zerorouter.type` or the Gemini `supportedGenerationMethods`.
  - A provider with no account is absent (US6-1, US6-2).
  - `get_model` works for a model containing `/`.
- [ ] T111 [P] [US6] Write `crates/zerorouter-server/tests/count.rs`.
  - An anthropic target: the count comes from the mock's `count_tokens` endpoint (US6-3).
  - An openrouter target: the estimate equals the oracle, the response has the `x-0router-estimate: true` header, and the record is `estimated` (US6-4).
  - Run in all three count styles, including the same retry order on a transient failure.

### Implementation for User Story 6

- [ ] T112 [P] [US6] Port the estimator in `crates/zerorouter-wire/src/estimate.rs`: ceil(chars/4) over system, tools and message parts of the request translated into the Messages shape, with exact parity to `tests/fixtures/9router/count/`.
- [ ] T113 [US6] Implement token counting in `crates/zerorouter-server/src/count.rs` and the engine. Route `op = count_tokens` resolves the target like generation. If the text endpoint declares `[token_count]`, translate to that wire and call it with the same retry order; otherwise estimate. Response shapes follow [contracts/client-surface.md § Token counting](contracts/client-surface.md#token-counting).
- [ ] T114 [US6] Implement model lists in `crates/zerorouter-server/src/models.rs`: `list_models` and `get_model` for the OpenAI shape (default), the Anthropic shape (discriminated by `anthropic-version`) and the Gemini shape, over unified models plus direct models on providers with at least one enabled account.
- [ ] T115 [US6] Add the Gemini routes `:countTokens`, `GET /v1beta/models` and `GET /v1beta/models/{model*}` to `styles/bundled/gemini.toml`. Make sure the count and list routes declared in T049 and T051 have their codecs.
- [ ] T116 [US6] Run `cargo test -p zerorouter-server --test models --test count`, then fix until green.

**Checkpoint**: Claude Code, Codex CLI and Gemini CLI can discover models and count tokens.

---

## Phase 9: User Story 7 — Provider and API-style specifics are data (Priority: P3)

**Goal**: Forwarding declarations move exactly the declared headers and body parts under the
floor, in-band errors are classified by declaration, and the gate rejects malformed styles
and plugins with actionable messages.

**Independent Test**: [quickstart §8](quickstart.md#8-specifics-as-data-security-floor-us7).

### Tests for User Story 7 ⚠️

- [ ] T117 [P] [US7] Add the style gate corpus in `crates/zerorouter-registry/tests/gate/invalid/styles/`, one file per case with an `.expected` diagnostic: `unknown-key`, `unknown-placeholder` (`{request.api_key}`), `bad-path-template`, `route-collision` (two files), `missing-codec`, `ambiguous-stream-rules`, `finish-map-incomplete`, `error-template-missing-message`, `bad-carrier-scheme`, `unknown-session-extractor`, `unknown-framing` and `expression-in-template` (`{a+b}`).
- [ ] T118 [P] [US7] Add the schema-2 provider corpus in `crates/zerorouter-registry/tests/gate/invalid/providers/`: `endpoint-unknown-type`, `url-private-ip`, `url-localhost`, `placeholder-in-host`, `wire-and-body`, `unknown-body-placeholder`, `body-secret-key`, `forwarding-wildcard-bare`, `forwarding-bad-merge`, `forwarding-body-secret-path`, `continuation-unknown-method`, `token-count-without-count-style`, `error-rule-bad-status`, `schema2-with-transport` and `model-type-without-endpoint`. Add `crates/zerorouter-registry/tests/gate/strict/forwarding-authorization.toml`, which loads with a diagnostic and the entry stripped in normal mode and is an error in strict mode.
- [ ] T119 [US7] Extend `crates/zerorouter-registry/tests/gate.rs`.
  - Every corpus file gets exactly its expected diagnostic.
  - The four shipped styles and the five bundled plugins pass in strict mode (US7-4).
  - `url-localhost` passes when `allow_private_endpoints = true`.
- [ ] T120 [P] [US7] Write `crates/zerorouter-engine/tests/forwarding.rs`.
  - From a cross-style client, a declared `anthropic-beta` reaches the anthropic mock and is appended to any static value, and an undeclared client header doesn't reach it (US7-1). From a same-style client, the undeclared header does reach it (FR-039).
  - The mock's `request-id` and `anthropic-ratelimit-requests-remaining` reach the client (US7-2).
  - A client's `x-api-key`, `authorization` and `cookie` never reach any mock, and a mock `set-cookie` never reaches the client (US7-3).
  - A declared header whose value contains a configured secret is dropped. A value with CR/LF is rejected.
- [ ] T121 [P] [US7] Write `crates/zerorouter-engine/tests/inband.rs`. A test plugin declares `errors.body` for an error inside a 200 body and `errors.stream` for a stream error event with a `status_map`. Both are classified with the declared status and trigger fallback (US7-5, FR-024).

### Implementation for User Story 7

- [ ] T122 [US7] Implement forwarding in `crates/zerorouter-engine/src/forwarding.rs`.
  - Upstream, cross-style attempts: client headers from the declaring `from_styles` only, with `merge = replace | append_csv`, then the floor, the secret-value check and the CR/LF check. Same-style attempts use T151's rule.
  - Downstream: provider headers through the `to_client.headers` allowlist (with `-*` suffix wildcards), then the floor, then the core headers.
  - `to_client.body` paths are copied verbatim, in native pairs only.
  - Use it from `upstream.rs::build_request` and the relay.
- [ ] T123 [US7] Declare the anthropic forwarding in `plugins/bundled/anthropic.toml`.
  - Upstream: `anthropic-beta` (`append_csv`) and `anthropic-version` (`replace`), both `from_styles = ["anthropic-messages"]`.
  - To the client: `request-id`, `retry-after` and `anthropic-ratelimit-*`.
  - Add the equivalent declarations for openrouter and opencode from their seeds, where 9router forwards anything.
- [ ] T124 [US7] Implement in-band error detection in `crates/zerorouter-engine/src/attempt.rs`. Endpoint `errors.body` rules are checked on 200 bodies, and `errors.stream` rules on stream frames. A match becomes a classified failure with the declared or mapped status and class `in_band`.
- [ ] T125 [US7] Run `cargo test -p zerorouter-registry --test gate` and `-p zerorouter-engine --test forwarding --test inband`, then fix until green.

**Checkpoint**: The core stays generic. Every provider- and style-specific fact is data that
passes the gate.

---

## Phase 10: User Story 8 — Other providers become installable community plugins (Priority: P3)

**Goal**: The bundle holds exactly the chosen five. The other 116 are an embedded community
set that installs whole or is refused whole with a message naming every unsupported part.

**Independent Test**: [quickstart §9](quickstart.md#9-community-plugins-us8-sc-012).

### Tests for User Story 8 ⚠️

- [ ] T126 [P] [US8] Add the fit corpus in `crates/zerorouter-registry/tests/gate/unsupported/`, each case with a golden `.expected` message in the [contract format](contracts/provider-schema-v2.md#fit-check): `oauth-auth`, `cookie-category`, `kiro-format`, `quirk`, `hook`, `executor-requires`, `web-search-section`, `systemone-section`, `schema-3`, `unknown-wire-style` and `mixed` (several parts, all listed).
- [ ] T127 [P] [US8] Write `crates/zerorouter-registry/tests/community.rs`.
  - A sweep over all 116 community plugins: each either fits and loads, or is refused with every unsupported part listed, file:line:col for each. No panic (SC-012).
  - A refused plugin contributes nothing to the snapshot: no models, no aliases, no unified-model members (US8-3).
  - The goldens from T126 match.
- [ ] T128 [P] [US8] Write `crates/zerorouter-cli/tests/plugins.rs`.
  - `plugins list --community` shows 116 entries with their fit status.
  - `plugins install qoder` exits 3 with the refusal message.
  - `plugins install deepseek` installs, appears in `zerorouter providers`, and serves a request against the mock once an account is added (US8-1).
  - `plugins uninstall` removes it.

### Implementation for User Story 8

- [ ] T129 [US8] Change `tools/gen-bundled/generate.mjs` to write the 116 non-chosen providers to `plugins/community/*.toml` (schema 1), to delete them from `plugins/bundled/`, and to stop emitting web search and web fetch sections.
  - Emit `requires = ["9router-executor:<id>"]` for providers that have a specialised 9router executor (`open-sse/executors/*` other than the default), and add the `requires` key to the schema-1 structs in `crates/zerorouter-registry/src/schema/plugin.rs`.
  - Commit the output on its own, naming the ref SHA.
- [ ] T130 [US8] Implement the schema 1 → 2 conversion in `crates/zerorouter-registry/src/convert.rs` per [contracts/provider-schema-v2.md § Schema 1 → 2 conversion](contracts/provider-schema-v2.md#schema-1--2-conversion-community-plugins).
  - `[transport]` → `[endpoints.text]`, and each capability endpoint → `[endpoints.<type>]`.
  - The format maps to a wire: `openai` → `openai-chat`, `claude` → `anthropic-messages`, `openai-responses`, `gemini`.
  - `image_to_text` → `vision = true`.
  - Keep the source spans for fit messages.
- [ ] T131 [US8] Implement the fit check in `crates/zerorouter-registry/src/fit.rs` ([R19](research.md#r19-fit-or-refuse-and-the-community-set)).
  - `FitVerdict::Fits | Unsupported { parts: [UnsupportedPart { span, path, value, reason }] }`.
  - Unsupported: OAuth or cookie/web-cookie auth (even alongside an API key), wires other than the four, web search, web fetch and systemone sections, quirks, hooks, `executor_params`, credential fallback, regions, media formats not implemented, any `requires` entry, and a schema other than 1 or 2.
  - The message format follows the contract exactly, ending "No part of this plugin was loaded."
- [ ] T132 [US8] Implement the community set in `crates/zerorouter-registry/src/community.rs` and `build.rs`: embed `plugins/community/*.toml`, precompute each `FitVerdict`, and provide `install(id, home)` (gate + fit, then copy to `$ZEROROUTER_HOME/plugins/`) and `uninstall(id, home)`.
- [ ] T133 [US8] Apply fit-or-refuse at load in `crates/zerorouter-registry/src/load.rs`. Every user plugin is fit-checked on every load, and an `Unsupported` one is skipped whole and reported in `LoadReport`. Add a test-only `parity_set()` (feature `parity`) that loads bundled + community with the fit check off.
- [ ] T134 [US8] Move slice 002's parity tests to `parity_set()` in `crates/zerorouter-registry/tests/parity/main.rs`, so they still see 121 providers (FR-036, US8-4), and make sure every chosen-five difference has a `[[deviation]]` row.
- [ ] T135 [US8] Implement `plugins list [--community]`, `plugins install <id>` (exit 3 on Unsupported) and `plugins uninstall <id>` in `crates/zerorouter-cli/src/cmd/plugins.rs`, followed by a socket `reload`.
- [ ] T136 [US8] Run `cargo test -p zerorouter-registry` (all targets) and `-p zerorouter-cli --test plugins`, then fix until green.

**Checkpoint**: All eight user stories work independently.

---

## Phase 11: Polish & Cross-Cutting Concerns

- [ ] T137 [P] Write the Criterion benches ([R24](research.md#r24-performance-and-benchmarks), SC-013).
  - `crates/zerorouter-wire/benches/wire.rs`: request translation for each of the 4×4 pairs, stream event translation throughput, usage extraction.
  - `crates/zerorouter-engine/benches/engine.rs`: the attempt loop against an instant mock, measuring the time to first byte that 0router adds (target p95 ≤ 10 ms).
  - `crates/zerorouter-server/benches/server.rs`: the access-key check, route matching, and an end-to-end loopback request.
- [ ] T138 Run the benches with `--save-baseline slice-003` and commit the summary table in `specs/003-request-pipeline/bench-baseline.md` (machine, date, median and p95 per bench).
- [ ] T139 Write the connection-reuse test in `crates/zerorouter-server/tests/reuse.rs`: N sequential requests to one mock host within the keep-alive window use one accepted connection (SC-011, FR-021).
- [ ] T140 [P] Write the documentation:
  - `docs/api-styles.md`, from [contracts/api-style-schema.md](contracts/api-style-schema.md);
  - updates to `docs/plugins.md` for schema 2, forwarding, the floor and the fit check;
  - updates to `docs/operator-config.md` for `accounts.toml`, `keys.toml`, `serve`, `records`, break behaviour and `allow_private_endpoints`.
- [ ] T141 Run `/rust-parity-audit` on `crates/zerorouter-engine/src/classify.rs`, `cooldown.rs`, `attempt.rs` (retry budgets and timeouts), `crates/zerorouter-wire/src/codec/`, `usage.rs` and `estimate.rs`. Fix every must-fix finding, and record the [R26](research.md#r26-deliberate-deviations-from-9router-summary) deviations as accepted.
- [ ] T142 Run a security review with the `security-auditor` agent. It covers the secret paths (accounts → upstream injection, redactor coverage, the floor, CLI input), SSRF (gate plus the resolved-IP re-check, no redirects), and the operator socket permissions. Fix every High finding.
- [ ] T143 Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`, and fix everything they report.
- [ ] T144 Run all 10 sections of [quickstart.md](quickstart.md) with a scratch `ZEROROUTER_HOME`, and correct the quickstart wherever the real output differs.
- [ ] T145 [P] Update `CLAUDE.md`: add the three new crates, `styles/bundled/`, `plugins/community/` and `tests/harness/` to the workspace table, and add the `zerorouter serve` run command.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: T001 blocks everything.
- **Foundational (Phase 2)**: depends on Setup and blocks every story.
  - Registry (T010–T021): the enums, templates, SSRF and floor (T010–T013) come first. Then the schemas (T014–T017), the gates (T018–T019), the load (T020) and the views (T021).
  - Wire (T022–T029) needs T011 only, so it can run in parallel with the registry gate work.
  - Engine (T030–T036) needs T012 and T020 for `upstream.rs` and `state.rs`. The rest is independent.
  - Server (T037–T040) needs T020, T026, T031 and T033.
- **US1 (Phase 3)**: depends on Foundational. It is the MVP. The pass-through amendment (T146–T152) comes before T049: T148–T151 change Phase 2 code that T049–T056 build on. T146, T147 and T152 can be written in parallel with them.
- **US2 (Phase 4)**: depends on US1's attempt path (T055–T056).
- **US3 (Phase 5)**: depends on US1 (T055) and US2 (T071, so fallback applies to every type).
- **US4 (Phase 6)**: depends on US2 (the attempt loop and preamble hold, T071–T072).
- **US5 (Phase 7)**: depends on US1. The record details from US2–US4 show up when those exist. T104 (socket) is needed for hot apply in T094 and T107.
- **US6 (Phase 8)**: depends on US1. It is independent of US2–US5.
- **US7 (Phase 9)**: depends on US1 (and US2 for the fallback in T121).
- **US8 (Phase 10)**: depends on Foundational and US1's T054 (the seeds). It is independent of US2–US7.
- **Polish (Phase 11)**: depends on every story.

### Story Graph

```text
Setup → Foundational → US1 (MVP) ─┬─▶ US2 ─┬─▶ US3
                                  │        ├─▶ US4
                                  │        └─▶ US7
                                  ├─▶ US5   (hot apply joins US4's T094)
                                  ├─▶ US6
                                  └─▶ US8   (needs only T054 from US1)
                                               ─▶ Polish
```

### Within Each Story

- Write the tests (and any oracle generation), then watch them fail.
- Then the data (style and plugin TOML), then the wire and engine, then the server and CLI.
- Finish with the green-run task.
- Operator-run live checks come after the mock tests pass. A live result can only remove or
  narrow a declaration.

---

## Parallel Examples

```text
# Phase 2: registry primitives together
T010 primitives.rs   T011 template.rs   T012 ssrf.rs   T013 floor.rs
# …with the wire IR and framing (no registry gate needed)
T022 ir/request.rs   T023 ir/event.rs   T025 framing.rs
# …and the engine state pieces
T030 accounts.rs   T031 keys.rs   T032 redact.rs   T035 records.rs   T036 mock_upstream.rs

# US1: four style files together, then tests together
T049 anthropic-messages   T050 openai-chat   T051 openai-responses   T052 gemini
T043 parity_translate   T044 deviations   T045 styles   T046 auth   T047 cancel   T048 passthrough

# US2 tests together
T062 classify   T063 retry   T064 fallback   T065 stay_warm   T066 timeouts   T067 errors

# After US1: US5, US6 and US8 can run in parallel with US2
US5: T099–T103   US6: T110–T112   US8: T126–T128
```

---

## Implementation Strategy

### MVP (User Story 1)

1. Phase 1, including the T001 toolchain check and the T008 harness check.
2. Phase 2.
3. Phase 3. **Stop and validate**: two harnesses in different styles get text answers from
   every text provider (mocks), and the live smoke test (T060) passes on the operator's
   accounts.

### Incremental Delivery

1. US1: a usable text gateway.
2. US2: failures absorbed. Together with US1, this meets the slice's first two fail
   conditions.
3. US3: every model type.
4. US4: stream breaks handled.
5. US5: full records and operator management.
6. US6: model discovery and token counting.
7. US7: the forwarding floor and declared errors, fully tested.
8. US8: the community set.
9. Polish: bench baseline, parity audit, security review, docs.

A commit per task or per logical group is fine. Generated artefacts (T042, T061, T098, T109,
T129) each go in their own commit, whose message names the `ref/9router` SHA.

---

## Notes

- Constraints quoted from the data model are binding. Do not relax them during
  implementation:
  - "`None` = not reported";
  - "`[a-z0-9_-]{1,32}`" for account names;
  - "`ak_` + 8 chars" for key ids;
  - "`rq_` + 26-char time-sortable id" for record ids;
  - "Ring of 10 000 records, oldest evicted";
  - "≤ 256 chars" for session ids;
  - "`backoff_level: u8 (≤ 15)`";
  - "updated on successful completion only" for the warm map.
- Never rewrite prompt content (Constitution IV). A part the target can't carry skips the
  target; it is never dropped.
- Never follow upstream redirects, and never pass a secret to anything plugin-visible.
- Never edit `ref/9router/` or `tests/fixtures/9router/` by hand. Regenerate them.
- Never add `tokio` or `unsafe` to `zerorouter-registry` or `zerorouter-wire`.
- The chosen five are hand-maintained after T054 and T083. `generate.mjs` must never write
  them again.
