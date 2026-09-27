---

description: "Task list for 002-provider-model-registry"
---

# Tasks: Provider Entity & Unified Model Registry

**Input**: Design documents from `specs/002-provider-model-registry/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included. Constitution VI requires parity tests against 9router, the spec
gives acceptance scenarios for every story, and the constitution's benchmark gate
requires Criterion on the hot path. Within each story, write the tests first and confirm
they fail before implementing.

**Organization**: Tasks are grouped by user story so each story can be implemented and
tested on its own.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US4)

## Path Conventions

Cargo workspace at the repo root ([plan § Project Structure](plan.md#project-structure)):

- `crates/zerorouter-registry/`: library (`src/`, `tests/`, `benches/`, `build.rs`)
- `crates/zerorouter-cli/`: operator CLI
- `plugins/bundled/`: generated plugin TOML
- `tools/gen-bundled/`: Node generator (dev only)
- `tests/fixtures/9router/`: generated parity oracle, shared across crates

Two refinements to the plan's tree, made so tasks can run in parallel:

- The `parity` test target is a directory, `tests/parity/main.rs` plus one module per view.
  It is still run as `--test parity`.
- The CLI is split into `src/cmd/<command>.rs`.

`ref/` is git-ignored. Generated artefacts record the `ref/9router` SHA in their header.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Workspace and crate skeletons that compile.

- [X] T001 Verify the toolchain prerequisite ([research R1](research.md#r1-toolchain-availability)).
  - `cargo --version` and `rustc --version` must both succeed inside the `claude-0router` gate.
  - If either fails, **stop** and ask the user to update the gate. Never edit `identity/`.
- [X] T002 Create the workspace manifest `Cargo.toml` at the repo root.
  - `members = ["crates/*"]` and `resolver = "3"`.
  - `[workspace.package]`: `edition = "2024"`, `rust-version = "1.85"`, `license`.
  - `[workspace.dependencies]` for every crate in [R12](research.md#r12-crates), plus `indexmap` with the `serde` feature (ordered `headers`/`endpoints` maps; R12 omitted it).
  - `[workspace.lints.rust] unsafe_code = "forbid"`.
  - `[workspace.lints.clippy] all = "warn"`.
- [X] T003 Create `crates/zerorouter-registry/Cargo.toml` and the library root.
  - Dependencies: `serde` (derive), `toml`, `serde_path_to_error`, `thiserror`, `arc-swap`, `url`, `regex-lite`, `indexmap`.
  - Dev-dependencies: `serde_json`, `criterion`.
  - `[[bench]] name = "resolve"`, `harness = false`.
  - `build = "build.rs"`, `lints.workspace = true`.
  - `src/lib.rs` declares the modules `schema`, `validate`, `credentials`, `load`, `registry`, `lookup`, `resolve`, `views`, each with an empty `mod.rs` or file so the crate compiles.
- [X] T004 [P] Create `crates/zerorouter-cli/Cargo.toml` (deps: `clap` derive, `serde_json`, `zerorouter-registry` by path) and `crates/zerorouter-cli/src/main.rs`.
  - Five clap subcommands `check`, `validate`, `resolve`, `model`, `providers`, each dispatching to an empty `src/cmd/<name>.rs`.
  - Exit codes per [contracts/registry-api.md § CLI](contracts/registry-api.md#cli-zerorouter-cli): 0 ok, 1 errors, 2 not found.
  - `--home DIR` overrides `$ZEROROUTER_HOME`.
- [x] T005 [P] Append `target/` and `.cargo-home/` to `.gitignore`.
- [X] T006 Run `cargo build --workspace && cargo clippy --workspace` and confirm both pass on the skeleton.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Plugin schema, validation gate, generated bundled set, and a bundled-only
registry. Every story depends on these.

**⚠️ CRITICAL**: No user story work can start until this phase is complete.

### Schema (Serde types)

Rule for every struct below: `#[serde(deny_unknown_fields)]`, `snake_case` keys, and
`Option` wherever [data-model.md](data-model.md) says absence is distinct.

- [X] T007 [P] Define the closed enums in `crates/zerorouter-registry/src/schema/enums.rs`. Each enum implements `Display` and a `pub const ALLOWED: &[&str]` so error messages can list the allowed values.
  - `Category`: exactly `apikey | oauth | freeTier | free | webCookie`, with serde renames that keep the camelCase spellings.
  - `WireFormat`: the 13 values `openai`, `openai-responses`, `claude`, `gemini`, `gemini-cli`, `vertex`, `antigravity`, `kiro`, `cursor`, `commandcode`, `ollama`, `grok-web`, `perplexity-web`.
  - `Quirk`: `preserve_cache_control`, `drop_client_metadata`, `cline_envelope`, `drop_output_config`, `require_claude_tool_type`, `cloak_tools_on_oauth`.
  - `AuthHook`: `cline_headers`, `kimi_headers`, `kilocode_org`.
  - `AuthKind`: `apikey | oauth`.
  - `AuthScheme`: `bearer | raw`.
  - `CapabilityKind`: `llm`, `image`, `image_to_text`, `video`, `tts`, `stt`, `embedding`, `web_search`, `web_fetch`, `systemone`. Mark it `#[non_exhaustive]`.
  - `ModelKind`: same value set as `CapabilityKind`.
  - `ContentKind`: `image`, `audio`.
