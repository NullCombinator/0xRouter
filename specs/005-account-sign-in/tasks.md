---

description: "Task list for 005-account-sign-in"
---

# Tasks: Account Sign-In

**Input**: Design documents from `specs/005-account-sign-in/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included. The spec's failure conditions and success criteria (SC-001 to SC-010) are
test-matrix outcomes, Constitution VI requires parity checks for OAuth flows, and the benchmark
gate covers the request path. Within each story, write the tests first and confirm they fail
before implementing.

**Organization**: Tasks are grouped by user story, so each story can be implemented and tested
on its own.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US6)

## Path Conventions

Cargo workspace at the repo root ([plan § Project Structure](plan.md#project-structure)):

- `crates/nullrouter-registry/`: schema sections, gate, fit check (sync, no tokio)
- `crates/nullrouter-engine/`: accounts, tokens, sign-in flows, refresh, quota, maintenance
- `crates/nullrouter-server/`: operator socket ops, end-to-end tests
- `crates/nullrouter-cli/`: `accounts signin`, `quota`
- `plugins/bundled/`: the chosen seven after this slice
- `crates/nullrouter-engine/src/testkit/`: mock upstream (slice 003) plus the new mock identity
  provider and mock quota endpoints, behind the `testkit` feature

Build commands need `export CARGO_HOME=$PWD/.cargo-home`. Respect the build budget: `-j 2`,
`nice`, one crate at a time, never workspace test and clippy chained.

**Live checks** (tasks marked *operator-run*) need the operator's real accounts. Ask the user to
run the given command with `! …` in the session. Never ask for tokens, codes or keys.

---

## Phase 1: Setup (Shared Infrastructure)

- [X] T001 Raise `rust-version` from `"1.85"` to `"1.89"` in `Cargo.toml` (needed for `std::fs::File::lock`, research R1), and confirm `nice cargo check -p nullrouter-engine -j 2` still builds on the pinned 1.93.1 toolchain. Then, before any engine change, record a fresh Criterion baseline of the current engine bench (`nice cargo bench -p nullrouter-engine -j 2 -- --save-baseline pre-005`): the slice 002/003 baselines were lost to a `cargo clean` on 2026-10-02.
- [X] T002 [P] Create the module skeletons with `//!` headers only, wired into `lib.rs`: `crates/nullrouter-engine/src/tokens.rs`, `src/signin/mod.rs` (`device_code.rs`, `pkce.rs`, `loopback.rs`, `refresh.rs`, `classify.rs`, `dedup.rs`), `src/quota/mod.rs` (`extract.rs`, `grpc_web.rs`, `poll.rs`, `history.rs`, `tally.rs`), `src/maintenance.rs`, `src/identity.rs`.
- [X] T003 [P] Create the schema module skeletons in `crates/nullrouter-registry/src/schema/`: `signin.rs`, `identity.rs`, `quota.rs`, `models_live.rs`, re-exported from `schema/mod.rs`.
- [X] T004 [P] Create `crates/nullrouter-cli/src/cmd/quota.rs` with an empty `Command` enum and register the `quota` subcommand in `crates/nullrouter-cli/src/cmd/mod.rs`.

---

## Phase 2: Foundational (Blocking Prerequisites)

**⚠️ No user story can start until this phase is complete.**

### Registry: plugin sections and gate

