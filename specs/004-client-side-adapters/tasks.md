---

description: "Task list for 004-client-side-adapters"
---

# Tasks: Client Side, Harness Adapters

**Input**: Design documents from `specs/004-client-side-adapters/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included, for three reasons:
- The spec's success criteria are test matrices: the hostile suite, guardrail, gate, tamper,
  lifecycle, scrambler, catalogue and kit-upgrade tests.
- Constitution VI requires parity tests against 9router.
- The benchmark gate covers the adapter hot path (FR-030).

Within each story, write the tests first and confirm they fail before implementing.

**Organization**: Tasks are grouped by user story, so each story can be implemented and tested
on its own. Phase 4 is a second foundation: it blocks US2–US5 but not US1.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US6)

## Path Conventions

Cargo workspace at the repo root ([plan § Project Structure](plan.md#project-structure)):

| Path | What it is |
|---|---|
| `crates/nullrouter-adapter-kit/` | Guest library: edit model, context, ABI glue. The only crate allowed `unsafe` |
| `crates/nullrouter-sandbox/` | wasmtime host: engine, module load, per-call instance, limits |
| `crates/nullrouter-adapters/` | Selectors, apply, guardrail, hermes, gate, scrambler, review, store, catalogue, alerts |
| `crates/nullrouter-builder/` | Separate binary; never a dependency of any other crate |
| `crates/nullrouter-engine/`, `-server/`, `-cli/` | Extended |
| `adapters/community/claude-code/` | The proof adapter. Outside the workspace (`exclude`) |
| `catalogue/index.toml` | The catalogue |
| `crates/nullrouter-adapters/tests/` | `gate/invalid/`, `hostile/` (sources plus checked-in `.wasm`), `guard/`, `fixtures/` |
| `tests/parity/deviations.toml` | Gains the slice 004 deviations |
| `tests/harness/` | Gains the hermes and Claude Code runners |

One addition to the plan's tree:
- The testkit that loads a fixture adapter straight into a store, skipping install, lives in
  `nullrouter-adapters` behind a `testkit` feature (`src/testkit.rs`). It lets US3, US4 and US6
  tests run without the builder.

**Where things run**:
- **CI** runs every `cargo test` and `clippy` (`.github/workflows/ci.yml`). Never run `cargo`
  in the session, not even `check`. Commit, push a group with the user's OK, and read the run.
- ***operator-run*** tasks need the operator's own accounts, a review model, a local build, or a
  write to `~/.rustup`. Ask the user to run the given command with `! …` in the session. Never
  ask for keys.
- Commands that write `~/.rustup` fail under Landlock even with `! …`. Ask the user to run
  those in a terminal outside the session.

**Before any task**: this worktree (`.worktrees/004`) is on branch `004-client-side-adapters`,
and its `.specify/feature.json` points at this slice. If either differs, stop and ask.

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Prerequisites confirmed, and new crates and directories that compile.

- [X] T001 Confirm the hook points exist (spec Assumptions: slices 003 and 005–009 are
  complete).
  - These slice 003 tasks must be `[X]` in `specs/003-request-pipeline/tasks.md`:
    - T049–T054 (the four style files and their codecs);
    - T055 (happy-path attempt) and T071 (full attempt loop with fallback) in
      `crates/nullrouter-engine/src/attempt.rs`;
    - T056 (text generation wired through `router.rs` and `relay.rs`);
    - T105 (record filling).
  - Slice 003's open live checks (T060, T087, T096) don't block this slice.
  - `attempt.rs` must still have `body_for` and `count_body`, `route.rs` must still have
    `decide`, and `journal/records.rs` must still read records as
    `serde_json::to_value(&RequestRecord)`. The seam in
    [R2](research.md#r2-where-an-adapter-runs-and-what-it-sees) (Update 2026-10-06) depends on
    all three.
  - If a check fails, **stop** and report what moved.
- [X] T002 Check the toolchain for adapters ([R1](research.md#r1-toolchain-and-crates)).
  - `rustc --version` must report 1.93.x.
  - `rustup target list --installed` must include `wasm32-unknown-unknown`. If it doesn't,
    this is *operator-run* outside the session: ask the user to run
    `rustup target add wasm32-unknown-unknown` in a terminal outside `claude-0router`. Only
    local fixture builds need it, so this doesn't block Phase 1. Never edit `identity/`.
  - CI resolves `wasmtime` 45 in T009. If it fails to resolve, **stop** and report.
- [X] T003 Update the workspace manifest `Cargo.toml`.
  - `rust-version = "1.93"`.
  - `exclude = ["adapters"]`.
  - Add to `[workspace.dependencies]`:
    - `wasmtime` 45 (`default-features = false`, features `cranelift`, `async`,
      `pooling-allocator`, `runtime`);
    - `syn` 2 (`full`, `visit`, `visit-mut`, `extra-traits`);
    - `prettyplease` 0.2, `proc-macro2` (`span-locations`), `flate2`, `tar`, `semver`;
    - path dependencies for the four new crates.
  - Do not add `wasi` or `component-model` features.
- [X] T004 [P] Create `crates/nullrouter-adapter-kit/Cargo.toml` and `src/lib.rs`.
  - Dependencies: `serde` (`derive`) and `serde_json` only. `version = "1.0.0"`.
  - Modules `edit`, `context`, `input`, `abi`, each an empty file.
  - Lints: the kit does **not** use `lints.workspace = true`. The workspace sets
    `unsafe_code = "forbid"`, which no inner `allow` can lift, and Cargo can't override one
    workspace lint. The kit declares its own `[lints.rust] unsafe_code = "deny"` and
    `[lints.clippy] all = { level = "warn", priority = -1 }`. Only `src/abi.rs` gets
    `#![allow(unsafe_code)]`. Record the reason in a comment that points to plan § Complexity
    Tracking.
- [X] T005 [P] Create `crates/nullrouter-sandbox/Cargo.toml` and `src/lib.rs`.
  - Dependencies: `wasmtime`, `tokio` (`time`, `rt`), `sha2`, `serde_json`, `thiserror`,
    `tracing`, `nullrouter-adapter-kit` (for the shared JSON types).
  - Modules `engine`, `module`, `call`, `abi`.
  - `[[bench]] name = "sandbox"`, `harness = false`. `lints.workspace = true`.
- [X] T006 [P] Create `crates/nullrouter-adapters/Cargo.toml` and `src/lib.rs`.
  - Dependencies: `nullrouter-adapter-kit`, `nullrouter-sandbox`, `nullrouter-wire`,
    `nullrouter-registry`, `serde`, `serde_json`, `toml`, `syn`, `prettyplease`,
    `proc-macro2`, `sha2`, `flate2`, `tar`, `semver`, `reqwest`, `tokio`, `thiserror`,
    `tracing`, `base64`.
  - Feature `testkit`.
  - Modules `selector`, `apply`, `runner`, `guard`, `builtin` (`hermes`), `gate`, `scramble`,
    `review`, `store`, `builder_client`, `catalogue`, `alerts`, `fingerprint`, and `testkit`
    (cfg feature).
  - `[[bench]] name = "adapters"`, `harness = false`.
- [X] T007 [P] Create `crates/nullrouter-builder/Cargo.toml` and `src/main.rs`.
  - A binary named `nullrouter-builder`, depending on `serde`, `serde_json`, `sha2`, `clap`
    and `thiserror`. No other workspace crate may depend on it.
  - Subcommands `setup` and `build`. `build` reads a JSON job from stdin.