- [X] T008 [P] Define `Model` in `crates/zerorouter-registry/src/schema/model.rs` per [data-model § Model](data-model.md#model).
  - `id` is required. Every other field is `Option`: `name`, `kind`, `upstream_id`, `target_format`, `supported_formats`, `quota_family`, `strip`, `context_length`, `max_output_tokens`, `dimensions`, `rate_multiplier`, `capabilities`, `params`, `description`.
  - "`kind`: `None` = no declared type (FR-004)". Never default it to `llm`.
  - An entry may be a bare ID string (9router `normalizeModel`): implement a string-or-table `Deserialize` so `"acme-small"` becomes `Model { id: "acme-small", .. }`. A value that is neither a string nor a table must still produce a path + span error.
- [X] T009 [P] Define `Transport` and `TransportAuth` in `crates/zerorouter-registry/src/schema/transport.rs` per [data-model § Transport](data-model.md#transport).
  - `headers: IndexMap<String, String>`. Keep order for parity.
  - `quirks: Vec<Quirk>`, plus `claude_supported_tool_types` and `force_auto_tool_choice_models`.
  - `regions` / `default_region`: "`default_region` must be a key of `regions`".
  - `executor_params: Option<ExecutorParams>`, a closed struct (`deny_unknown_fields`): `cli_version`, `client_version`, `api_client`, `client_identifier`, `token_auth`, `no_auth`, `auth_type`, and `copilot: Option<CopilotParams { vscode_version, chat_version, user_agent, api_version }>`.
  - All URL fields listed in the data model, including `token_url`, `refresh_url`, and `auth_url`, plus `client_id`. These are declared on the transport by some bundled providers (client_id: antigravity, gemini-cli, gemini, kimi, xai; token_url: antigravity, cline, kimi, kiro, xai). There is no `client_secret` field.
- [X] T010 [P] Define `OAuthDecl` in `crates/zerorouter-registry/src/schema/oauth.rs`.
  - Fields: `client_id`, `authorize_url`, `token_url`, `refresh_url`, `device_code_url`, `user_info_url`, `scopes: Vec<String>`, `code_challenge_method`, `refresh_lead_ms`, `endpoints: IndexMap<String, Url>`, `params: IndexMap<String, Scalar>`.
  - `params` keys must be in `KNOWN_OAUTH_PARAMS` (generated by T016 into `crates/zerorouter-registry/src/schema/oauth_params.rs`); anything else → `oauth.params.<key>: unknown OAuth parameter; allowed: …`.
  - Add a free function `host_set(oauth: Option<&OAuthDecl>, transports: &[&Transport]) -> BTreeSet<Host>` (FR-012a): the hosts of `oauth.{authorize,token,refresh}_url` plus `token_url`/`refresh_url`/`auth_url` on `transport` and every `transports[]` entry. It excludes `user_info_url`, `device_code_url`, and `oauth.endpoints`.
- [X] T011 [P] Define `CapabilitySection`, `SectionEndpoint`, and `SectionModel` in `crates/zerorouter-registry/src/schema/capability.rs` per [data-model § CapabilitySection](data-model.md#capabilitysection).
  - `endpoint`, `models`, `limits`, `hidden`.
  - `SectionEndpoint` has `base_url`, `auth_type`, `auth_header`, `format`, `headers`, `method`, `timeout_ms`, `default_model`, `poll_url`, `body_fields`, `model_map`.
- [X] T012 Define `PluginFile`, `AuthDecl`, `Display`, `ProviderEntity`, and `PluginSource { Bundled, User(PathBuf) }` in `crates/zerorouter-registry/src/schema/plugin.rs` and re-export them from `schema/mod.rs`. Depends on T007–T011.
  - Top-level keys exactly as in [contracts/plugin-schema.md § Top-level keys](contracts/plugin-schema.md#top-level-keys).
  - `models: Option<Vec<Model>>`: "`None` = catalog unknown; `Some([])` = offers no models". Elements use the string-or-table form from T008.
  - `capabilities: BTreeMap<CapabilityKind, CapabilitySection>`.
  - `passthrough_models` and `version_separator_tolerance` default to `false`.

### Validation gate (FR-007 – FR-010)

- [X] T013 [P] Implement `ValidationError { file, line, col, path, rule }` in `crates/zerorouter-registry/src/validate/errors.rs`.
  - `Display` renders `file:line:col field.path: rule`.
  - Include a byte-span → line/col helper.
  - Add unit tests for rendering.
- [X] T014 [P] Implement the secret checks from [R5](research.md#r5-keeping-secrets-out-of-plugins) in `crates/zerorouter-registry/src/validate/secrets.rs`, with unit tests for each denylisted key and URL case.
  - The key denylist applies to **open maps only**: `transport.headers` (and `transports[].headers`, section endpoint `headers`) and every model `params`. Closed structs (`executor_params`, `oauth.params`) are already guarded by their key sets. Add a unit test that none of the 39 header names in the bundled set is rejected.
  - `check_map_key(key) -> Option<&'static str>` matches case-insensitively against `secret`, `password`, `passwd`, `api_key`/`apikey`, `access_token`, `refresh_token`, `cookie`, `authorization`, `x-api-key`, `proxy-authorization`, `x-goog-api-key`. It also matches any key containing `token` unless the key ends in `url`, `_url`, or `endpoint`.
  - `check_url(url)` rejects userinfo and any query parameter whose key hits the same denylist.
- [X] T015 Implement the validation gate in `crates/zerorouter-registry/src/validate/gate.rs`. Depends on T012–T014.
  - Signature: `pub fn validate(src: &str, source: PluginSource, file: &str) -> Result<ProviderEntity, Vec<ValidationError>>`.
  - Parse with `toml::Deserializer` + `serde_path_to_error`, so unknown fields and bad enum values produce path + span errors that list the allowed values.
  - Then run semantic checks and **collect all errors**, not just the first:
    - `schema` is absent or `1`;
    - `id` and `alias` match `[a-z0-9][a-z0-9-]*`;
    - the secret-key check on the open maps from T014 (`headers` maps and every model `params`);
    - every `oauth.params` key is in `KNOWN_OAUTH_PARAMS`;
    - `check_url` on every URL field;
    - "a capability section with no endpoint of its own requires `transport` to be set";
    - duplicate model `id` → `models[i].id: duplicate of models[j]`;
    - `transports` non-empty requires `transport`;
    - `default_region` ∈ `regions`.
  - Cross-plugin references (`auth.credential_fallback`) are checked in `load.rs`, not here.

### Bundled set generator (R8, R9)

All three generator parts touch the same file, so they run in order.

- [X] T016 Write part 1 of `tools/gen-bundled/generate.mjs`: plugin files. Plain Node ≥ 22 ESM, no npm dependencies, a small hand-written TOML emitter.
  - Import `ref/9router/open-sse/providers/registry/index.js` (the evaluated registry).
  - For each of the 121 active entries, write `plugins/bundled/<id>.toml`.
  - Map every observed key per [R4](research.md#r4-what-the-schema-must-carry-registry-census):
    - camelCase → snake_case;
    - `type` → `kind`;
    - `scope` → `scopes`;
    - `upstreamModelId` → `upstream_id`;
    - the quirks object → the `quirks` list plus the two list-valued fields;
    - the capability configs (`ttsConfig`, …) → `[capabilities.<kind>]`;
    - the 8 TTS tables from `open-sse/config/ttsModels.js` → the owning provider's `capabilities.tts.models`;
    - long-tail OAuth values → `oauth.endpoints` / `oauth.params`;
    - long-tail transport values → `transport.executor_params` (closed key set, T009);
    - models that 9router lists as bare strings stay bare strings;
    - kiro gets `version_separator_tolerance = true`;
    - the 14 passthrough providers get `passthrough_models = true`.
  - Drop every `clientSecret`.
  - Write `crates/zerorouter-registry/src/schema/oauth_params.rs` with `pub const KNOWN_OAUTH_PARAMS: &[&str]`, the sorted set of `oauth.params` keys emitted (e.g. gitlab `token_url_path`).
  - **Exit non-zero on any unmapped key**, including an executor param outside the T009 key set.
  - The first line of each file is `# Generated from ref/9router@<sha> by tools/gen-bundled/generate.mjs — do not edit.`, where `<sha>` comes from `git -C ref/9router rev-parse --short HEAD`.
- [X] T017 Extend `tools/gen-bundled/generate.mjs` to write `crates/zerorouter-registry/src/credentials/bundled.rs` (part 2).
  - One static entry for each of antigravity, gemini-cli, gemini, and iflow: `provider_id`, `client_secret`, and `bound_hosts`.
  - `bound_hosts` = the bundled plugin's host set as defined in T010 (`oauth.{authorize,token,refresh}_url` plus transport `token_url`/`refresh_url`/`auth_url`; not `user_info_url`, `device_code_url`, or `oauth.endpoints`). gemini declares none, so use the hosts of `OAUTH_ENDPOINTS.google` from `ref/9router/open-sse/config/appConstants.js` (oauth2.googleapis.com, accounts.google.com).
  - Exit non-zero if any entry would have no hosts, or if the number of secrets found ≠ 4.
- [X] T018 Extend `tools/gen-bundled/generate.mjs` to write the oracle into `tests/fixtures/9router/` (part 3, [R9](research.md#r9-parity-oracle)).
  - `providers.json`: the current `PROVIDERS`.
  - `alias.json`: the `ALIAS_TOKENS` list and shape from `ref/9router/tests/__baseline__/verify-alias.mjs`.
  - `oauth-urls.json`: the shape of `verify-oauth-urls.mjs`.
  - `lookup.json`: one row per (alias, model) in `PROVIDER_MODELS`, plus the edge inputs: thinking suffix, nested paren, dash/dot variant, undeclared ID, trailing whitespace, preset suffix.
    - Each row records `isValidModel`, `getModelUpstreamId`, `getModelType`, `getModelTargetFormat`, `getModelSupportedFormats`, `getModelQuotaFamily`, `getModelStrip`, and `findModelName` from `open-sse/config/providerModels.js`.
    - Exclude inputs that hit the Codex review-suffix and Muse Spark branches.
    - Exclude the 8 synthetic TTS keys in `PROVIDER_MODELS`; they are not provider aliases.
  - `tts-tables.json`: synthetic TTS key → `{ provider, models }`, from `open-sse/config/ttsModels.js`.
- [X] T019 Run `node tools/gen-bundled/generate.mjs` and commit the generated output (quickstart step 1).
  - Check that there are 121 files in `plugins/bundled/`, 4 entries in `credentials/bundled.rs`, and that `schema/oauth_params.rs` exists.
  - Check that `grep -rl 'client_secret\|GOCSPX-' plugins/` is empty.

### Registry core

- [X] T020 Create `crates/zerorouter-registry/build.rs`.
  - List `../../plugins/bundled/*.toml` in sorted order.
  - Write `$OUT_DIR/bundled_plugins.rs` containing `pub static BUNDLED: &[(&str, &str)] = &[("<file>", include_str!("<abs path>")), …];`.
  - Emit `cargo:rerun-if-changed=../../plugins/bundled`.
- [X] T021 [P] Implement the credentials module in `crates/zerorouter-registry/src/credentials/mod.rs`.
  - `SecretString`: `Debug`/`Display` print `***`, and it is not `Serialize`. Its only public method is `matches(&self, candidate: &str) -> bool`; `expose(&self) -> &str` is `pub(crate)`.
  - `CredentialEntry`, and `include!("bundled.rs")` behind `std::sync::LazyLock`.
  - `ResolvedCredential { Available(&SecretString), Withheld { offending_url: Url } }`.
  - `fn bind(entry, active: &ProviderEntity) -> ResolvedCredential`. It computes the active plugin's host set with `host_set` (T010, including its transports) and returns `Available` only if that set ⊆ `bound_hosts`. An empty host set on the active plugin also counts as a match (gemini).
  - `SecretString` and `ResolvedCredential` are public type names; the table, `CredentialEntry`, and `bind` are `pub(crate)`.
- [X] T022 Implement `Registry` and its indices in `crates/zerorouter-registry/src/registry.rs`.
  - Fields: `providers: Vec<ProviderEntity>`, `alias_index: HashMap<Box<str>, ProviderIdx>` (ids + `alias` + `aliases`, never `ui_alias`), `credentials`, `report: LoadReport`.
  - Methods: `provider(token) -> Result<&ProviderEntity, NotFound>`, `providers()`, `capability(provider, kind) -> Result<Option<&CapabilitySection>, NotFound>` (`Ok(None)` = "not offered").
  - Alias conflicts are errors that name both sources: an alias equal to another provider's id, or a token claimed by two providers.
  - Define `NotFound` in `crates/zerorouter-registry/src/resolve.rs` with `Provider { token }`, `UnifiedModel { name }`, `Model { provider, model }`, and `EmptyTarget`.
- [X] T023 Implement bundled loading in `crates/zerorouter-registry/src/load.rs`.
  - `LoadReport`: counts, pending conflicts, declined plugins, withheld credentials, skipped plugins with errors.
  - `OperatorHome::resolve()`: `$ZEROROUTER_HOME`, else `~/.0router`. A missing home, config, or plugins directory is not an error.
  - `load_bundled() -> Result<Vec<ProviderEntity>, StartupError>` runs every `BUNDLED` entry through `validate::gate::validate`. **Any error is fatal.**
  - Cross-plugin check: `auth.credential_fallback` must name an existing provider → `auth.credential_fallback: unknown provider "<x>"`.
- [X] T024 Implement `RegistryHandle` in `crates/zerorouter-registry/src/lib.rs` with an `ArcSwap<Registry>`, a reload `Mutex<()>`, and the `OperatorHome`.
  - `open(home) -> Result<Self, StartupError>` builds from the bundled set only for now. US3 adds config.toml and US4 adds user plugins.
  - `snapshot() -> Arc<Registry>`.
  - Re-export the public types listed in [contracts/registry-api.md](contracts/registry-api.md).
- [X] T025 Add a smoke test in `crates/zerorouter-registry/tests/smoke.rs`: `RegistryHandle::open` on an empty temp home succeeds and yields 121 providers. Run `cargo test -p zerorouter-registry`.

**Checkpoint**: Bundled plugins embed, validate, and load. User stories can start.

---

## Phase 3: User Story 1: Bundled provider set is available and faithful (Priority: P1) 🎯 MVP

**Goal**: The bundled registry reproduces 9router's transport, alias, and OAuth views,
with no secrets in any plugin file.

**Independent Test**: `cargo test -p zerorouter-registry --test parity` (transport, alias,
and OAuth modules) and `--test secrets` pass against `tests/fixtures/9router/`, with
bundled plugins only and no user config (quickstart steps 2–3).

### Tests for User Story 1 ⚠️ write first, confirm they fail

- [X] T026 [US1] Create the parity test harness in `crates/zerorouter-registry/tests/parity/main.rs`.
  - `mod transport; mod alias; mod oauth; mod lookup;`
  - Helpers: load a fixture from `$CARGO_MANIFEST_DIR/../../tests/fixtures/9router/<name>.json`, and open a bundled-only registry on an empty temp `ZEROROUTER_HOME`.
  - `lookup.rs` starts as an empty module and is filled in by US2.
- [X] T027 [P] [US1] Write the transport parity test in `crates/zerorouter-registry/tests/parity/transport.rs` (US1 scenario 1, SC-001).
  - 121 entities in total.
  - Exactly 83 have a transport; 38 are catalog-only.
  - For each of the 83, `serde_json::to_value(composed_transport(id))` equals `providers.json[id]` with `clientSecret` removed: endpoint, format (defaults to `openai` when absent), headers in order, quirks, and the `clientId`/`tokenUrl` fields.
  - Where the fixture has `clientSecret`, assert `client_secret.unwrap().matches(fixture["clientSecret"])`; where it has none, assert `client_secret.is_none()`.
  - For each entry of `tts-tables.json`, the owning provider's `capabilities.tts.models` lists the same model IDs in the same order.
- [X] T028 [P] [US1] Write the alias parity test in `crates/zerorouter-registry/tests/parity/alias.rs` (US1 scenarios 2 and 5, SC-001).
  - The 113 owned tokens in `alias.json` resolve to the same provider id.
  - `qw`, `dv`, `devin`, and `devin-cli` return `NotFound::Provider` (the R10 deviation, asserted explicitly).
  - The 83-entry id-to-alias map matches.
  - `provider("kr")` returns kiro.
- [X] T029 [P] [US1] Write the OAuth parity test in `crates/zerorouter-registry/tests/parity/oauth.rs` (US1 scenario 3, SC-001). All five sections of `oauth-urls.json` must match with zero differences: endpoints, token URLs, auth URLs, refresh URLs, and client IDs.
- [X] T030 [P] [US1] Write the secrets test in `crates/zerorouter-registry/tests/secrets.rs` (US1 scenario 4, SC-002).
  - Scan every `BUNDLED` source for each of the 4 credential values and for `client_secret`, and expect none.
  - For each of the 4 providers, the composed transport's `client_secret` is `Some`, and `matches` succeeds against the fixture value.
  - `format!("{:?}", secret)` and `format!("{}", secret)` are `***`.
- [X] T031 [P] [US1] Add `compile_fail` doc-tests to `crates/zerorouter-registry/src/credentials/mod.rs` ([contracts/registry-api.md](contracts/registry-api.md#behavioural-guarantees)): one calls `SecretString::expose()` from outside the crate, and one names the credential table. Both must fail to compile.

### Implementation for User Story 1

- [X] T032 [US1] Implement the public `Registry::composed_transport(&self, provider) -> Option<ComposedTransport<'_>>` in `crates/zerorouter-registry/src/views.rs`, per [contracts/registry-api.md § Composed transport](contracts/registry-api.md#composed-transport-public-secret-stays-opaque).
  - Start from the plugin `transport`.
  - Fill `format = "openai"` when it is absent.
  - Copy `client_id`/`token_url` from `oauth` only when the transport does not declare them (9router `OAUTH_INJECT_FIELDS`); kiro keeps its own transport `token_url`.
  - Set `client_secret: Option<&SecretString>` only when the credential is `ResolvedCredential::Available`.
  - Serialise to 9router's camelCase shape by exactly reversing the generator's key mapping (T016). `client_secret` is skipped during serialisation.
- [X] T033 [US1] Add `alias_view(token)` and `id_to_alias()` to `crates/zerorouter-registry/src/views.rs`. `id_to_alias` gives `alias` if set, else `id`, for the 83 entries in the baseline map. Also add `oauth_urls_view()`, which returns the 5-section shape of `oauth-urls.json`.
- [X] T034 [US1] Compute `credentials: HashMap<ProviderId, ResolvedCredential>` in `crates/zerorouter-registry/src/registry.rs` when the snapshot is built, by calling `credentials::bind(entry, active_plugin)` for each credential entry.
- [X] T035 [US1] Implement `crates/zerorouter-cli/src/cmd/providers.rs`: a list of id, alias, category, and capabilities, filtered by `--capability KIND`, with `--json`.
- [X] T036 [US1] Implement the basic `crates/zerorouter-cli/src/cmd/check.rs`: open the handle and print provider counts and all load errors. Exit 0 on success, 1 on error.
- [X] T037 [US1] Run `cargo test -p zerorouter-registry --test parity --test secrets` and fix any differences in the generator (T016) or the views (T032–T033), never in the fixtures.

**Checkpoint**: The bundled catalog is verified faithful to 9router. MVP.

---

## Phase 4: User Story 2: Model lookup and upstream ID resolution (Priority: P1)

**Goal**: `(provider, model id)` → validity, upstream ID, type, target format, and the
other model fields, matching 9router's `providerModels.js`.

**Independent Test**: The `lookup` module of `--test parity` passes every row of
`lookup.json`, and the `lookup.rs` unit tests pass (quickstart step 2).

### Tests for User Story 2 ⚠️ write first, confirm they fail

- [X] T038 [P] [US2] Write the lookup parity test in `crates/zerorouter-registry/tests/parity/lookup.rs` (SC-004). For every row in `lookup.json`, `Registry::model(alias, model)` must match the fixture. Map through the parity conventions:
  - `declared` ↔ `isValidModel`;
  - `upstream_id` ↔ `getModelUpstreamId`;
  - `kind: None` ↔ `null`;
  - `target_format`, `supported_formats`;
  - `quota_family: None` ↔ `"normal"`;
  - `strip: None` ↔ `[]`;
  - `name` ↔ `findModelName`.
- [X] T039 [P] [US2] Write explicit US2 scenario unit tests in a `#[cfg(test)] mod tests` block in `crates/zerorouter-registry/src/lookup.rs`:
  - `(high)` is stripped and re-appended;
  - a preset suffix is kept when the request has none and replaced when it has one;
  - `m(a(b))` is not stripped;
  - trailing whitespace after the suffix is handled;
  - on `kr`, `claude-sonnet-4-5(high)` → `claude-sonnet-4.5(high)`;
  - a non-tolerant provider does exact match only;
  - an undeclared model → base ID + suffix, with `declared = false`;
  - an untyped model → `kind == None`.

### Implementation for User Story 2

- [X] T040 [US2] Implement the pure lookup functions in `crates/zerorouter-registry/src/lookup.rs`, following the behaviour of `ref/9router/open-sse/config/providerModels.js` (`isValidModel`, `findModelName`, `getModelUpstreamId`).
  - `split_suffix(id) -> (&str, Option<&str>)`: a single `regex_lite::Regex` `\([^()]+\)\s*$` in a `LazyLock`, "only a final parenthesized group containing no parentheses counts" (FR-019).
  - `normalise_version_sep`: a digit-dash-digit sequence → digit-dot-digit (FR-020).
  - `upstream_id(model: Option<&Model>, base, suffix) -> String`: declared `upstream_id`, else the model id; then the request suffix, else the declared preset suffix. An undeclared model gives base + suffix (FR-021).
  - `derive_model_name`: a port of `ref/9router/open-sse/providers/models/namePatterns.js` `deriveModelName`.
- [X] T041 [US2] Add the per-provider model indices to `crates/zerorouter-registry/src/registry.rs`: an exact `HashMap<Box<str>, ModelIdx>`, plus a normalised-key map built **only** when `version_separator_tolerance` is true ([R11](research.md#r11-performance)).
  - Implement the catalog view on `CapabilitySection`: provider models whose `kind == Some(kind)`, plus `section.models`. For `llm`, include models with `kind == None` without changing their `kind`.
- [X] T042 [US2] Implement `Registry::model(provider, model_id) -> Result<ModelInfo, NotFound>` and `Registry::upstream_id(...)` in `crates/zerorouter-registry/src/registry.rs`, with `ModelInfo` as specified in [contracts/registry-api.md](contracts/registry-api.md#library).
  - Passthrough providers return `declared = true` for any id.
  - `upstream_id` never fails for a known provider.
  - An unknown provider → `NotFound::Provider`.
- [X] T043 [US2] Implement `crates/zerorouter-cli/src/cmd/model.rs`: `model PROVIDER MODEL [--json]` prints `ModelInfo`. Exit 0 when the provider is found, 2 on `NotFound`.
- [X] T044 [US2] Run `cargo test -p zerorouter-registry --test parity` (all modules) and the `lookup` unit tests until they are green.

**Checkpoint**: US1 and US2 both pass independently.

---

## Phase 5: User Story 3: Operator declares a unified model (Priority: P1)

**Goal**: `config.toml` declares unified models and per-provider settings. `resolve()`
classifies targets by shape, and an explicit reload swaps state atomically.

**Independent Test**: `--test unified` and `--test reload` pass, and quickstart steps 5, 6,
and 8 give the documented output.

### Tests for User Story 3 ⚠️ write first, confirm they fail

- [ ] T045 [P] [US3] Write the US3 tests in `crates/zerorouter-registry/tests/unified.rs`, each writing a temp `config.toml`:
  - s1: `sonnet` over `kr/claude-sonnet-4-5` and `openrouter/anthropic/claude-sonnet-4.5` → two members in declaration order; kiro upstream `claude-sonnet-4.5`; openrouter upstream unchanged.
  - s2: an unknown member provider, or an uncatalogued member model on a non-passthrough provider, → an error naming `unified_model[i].members[j]`.
  - s3: conflicting member kinds → rejected; an untyped member does not conflict.
  - s4: bare `claude-sonnet-4.5` → `NotFound::UnifiedModel`, with no provider inference.
  - s5: `kr/claude-sonnet-4-5` → `Resolution::Direct` with one provider.
  - s6: `openai/brand-new-model` → `Direct { catalogued: false }` with the requested id unchanged.
  - s7: `[provider.openai] allow_uncatalogued_models = false` → `NotFound::Model`, while a passthrough provider still resolves.
  - Edge cases:
    - a name containing `/` → rejected;
    - a duplicate name → rejected;
    - the same provider twice → rejected;
    - `[provider.nope]` → rejected;
    - `openrouter/meta-llama/llama-3` splits at the first `/`;
    - `""`, `"/gpt-4o"`, and `"openai/"` → `NotFound::EmptyTarget`;
    - `members = []` → `unified_model[i].members: must not be empty`;
    - a missing `config.toml` → no unified models.
- [ ] T046 [P] [US3] Write the reload test in `crates/zerorouter-registry/tests/reload.rs` (FR-024 – FR-026, SC-007).
  - 8 reader threads call `snapshot().resolve("probe")` in a loop while the main thread reloads 200 times, alternating config A (`probe` has 1 member) and config B (`probe` has 2 members). Every observed resolution has 1 or 2 members, never an error or a mix.
  - A reload with invalid `config.toml` returns `Err` with every error listed, and the old snapshot is still served.
  - A no-change reload gives the same provider count and no duplicates.
  - An `Arc<Registry>` held before a reload keeps resolving to its original result.

### Implementation for User Story 3

- [ ] T047 [P] [US3] Define `OperatorConfig` in `crates/zerorouter-registry/src/schema/config.rs` per [contracts/operator-config.md](contracts/operator-config.md), with `deny_unknown_fields` throughout.
  - `schema`.
  - `unified_model: Vec<UnifiedModelDecl { name, kind: Option<ModelKind>, members: Vec<MemberDecl { provider, model }> }>`.
  - `provider: BTreeMap<String, ProviderSettings { allow_uncatalogued_models: bool = true }>`.
  - `plugin_decisions: BTreeMap<String, Decision { Replace, Decline }>`.
- [ ] T048 [US3] Implement config loading and validation in `crates/zerorouter-registry/src/load.rs`. A missing file means the default config. Use exactly the error shapes of [operator-config.md § Validation](contracts/operator-config.md#validation):
  - "must be non-empty and must not contain `/`";
  - a duplicate name;
  - "`members`: must not be empty";
  - an unknown member provider (resolved via the alias index);
  - a model "not declared by provider" unless the provider is passthrough (suffix-stripped, tolerance applied);
  - a provider that is "already a member";
  - kind conflicts;
  - an unknown `[provider.<id>]`;
  - a `plugin_decisions` key that is not a bundled id.
  - Resolve each member's `upstream_id` once, at load, with `lookup::upstream_id` (FR-021).
  - Expose this as one function, `validate_config(config, providers, mode) -> (Vec<UnifiedModel>, Vec<ValidationError>, Vec<Dropped>)`, which both startup and reload (T060) call against the candidate provider set.
  - Startup exception ([operator-config.md § Validation](contracts/operator-config.md#validation)): in startup mode, a unified model whose member names the id of a user plugin skipped at startup is dropped and recorded in `LoadReport`, not fatal. Every other error is fatal at startup. In reload mode there is no exception.
- [ ] T049 [US3] Add `unified_models: HashMap<Box<str>, UnifiedIdx>`, `settings`, `unified_model(name)`, and `unified_models()` to `crates/zerorouter-registry/src/registry.rs`. `settings` falls back to the `ProviderSettings` default for providers that are not listed.
- [ ] T050 [US3] Implement `Registry::resolve(target) -> Result<Resolution, NotFound>` in `crates/zerorouter-registry/src/resolve.rs`.
  - Empty → `EmptyTarget`.
  - Contains `/` → split at the first `/`. An empty provider part or an empty model part → `EmptyTarget`. Otherwise provider via the alias index, then model lookup:
    - catalogued, or a passthrough provider → `Direct { catalogued: true, … }`;
    - uncatalogued and `allow_uncatalogued_models` → `Direct { catalogued: false, upstream_id: requested id + suffix unchanged }`;
    - otherwise `NotFound::Model`.
  - Otherwise → `Unified` or `NotFound::UnifiedModel`. Never infer a provider (FR-014a).
  - The only allocation is the upstream ID.
- [ ] T051 [US3] Complete `RegistryHandle` in `crates/zerorouter-registry/src/lib.rs`.
  - `open(home)` also loads `config.toml`. An invalid config is fatal at startup.
  - `reload()` takes the reload mutex, builds a full candidate from disk, and validates everything. On any error it returns `ReloadError { errors }` and leaves the active snapshot unchanged. On success it calls `store(Arc::new(candidate))` and returns its `LoadReport`.
  - Document that reload does blocking I/O and that async callers should use `spawn_blocking`.
  - No file watching (FR-026).
- [ ] T052 [US3] Implement `crates/zerorouter-cli/src/cmd/resolve.rs`: `resolve TARGET [--home DIR] [--json]`.
  - Print the resolution: provider(s), upstream IDs, and the catalogued flag.
  - Exit 0 when found, 2 on `NotFound`.
  - The `--json` shape is stable because the quickstart parses it.
- [ ] T053 [US3] Run `cargo test -p zerorouter-registry --test unified --test reload` and quickstart steps 5–6 until they are green.

**Checkpoint**: Unified models and direct addressing work on top of the bundled set, and
reload is safe.

---

## Phase 6: User Story 4: Plugin author installs a third-party provider (Priority: P2)

**Goal**: User plugins load through the same gate with precise errors, conflicts with
bundled ids follow the replace/decline state machine, and credentials are bound to hosts.

**Independent Test**: `--test gate` and `--test plugins` pass, `zerorouter-cli validate` on
the invalid corpus exits 1 with one well-formed error per file, and quickstart steps 4
and 7 give the documented output.

### Tests for User Story 4 ⚠️ write first, confirm they fail

- [ ] T054 [P] [US4] Create the gate corpus in `crates/zerorouter-registry/tests/gate/`.
  - `valid/minimal.toml` and `valid/typical.toml`, copied from [plugin-schema.md](contracts/plugin-schema.md).
  - `valid/llm-embedding.toml`, which declares only `llm` and `embedding` sections.
  - `valid/bare-models.toml`, which declares `models = ["m-a", "m-b"]`.
  - One `invalid/<rule>.toml` per rule. Each invalid file's first line is `# expect: <substring of the error>`. The rules:
    - `unknown-key` (`oauth.client_secret`);
    - `secret-header` (`Authorization`);
    - `unknown-executor-param` (`[transport.executor_params] api_key = "x"`);
    - `unknown-oauth-param` (`[oauth.params] refresh_token = "x"`);
    - `secret-model-param`;
    - `bad-model-entry` (`models = [1]`);
    - `url-userinfo`;
    - `url-query-key` (`?api_key=`);
    - `bad-category`;
    - `missing-id`;
    - `missing-category`;
    - `bad-schema-version`;
    - `bad-id-chars`;
    - `unknown-quirk`;
    - `unknown-hook`;
    - `unknown-format`;
    - `unknown-capability`;
    - `unreachable-section`;
    - `duplicate-model-id`;
    - `transports-without-transport`;
    - `bad-default-region`;
    - `bad-credential-fallback`;
    - `parse-error`.
- [ ] T055 [US4] Write the gate test in `crates/zerorouter-registry/tests/gate.rs` (US4 s1–s4, SC-003). Depends on T054.
  - Every `valid/*` file is accepted, and `minimal.toml` has no transport.
  - Every `invalid/*` file is rejected. Its rendered error contains the file name, `line:col`, the field path, the `# expect:` substring, and, for enum rules, the allowed values.
  - `llm-embedding.toml`: `capability(llm)` and `capability(embedding)` are `Some`, and `capability(tts)` is `Ok(None)`.
  - `bare-models.toml`: two models with ids `m-a` and `m-b` and derived names.
  - Catalog states (FR-005): `minimal.toml` has `models == None` (catalog unknown); a bundled provider that 9router lists with an empty model list (e.g. zed) has `models == Some([])` (offers none). Pick the provider by reading its generated file, not by assumption.
  - Every one of the 121 `BUNDLED` sources passes `validate`.
- [ ] T056 [P] [US4] Write the plugin tests in `crates/zerorouter-registry/tests/plugins.rs`, each on a temp `ZEROROUTER_HOME`:
  - s5: a user `kiro.toml` with no decision → bundled kiro is active and the report shows a pending conflict. `replace` → the user plugin is active. `decline` → bundled is active and the user plugin is reported as declined.
  - s6: a user `gemini-cli.toml` identical to the bundled one + `replace` → the credential is `Available`. The same file with `oauth2.googleapis.com` → `evil.example` → `Withheld`, and the report names the offending URL.
  - s7: two user plugins with the same id → both are skipped and reported with both paths. Two providers claiming one alias → a reported conflict naming both.
  - Startup: an invalid user plugin is skipped and reported, and the other plugins still load. `plugins/sub/x.toml` is ignored (top level only).
  - Startup exception: a unified model whose member names that skipped plugin's id is dropped and listed in the report; the other unified models load and `open` succeeds.
  - Settings: `[provider.gemini-cli]` survives a replace.
  - Reload: adding an invalid user plugin → the whole reload is rejected and the old snapshot is kept. Removing a user plugin that a unified model references → rejected, with an error naming the unified model and the missing provider.

### Implementation for User Story 4

- [ ] T057 [US4] Implement user plugin discovery in `crates/zerorouter-registry/src/load.rs`.
  - Read `$ZEROROUTER_HOME/plugins/*.toml`: top level only, sorted, other extensions ignored.
  - Run each file through `validate` with `PluginSource::User(path)`.
  - At startup (FR-010), skip invalid files and record them in `LoadReport.skipped`.
  - On reload (FR-024), any invalid file fails the whole candidate.
- [ ] T058 [US4] Implement conflict resolution in `crates/zerorouter-registry/src/load.rs` following the [data-model state machine](data-model.md#plugin-conflict-fr-013).
  - A bundled id with a user duplicate and no decision → `Pending`: the bundled plugin is active and the conflict is reported.
  - `replace` → the user plugin is active.
  - `decline` → the bundled plugin is active and the user plugin is reported as declined.
  - Two user plugins with the same id → both skipped at startup, and the reload is rejected.
  - Then run the `credential_fallback` reference check over the final active set.
- [ ] T059 [US4] Record withheld credentials in the report in `crates/zerorouter-registry/src/registry.rs`. When `credentials::bind` returns `Withheld { offending_url }` for a replaced provider, add it to `LoadReport.withheld_credentials` with a message saying OAuth for `<id>` will not work because of `<url>` (FR-012a).
- [ ] T060 [US4] Wire user plugins into config validation in `crates/zerorouter-registry/src/lib.rs`. Do not add a second validator; call T048's `validate_config` against the candidate provider set (bundled + active user plugins).
  - `open()`: startup mode, passing the ids of skipped user plugins so their unified models are dropped and reported.
  - `reload()`: reload mode. A member whose provider is missing from the candidate → an error naming the unified model and the provider, and the whole reload is rejected.
- [ ] T061 [P] [US4] Implement `crates/zerorouter-cli/src/cmd/validate.rs`: `validate FILE…` runs the gate on each file and prints `OK <file>` or each rendered `ValidationError`. Exit 0 if all files are valid, else 1.
- [ ] T062 [US4] Extend `crates/zerorouter-cli/src/cmd/check.rs` to print the full `LoadReport`: pending conflicts, declined plugins, withheld credentials with their offending URL, and skipped user plugins with their errors.
- [ ] T063 [US4] Run `cargo test -p zerorouter-registry --test gate --test plugins --test reload` and quickstart steps 4 and 7 until they are green.

**Checkpoint**: All four user stories are independently functional.

---

## Phase 7: Polish & Cross-Cutting Concerns

- [ ] T064 [P] Write the Criterion benchmark in `crates/zerorouter-registry/benches/resolve.rs` ([R11](research.md#r11-performance), constitution benchmark gate).
  - `load`: a full `RegistryHandle::open` with a two-unified-model config. Target < 50 ms.
  - `resolve`: `kr/claude-sonnet-4-5`, `kr/claude-sonnet-4-5(high)`, a unified `sonnet`, `openai/brand-new-model`, and a not-found bare name. Target p50 < 1 µs.
  - Save the results with `cargo bench -p zerorouter-registry --bench resolve -- --save-baseline slice-002`.
- [ ] T065 [P] Write operator and plugin-author documentation in `docs/plugins.md` and `docs/operator-config.md`, drawn from `contracts/`. They must be enough on their own to declare a two-member unified model (SC-005).
- [ ] T066 Run `/rust-parity-audit` on `crates/zerorouter-registry/src/lookup.rs` and `crates/zerorouter-registry/src/views.rs` (constitution parity-audit gate).
  - Fix every **must-fix** finding.
  - Record the deliberate deviations from [R10](research.md#r10-where-0router-deliberately-differs-from-9router-lookups) as accepted.
- [ ] T067 Run `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`, and fix everything they report.
- [ ] T068 Run all 9 steps of [quickstart.md](quickstart.md) end to end with a scratch `ZEROROUTER_HOME`, and correct the quickstart wherever the real output differs.
- [ ] T069 [P] Update the "What this is" section of `CLAUDE.md` to replace "pre-code … first Rust crate has not yet been written" with the workspace layout, and add the `generate.mjs` regeneration command.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: T001 blocks everything. The toolchain must work inside the gate.
- **Foundational (Phase 2)**: Depends on Setup and blocks all stories.
  - The schema (T007–T012) comes before the gate (T013–T015).
  - The generator (T016–T019) can run in parallel with the schema and gate work: it is Node and needs no cargo. T019's output must pass the gate once T015 exists.
  - T020–T025 need both.
- **US1 (Phase 3)**: Depends on Foundational.
- **US2 (Phase 4)**: Depends on Foundational. It is independent of US1, apart from the shared `tests/parity/main.rs` harness (T026) and `src/registry.rs`: T034 (US1) and T041–T042 (US2) both edit it. When the two stories run in parallel, land T034 first and rebase T041–T042 on it. The same applies to T049 and T059.
- **US3 (Phase 5)**: Depends on Foundational and on US2's `lookup` (T040–T042), because member upstream IDs and direct resolution use the model lookup.
- **US4 (Phase 6)**: Depends on Foundational and US3: decisions and settings live in `config.toml` (T047–T048), and reload lives in T051.
- **Polish (Phase 7)**: Depends on every story.

### Story Graph

```text
Setup → Foundational ─┬─▶ US1 (MVP)
                      └─▶ US2 ─▶ US3 ─▶ US4 ─▶ Polish
               (US1 and US2 can run in parallel; Polish also waits for US1)
```

### Within Each Story

- Write the tests, then watch them fail.
- Then do the schema/types, the registry/load logic, and the CLI subcommand, in that order.
- Finish with the green-run task.

---

## Parallel Examples

```text
# Phase 2: schema types together (separate files)
T007 enums.rs   T008 model.rs   T009 transport.rs   T010 oauth.rs   T011 capability.rs
# …while the generator is written (Node, no cargo needed)
T016 → T017 → T018 (same file, sequential)

# US1: parity modules together
T027 parity/transport.rs   T028 parity/alias.rs   T029 parity/oauth.rs   T030 secrets.rs   T031 doc-test

# US2 can start alongside US1 once T026 exists
T038 parity/lookup.rs   T039 lookup.rs unit tests

# US3 tests together
T045 unified.rs   T046 reload.rs   T047 schema/config.rs

# US4
T054 gate corpus   T056 plugins.rs   T061 cli validate.rs
```

---

## Implementation Strategy

### MVP (User Story 1)

1. Phase 1, including the T001 toolchain check.
2. Phase 2, including the generated bundled set.
3. Phase 3. **Stop and validate**: parity and secrets are green. The bundled catalog is
   now faithful to 9router.

### Incremental Delivery

1. US1 → faithful catalog.
2. US2 → correct upstream IDs.
3. US3 → unified models, direct addressing, and reload. This is the first slice that
   later routing work can consume.
4. US4 → third-party plugins and conflicts.
5. Polish → benchmark baseline, parity audit, docs.

A commit per task or per logical group is fine. The generated artefacts (T019) go in
their own commit, whose message names the `ref/9router` SHA.

---

## Notes

- Constraints quoted from the data model are binding. Do not relax them during
  implementation:
  - "`None` = no declared type";
  - "`default_region` must be a key of `regions`";
  - "only a final parenthesized group containing no parentheses counts".
- Never edit `ref/9router/` or `tests/fixtures/9router/` by hand. Regenerate them.
- Never add `inventory`, `tokio`, or `unsafe` to `zerorouter-registry`.