- [X] T005 Define `SignInDecl` in `crates/nullrouter-registry/src/schema/signin.rs` per [contracts/signin-quota-schema.md § `[signin]`](contracts/signin-quota-schema.md): `flow` (`pkce` | `device_code`), `client_id`, `scopes`, `discovery_url?`, `authorize_url?`, `token_url`, `device_url?`, `redirect` (list of `{uri, kind = "loopback" | "code_page"}`), `params` (keys limited to `plan`, `referrer`, `code`, `audience`, `prompt`, `nonce`; values a fixed string or `{random.hex16}`), `body` (`form` | `json`), `verifier_bytes` (32–96), `refresh_lead` (duration), `auth` (`{header, scheme}`), `terms_warning` (bool), `profile?`, `refused` (list of `{status, body_contains}`). `deny_unknown_fields`.
- [X] T006 [P] Define `IdentityDecl` in `crates/nullrouter-registry/src/schema/identity.rs`: a header map whose values are a fixed string or exactly one placeholder from the closed set `{session.id}`, `{request.id}`, `{session.turn}`, `{model.upstream}`, `{account.email}`, `{account.user_id}`, `{install.id}` (spec Clarifications Q5, research R7). Parse into `HeaderValue::Fixed(String) | HeaderValue::Core(Placeholder)`.
- [X] T007 [P] Define `QuotaDecl` in `crates/nullrouter-registry/src/schema/quota.rs`: `accounts` (`signin` | `key` | `any`), `request` (`{url, method, headers, body = "none" | "grpc_web_empty"}`), `windows` (rules: `path` with `*`, `where`, `name` template with `{1}` and `{path|lower}`, `unit` in `percent` | `credits` | `requests` | `tokens`, `used` / `limit` / `remaining` (at least one), `resets_at`, `resets_format` in `auto` | `epoch_s` | `epoch_ms` | `rfc3339`, `unwrap_val`), optional `fallback` with `decoder = "json" | "grpc_web_ratio"`. Value paths accept `first_of` alternatives separated by `|`.
- [X] T008 [P] Define `ModelsLiveDecl` in `crates/nullrouter-registry/src/schema/models_live.rs`: `url`, `headers`, `list`, `id`, `name`, `context`, `max_output`, `type` (default `text`), `refresh` (duration).
- [X] T009 Add `force` to endpoint and `[[models]]` declarations in `crates/nullrouter-registry/src/schema/endpoint.rs` and `schema/plugin.rs`: keys limited to `store`, `reasoning.summary`, `reasoning.effort`, `include` (research R8). Add `signin`, `identity`, `quota`, `models_live` as optional fields on `PluginFile` and `ProviderEntity` (`schema/plugin.rs:19-110`).
- [X] T010 Gate rules in `crates/nullrouter-registry/src/validate/gate.rs` for T005–T009, with the messages in [contracts § Gate errors](contracts/signin-quota-schema.md#gate-errors-added): SSRF checks on every URL; every URL host within the plugin's endpoint hosts plus `[signin]` / `[quota]` hosts; unknown placeholder; forced parameter outside the set; security-floor header names refused in `[identity]`; flow-specific required and forbidden fields; secret-like values refused with the existing detector.
- [X] T011 Add `ProviderEntity::token_hosts()` in `crates/nullrouter-registry/src/schema/plugin.rs` returning endpoint hosts ∪ `[signin]` hosts ∪ `[quota]` (including fallback) hosts ∪ `[models_live]` host.
- [X] T012 Fit check in `crates/nullrouter-registry/src/fit.rs`: accept `[signin]`, `[quota]`, `[identity]` with placeholders and `[models_live]` only in bundled plugins; a community plugin declaring any of them is refused whole with "not supported by this core: account sign-in" (or "quota"). Keep the schema-1 `category = oauth` / `[oauth]` refusals for community plugins unchanged (FR-029).
- [X] T013 [P] Gate corpus: add invalid cases under `crates/nullrouter-registry/tests/gate/invalid/providers/` (unknown placeholder, foreign host in `[quota]`, forced `messages`, `x-api-key` in `[identity]`, device flow with `redirect`, secret-like `client_id`) and golden `.expected` messages; extend `crates/nullrouter-registry/tests/gate.rs` to run them.

### Engine: accounts, tokens, identity, records

- [X] T014 Accounts schema 2 in `crates/nullrouter-engine/src/accounts.rs`: `kind` (`key` | `signin`), `poll_interval` (optional duration, "default 10 min, floor 2 min"); `parse` accepts schema 1 (all accounts become `kind = "key"`) and 2, `to_toml` always writes 2; a `signin` account has no `secret`; name rule unchanged "`[a-z0-9_-]{1,32}`". Update `crates/nullrouter-cli/tests/accounts_keys.rs` for the schema-1 → 2 rewrite.
- [X] T015 Token store in `crates/nullrouter-engine/src/tokens.rs` per [data-model § Sign-in credentials](data-model.md#sign-in-credentials-tokenstoml-new): `TokenEntry` (`access_token`, `refresh_token?` as `SecretString`, `expires_at`, `scope`, `claims {email?, user_id?, tier?}`, `hosts`, `signed_in_at`, `last_refresh_at`, `state`, `state_since`, `state_reason`); `read_private` (refuse unless mode 0600); `update(home, provider, name, f)` that takes an exclusive `File::lock` on `tokens.lock`, re-reads, applies `f`, writes with `write_private`. Unit tests: mode refusal, two concurrent writers in threads never lose an update.
- [X] T016 Live token cells in `crates/nullrouter-engine/src/tokens.rs`: `TokenCells` (`ArcSwap<TokenView>` per sign-in account) on `Engine` (`state.rs:80-94`), loaded at `assemble`, kept across `reload_blocking` unless the file is newer; `AccountState` enum `Active | Refreshing{since, attempts} | NeedsSignIn{since, reason} | Refused{since, reason} | Disabled`.
- [X] T017 `accounts::release` (`accounts.rs:270-279`) for sign-in accounts: hand out the current access token only to a host in the entry's `hosts`; return `Withheld::NeedsSignIn`, `Withheld::Refused`, `Withheld::Refreshing` for out-of-service states. Extend `crates/nullrouter-engine/tests/host_binding.rs` with a sign-in account.
- [X] T018 [P] Install id in `crates/nullrouter-engine/src/identity.rs`: `install_id(home)` creates `$NULLROUTER_HOME/install-id` (random UUID v4, mode 0600) once and reads it after; `fill(placeholder, ctx)` for the seven placeholders, where `{session.id}` is the slice 003 session id or a random id kept per agent, `{request.id}` a fresh UUID, `{session.turn}` the count of user turns in the request, `{model.upstream}` the upstream id, `{account.*}` from the token claims (header omitted when the claim is missing).
- [X] T019 [P] Records in `crates/nullrouter-engine/src/records.rs`: add `ErrorClass::NeedsSignIn`, `ErrorClass::Refused`, `ErrorClass::TokenRefreshing`; add `forced: Vec<(String, Value)>` to `Attempt` next to dropped fields.
- [X] T020 Redactor generations in `crates/nullrouter-engine/src/redact.rs` and `state.rs`: `Redactor::new` takes key secrets plus current and previous access and refresh tokens; add `Engine::rebuild_redactor()` that swaps both the snapshot's and `Engine.redactor`'s copy without a full reload.
- [X] T021 Out-of-service accounts become recorded skips in `crates/nullrouter-engine/src/plan.rs` (`member()`, 144-182): instead of filtering, emit `Step::Skip` with class and reason `needs sign-in: run nullrouter accounts signin <provider> <name>`, `refused by provider: <reason>; run nullrouter accounts signin <provider> <name>`, or `token expired, refresh retrying`. Disabled accounts stay silently filtered (slice 003 behaviour).

### Engine: testkit

- [ ] T022 Mock identity provider in `crates/nullrouter-engine/src/testkit/mock_idp.rs`: device endpoint and token polling (`authorization_pending`, `slow_down`, `expired_token`, `access_denied`), PKCE authorize redirect and code exchange checking verifier and `state`, discovery document, refresh with optional rotation, scripted failures (permanent codes from research R10, 5xx, timeout), token lifetime per test (down to 2 s), counters for refresh calls.
- [ ] T023 [P] Mock quota endpoints in `crates/nullrouter-engine/src/testkit/mock_quota.rs`: anthropic `/api/oauth/usage`, grok-cli `/v1/billing` and `/v1/user` and the gRPC-web credits RPC, opencode `/zen/go/v1/usage` and `/zen/v1/usage`; responses settable per test, built from the shapes in research R11 and 9router's parser cases (`ref/9router/open-sse/services/usage/*`, `tests/unit/opencode-go-usage.test.js`).

**Checkpoint**: plugins with the new sections load or are refused; accounts carry kinds; tokens can be stored, read, released and redacted.

---

## Phase 3: User Story 1 — Sign in a subscription account and serve from it (Priority: P1) 🎯 MVP

**Goal**: The operator signs in anthropic, xai or grok-cli from the CLI, also over SSH with no
browser, and the account serves through slice 003's pipeline like a key account.

**Independent Test**: [quickstart §1–3](quickstart.md). With the mock identity provider and no
display variables, sign in one account per provider; two harnesses in different styles get
answers through each, streamed and not.

### Tests for User Story 1 ⚠️

- [ ] T024 [P] [US1] Flow tests in `crates/nullrouter-engine/tests/signin_flows.rs`: device code (pending → approved, slow_down adds 5 s, expired and denied end with that reason and write nothing); PKCE with loopback redirect; PKCE with pasted full URL, bare code (only for `code_page`), `code#state`; state mismatch refused; discovery off the declared hosts falls back to the declared URLs; 10-minute browser-flow timeout (shortened in test) writes nothing.
- [ ] T025 [P] [US1] CLI tests in `crates/nullrouter-cli/tests/signin.rs` with `DISPLAY`/`WAYLAND_DISPLAY` unset: `accounts signin grok-cli work` prints link, code and expiry; `accounts signin xai main` reads the pasted address from stdin; `accounts signin anthropic max` prints the terms warning, "n" exits 5 with nothing written, "y" completes; the warning appears again on a second sign-in of the same account (Clarifications Q2); `--accept-terms-risk` still prints it; replacing an existing account says so.
- [ ] T026 [P] [US1] Serving tests in `crates/nullrouter-server/tests/signin_serving.rs`: sign-in accounts on mock anthropic, xai, grok-cli serve Chat Completions, Messages and Responses clients, streamed and not; anthropic sign-in requests carry `Authorization: Bearer` and no `x-api-key`; declared `[identity]` headers arrive with placeholders filled; the request body reaching the mock equals the client's byte for byte on a same-style route with no forced parameters, and with an effort-suffixed model id differs only by `reasoning.effort`, which is in the record (FR-004a, FR-004c, research R8); xai image and video requests succeed and are recorded with their model type.
- [ ] T027 [P] [US1] Fallback test in `crates/nullrouter-engine/tests/fallback_signin.rs`: two grok-cli sign-in accounts plus a key-account member; injected 429, 5xx, timeout and connection failure on the first never reach the client; attempt order equals slice 003's (SC-004).
- [ ] T028 [P] [US1] `accounts list` test in `crates/nullrouter-cli/tests/accounts_keys.rs`: `kind` column, `…last4` of the access token, `--long` shows email, tier, expiry and last refresh; no token appears in full.

### Implementation for User Story 1

- [ ] T029 [P] [US1] PKCE helpers in `crates/nullrouter-engine/src/signin/pkce.rs`: verifier of `verifier_bytes` random bytes (base64url, no padding), S256 challenge, random `state`, `{random.hex16}` nonce; authorize URL builder with declared `params` and `%20`-encoded scopes; `parse_paste(input, redirect_kind)` accepting a full URL (state must match), `code#state`, or a bare code (only `code_page`).
- [ ] T030 [P] [US1] Loopback listener in `crates/nullrouter-engine/src/signin/loopback.rs`: bind the declared `127.0.0.1:<port>` (or `localhost` with an ephemeral port), accept one `GET /callback?…`, answer a short "you can close this page" HTML, return code and state; report `port busy` without failing the sign-in (paste-back still works).
- [ ] T031 [P] [US1] Device flow in `crates/nullrouter-engine/src/signin/device_code.rs` (RFC 8628) per research R3, using the engine's SSRF-checking, redirect-free client.
- [ ] T032 [US1] Token exchange and profile in `crates/nullrouter-engine/src/signin/mod.rs`: exchange code or device grant with `body = form | json` (anthropic JSON body includes `state`, per `ref/9router/src/lib/oauth/providers/claude.js:29-43`); discovery for xai with the host check (`ref/9router/src/lib/oauth/services/xai.js:26-47`); read claims from the id token payload (no signature check, display only) and the optional `[signin.profile]` call; return a `TokenEntry` with `hosts = token_hosts()`.
- [ ] T033 [US1] `accounts signin` in `crates/nullrouter-cli/src/cmd/accounts.rs` per [contracts/operator-cli.md § `accounts signin` output](contracts/operator-cli.md): open the browser only when a display is present (`xdg-open`/`open` via `std::process::Command`), race the loopback listener against a stdin paste, print the anthropic terms warning text from research R4 before the link and require `y`/`yes` every time; write the account (`kind = "signin"`) and the token entry under the lock; send `reload`; exit codes 0/1/5 per the contract.
- [ ] T034 [US1] Sign-in requests in `crates/nullrouter-engine/src/upstream.rs` (`auth_header`, 166-189; `build_request`, 200-241): for a sign-in account use `[signin] auth` placement; add `[identity]` headers after endpoint static headers, filled by `identity::fill`; apply `force` parameters to the outgoing body (same-style and translated) and push them to `Attempt.forced`.
- [ ] T035 [US1] Read the token from `TokenCells` in `Run::outgoing` (`crates/nullrouter-engine/src/attempt.rs:754-783`) with one atomic load; jobs (`jobs.rs`) release sign-in tokens the same way for xai video polling.
- [ ] T036 [P] [US1] Bundled `plugins/bundled/xai.toml` (schema 2, hand-written from `plugins/community/xai.toml` and research R6): `[endpoints.text]` openai-chat `https://api.x.ai/v1/chat/completions`, `[endpoints.image]` `https://api.x.ai/v1/images/generations`, `[endpoints.video]` submit and poll `https://api.x.ai/v1/videos/…`, `[signin]` PKCE with discovery, `redirect = [{ uri = "http://127.0.0.1:56121/callback", kind = "loopback" }]`, `verifier_bytes = 96`, params `plan = "generic"`, `referrer = "cli-proxy-api"`, `nonce = "{random.hex16}"`, `refresh_lead = "5m"`; models from the community file. Delete `plugins/community/xai.toml` (`build.rs` embeds the directory, so nothing else lists it).
- [ ] T037 [P] [US1] Bundled `plugins/bundled/grok-cli.toml`: `[endpoints.text]` openai-responses `https://cli-chat-proxy.grok.com/v1/responses`, `force_stream`, retry overrides (429 2×2000 ms, 502/503 2×1500 ms), no endpoint-level `force` (the body goes as the client sent it, spec FR-004c; live check L3 adds one only if grok-cli fails without it), per-model `upstream_id` + `force."reasoning.effort"` for the effort-suffixed ids; `[signin]` device code at `https://auth.x.ai/oauth2/device/code` with the grok-cli scopes and `referrer = "grok-build"`, `refresh_lead = "5m"`, `[signin.profile]` `…/v1/user`; `[identity]` fixed `User-Agent`, `x-grok-client-identifier`, `x-grok-client-version` plus `x-grok-session-id = "{session.id}"`, `x-grok-conv-id = "{session.id}"`, `x-grok-req-id = "{request.id}"`, `x-grok-turn-idx = "{session.turn}"`, `x-grok-model-override = "{model.upstream}"`, `x-email = "{account.email}"`, `x-userid = "{account.user_id}"`, `x-grok-agent-id = "{install.id}"`. Delete `plugins/community/grok-cli.toml`.
- [ ] T038 [US1] anthropic `[signin]` in `plugins/bundled/anthropic.toml`: PKCE, `authorize_url = "https://claude.ai/oauth/authorize"`, `token_url = "https://api.anthropic.com/v1/oauth/token"`, Claude Code's public client id and scopes `org:create_api_key user:profile user:inference` (from `ref/9router/open-sse/providers/registry/claude.js:67-76`), `params = { code = "true" }`, `redirect` = hosted code page (`code_page`) then `http://localhost:0/callback` (`loopback`), `body = "json"`, `refresh_lead = "4h"`, `auth = { header = "Authorization", scheme = "bearer" }`, `terms_warning = true`, `[identity]` fixed headers from `plugins/community/claude.toml:12` plus `anthropic-beta = "oauth-2025-04-20"` (merged with the client's beta list). No body change of any kind (Clarifications Q1).
- [ ] T039 [US1] Update `tests/parity/deviations.toml` with the research R19 rows and assert each in `crates/nullrouter-engine/tests/` (no cloaking: the outgoing anthropic body equals the input body; grok-cli input items untouched).
- [ ] T040 [US1] Parity fixtures from 9router's own tests (Constitution VI). Extend `tools/gen-bundled/generate.mjs` to run the inputs of `ref/9router/tests/unit/{xai-oauth-service,xai-tokenRefresh,token-refresh-dispatch,token-refresh-generic,background-token-refresh,grok-cli-oauth-probe,grok-cli-usage,grok-cli-quota-frame,grok-cli-models,opencode-go-usage,usage-dispatch}.test.js` through 9router's modules and write the outputs to `tests/fixtures/9router/oauth/` (authorize-URL parameters, token and refresh request bodies, refresh-error classification) and `tests/fixtures/9router/usage/` (quota responses in, windows out). Assert them in `crates/nullrouter-engine/tests/parity_oauth.rs` and `parity_usage.rs`; cases covered by research R19 are asserted as deviations, not skipped. Commit the generated output on its own, naming the ref SHA.
- [ ] T041 [US1] Add `"xai"` and `"grok-cli"` to `CHOSEN` in `tools/gen-bundled/generate.mjs:601` and write their seeds to `tools/gen-bundled/seeds/{xai,grok-cli}.json`, so regenerating neither deletes the bundled files nor recreates them as community plugins. Same commit as T036, T037 and the count update.
- [ ] T042 [US1] Update the bundle count from five to seven wherever slice 003 asserts it (`crates/nullrouter-registry/tests/plugins.rs`, `community.rs`, `smoke.rs`) and the community count from 116 to 114.
- [ ] T043 [US1] Harness run in `tests/harness/run.sh` and `crates/nullrouter-server/tests/harness.rs`: add sign-in mock accounts (anthropic, xai, grok-cli) and run the Python and Node SDK scripts and Claude Code against them (SC-002).
- [ ] T044 [US1] Green run: `nice cargo test -p nullrouter-registry -j 2`, then `-p nullrouter-engine`, `-p nullrouter-server`, `-p nullrouter-cli`, one at a time.
- [ ] T045 [US1] *operator-run* Live check L1 and L3 (research R17): `! NR_LIVE=1 cargo test -p nullrouter-engine --test live -- signin_anthropic signin_grok_cli --nocapture`. Record whether the hosted code page works for the anthropic client id, what a subscription refusal body looks like, which grok-cli identity headers are required, and whether grok-cli serves the client's body untouched (no `store`, `reasoning.summary`, `include`). Narrow the identity headers to what's needed, and add a forced parameter only for a demonstrated failure; set anthropic's `[[signin.refused]]` text from the observed body.

**Checkpoint**: sign-in works headless for all three providers and they serve like key accounts.

---

## Phase 4: User Story 2 — Tokens stay fresh without the client noticing (Priority: P1)

**Goal**: No client request fails because a token expired, with or without traffic, across
restarts.

**Independent Test**: [quickstart §1](quickstart.md) `expiry_soak`: 2 s tokens over 20+
lifetimes with idle gaps from two harnesses; zero expiry failures.

### Tests for User Story 2 ⚠️

- [ ] T046 [P] [US2] `crates/nullrouter-engine/tests/refresh.rs`: proactive refresh fires at `expires_at − lead` with no traffic; lead clamp (floor 2 min, ceiling half the lifetime; shortened in test); a token within 30 s of expiry is refreshed before use; 401 → one refresh → one retry on the same account, client sees only the answer; refresh on 401 also for xai (research R19); rotation keeps the new refresh token, or the old one when omitted; a refresh during a running stream leaves that stream on its original token to the end (spec edge case).
- [ ] T047 [P] [US2] `crates/nullrouter-engine/tests/refresh_dedup.rs`: 50 concurrent requests at expiry cause exactly one refresh call at the mock (FR-012); a transient refresh failure while the token is still valid keeps the account `Active` and retries; once the token has expired it becomes `Refreshing` (backoff 10 s, 30 s, 1 min, then every 2 min), requests fall back, and it returns to `Active` on success (FR-013).
- [ ] T048 [P] [US2] `crates/nullrouter-engine/tests/refresh_crash.rs`: after a rotated refresh is written, a simulated crash before the in-memory swap still leaves the newest refresh token in `tokens.toml`; restart with an expired access token refreshes before serving (FR-014).
- [ ] T049 [P] [US2] Soak `crates/nullrouter-server/tests/expiry_soak.rs`: 2 s tokens, 1 s lead, Python and Node SDK clients plus idle gaps of 5 s, ≥ 20 lifetimes; zero failed requests and zero records with an expiry class (SC-003).

### Implementation for User Story 2

- [ ] T050 [US2] Refresh call in `crates/nullrouter-engine/src/signin/refresh.rs`: form or JSON body per `[signin] body` (`grant_type=refresh_token`, `refresh_token`, `client_id`), 15 s timeout, rotation rule, write under the `tokens.lock` before swapping the `TokenCells` entry, then `rebuild_redactor()`.
- [ ] T051 [US2] Classification in `crates/nullrouter-engine/src/signin/classify.rs` per research R10: permanent for `invalid_grant`, `invalid_request`, `unauthorized_client`, `refresh_token_expired`, `refresh_token_reused`, `refresh_token_invalidated` and any other 400/401/403 from the token endpoint; transient for timeout, connection failure, 5xx, 429.
- [ ] T052 [US2] Dedup in `crates/nullrouter-engine/src/signin/dedup.rs`: one in-flight refresh per `(provider, name)`, shared by all waiters, cleared on completion (ports `ref/9router/open-sse/services/tokenRefresh/dedup.js`, keyed per account).
- [ ] T053 [US2] Use-time check and refresh-and-retry in `crates/nullrouter-engine/src/attempt.rs`: before `outgoing`, refresh a token within 30 s of expiry; on 401, or 403 matching the plugin's auth-rejection text, on a sign-in account: dedup refresh, then retry once on the same account without counting a fallback; a transient refresh failure becomes a `TokenRefreshing` skip and fallback continues.
- [ ] T054 [US2] Maintenance task in `crates/nullrouter-engine/src/maintenance.rs`: timer queue keyed per account for next refresh (and later polls and live models), at most 4 jobs at once, `CancellationToken` per job; rebuild the queue on reload keeping last-run times. Spawn and stop it in `crates/nullrouter-cli/src/cmd/serve.rs:41-64` with the existing shutdown `watch`.
- [ ] T055 [US2] Green run for US2 tests, one crate at a time.
- [ ] T056 [US2] *operator-run* Live check L4: `! NR_LIVE=1 cargo test -p nullrouter-engine --test live -- token_lifetimes --nocapture` prints each provider's real `expires_in` and whether refresh rotates. Adjust `refresh_lead` in the bundled plugins only if a lifetime is shorter than 2 × lead.

**Checkpoint**: signed-in accounts stay usable indefinitely without the client noticing.

---

## Phase 5: User Story 3 — An account that can't be refreshed is named, and service carries on (Priority: P1)

**Goal**: A permanently failed refresh or a provider refusal takes the account out, other
accounts serve, and the operator sees which account and the command to fix it in three places.

**Independent Test**: [quickstart §6](quickstart.md#6-out-of-service-accounts-by-hand) with the
mock: a permanent refresh failure on one of two accounts; requests from two harnesses succeed;
list, records and (with one account) the informational error name it.

### Tests for User Story 3 ⚠️

- [ ] T057 [P] [US3] `crates/nullrouter-server/tests/needs_signin.rs`: permanent refresh failure → `accounts.state` shows `needs sign-in` with time and reason; records of later requests show a `Skipped` attempt with class `NeedsSignIn` and the command; with no other account the informational error names the account and command in the message and in the structured details, in all four styles; state survives a restart; signing in again returns the account to service without a restart, same name (FR-015–FR-017, SC-005).
- [ ] T058 [P] [US3] `crates/nullrouter-server/tests/refused.rs`: a fresh token rejected again after refresh, and a response matching `[[signin.refused]]`, both mark the account `Refused` with the provider's reason (FR-004b); a 403 that slice 003's rules read as model access stays an ordinary failure; `accounts enable` clears `Refused`.
- [ ] T059 [P] [US3] CLI test in `crates/nullrouter-cli/tests/accounts_keys.rs`: the `accounts list` state column and hint line exactly as in [contracts/operator-cli.md § `accounts list`](contracts/operator-cli.md).

### Implementation for User Story 3

- [ ] T060 [US3] State transitions in `crates/nullrouter-engine/src/tokens.rs` per [data-model § Account state machine](data-model.md#account-state-machine-r10): persist `NeedsSignIn` and `Refused` with `state_since` and redacted `state_reason` in `tokens.toml`; a successful sign-in resets to `Active`; `enable` clears `Refused`; one `warn` log line per state change.
- [ ] T061 [US3] Refusal detection in `crates/nullrouter-engine/src/attempt.rs` and `classify.rs`: after a refresh-and-retry is rejected again with 401/403, or when a response matches the plugin's `refused` rules, set `Refused` and fall back as for any non-serving account.
- [ ] T062 [US3] Informational error lines in `crates/nullrouter-wire/src/error_body.rs` (`Tried::line`, 38): render `NeedsSignIn` / `Refused` skips as `<provider>/<account>: needs sign-in — run nullrouter accounts signin <provider> <account>`.
- [ ] T063 [US3] `accounts.state` op in `crates/nullrouter-server/src/operator.rs` (86-140) returns `kind`, `state`, `state_since`, `state_reason`, `expires_at`; `accounts list` / `enable` in `crates/nullrouter-cli/src/cmd/accounts.rs` render and act on them.
- [ ] T064 [US3] `accounts remove` in `crates/nullrouter-cli/src/cmd/accounts.rs` also deletes the account's `tokens.toml` entry under the lock; the server's reload drops its token cell and rebuilds the redactor. Its poll history stays (Clarifications Q4). Test in `crates/nullrouter-cli/tests/accounts_keys.rs`: after removal no token remains in `tokens.toml` and the history files still exist.
- [ ] T065 [US3] Green run for US3 tests.

**Checkpoint**: P1 complete. All P1 fail conditions of the slice are covered.

---

## Phase 6: User Story 4 — Quota is visible from the CLI (Priority: P2)

**Goal**: Every chosen account whose provider reports quota is polled on a steady slow schedule,
also while idle; the CLI shows each window, reset time and poll time, or "quota not reported".

**Independent Test**: mock quota endpoints with known values; `quota` shows exactly those values
with poll times; changes appear after the next poll; idle polling keeps poll times fresh.

### Tests for User Story 4 ⚠️

- [ ] T066 [P] [US4] Extractor tests in `crates/nullrouter-engine/tests/quota_extract.rs`, one case per 9router parser branch: anthropic `five_hour`, `seven_day`, `seven_day_<model>`, `limits[*]` `weekly_scoped`, missing `utilization` skipped; grok-cli billing with `{val: n}`, each reset-field alternative, monthly / on-demand / prepaid / weekly-percent windows; opencode-go/zen rolling, weekly, monthly with string and number percents; reset formats epoch s, epoch ms and RFC 3339.
- [ ] T067 [P] [US4] gRPC-web decoder tests in `crates/nullrouter-engine/tests/quota_grpc.rs`: framed and unframed buffers, fixed32 float and fixed64 double ratio, missing ratio = 0% used, Timestamp to RFC 3339, trailer-only and malformed input yield no window (ports `ref/9router/open-sse/services/usage/grokCliQuotaFrame.js`).
- [ ] T068 [P] [US4] Polling tests in `crates/nullrouter-server/tests/quota_poll.rs`: polls run with no traffic at the account's interval (shortened); jitter stays within ±10%; a failed poll retries once after 1 min (shortened) and then waits; 429 skips the retry; 401 on a sign-in account refreshes and retries; a failed poll keeps the last good windows with their own time plus the failure (FR-021); `quota interval` changes only that account; key accounts on opencode-go/zen are polled; an anthropic key account and xai show "quota not reported"; a 0% account is still tried in order (FR-022, SC-006, SC-007).
- [ ] T069 [P] [US4] CLI test in `crates/nullrouter-cli/tests/quota.rs`: `quota` output exactly as [contracts/operator-cli.md § `quota`](contracts/operator-cli.md); `quota poll` exits 4 when no server runs (`quota interval` saves and says it applies at next start).

### Implementation for User Story 4

- [ ] T070 [P] [US4] Path engine in `crates/nullrouter-engine/src/quota/extract.rs`: paths with `*` over keys and items binding `{1}`, `first_of` alternatives, `where` equality filter, `unwrap_val`, name templates with `{path|lower}`, reset parsing like `parseResetTime` (`ref/9router/open-sse/services/usage/shared.js:15-43`); output `QuotaWindow {name, unit, used?, limit?, remaining?, resets_at?}` with percent windows `limit = 100`.
- [ ] T071 [P] [US4] gRPC-web decoder in `crates/nullrouter-engine/src/quota/grpc_web.rs` (no `unsafe`, no new crate): frame reader, minimal protobuf varint/fixed32/fixed64/length-delimited reader, field 1 → nested 1 (ratio) and 5 (Timestamp).
- [ ] T072 [US4] Poller in `crates/nullrouter-engine/src/quota/poll.rs`: build the request with the account's secret or token (released to bound hosts only), `[identity]` headers for sign-in accounts, the engine's client; run `fallback` when the first request yields no window; classify failures; one retry policy per research R11; result is a `QuotaPoll`.
- [ ] T073 [US4] Schedule polls in `crates/nullrouter-engine/src/maintenance.rs`: default interval 10 min, floor 2 min, per-account `poll_interval`, ±10% jitter; keep the latest poll and last failure per account in memory.
- [ ] T074 [P] [US4] `[quota]` sections: anthropic (`accounts = "signin"`, `https://api.anthropic.com/api/oauth/usage`, `anthropic-beta = "oauth-2025-04-20"`, the three window rules from the contract example) in `plugins/bundled/anthropic.toml`; opencode-go (`https://opencode.ai/zen/go/v1/usage`, `accounts = "key"`) in `plugins/bundled/opencode-go.toml`; opencode-zen (`https://opencode.ai/zen/v1/usage`) in `plugins/bundled/opencode-zen.toml`; grok-cli billing and user requests plus the gRPC-web fallback at `https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig` in `plugins/bundled/grok-cli.toml`.
- [ ] T075 [US4] Operator ops `quota.list` and `quota.poll` in `crates/nullrouter-server/src/operator.rs`; `quota`, `quota poll`, `quota interval` in `crates/nullrouter-cli/src/cmd/quota.rs` (interval writes `poll_interval` to `accounts.toml` and sends `reload`).
- [ ] T076 [P] [US4] grok-cli live model list: `[models_live]` in `plugins/bundled/grok-cli.toml` (`https://cli-chat-proxy.grok.com/v1/models`, `x-xai-token-auth = "xai-grok-cli"`, `x-grok-client-mode = "headless"`, paths from research R14); fetch at start and every 6 h in `maintenance.rs`; merge with static `[[models]]`, static list on failure; listed models appear in every style's model list with their type (default text) (FR-008, SC-010).
- [ ] T077 [US4] Green run for US4 tests.
- [ ] T078 [US4] *operator-run* Live checks L2 and L5: `! NR_LIVE=1 cargo test -p nullrouter-engine --test live -- quota --nocapture` prints each real quota response next to the extracted windows, and the `x-ratelimit-*` headers of `api.x.ai/v1/models`. Fix the declared paths (data only); add an xai `[quota]` only if L2 finds headers (then a header-reading rule is a new task).

**Checkpoint**: the operator sees every reported quota with its poll time.

---

## Phase 7: User Story 5 — Each poll is kept with a tally of 0router's own traffic (Priority: P2)

**Goal**: Every poll is kept locally with a per-model tally of tokens 0router sent through the
account since the previous poll, surviving restarts and kept until the operator prunes it.

**Independent Test**: known requests between two polls; the second entry's tally equals the
summed reported usage, per model; history survives a restart.

### Tests for User Story 5 ⚠️

- [ ] T079 [P] [US5] `crates/nullrouter-engine/tests/tally.rs`: tally equals the sum of reported `input`, `output`, `cache_read`, `cache_write` per upstream model (SC-008); unreported usage counts in `requests_usage_unreported` and adds no tokens; a request retried across accounts tallies each attempt on its own account; key and sign-in accounts both tally.
- [ ] T080 [P] [US5] `crates/nullrouter-engine/tests/quota_history.rs`: JSONL entries per [data-model § Quota poll](data-model.md#quota-poll-history-entry-quotaprovideraccountjsonl) with `v = 1`; files mode 0600; checkpoint every 10 s and at shutdown, so a graceful restart loses nothing; `prune --before` removes only older entries; `forget` deletes one account's files; `accounts remove` stops polls but keeps history (FR-025, Clarifications Q4).

### Implementation for User Story 5

- [ ] T081 [P] [US5] Tally in `crates/nullrouter-engine/src/quota/tally.rs`: per account, per upstream model counters `requests`, `requests_usage_unreported`, `input`, `output`, `cache_read`, `cache_write` (the quota meter's names, FR-026); fed from `Run::end_attempt` (`attempt.rs:1388-1397`) with the attempt's account and provider-reported usage; lock-free or a short `Mutex` per account; taken and reset when a poll entry is written.
- [ ] T082 [US5] History in `crates/nullrouter-engine/src/quota/history.rs`: append one JSON line per poll to `quota/<provider>/<account>.jsonl` (0600) in `spawn_blocking`; checkpoint the running tally to `.tally.json` with `write_private` every 10 s and at shutdown; reload the checkpoint at start; tail reader for the newest N entries.
- [ ] T083 [US5] `quota history`, `quota prune`, `quota forget` in `crates/nullrouter-cli/src/cmd/quota.rs` and `quota.checkpoint` in `crates/nullrouter-server/src/operator.rs` (called before `prune`/`forget` when a server runs).
- [ ] T084 [US5] Bench case in `crates/nullrouter-engine/benches/engine.rs`: a request through a sign-in account (token cell load, identity headers, forced parameters, tally update); compare with the `pre-005` baseline recorded in T001 (`--baseline pre-005`); no regression beyond noise.
- [ ] T085 [US5] Green run for US5 tests.

**Checkpoint**: slice 006 has poll history and per-model traffic from the first day.

---

## Phase 8: User Story 6 — Sign-in and quota are generic core, specifics are plugin data (Priority: P3)

**Goal**: The flows and polling are generic, every provider specific is plugin data, only bundled
plugins may use it in this slice, and no plugin holds a secret.

**Independent Test**: the gate and fit corpora: the seven bundled plugins validate; community
plugins with sign-in or quota sections are refused naming them; secret-like and off-host
declarations are refused.

### Tests for User Story 6 ⚠️

- [ ] T086 [P] [US6] `crates/nullrouter-registry/tests/community.rs`: a community plugin carrying each new section is refused whole with the named part; all 114 community plugins still load or are refused as before (FR-029).
- [ ] T087 [P] [US6] `crates/nullrouter-registry/tests/secrets.rs`: no `SignInDecl`, `IdentityDecl`, `QuotaDecl` or `ModelsLiveDecl` field can carry a secret; secret-like values refused; a test asserting no plugin-visible type has a field typed `SecretString` or holding a token.
- [ ] T088 [P] [US6] `crates/nullrouter-engine/tests/host_binding.rs`: a token is never released to a host outside `token_hosts()`, including through a redirect (never followed) or a discovery document naming another host (FR-030, FR-031).

### Implementation for User Story 6

- [ ] T089 [US6] Make any test in T086–T088 pass that the foundational gate and fit work (T010–T012, T017) didn't already cover; no new behaviour beyond the contract.
- [ ] T090 [US6] Document the new sections for plugin authors in `docs/plugins.md` (bundled-only in this slice) and the operator flows in `docs/operator-config.md` (`accounts signin`, `quota`, `tokens.toml`, history files).

**Checkpoint**: all six stories are complete.

---

## Phase 9: Polish & Cross-Cutting Concerns

- [ ] T091 [P] Secrets sentinel in `crates/nullrouter-server/tests/secrets.rs`: sentinel access tokens, refresh tokens, device codes, authorization codes, PKCE verifiers and the anthropic usage token across logs (tracing output), records, informational errors, CLI output (`signin`, `list`, `quota`), operator socket answers, `quota/*.jsonl`, and plugin-visible data; zero occurrences (SC-009).
- [ ] T092 [P] `nullrouter check` in `crates/nullrouter-cli/src/cmd/check.rs`: report `tokens.toml` entries without a matching account, sign-in accounts without tokens, and file modes of `tokens.toml`, `install-id` and `quota/`.
- [ ] T093 Bench baseline: run the engine bench (T084) on an idle machine and append the summary to `specs/005-account-sign-in/bench-baseline.md`.
- [ ] T094 `/rust-parity-audit` on `crates/nullrouter-engine/src/signin/` and `src/quota/` against `ref/9router/src/lib/oauth/*`, `open-sse/services/tokenRefresh*` and `open-sse/services/usage/*`; deliberate deviations are those in research R19 and must not be reported as gaps; fix High findings.
- [ ] T095 Security review of the slice diff (`/security-review`): token file locking and modes, host binding, loopback listener (binds 127.0.0.1 only, single request, state check), paste parsing, redaction generations.
- [ ] T096 Run [quickstart.md](quickstart.md) §1–2 end to end; fix what fails.
- [ ] T097 *operator-run* [quickstart §3–4 and §6](quickstart.md) with real accounts over SSH: `! nr accounts signin …` for each provider, a request from two harnesses each, `! nr quota`; confirm SC-001 (under 3 minutes each) and that quota matches the providers' own pages.
- [ ] T098 `cargo fmt --all`, then `nice cargo clippy -p <crate> -j 2 -- -D warnings` crate by crate.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: none.
- **Foundational (Phase 2)**: depends on Setup; blocks every story.
- **US1 (Phase 3)**: depends on Foundational. The MVP.
- **US2 (Phase 4)**: depends on US1 (needs signed-in accounts and the plugins T036–T038).
- **US3 (Phase 5)**: depends on US2 (states come from refresh outcomes).
- **US4 (Phase 6)**: depends on Foundational and US2's maintenance task (T054). Key-account polling (opencode-go/zen) needs nothing from US1; sign-in polling needs US1.
- **US5 (Phase 7)**: depends on US4 (a tally is written with each poll).
- **US6 (Phase 8)**: depends on Foundational only; can run in parallel with US1–US5.
- **Polish (Phase 9)**: depends on every story.

### Story Graph

```text
Setup → Foundational ─┬─▶ US1 (MVP) ─▶ US2 ─┬─▶ US3
                      │                     └─▶ US4 ─▶ US5
                      └─▶ US6 (any time after Foundational)
                                                        ─▶ Polish
```

### Within Each Story

- Tests first; watch them fail.
- Then data (plugin TOML), then registry and engine, then server and CLI.
- Finish with the green-run task.
- Operator-run live checks come after the mock tests pass. A live result can only narrow or
  correct a declaration; it never adds a disguise (Clarifications Q1).

---

## Parallel Examples

```text
# Phase 2: schema sections together, then engine pieces together
T006 identity.rs   T007 quota.rs   T008 models_live.rs
T018 identity fill   T019 records   T023 mock_quota

# US1: tests together, then flow pieces and plugins together
T024 signin_flows   T025 cli signin   T026 signin_serving   T027 fallback_signin   T028 list
T029 pkce   T030 loopback   T031 device_code   T036 xai.toml   T037 grok-cli.toml

# US4: extractor work in parallel with plugin data
T066 T067 T068 T069 (tests)   T070 extract   T071 grpc_web   T074 [quota] sections   T076 models_live

# US6 alongside US1–US5
T086 T087 T088
```

---

## Implementation Strategy

### MVP (User Story 1)

1. Phases 1 and 2.
2. Phase 3. **Stop and validate**: headless sign-in for all three providers against the mock,
   two harnesses served through each, and live check L1/L3 on the operator's accounts.

### Incremental Delivery

1. US1: subscription accounts usable.
2. US2: tokens never expire under a client. With US1, the slice's expiry fail condition is met.
3. US3: out-of-service accounts named. All P1 fail conditions met.
4. US4: quota visible.
5. US5: history and tally for slice 006.
6. US6: generic-core guarantees tested and documented.
7. Polish: sentinel, parity audit, security review, live run.

A commit per task or per logical group is fine. The xai and grok-cli moves from the community
set to the bundle (T036, T037) go in one commit together with T041 (generator `CHOSEN`) and T042 (counts).

---

## Notes

- Constraints quoted from the data model are binding:
  - "`[a-z0-9_-]{1,32}`" for account names (unchanged from slice 003);
  - "default 10 min, floor 2 min" for poll intervals;
  - "`verifier_bytes` 32–96";
  - placeholders only from the closed set of seven (Clarifications Q5);
  - forced parameters only `store`, `reasoning.summary`, `reasoning.effort`, `include`;
  - `tokens.toml`, `install-id` and `quota/*` mode 0600;
  - tally token names `input`, `output`, `cache_read`, `cache_write` (quota meter).
- Never change the request body beyond declared forced parameters: no tool renaming, decoy
  tools, injected system text, or invented ids, for any provider (Clarifications Q1).
- Never pass a token, code or verifier to anything plugin-visible, a log, a record, an error or
  the operator socket.
- Never follow upstream redirects, including during sign-in and polling.
- Never add `tokio` or `unsafe` to `nullrouter-registry`.
- Never edit `ref/9router/` or `tests/fixtures/9router/` by hand.