- [X] T008 [P] Create the directories and placeholders.
  - `adapters/community/.cargo/config.toml` with
    `[patch.crates-io] nullrouter-adapter-kit = { path = "../../crates/nullrouter-adapter-kit" }`,
    so `cargo test` inside a community adapter resolves the unpublished kit
    ([R8](research.md#r8-the-builder), Kit source). It sits outside every package, so the gate
    never sees it, and the builder never reads it because it builds in a temp dir with its own
    `CARGO_HOME`.
  - `catalogue/index.toml` holding only `schema = 1`.
  - `crates/nullrouter-adapters/tests/{gate/invalid,hostile,guard,fixtures}/.gitkeep`.
- [X] T009 CI for adapters, and the skeleton through it.
  - In `.github/workflows/ci.yml`, add `targets: wasm32-unknown-unknown` to the
    `dtolnay/rust-toolchain` step, so the builder and install tests (T037, T053) run in CI
    instead of skipping.
  - Confirm with `grep -l 'nullrouter-builder' crates/*/Cargo.toml` that only the builder's
    own manifest names it.
  - Commit the skeleton and push it with the user's OK. CI's build and clippy must be green.

---

## Phase 2: Foundational (Blocking Prerequisites for all stories)

**Purpose**: The adapter seam in the engine, and everything hermes and third-party adapters
share: edits, selectors, key binding, records.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

### Tests for the foundation ⚠️

- [X] T010 [P] Selector tests in `crates/nullrouter-adapters/tests/selector.rs`.
  - Parse `messages[*].content[*]`, `tools`, `$`, `messages[2].images` and quoted keys.
  - Refuse more than 8 segments, empty segments and bad brackets.
  - Extract matches with concrete paths from a nested body. `$` yields the whole body at
    path `$`.
  - No match yields an empty list.
- [X] T011 [P] Edit-check and apply tests in `crates/nullrouter-adapters/tests/apply.rs`, one
  test per rule of [R5](research.md#r5-checking-an-adapters-edits-before-the-guardrail). Each
  rule is refused with its named `invalid_output` rule:
  - a path not under a selector;
  - a missing path;
  - an overlapping or prefix pair;
  - more than 1,024 edits;
  - a `replace` value over 4 MiB;
  - `removed` with `replace`, or `converted` with `remove`;
  - an unknown reason.

  Removing array elements at several indices applies from the highest index down, so the
  paths stay valid. The original body is untouched after apply.
- [X] T012 [P] Key binding tests in `crates/nullrouter-engine/tests/keys_harness.rs`.
  - `harness` round-trips through `keys.toml`, and files without it still load.
  - Reserved names (`opencode`, `grok-build`, `zcode`) and malformed names are refused.
  - `^[a-z][a-z0-9-]{1,31}$` is enforced.
  - An unknown well-formed name is accepted.
- [X] T013 [P] Record tests in `crates/nullrouter-engine/tests/records_adapter.rs`.
  - `AdapterRun`, `ContentChange` and `GuardrailEvent` serialise with exactly the
    [data-model.md](data-model.md#adapterrun-on-attempt-and-response_adapter-on-requestrecord)
    fields.
  - A record built from a run that removed a string containing a sentinel secret and a
    sentinel prompt holds neither (FR-025).
  - A record with an `AdapterRun` survives slice 006's journal write and read unchanged.
    Journal lines written before this slice still read.
- [X] T014 [P] Engine seam tests in `crates/nullrouter-engine/tests/adapter_seam.rs`, using a
  test adapter that removes a field.
  - (a) The request reaching the mock upstream lacks the field, and the attempt's record lists
    the change.
  - (b) On fallback to a second provider, the adapter runs again with the new `AttemptContext`,
    and each attempt has its own changes.
  - (c) A key without a harness runs no adapter and records no `adapter`.
  - (d) The context carries no header, key, agent id or session (FR-004). Assert it on the
    serialised context.
  - (e) A response-side adapter runs per stream event, and the first event reaches the client
    before the upstream sends the second (FR-005).
  - (f) A client disconnect during the adapter call cancels the upstream request.
  - (g) Cross-style: the edited body is decoded again, and the upstream body is encoded from
    that IR. An adapter with no edits causes no second decode.
  - (h) A token-count request runs the request side. A media request from a bound key runs no
    adapter and records `not_run{media_request}`.
  - (i) The routing prefix chain is the same with and without the adapter, so the warm lookup
    still finds the account.

### Implementation

- [X] T015 [P] Kit edit model in `crates/nullrouter-adapter-kit/src/edit.rs`.
  - `Path`: a vector of key or index segments, with `Display` as `a[1].b` and a round-trip
    parser.
  - `Kind { Removed, Converted }`.
  - `Reason`, the closed set `target_rejects_field`, `target_cannot_carry_block`,
    `foreign_block`, `format_conversion`, `param_unsupported_by_model`,
    `empty_after_removal`, `duplicate_tool`, `role_not_accepted`, with serde snake_case.
  - `Edit { op, path, kind, value? }`, and `Edits` with `remove(path, reason)` and
    `convert(path, value, reason)`.
  - JSON shapes as in [contracts/adapter-kit.md](contracts/adapter-kit.md#call-sequence).
- [X] T016 [P] Kit context and input in `crates/nullrouter-adapter-kit/src/context.rs` and
  `src/input.rs`.
  - `Context { direction, provider, target_style, same_style, model, model_type,
    capabilities { vision, file_input, reasoning }, stream, attempt }`, with capabilities
    `Option<bool>`.
  - `Input { parts: Vec<Part { path, value }> }`.
  - The `Adapter` trait with `on_request`, and default no-op `on_response` and `on_event`.
  - `KIT_ABI: u32 = 1`.
- [X] T017 Selectors in `crates/nullrouter-adapters/src/selector.rs`.
  - `Selector::parse`, with at most 8 segments.
  - `extract(&Value, &[Selector]) -> Vec<Part>`, a native walk that clones only the matched
    subtrees.
  - `covers(&[Selector], &Path) -> bool`.
  - Make T010 pass.
- [X] T018 Edit checks and apply in `crates/nullrouter-adapters/src/apply.rs`.
  - `check(&Value, &[Selector], &[Edit]) -> Result<(), InvalidOutput{rule}>` with the R5
    limits.
  - `apply(&Value, &[Edit]) -> Value` works on a clone. Array removals run in descending
    index order.
  - `changes(&[Edit]) -> Vec<ContentChange>`.
  - Make T011 pass.
- [X] T019 `AgentKey.harness` in `crates/nullrouter-engine/src/keys.rs`, plus `HarnessName`
  with its validation and the `BUILTIN` and `RESERVED` name tables in
  `crates/nullrouter-adapters/src/lib.rs`. Keep `deny_unknown_fields`. Make T012 pass.
- [X] T020 Record types in `crates/nullrouter-engine/src/records.rs`.
  - `Attempt.adapter: Option<AdapterRun>` and `RequestRecord.response_adapter:
    Option<AdapterRun>`.
  - `AdapterRun { harness, version, outcome, changes, guardrail, duration_us }`.
  - Outcomes: `ran`, `not_run{reason}` with the reasons listed in data-model.md, `failed`
    with `trap`, `deadline`, `memory` or `invalid_output{rule}`, and `blocked`.
  - `GuardrailEvent` and `ContentChange` exactly as in the data model.
  - Run the redactor over `path` strings.
  - Make T013 pass.
- [X] T021 `[adapters]` config: add `pub adapters: AdaptersSettings` (`#[serde(default)]`,
  `deny_unknown_fields`) to `OperatorConfig` in `crates/nullrouter-registry/src/schema/config.rs`,
  and pass it into the engine state in `crates/nullrouter-engine/src/state.rs`. Fields and
  defaults: `catalogue_url`, `builder`, `request_deadline_ms = 20` (1–1000),
  `event_deadline_ms = 2` (1–100), `memory_mib = 64` (1–512), `max_instances = 64`.
- [X] T022 The runner in `crates/nullrouter-adapters/src/runner.rs`.
  - `enum AdapterRunner { Builtin(Builtin), Wasm(WasmHandle) }`. The `Wasm` arm is a stub
    returning `not_run` until T046.
  - `run_request(&self, ctx, &Value) -> RunOutcome { body: Cow<Value>, run: AdapterRun }`,
    and `run_response` and `run_event` likewise.
  - Built-ins are resolved from a static table by `HarnessName`. No `Box<dyn>`.
- [X] T023 Engine seam in `crates/nullrouter-engine/src/attempt.rs` and `src/state.rs`.
  - `EngineState` gains the adapter view: the runner per harness.
  - For each attempt, when the key has a harness, build the `AttemptContext` from the
    attempt's provider, endpoint wire style, same_style flag, upstream model, model type and
    model capabilities. Run the request side on the **client-style body** before `body_for`
    and `count_body`. Put the `AdapterRun` on the `Attempt`.
  - Cross-style with edits: decode the edited body with the client's codec, and pass that IR
    to `encode`. Otherwise pass the request's own IR, as today.
  - Media requests skip the runner and record `not_run{media_request}`.
  - `route.rs` keeps building its chain from the request as received.
  - Fallback and stream-break resumes re-run the adapter against the original client body.
    The resume's continuation is added after the adapter.
  - Adapter work runs inside the attempt's `CancellationToken` scope.
- [X] T024 Response seam in `crates/nullrouter-server/src/relay.rs` (streamed) and in the
  engine's non-stream path.
  - After the event is re-encoded for the client (`for_client`), run `run_event` per event
    without buffering. Run `run_response` once on a non-stream body.
  - Pass-through frames are parsed for this only when the active adapter declares response
    selectors.
  - Aggregate the changes into `RequestRecord.response_adapter`, prefixing paths with
    `event[N].`, where `N` counts client events across attempts.
  - Make T014 pass.
- [X] T025 `keys` CLI in `crates/nullrouter-cli/src/cmd/keys.rs`, per
  [contracts/operator-cli.md](contracts/operator-cli.md#commands).
  - `keys issue <name> --harness H`.
  - `keys set-harness <name|id> <H>` and `keys set-harness <name|id> --clear`.
  - On merging main (2026-10-08) these became `--adapter` and `keys set-adapter`, and the field
    `AgentKey.adapter`: slice 010 owns `harness` as a display-only tag (research R11).
  - A `harness` column in `keys list`.
  - Writes are atomic and followed by a reload.

**Checkpoint**: a key can name a harness, the engine calls a runner per attempt and per
event, and records carry `AdapterRun`.

---

## Phase 3: User Story 1 — hermes works fully through 0router (Priority: P1) 🎯 MVP

**Goal**: A hermes key needs only a base URL. Tools, reasoning across turns, images and
attachments work on anthropic, openrouter, opencode-zen, opencode-go, xai and grok-cli,
streamed and not streamed.

**Independent Test**: [quickstart § 2](quickstart.md#2-hermes-us1-fr-006fr-009-sc-001). With
mocks, then live, every turn completes and every change is recorded.

### Tests for User Story 1 ⚠️

- [X] T026 [P] [US1] hermes unit tests in `crates/nullrouter-adapters/tests/hermes.rs`.
  - (a) `reasoning_content`, `reasoning` and `reasoning_details` on assistant messages are
    removed with `target_rejects_field` only when the provider is in the reject table and
    `same_style` is true.
  - (b) They are kept for a provider not in the table, with no change recorded (US1-2).
  - (c) They are left alone on cross-style attempts, since slice 003's encoder handles those.
  - (d) `messages[i].images` becomes `image_url` content parts:
    - covers raw base64 strings, `{data, mime}` objects and `{url}` data URLs;
    - a string `content` becomes `[{type:"text"}] + parts`;
    - the MIME type is sniffed from magic bytes for PNG, JPEG, GIF and WebP;
    - `content` is recorded `converted` / `format_conversion`, and the emptied `images` field
      `removed` / `format_conversion` (R5 ties `converted` to `replace`, and a key can only be
      dropped by `remove`).
  - (e) `attachments` and `experimental_attachments` entries map by MIME type: images to
    `image_url`, PDFs to `file` with `file_data` and `filename`.
  - (f) A MIME type no model can read is left unconverted.
  - (g) No edit ever touches `tool_calls`, `tools` or a `role:"tool"` message.
- [X] T027 [P] [US1] hermes engine tests in `crates/nullrouter-engine/tests/hermes_e2e.rs`,
  with the scripted mock for each chosen provider's wire style, streamed and not.
  - A three-turn session with a tool call and result, echoed reasoning, and an image turn.
  - Assert what each mock received, that every turn succeeds, and the record's changes.
  - US1-5: the same body from a key without a harness goes through unchanged, and no
    adapter is recorded.
- [ ] T028 [P] [US1] hermes harness runner in `tests/harness/hermes/`.
  - A script that drives the real hermes agent against `nullrouter serve`. No stand-in client:
    SC-001 is about hermes itself. If hermes can't be installed under Landlock, the script is
    *operator-run*, like T031, and the user installs hermes outside the session.
  - It covers tools, reasoning across turns, an image, an attachment, and streamed and
    non-streamed runs.
  - Gate it behind `NR_LIVE=1` in `crates/nullrouter-server/tests/harness_hermes.rs`
    (`#[ignore]`).

### Implementation for User Story 1

- [X] T029 [US1] The hermes adapter in `crates/nullrouter-adapters/src/builtin/hermes.rs`.
  - Request selectors: `messages[*].reasoning_content`, `messages[*].reasoning`,
    `messages[*].reasoning_details`, `messages[*].images`, `messages[*].attachments`,
    `messages[*].experimental_attachments`, `messages[*].content`, `messages[*].role`.
  - Echoed-reasoning removal per a `REJECTS_ECHOED_REASONING: &[(&str provider, &[&str
    field])]` table. The default is keep. Seed it with 9router's `paramSupport.js` set
    (groq, mistral, cerebras) for community providers. Chosen providers are added only by
    T031.
  - Image and attachment conversion per [R4](research.md#r4-hermes-built-in). Leave a MIME
    type the model can't read unconverted, so slice 003 skips the target as `cannot_carry`.
  - Make T026 and T027 pass.
- [X] T030 [US1] Register hermes in the built-in table (`runner.rs`), and add these entries to
  `tests/parity/deviations.toml`:
  - `hermes images: converted, 9router deletes (modality.js)`;
  - `echoed reasoning: removed by adapter per table, not in core (paramSupport.js)`.

  Assert both in a parity test in `crates/nullrouter-adapters/tests/hermes.rs`.
- [ ] T031 [US1] *operator-run* live check that fills the reject table.
  - Ask the user to run
    `! NR_LIVE=1 cargo test -p nullrouter-server --test harness_hermes -- --ignored --nocapture`,
    once with the table empty for the six chosen text providers. xai and grok-cli need
    signed-in accounts (slice 005).
  - Add only the providers that returned a 400 naming an echoed reasoning field to
    `REJECTS_ECHOED_REASONING`, and cite the run date in a comment.
  - Re-run until every turn passes (SC-001). Record the results in
    `tests/harness/README.md`.
- [X] T032 [US1] Add `nullrouter adapters list` in `crates/nullrouter-cli/src/cmd/adapters.rs`
  (new) that shows hermes as `built-in`, for US1's operator view. Other subcommands return
  "not available yet" until US2.

**Checkpoint**: hermes works end to end. This is the MVP, and it ships with no sandbox or
builder.

---

## Phase 4: Third-Party Foundation (blocks US2–US5, not US1)

**Purpose**: Everything a third-party adapter needs before any of it may serve: the kit ABI,
the sandbox, the store and state machine, alerts, the builder, the source-match check and the
guardrail. FR-015 forbids serving an unguarded third-party change, so the guardrail is
foundation here, not a later story.

### Tests ⚠️

- [X] T033 [P] Sandbox tests in `crates/nullrouter-sandbox/tests/sandbox.rs`, using WAT
  fixtures compiled with `wasmtime::Module::new` in-test.
  - All cases done (CI #117 load, #119 call). The deadline test allows deadline + 100 ms, not
    + 5 ms: a shared runner can pause longer than 5 ms, so the tight figure belongs in the bench.
  - A module importing anything other than `nr.abi_version` or `nr.log` (for example
    `wasi_snapshot_preview1.fd_write` or `env.socket`) is refused at load, with the import
    named.
  - A missing `zr_on_request`, `zr_alloc` or `memory` export is refused.
  - A module without the `nr.abi` section, or with an unsupported ABI, is refused.
  - An infinite loop returns `deadline` within the deadline plus 5 ms, without blocking
    other Tokio tasks: a concurrent timer task keeps ticking.
  - A `memory.grow` past 64 MiB returns `memory`.
  - `unreachable` returns `trap`.
  - A valid module's output round-trips.
- [X] T034 [P] Guardrail tests in `crates/nullrouter-adapters/tests/guard.rs`, with one
  module per client style under `tests/guard/` (`openai_chat.rs`, `anthropic_messages.rs`,
  `openai_responses.rs`, `gemini.rs`), per [R6](research.md#r6-the-guardrail).
  - Request cases, each giving its named rule:
    - add a tool call; change a tool call's name, arguments or id;
    - add or change a tool definition (name, description, schema);
    - add or change a tool result;
    - add an opaque block;
    - add an unplaced key.
  - The same rules for a non-stream response.
  - Stream events: add a tool-call start, change an argument delta, add an opaque block.
  - Allowed cases: remove a tool call, a definition, a result or an opaque block;
    convert non-tool content. These must pass (SC-003, 0% false positives).
  - Undecodable cases: an edit that leaves the body or event undecodable in the client style
    gives `failed{invalid_output: undecodable}`, sends the original, raises `adapter_failed`,
    and does **not** mark the adapter suspect ([R6](research.md#r6-the-guardrail) step 4).
  - Done (green in CI #127): `tests/guard.rs` with one line per style in `tests/guard/`,
    expanding a shared suite of 18 cases, and the stream-event cases in `guard.rs`. The event
    IR has no opaque block, so "add an opaque block" is covered for requests only.
- [X] T035 [P] Store and state-machine tests in `crates/nullrouter-adapters/tests/store.rs`.
  - Every transition in the
    [data-model.md state machine](data-model.md#adapterversion-state-machine), and every
    forbidden transition refused. For example, approving from `in_review` is refused.
  - `VersionId` form: `v` + semver + `-` + the first 8 hex of `source_fp`.
  - `index.toml` round-trips, and writes are atomic.
  - Files are mode 0600 and directories 0700.
  - `serve` refuses a group-readable `adapters/`.
  - Done at the store level (CI #121): `Store::open` refuses it. The `serve` call waits for the
    engine loader.
  - Done (2026-10-09): `serve` calls `Engine::open_adapters` before listening, which refuses it;
    covered by `engine/tests/adapter_store.rs::a_group_readable_adapters_directory_is_refused`.
- [X] T036 [P] Fingerprint and tamper tests in `crates/nullrouter-adapters/tests/tamper.rs`.
  - `source_fp` is stable across file order and mtime, and changes on any byte, rename or
    added file.
  - Loading after editing `source/src/lib.rs` or `module.wasm` is refused with
    `source_mismatch`. An alert is raised, and a request from a bound key completes as a
    plain client (SC-005, US2-6).
  - Fingerprint and `Store::verify` done (CI #121). The alert and the plain-client request wait
    for alerts (T044) and the engine loader.
  - Done (2026-10-09): `adapters/tests/loader.rs` (`an_edited_source_…`, `an_edited_module_…`
    raise `source_mismatch`) and `engine/tests/adapter_store.rs::a_version_edited_after_approval_runs_as_a_plain_client_and_the_reload_notices`.
- [X] T037 [P] Builder tests in `crates/nullrouter-builder/tests/build.rs`, which skip with a
  message when the wasm32 target is missing.
  - The fixture `crates/nullrouter-adapters/tests/fixtures/noop/` builds offline after
    `setup`, resolving the kit from the local registry with no crates.io access.
  - It runs in CI, where T009 installs the target, and skips locally.
  - Two builds give an identical `wasm_hash`.
  - A fixture with a `compile` error returns `{"ok":false,"error":"compile"}` with at most
    40 lines of detail.
  - `-F unsafe_code` is active: an `unsafe` block in a fixture fails to compile, even when
    the gate is bypassed.
  - `export!` expansions don't trip the lint.
  - The kit itself compiles under `-F unsafe_code` only because Cargo passes `--cap-lints` to
    registry dependencies. Assert that the kit resolves from the local registry, never as a
    path dependency, in builder builds.
  - The environment passed to rustc contains only `PATH` and `CARGO_*`.

### Implementation

- [X] T038 [P] Kit ABI in `crates/nullrouter-adapter-kit/src/abi.rs`, the file allowed
  `unsafe`.
  - `zr_alloc`, and input decode from `(ptr, len)`.
  - The output is packed as `(ptr << 32) | len`, and `0` means no edits.
  - The `nr.abi` custom section, via `#[link_section]` inside the macro.
  - `macro_rules! export!` generates `zr_on_request`, `zr_on_response` and `zr_on_event`
    for a type implementing `Adapter`.
  - A `log!` macro calling the `nr.log` import, capped at 512 bytes.
  - Document that `#[no_mangle]` appears only inside `export!`.
  - Done (green in CI #152). The macro writes `#[unsafe(no_mangle)]` and `#[unsafe(link_section)]` (edition
    2024). The `nr.abi` section is gated to `wasm32`. `run_bytes` holds the logic with no raw
    pointers, so host tests cover it. Whether `-F unsafe_code` ignores the macro's expansion in
    an author's crate is checked by the builder test (T037), which needs the wasm32 target.
- [X] T039 [P] Sandbox engine in `crates/nullrouter-sandbox/src/engine.rs`.
  - One process-wide `Engine`, with `epoch_interruption(true)`, `async_support(true)`,
    `consume_fuel(false)`, and threads, relaxed SIMD and multi-memory off.
  - Pooling allocator sized by `max_instances`.
  - A Tokio ticker task that increments the epoch every 1 ms and stops on shutdown.
- [X] T040 Sandbox module load in `crates/nullrouter-sandbox/src/module.rs`.
  - Inputs: `wasm` bytes, the expected `wasm_hash` and the manifest flags.
  - Checks: SHA-256 matches; `Module::new` (never `deserialize`); imports ⊆ {`nr.abi_version`,
    `nr.log`}; required exports present; `nr.abi` supported (current and previous major).
  - Returns an `InstancePre`, or a `LoadError` naming the reason.
  - Make T033's load cases pass.
- [X] T041 Sandbox call in `crates/nullrouter-sandbox/src/call.rs` and `src/abi.rs`.
  - A fresh `Store` per call, with a `ResourceLimiter` of 64 MiB memory, 10,000 table
    elements and 1 instance.
  - `set_epoch_deadline`, with `epoch_deadline_async_yield_and_update`.
  - Write the input through `zr_alloc`, call, and read the output with bounds checks
    (16 MiB in and out).
  - Map traps to `deadline`, `memory`, `trap` or `invalid_output`.
  - `nr.log` is rate-limited to 8 calls per invocation, redacted, and written at `debug`.
  - The previous-ABI shim goes in `abi.rs`.
  - Make T033 pass.
- [X] T042 [P] Fingerprint in `crates/nullrouter-adapters/src/fingerprint.rs`.
  - The canonical tree hash, over sorted relative paths, each `path\0len\0bytes`.
  - Output `sha256:<hex>`.
- [X] T043 Store in `crates/nullrouter-adapters/src/store.rs`, per
  [data-model.md](data-model.md#adapterindex-adaptersindextoml).
  - The layout `adapters/<harness>/<version-id>/{source/, module.wasm, build.json,
    review.json, decision.json}`.
  - `index.toml` with `[review]`, `[[harness]]` (`active`) and `[[harness.version]]`
    (`state`, `state_reason`, `source_fp`, `wasm_hash`, `kit_abi`, `origin`, `submitted`,
    `rebuilding`).
  - A `transition(from, to)` table that refuses anything else.
  - Atomic writes, mode 0600 files and 0700 directories.
  - Make T035 pass.
- [X] T044 [P] Alerts in `crates/nullrouter-adapters/src/alerts.rs`.
  - `adapters/alerts.toml` holds `Alert { id: al_+10, kind, harness, version, record?,
    detail, at, acked? }`. Kinds: `guardrail`, `adapter_failed`, `source_mismatch`,
    `module_refused`, `rebuild_failed`, `quarantined`, `refused`.
  - Repeated `adapter_failed` alerts for the same version and reason within 60 s fold into
    one alert with a count.
  - Every alert is also logged at `warn`, and `detail` is fixed text plus codes only.
- [X] T045 The builder in `crates/nullrouter-builder/src/main.rs`, per
  [R8](research.md#r8-the-builder) and
  [contracts/adapter-package.md](contracts/adapter-package.md#after-the-gate).
  - Kit embedding ([R8](research.md#r8-the-builder), Kit source): the kit is not on
    crates.io.
    - `tools/package-kit.sh` runs `cargo package -p nullrouter-adapter-kit --no-verify`, and
      writes the `.crate` and a pinned `Cargo.lock` (kit, `serde`, `serde_json`) to
      `crates/nullrouter-builder/kit/`.
    - The builder embeds both with `include_bytes!`.
    - A test in `crates/nullrouter-builder/tests/kit_embed.rs` fails if the embedded `.crate`'s
      sources differ from `crates/nullrouter-adapter-kit/`, so the two never drift.
  - `setup`:
    - write `$NULLROUTER_HOME/builder/rust-toolchain.toml`, pinned to 1.93.1 with
      `wasm32-unknown-unknown`;
    - unpack the embedded kit into `builder/vendor/nullrouter-adapter-kit-<ver>/`, with its
      `.cargo-checksum.json`;
    - fetch `serde` and `serde_json` at the locked versions into the same directory. This is
      the builder's only network use, run once on the operator's command;
    - write `builder/config.toml` with `source.crates-io.replace-with = "vendored"`.
  - `tools/build-adapter-fixtures.sh`: *operator-run*, outside the session. It runs the
    builder over every hostile source (T065) and `adapters/community/claude-code` (T080), and
    writes each `.wasm` and `build.json` to its fixture directory. Ask the user to run it
    whenever one of those sources changes. CI's T080 companion check catches a stale fixture.
  - `build`:
    - copy the source to a fresh temp dir, adding `crate-type = ["cdylib"]` through a
      generated `[lib]` (the author may not set it), and copy in the builder's pinned
      `Cargo.lock` (packages carry none);
    - build `--release --offline --locked --target wasm32-unknown-unknown`, with
      `CARGO_HOME` set to the builder's home;
    - `RUSTFLAGS=-F unsafe_code --remap-path-prefix=<tmp>=/src -C debuginfo=0 -C strip=symbols`;
    - `env_clear` plus `PATH` and `CARGO_*` only;
    - `setrlimit` of 120 s CPU and 2 GiB address space;
    - build twice and compare the hashes;
    - output the JSON result.
  - Make T037 pass.
  - Done (2026-10-09), with deviations recorded in R8: the kit's sources are embedded with
    `include_str!` (no `.crate`, no `tools/package-kit.sh`, no `kit_embed.rs`), `setup` vendors
    from the embedded workspace `Cargo.lock`, and the limits come from `prlimit(1)`, plus a
    600 s wall cap, so the builder has no `unsafe`. Library entry points for T037 and T046:
    `setup(dir)`, `build(dir, &Job)`, `builder_dir(home)`, `source_fp`.
- [X] T046 Wasm runner arm in `crates/nullrouter-adapters/src/runner.rs` and
  `builder_client.rs`.
  - `WasmHandle { harness, version, manifest selectors, InstancePre }` is loaded at reload
    from the store's active version.
  - Before load, recompute `source_fp` from `source/` and `wasm_hash` from `module.wasm`, and
    compare both with the index. A mismatch gives `not_run{source_mismatch}` and a
    `source_mismatch` alert.
  - Compiled modules are cached by `wasm_hash` across reloads.
  - Call path: selectors extract, sandbox call, T018 checks, apply, T048 guardrail, then the
    record.
  - Every `failed{…}` outcome (`trap`, `deadline`, `memory`, `invalid_output{rule}`) raises an
    `adapter_failed` alert through T044, folded per version and reason within 60 s (spec Edge
    Cases: "the record and an alert say why").
  - `builder_client.rs` spawns the configured builder binary with a JSON job and a timeout.
    A missing binary gives `builder_not_installed`.
  - Make T036 pass.
  - Request side done (green in CI #132, whose one failure was an unrelated startup race, fixed
    separately): the runner's `run_request`, `run_response` and
    `run_event` are now `async`, because the sandbox call yields to Tokio; the engine's and
    server's callers `.await` them. `WasmHandle` holds the loaded module and, per request, the
    client style (`with_client`) that the guardrail decodes with. Without a client style an
    edit is never applied. Tests: `tests/wasm_runner.rs`, on WAT modules.
  - Loading done (green in CI #135): `loader.rs` reads the serving version, re-hashes its
    source and module (`not_run{source_mismatch}` plus a `source_mismatch` alert on a
    difference), refuses a module the load gate refuses (`module_refused` alert), and keeps
    compiled modules by `wasm_hash`. `WasmHandle` now carries the `not_run` reason, so a suspect
    version records `suspect`. Tests: `tests/loader.rs`.
  - Engine wiring done (green in CI #137): `Engine::open_adapters` (called by `serve`, which
    refuses a group-readable `adapters/`) and `refresh_adapters` (called at every reload) put
    one runner per harness in the store into the engine. Tests: `engine/tests/adapter_store.rs`;
    T047's `install_fixture` is in `testkit.rs`, `wat_adapter` is not.
  - Run outcomes done (green in CI #144): the attempt loop hands every `failed` or `blocked`
    third-party run to `Engine::note_adapter_run`, which raises an `adapter_failed` alert (folded)
    or a `guardrail` alert, and for a block marks the version `suspect` and loads the store
    again, so the next request records `not_run{suspect}`. Test: `adapter_store.rs`. The index is
    read, changed and written whole, so an operator command running at the same moment can lose
    the mark; the next block marks it again.
  - Response and event sides done (green in CI #145): the arm calls `zr_on_response` on a whole
    answer and `zr_on_event` on each stream event when the manifest declares response selectors
    (`events = true` for events; 20 ms and 2 ms deadlines). `reads_responses()` is true for such a
    module. The guardrail checks an answer with `check_response` and an event with the new
    `check_event_frame`, which reads the original and the edited event with a fresh stream reader
    each, so no state is carried across events: a tool-argument delta opens a call with no id on
    both sides, and only an added or changed start or fragment is a violation. Failed and blocked
    response runs raise alerts and mark the version suspect, like request runs
    (`Engine::settle_adapter_run`). Tests: `tests/wasm_runner.rs`.
  - Stream path through the server (green in CI #150): `crates/nullrouter-server/tests/adapter_stream.rs`
    drives a store version over a real streamed answer: an event edit reaches the client and the
    record, and a blocked event leaves the stream as it was, marks the version suspect and the
    next request records `not_run{suspect}`.
  - `builder_client.rs` done (2026-10-09): `run_builder` spawns the builder with `--home <home> build`,
    the job on stdin and a timeout, and checks the result's `source_fp`. Outcome codes: the
    builder's refusals, `builder_not_installed`, `builder_timeout`, `builder_failed`,
    `builder_bad_output`, `source_mismatch`. Tests: `tests/builder_client.rs`, on fake builders.
- [X] T047 [P] `nullrouter-adapters` testkit in `src/testkit.rs` (feature `testkit`).
  - `install_fixture(home, harness, wasm_bytes, source_dir, state)` writes a store entry
    directly, with the correct hashes.
  - `wat_adapter(behaviour)` builds small WAT modules for hostile and guard cases, so tests
    don't need the builder.
  - Done (2026-10-09): `install_fixture(home, harness, manifest, wasm)` (the manifest stands in
    for `source_dir`; the state is walked to `approved`), and `wat_adapter(Behaviour)` returning
    a binary module (`wat` is an optional dependency behind `testkit`).
- [X] T048 The guardrail in `crates/nullrouter-adapters/src/guard.rs`, per
  [R6](research.md#r6-the-guardrail).
  - `check_request(style, &before_ir, &after_body)`, `check_response(...)` and
    `check_event(style, &before_event, &after_event)`.
  - They use slice 003's `nullrouter-wire` codecs and return `Ok`, `Violation{rule, paths}`,
    or `Undecodable`. `Undecodable` is handled as a failure (`invalid_output` rule
    `undecodable`): send the original, raise `adapter_failed`, and don't mark the adapter
    suspect.
  - The multisets cover:
    - tool calls: id, name and canonical JSON args (sorted keys);
    - tool results: id and canonical content;
    - tool definitions: name, description and canonical schema;
    - opaque blocks: the canonical hash;
    - unplaced key paths.
  - It runs only when the edits are non-empty, and only for the `Wasm` arm.
  - On a violation:
    - discard the edits and send the original;
    - set the version to `suspect` in the store, and reload;
    - raise a `guardrail` alert;
    - record a `GuardrailEvent`, with outcome `blocked`.

    Mid-stream, the rest of the response goes out without the adapter: the runner is dropped
    for that request.
  - Make T034 pass.

**Checkpoint**: an approved WASM adapter placed through the testkit serves bound keys inside
the sandbox, under the guardrail, and only while its hashes match.

---

## Phase 5: User Story 2 — Install, review, decide (Priority: P1)

**Goal**: The operator installs from a local folder, an archive or the catalogue. Each
install passes the gate, is built, and goes to review, and the operator decides. Nothing
serves before approval, and an operator with no adapters needs none of it.

**Independent Test**: [quickstart § 3, 4, 9](quickstart.md#3-install-review-decide-us2-fr-019fr-022).

### Tests for User Story 2 ⚠️

- [X] T049 [P] [US2] Gate corpus in `crates/nullrouter-adapters/tests/gate/invalid/`.
  - One directory per code in
    [contracts/adapter-package.md § Gate rules](contracts/adapter-package.md#gate-rules-and-refusal-messages),
    21 codes, each with a golden `.expected` file (`refused: <code> at <location>: <message>`).
  - Plus `multi_reason/`, holding `foreign_dependency`, `build_script`, `proc_macro` and
    `opaque_blob` together, whose `.expected` lists all four.
  - Plus `valid/noop`, which must pass.
  - Test runner: `crates/nullrouter-adapters/tests/gate.rs`, covering SC-004.
- [X] T050 [P] [US2] Scrambler tests in `crates/nullrouter-adapters/tests/scramble.rs`.
  - Collect every identifier the fixture defines (items, fields, variants, bindings,
    lifetimes, labels, generics) with a `syn` visitor.
  - Assert that none appears in the scrambled output, as whole tokens.
  - Assert no `//`, `/*`, `///` or `#[doc` remains.
  - Assert that kit and `std` names and string literals survive.
  - Assert the scrambled output still parses (SC-007).
- [X] T051 [P] [US2] Review tests in `crates/nullrouter-adapters/tests/review.rs`, against a
  mock provider via the engine testkit.
  - (a) With no `[review]`, the state is `quarantined` / `no_review_model`, and no request is
    made.
  - (b) When estimate + reserve > budget, the state is `quarantined`, with a reason giving
    both numbers, and no request is made.
  - (c) The review request's body contains:
    - no `tools`;
    - the fixed system prompt;
    - only the scrambled source and manifest selectors in the user message;
    - `max_tokens = reserve_output`.

    It also contains no record, key or header value.
  - (d) A malformed report gets one retry, then `quarantined` / `review failed`.
  - (d2) If the first call's recorded usage plus the retry's estimate and reserve exceeds
    `budget_tokens`, there is no retry: `quarantined` / `budget exhausted`, with both
    numbers.
  - (e) A provider error gives `quarantined`, and a previously active version keeps serving.
  - (f) A valid report gives `reported`, with `review.json` written.
  - (g) The review request is recorded with agent `review:<harness>@<version>`.
- [X] T052 [P] [US2] Catalogue tests in `crates/nullrouter-adapters/tests/catalogue.rs`,
  against a local HTTPS mock (self-signed CA trusted in-test).
  - Index parse, with unknown keys refused.
  - Archive `sha256` mismatch gives `catalogue_hash_mismatch`, with nothing unpacked.
  - Fingerprint mismatch gives `catalogue_fp_mismatch`, before the gate.
  - Unpack refuses symlinks, hard links, `..` and absolute paths, more than 64 entries and
    more than 256 KiB.
  - Redirect to another host is refused. HTTP is refused. The size caps hold.
  - A local `.tar.gz` that the operator supplies goes through the same unpack rules (FR-031).
  - Catalogue and local installs of the same source produce identical store entries apart
    from `origin` (SC-012).
- [X] T053 [P] [US2] Install pipeline test in
  `crates/nullrouter-server/tests/adapter_install.rs`. It needs the builder and skips when
  the target is missing.
  - `adapters install fixtures/noop` goes queued → building → in_review → reported (mock
    review model).
  - A bound key's requests are served as a plain client, recording
    `not_run{no_approved_version}`, until `approve`. From the next request on, the adapter
    runs.
  - Keys bound to other harnesses are unaffected.
- [X] T054 [P] [US2] No-adapter run test in
  `crates/nullrouter-server/tests/no_adapters.rs` (SC-011, US2-7).
  - With `[adapters] builder` pointing at a missing path, and no `[review]`, `serve` starts
    and logs no adapter warning.
  - Slice 003's end-to-end smoke passes, and the process never spawns a child.
  - The rest of SC-011 is CI's full workspace run: no test outside this slice configures a
    builder or a review model.
- [X] T055 [P] [US2] Zero-contact test in `crates/nullrouter-server/tests/catalogue_quiet.rs`.
  - With `catalogue_url` pointing at a counting mock, run `serve` through a full request mix,
    a reload and a restart.
  - The mock counts 0 requests (SC-012, FR-031).

### Implementation for User Story 2

- [X] T056 [US2] The gate in `crates/nullrouter-adapters/src/gate.rs`, per
  [R7](research.md#r7-validation-gate-for-adapter-source) and the contract table.
  - The file walk uses `symlink_metadata`, and never follows links.
  - Size and count caps: 256 KiB total, 64 files, 64 KiB per `.rs` file.
  - UTF-8 and NUL checks.
  - `Cargo.toml` goes through a strict `toml::Table` walk, and `adapter.toml` through serde
    with `deny_unknown_fields`.
  - A `syn::visit` pass over each `.rs` file, for:
    - `unsafe`, `extern` blocks, `extern crate`;
    - `#[no_mangle]`, `#[export_name]`, `#[link_section]`, `#[link]`, `#[path]`;
    - the forbidden macros, matched by the last path segment;
    - literal blob runs: base64 or hex over 256 characters; integer arrays with more than 256
      elements or encoding more than 128 bytes.
  - Collect every reason, and sort them by file and line.
  - Make T049 pass.
- [X] T057 [US2] The scrambler in `crates/nullrouter-adapters/src/scramble.rs`, per
  [R10](research.md#r10-review-pipeline).
  - Pass 1 collects the definitions.
  - Pass 2 is a `VisitMut` that renames defined identifiers to `v1…vN` in first-seen order,
    leaving kit, `std`, `core` and `alloc` paths alone.
  - Strip `#[doc]` attributes, and print with `prettyplease`.
  - Keep a line map on the operator's side only, stored in `review.json`, never sent.
  - Make T050 pass.
- [X] T058 [US2] Internal requests in `crates/nullrouter-engine/src/internal.rs` (new).
  - `InternalRequest { agent_label, model, system, user, max_tokens }` runs through the
    normal plan and attempt loop on the operator's accounts, with no key check and no
    adapter.
  - It is recorded with the given agent label.
  - The review uses it. It is not reachable from any HTTP route.
- [X] T059 [US2] Review in `crates/nullrouter-adapters/src/review.rs`.
  - Resolve `[review] model`.
  - Estimate the input with `nullrouter_wire::estimate`, and add `reserve_output`. Compare
    with `budget_tokens`, and quarantine with both numbers when it doesn't fit.
  - The fixed system prompt frames the source as untrusted data, and requires the JSON
    `{risk, summary, findings[{location, concern}]}`.
  - Parse strictly: `summary` ≤ 2,000 chars, ≤ 50 findings. One retry, only if the tokens used
    so far plus the retry's estimate and reserve fit `budget_tokens`
    ([R10](research.md#r10-review-pipeline)).
  - Write `review.json` with `model`, `provider`, `tokens_in`, `tokens_out` and `record`.
  - Reviews run from a one-at-a-time background queue (a tokio task).
  - Make T051 pass.
- [X] T060 [US2] The install flow in `crates/nullrouter-adapters/src/lib.rs` (`install()`).
  - Unpack an archive, or read a directory.
  - Run the gate. A refusal writes a version entry with state `refused` and its reasons, and
    raises a `refused` alert.
  - Compute `source_fp`, store `source/`, and set the state to `queued`.
  - With the builder present, the state becomes `building`. Call it and check that the
    result's `source_fp` equals the stored one. Store `module.wasm` and `build.json`, and set
    the state to `in_review`.
  - Enqueue the review. A missing builder leaves `queued` / `builder_not_installed`.
  - Make T053 pass.
- [X] T061 [US2] The catalogue client in `crates/nullrouter-adapters/src/catalogue.rs`, per
  [contracts/catalogue.md](contracts/catalogue.md).
  - Fetch with slice 003's `reqwest` client and SSRF rules:
    - HTTPS only;
    - same-host redirects only, at most 3;
    - an index cap of 1 MiB and an archive cap of 2 MiB;
    - a 30 s timeout.
  - Check `sha256`, unpack safely into `adapters/.staging/`, and check `source_fp`. Then
    call `install()` with `origin = {catalogue = url}`.
  - Nothing in `serve` or reload references this module. A test in
    `crates/nullrouter-server/tests/catalogue_quiet.rs` scans `crates/nullrouter-server/src`
    and `crates/nullrouter-engine/src` and fails on any `catalogue::` path. (Module visibility
    can't do this: the CLI and the server use the same crate.) T055 is the runtime check.
  - Make T052 and T055 pass.
- [X] T062 [US2] `adapters` CLI in `crates/nullrouter-cli/src/cmd/adapters.rs`: `install`,
  `show`, `review [--retry]`, `build --retry`, `approve [--note]`, `reject [--note]` and
  `review-settings --model --budget [--reserve-output] | --clear`.
  - Exit codes per [contracts/operator-cli.md](contracts/operator-cli.md#commands). A gate
    refusal exits 3.
  - `approve` is allowed only from `reported`. It writes `decision.json`, sets `active`,
    supersedes the previous version, and reloads.
- [X] T063 [US2] `catalogue` CLI in `crates/nullrouter-cli/src/cmd/catalogue.rs`: `list`,
  `show <H>`, and `install <H> [<semver>]` (the newest version by default). Unreachable
  catalogue exits 5.
- [X] T064 [US2] Operator socket ops in `crates/nullrouter-server/src/operator.rs`:
  `adapters.state`, `adapters.review` (enqueue) and `alerts.list`.
  - Approve, reject, clear and remove are file writes followed by `reload`.
  - Make T054 pass.

**Checkpoint**: a local or catalogue install reaches `reported`. After approval it serves the
next request, and nothing serves before.

---

## Phase 6: User Story 3 — An adapter can't harm the operator or the agent (Priority: P1)

**Goal**: Hostile adapters are contained, tool edits are stopped, suspect adapters are held
until cleared, and every request completes.

**Independent Test**: [quickstart § 5](quickstart.md#5-hostile-adapters-and-the-guardrail-us3-sc-002-sc-003).

### Tests for User Story 3 ⚠️

- [X] T065 [P] [US3] Hostile corpus in `crates/nullrouter-adapters/tests/hostile/`.
  - Each case is a Rust source against the kit (where the attack is expressible), plus a
    checked-in `.wasm`, built by T045's `tools/build-adapter-fixtures.sh` (*operator-run*) or
    hand-written in WAT where the kit can't express the attack.
  - Cases:
    - `net`: imports `wasi_snapshot_preview1.sock_open`;
    - `file`: imports `path_open`;
    - `env`: imports `environ_get`;
    - `secret_probe`: scans its whole input for `sk-`, `Bearer` and key sentinels, and
      echoes any hit through `log!` and an edit;
    - `loop`;
    - `memory_bomb`;
    - `trap`;
    - `bad_output`: invalid JSON;
    - `out_of_selector`: edits a path outside its selectors;
    - `add_tool_call_request`;
    - `change_tool_args`;
    - `add_tool_call_response`;
    - `add_tool_call_event`;
    - `add_tool_def`;
    - `rewrite_tool_result`;
    - `add_unplaced_field`;
    - `legit_removal`.
  - Add a README listing what each case tries.
- [ ] T066 [US3] Hostile suite in `crates/nullrouter-server/tests/hostile.rs`.
  - Install each T065 case with the testkit as `approved`, bind a key, and send streamed
    and non-streamed requests through a mock upstream.
  - Assert, per case:
    - the request completes with the unmodified body or response;
    - the mock upstream saw no sentinel secret, and no outbound socket was opened by the
      process beyond the mock (count connections at the mock and at a second listener
      decoy);
    - the record outcome is `not_run{…}`, `failed{…}` or `blocked`, as the case expects;
    - an alert of the expected kind is raised;
    - the version state is `suspect` for guardrail cases only;
    - `legit_removal` goes through with its changes recorded.

  This covers SC-002, SC-003 and US3-1 to US3-8.
- [ ] T067 [P] [US3] Suspect lifecycle test in `crates/nullrouter-server/tests/suspect.rs`.
  - After a guardrail event, requests from **every** key bound to the harness are served as
    plain clients, with `not_run{suspect}`.
  - `adapters clear` returns the version to `approved`, and the next request runs it
    (US3-6).
  - A mid-stream violation: events before it went out edited, and events after it go out
    unedited. No event is recalled.
- [ ] T068 [P] [US3] Cancellation test in `crates/nullrouter-server/tests/adapter_cancel.rs`.
  - A client disconnect during a slow (`loop`) adapter call stops the upstream request
    within 1 s.
  - The sandbox call is dropped and no instance leaks: the pool count returns to 0.

### Implementation for User Story 3

- [ ] T069 [US3] Suspect handling in `crates/nullrouter-adapters/src/runner.rs`.
  - At reload, the runner for a harness whose active version is `suspect` is
    `not_run{suspect}`, and keys bound to it are served as plain clients.
  - The `blocked` path drops the runner for the rest of that request's stream.
  - Make T067 pass.
- [ ] T070 [US3] `adapters clear <H> <version>` in `crates/nullrouter-cli/src/cmd/adapters.rs`.
  It prints the version's guardrail events from alerts and records, asks for confirmation
  (`--yes` to skip), sets the state `suspect` → `approved`, and reloads.
- [ ] T071 [US3] Make T066 and T068 pass. Fix any gap in T040, T041, T046 or T048 that the
  corpus exposes, and record each fix in the commit message.

**Checkpoint**: SC-002 and SC-003 hold across the whole hostile corpus.

---

## Phase 7: User Story 4 — Updating never breaks a working setup (Priority: P2)

**Goal**: v1 serves until v2 is approved. Nothing updates by itself. Kit upgrades rebuild
the reviewed source.

**Independent Test**: [quickstart § 7, 8](quickstart.md#7-updates-never-break-a-working-setup-us4-fr-023-sc-006).

### Tests for User Story 4 ⚠️

- [ ] T072 [P] [US4] Lifecycle under load in
  `crates/nullrouter-server/tests/adapter_lifecycle.rs`.
  - 20 concurrent streaming clients on a key bound to `fixture-v1`, which is approved.
  - Move v2 through `queued`, `in_review`, `quarantined`, `rejected`, and then a second v2
    submission to `approved`.
  - Assert 0 failed requests.
  - Every request before the approval is served by v1, every request whose attempt starts
    after it by v2, and in-flight streams finish on v1 (SC-006, US4-1 to US4-3).
- [ ] T073 [P] [US4] Kit upgrade test in `crates/nullrouter-adapters/tests/kit_upgrade.rs`.
  - Store an approved version whose `build.json` has `kit_abi` = current − 2.
  - At startup it never loads, since the sandbox refuses the ABI. It is flagged
    `rebuilding`, and records show `not_run{rebuilding}`.
  - The builder, mocked through a stub binary on `PATH`, returns the same `source_fp` and a
    new hash. The version serves with no state change, and no review is enqueued.
  - A stub returning `compile` gives `rebuild_failed`, an alert, and plain-client keys
    (SC-013, FR-032).
- [ ] T074 [P] [US4] No-auto-update test in
  `crates/nullrouter-adapters/tests/catalogue.rs` (extend).
  - `catalogue check` against a mock index listing a newer version prints it.
  - The store is unchanged afterwards, byte for byte (US4-4).
- [ ] T075 [P] [US4] Removal test in `crates/nullrouter-server/tests/adapter_remove.rs`.
  - `adapters remove <H>` lists the bound keys and asks for confirmation.
  - Afterwards the keys are plain clients, with `not_run{removed}`.
  - Removing the active version without `--force` is refused.

### Implementation for User Story 4

- [ ] T076 [US4] Active-version swap in `crates/nullrouter-adapters/src/store.rs` and
  `runner.rs`.
  - `approve` sets `active`, and the previous version becomes `superseded`, in one atomic
    index write.
  - The runner is captured per request at its start from the `ArcSwap` snapshot, so in-flight
    requests keep their version.
  - Make T072 pass.
- [ ] T077 [US4] Kit-upgrade rebuild in `crates/nullrouter-adapters/src/lib.rs`
  (`startup_rebuilds()`), called from `serve`.
  - Find approved versions whose `kit_abi` is not supported, and set `rebuilding`.
  - Rebuild in the background through `builder_client`, and require an equal `source_fp`.
  - On success, replace `module.wasm` and `build.json`, clear the flag, and reload.
  - On failure, set `rebuild_failed` and raise an alert.
  - Make T073 pass.
- [ ] T078 [US4] CLI additions in `crates/nullrouter-cli/src/cmd/adapters.rs` and
  `catalogue.rs`.
  - `adapters rebuild [<H>]`: the same rebuild, run in the foreground before an upgrade.
  - `adapters remove <H> [<version>] [--force] [--yes]`.
  - `catalogue check`, with output per
    [contracts/catalogue.md § check output](contracts/catalogue.md#check-output).
  - Make T074 and T075 pass.

**Checkpoint**: updates, rejections, removals and kit upgrades never fail a request.

---

## Phase 8: User Story 5 — Claude Code proves the third-party pipeline (Priority: P2)

**Goal**: The Claude Code adapter is written against the kit, listed in the catalogue, and
installed, reviewed and approved like any other. Claude Code then works on every chosen text
provider.

**Independent Test**: [quickstart § 10](quickstart.md#10-claude-code-through-the-full-pipeline-us5-fr-027-fr-028-sc-008).

### Tests for User Story 5 ⚠️

- [ ] T079 [US5] Oracle fixtures for Claude Code: extend `tools/gen-bundled/generate.mjs`.
  - Import `normalizeClaudePassthrough` from
    `ref/9router/open-sse/translator/formats/claude.js`, and `dedupeTools` from where
    `chatCore.js:219` gets it.
  - Run them over input bodies in `tools/gen-bundled/seeds/claude-code/*.json`, and write
    `tests/fixtures/9router/adapters/claude-code/<case>.{in,out}.json`.
  - The seed cases:
    - adaptive thinking on haiku;
    - `output_config.effort` on haiku;
    - a bare content-block object;
    - a mid-conversation system message;
    - a thinking block with a foreign signature;
    - a `redacted_thinking` block with a foreign signature;
    - `server_tool_use` with a foreign id, plus its `web_search_tool_result`;
    - `server_tool_use` with a `srvtoolu_` id;
    - a tool_use without thinking while thinking is enabled (the placeholder case);
    - empty text and empty messages;
    - duplicate built-in and MCP tools.
  - Regenerate, and commit the fixtures alone, naming the `ref/9router` SHA.
- [ ] T080 [P] [US5] Parity test in `crates/nullrouter-adapters/tests/claude_code_parity.rs`.
  - Run the adapter as production does: build `adapters/community/claude-code` with the
    builder through T045's `tools/build-adapter-fixtures.sh` (*operator-run*), and check in
    the resulting `.wasm` and its `build.json` as
    `crates/nullrouter-adapters/tests/fixtures/claude-code/`. The test loads them with the T047
    testkit and calls them through the sandbox. No native linking into any workspace crate.
  - A companion check fails if the checked-in module's `source_fp` no longer matches
    `adapters/community/claude-code/`, so a source change forces a rebuild of the fixture.
  - For each fixture with an Anthropic context, apply the adapter's edits. The result must
    equal 9router's output, except the placeholder case, where the tool_use turn is left as
    is and a `tests/parity/deviations.toml` entry asserts it.
  - With a non-Anthropic context, `server_tool_use` and `web_search_tool_result` are removed
    with `target_cannot_carry_block`.
  - No edit adds a tool call or a tool definition: the guardrail passes on every fixture.
- [ ] T081 [P] [US5] Claude Code harness runner:
  `crates/nullrouter-server/tests/harness_claude_code.rs` (`#[ignore]`, `NR_LIVE=1`), with
  sessions using tools and web search against each chosen text provider. It reuses slice
  003's `tests/harness/` Claude Code runner, pointed at a key bound to `claude-code`.

### Implementation for User Story 5

- [ ] T082 [US5] Adapter package `adapters/community/claude-code/`.
  - `Cargo.toml` per the contract, with only the kit as a dependency.
  - `adapter.toml`: `harness = "claude-code"`, `style = "anthropic-messages"`, `kit = "1"`,
    request selectors `thinking`, `output_config`, `model`, `messages[*]`, `tools`, and no
    response selectors.
  - `src/lib.rs` implements [R15](research.md#r15-claude-code-adapter) steps 1–8:
    - it uses `Context.provider == "anthropic" && same_style` for the Anthropic branch;
    - `srvtoolu_` is the native id prefix;
    - a Claude signature is valid per 9router's check in `claude.js`: port the predicate;
    - no placeholder insertion;
    - `#[cfg(test)]` unit tests that use only the kit, run with `cargo test` inside
      `adapters/community/claude-code/`. The kit resolves through T008's
      `adapters/community/.cargo/config.toml`.
  - It must pass the T056 gate unchanged.
  - Make T080 pass.
- [ ] T083 [US5] Catalogue entry.
  - Write a `tools/package-adapter.sh` that builds a deterministic `.tar.gz`: sorted entries,
    zeroed mtimes and uid/gid, `gzip -n`.
  - Add `catalogue/index.toml` `[[entry]] claude-code` with version `0.1.0`, the release asset
    URL, `sha256`, `source_fp` and `kit = "1"`.
  - Add a test in `crates/nullrouter-adapters/tests/catalogue.rs` that parses the real
    `catalogue/index.toml`, and checks that packaging `adapters/community/claude-code` gives
    the listed `sha256` and `source_fp`.
  - The packaging script is plain `tar` and `gzip`, so it may run in the session.
  - Publishing the release asset is outward-facing: ask the user before running
    `gh release create`.
- [ ] T084 [US5] *operator-run* full pipeline, per
  [quickstart § 10](quickstart.md#10-claude-code-through-the-full-pipeline-us5-fr-027-fr-028-sc-008).
  - Ask the user to run the install, review with their review model, approve, key, and
    `NR_LIVE=1` harness commands with `! …`.
  - Record the result, the review report's risk level and the per-provider pass/fail in
    `tests/harness/README.md` (SC-008).

**Checkpoint**: Claude Code works through the full third-party pipeline with no special
path.

---

## Phase 9: User Story 6 — The operator sees what each adapter did (Priority: P2)

**Goal**: The records, the adapter list and the alerts show which adapter and version ran,
each change by path, kind and reason, and each guardrail event, and never any content.

**Independent Test**: after running the hermes, Claude Code and hostile tests, the records,
`adapters list` and `alerts list` match what happened.

### Tests for User Story 6 ⚠️

- [ ] T085 [P] [US6] Output tests in `crates/nullrouter-cli/tests/adapter_output.rs`.
  - `records show` text for three cases, matching the contract's layout exactly: an adapter
    that ran with changes, one that ran with none (it still names the adapter and version,
    US6-2), and a blocked one.
  - `records show --json` fields.
  - `adapters list` states and alert counts.
  - `alerts list` and `alerts ack <id>|--all`.
- [ ] T086 [P] [US6] No-content sentinel in
  `crates/nullrouter-server/tests/records_no_content.rs`.
  - Run hermes, the `legit_removal` fixture and the Claude Code fixture over bodies that hold
    unique sentinels in every removed or converted value.
  - Serialise the whole record store, `alerts.toml` and the log capture. Zero sentinels may
    appear.
  - Every change the adapters made appears, with path, kind and reason (SC-009).

### Implementation for User Story 6

- [ ] T087 [US6] `records show` rendering in `crates/nullrouter-cli/src/cmd/records.rs`.
  - Per attempt, `adapter <harness> <version|built-in>: <outcome>`, then the change lines,
    then the guardrail lines.
  - A `response adapter:` line, per
    [contracts/operator-cli.md](contracts/operator-cli.md#records-show-additions-text).
- [ ] T088 [US6] `alerts` CLI in `crates/nullrouter-cli/src/cmd/alerts.rs` (new): `list
  [--all]` and `ack <id>|--all`.
- [ ] T089 [US6] `adapters list` in full, in `crates/nullrouter-cli/src/cmd/adapters.rs`:
  harness, versions and states, the active version, `rebuilding` or `rebuild_failed` flags,
  and the unacknowledged alert count. hermes shows `built-in`.
- [ ] T090 [US6] Make T085 and T086 pass.

**Checkpoint**: every adapter action is visible, and no content is stored.

---

## Phase 10: Polish & Cross-Cutting Concerns

- [ ] T091 [P] Benchmarks, per [R14](research.md#r14-performance-and-benchmarks).
  - `crates/nullrouter-adapters/benches/adapters.rs`: `guard_request` at 10 KB, 100 KB and
    1 MB (anthropic-messages, one removal), `guard_event`, `selector_extract` on 1 MB, and
    `hermes_request` with images.
  - `crates/nullrouter-sandbox/benches/sandbox.rs`: `call_noop`, and `call_claude_code` with
    a 100 KB body.
  - Include a no-harness baseline on slice 003's `engine` bench, to show zero cost.
- [ ] T092 *operator-run*: benches stay local, and only the user runs them. Ask the user to run
  `! cargo bench -- --save-baseline slice-004`.
  - First, slice 003's `engine` bench with `--baseline slice-003`. The no-harness path must
    stay within Criterion's noise threshold. A regression blocks merge (FR-030, constitution
    Performance gate).
  - Write `specs/004-client-side-adapters/bench-baseline.md` in slice 003's format, with the
    p95 figures against SC-010: ≤ 5 ms per request with adapter plus guardrail, ≤ 1 ms per
    event.
  - If either target is missed, stop and report the numbers before optimising. The
    `optimize-perf` workflow is the route.
- [ ] T093 [P] Parity audit: `/rust-parity-audit` on
  `crates/nullrouter-adapters/src/builtin/hermes.rs` and
  `adapters/community/claude-code/src/lib.rs`, judged on chatCore's request path. Fix any
  High findings, and record Low ones as accepted.
- [ ] T094 [P] Security review: run the `security-auditor` agent over:
  - `nullrouter-sandbox`;
  - `gate.rs`, `catalogue.rs`, `guard.rs` and `store.rs`;
  - `nullrouter-builder`;
  - the kit's `abi.rs`.

  Focus on sandbox escape, gate bypass, path traversal, hash-check TOCTOU and secret flow.
  Fix Critical and High findings before the PR.
- [ ] T095 [P] Docs.
  - `docs/adapters/authoring.md`: the kit API, `adapter.toml`, selectors, reason codes, the
    gate rules, and building locally with the builder.
  - `docs/adapters/operating.md`: install, review settings, approve, clear, alerts, the
    catalogue, and kit upgrades with `adapters rebuild`.
  - Link both from `docs/`.
- [ ] T096 [P] Update `CLAUDE.md`.
  - Add rows to the layout table for the four crates, `adapters/community/` and
    `catalogue/`.
  - Update the "Plugin safety invariant" paragraph to point to the kit and builder paths.
- [ ] T097 [P] *operator-run*: ask the user to run every non-live section of
  `specs/004-client-side-adapters/quickstart.md`. Record any gap they report in the commit
  message.
- [ ] T098 Final gate.
  - Push with the user's OK. CI's `cargo test --workspace` and
    `cargo clippy --workspace --all-targets -- -D warnings` must be green.
  - Confirm that `unsafe` appears only in `crates/nullrouter-adapter-kit/src/abi.rs`:
    `grep -rn "unsafe" crates/ --include='*.rs'` must list only that file.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Setup (Phase 1)**: T001 blocks everything. Slice 003's attempt loop and codecs must
  exist.
- **Foundational (Phase 2)**: depends on Setup, and blocks every story.
- **US1 (Phase 3)**: depends on Phase 2 only. It needs no sandbox, builder or review.
- **Third-party foundation (Phase 4)**: depends on Phase 2, and blocks US2–US5. It can run in
  parallel with US1.
- **US2 (Phase 5)**: depends on Phase 4.
- **US3 (Phase 6)**: depends on Phase 4. Its tests use the testkit, not the US2 install flow,
  so it can run in parallel with US2. T070 (clear) edits the same CLI file as T062, so do
  those two in sequence.
- **US4 (Phase 7)**: depends on Phase 4, and on T062 (approve) and T061 (catalogue) from US2.
- **US5 (Phase 8)**: depends on US2 (the gate, install, review and catalogue) and on T048
  (the guardrail).
- **US6 (Phase 9)**: depends on Phase 2 for the records. Its tests use hermes (US1) and the
  hostile fixtures (T065).
- **Polish (Phase 10)**: after the stories that are in the release.

### Story Graph

```text
Setup ─► Foundational ─┬─► US1 (hermes, MVP) ───────────────┐
                       └─► Third-party foundation ─┬─► US2 ─┼─► US4
                                                   │        ├─► US5
                                                   └─► US3 ─┘
                                        US1 + T065 ─► US6 ─► Polish
```

### Within Each Story

- Tests first. Confirm they fail, then implement.
- Types come before the logic, the logic before the CLI, and the CLI before end-to-end tests.
- Commit per task or per logical group. The generated oracle fixtures (T079) go in their own
  commit, naming the `ref/9router` SHA.

---

## Parallel Examples

```text
# Phase 1: crate skeletons together
T004 kit   T005 sandbox   T006 adapters   T007 builder   T008 dirs

# Phase 2 tests together, then kit types together
T010 selector   T011 apply   T012 keys   T013 records   T014 seam
T015 edit.rs    T016 context.rs

# Phase 4 tests together
T033 sandbox   T034 guard   T035 store   T036 tamper   T037 builder

# US1 and Phase 4 in parallel (different crates and files)
US1: T026–T032        Phase 4: T038–T048

# US2 tests together
T049 gate   T050 scramble   T051 review   T052 catalogue   T053 install   T054 no-adapters   T055 quiet

# US2 and US3 in parallel once Phase 4 is done
US2: T056–T064        US3: T065–T069
```

---

## Implementation Strategy

### MVP (User Story 1)

1. Phase 1, including T001 (slice 003 ready) and T002 (toolchain; wasm32 is not needed for
   the MVP).
2. Phase 2.
3. Phase 3. **Stop and validate**: hermes on mocks for all six chosen text providers, then the
   T031 live check on the operator's accounts.

### Incremental Delivery

1. US1: hermes works. This meets the slice's first fail condition for built-in harnesses.
2. Phase 4, US2 and US3 together: third-party adapters install, review, serve and are
   contained. **Do not ship US2 without US3**, since the hostile suite is what proves the
   sandbox.
3. US4: updates and kit upgrades.
4. US5: Claude Code, the proof on a real closed-source harness.
5. US6: operator visibility.
6. Polish: the bench baseline, parity and security audits, docs.

---

## Notes

- Constraints quoted from the data model are binding. Do not relax them during
  implementation:
  - "`^[a-z][a-z0-9-]{1,31}$`" for harness names;
  - "`v` + semver + `-` + first 8 hex of `source_fp`" for version ids;
  - "≤ 1,024 edits", "`replace` value ≤ 4 MiB";
  - "1–32" request selectors, "at most 8 segments";
  - "64 MiB" memory, "20 ms" per request call and "2 ms" per event;
  - "256 KiB total, 64 files, 64 KiB per `.rs`";
  - "base64 or hex run over 256 characters";
  - "`summary` ≤ 2,000 chars, ≤ 50 findings";
  - "index ≤ 1 MiB, archive ≤ 2 MiB";
  - "`al_` + 10 chars" for alert ids.
- Adapters remove or convert. They never insert (Constitution IV). No change to the edit model
  may add an insert operation.
- Records, alerts and logs hold paths and codes, never removed or converted values.
- The core never calls `Module::deserialize`, and never depends on `nullrouter-builder`.
- No catalogue request is made without an operator command. Nothing in `serve`, reload or
  startup touches `catalogue.rs`.
- Never edit `ref/9router/` or `tests/fixtures/9router/` by hand. Regenerate them.
- `unsafe` is allowed only in `crates/nullrouter-adapter-kit/src/abi.rs`.
- No dashboard page changes in this slice (spec Assumptions). Records and keys gain optional
  fields with `serde(default)`, so the slice 009 pages keep reading them.
