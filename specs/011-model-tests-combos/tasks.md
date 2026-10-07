---

description: "Task list for slice 011, model tests and combos"
---

# Tasks: Model Tests and Combos

**Input**: Design documents from `specs/011-model-tests-combos/`

**Prerequisites**: plan.md, spec.md, research.md (R1–R16), data-model.md, contracts/cli.md,
contracts/config-and-plugins.md, contracts/operator-socket.md, quickstart.md. Scope brief:
`specs/briefs/2026-10-07-model-tests-combos.md`.

**Tests**: requested by the spec (SC-001 to SC-010 and each story's Independent Test), so each
story has test tasks. Write them first, in the same commit as or before the code they cover.

**Build**: no local cargo at all (no test, clippy, check or build). Commit, group commits into
one push with the user's OK, and read CI results with the github MCP (read-only). Check
`git branch --show-current` is `011-model-tests-combos` before each commit.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an unfinished task)
- **[Story]**: US1 test a pair, US2 BROKEN skips routing, US3 retests, US4 operator control and
  persistence, US5 combos route, US6 combo tests

---

## Phase 1: Setup

- [X] T001 Create `crates/nullrouter-engine/src/verdict/mod.rs` (doc comment pointing to `specs/011-model-tests-combos/data-model.md` § Verdict), `verdict/judge.rs`, `verdict/store.rs`, and `crates/nullrouter-engine/src/tests/mod.rs` (doc comment: test runs, research R1, R2, R6, R14) as empty modules; declare `pub mod verdict;` and `pub mod tests;` in `crates/nullrouter-engine/src/lib.rs`
- [X] T002 [P] Create `crates/nullrouter-registry/src/combos.rs` and `crates/nullrouter-registry/src/schema/rejection.rs` as empty modules with doc comments (research R12, R4); declare them in `crates/nullrouter-registry/src/lib.rs` and `crates/nullrouter-registry/src/schema/mod.rs`

---

## Phase 2: Foundational (blocking)

**⚠️ CRITICAL**: every story needs the settings, the verdict types, the board and its file.

### Settings and schema

- [X] T003 Add `TestSettings` (`[tests]`) to `crates/nullrouter-registry/src/schema/config.rs` with serde defaults exactly as data-model.md § Test settings: `retest` default `["1m","5m","30m","6h"]` ("1–10 steps, each ≥ 30 s, non-decreasing; the last repeats"), `broken_retest` default `"off"` (`"off"`, `"on"` (every 24 h), or an interval ≥ 1 h), `concurrency` default 4 (1–32), `timeout.text/.embedding/.tts/.stt` default `"30s"`, `timeout.image/.video` default `"5m"` (each 5 s–30 min). Parse durations with `schema/duration.rs`. Add `tests: TestSettings` to `OperatorConfig` and to `RuntimeSettings` in `crates/nullrouter-registry/src/registry.rs`
- [X] T004 Validate `[tests]` at load in `crates/nullrouter-registry/src/load.rs`, reporting `config.toml:L:C tests.<path>: <rule>` with the rule texts of contracts/config-and-plugins.md (`at least 30s`, `steps must not get shorter`, `1 to 10 steps`, `"off" or at least 1h`, `1 to 32`, `5s to 30m`, `unknown type "x"`); tests in `crates/nullrouter-registry/tests/smoke.rs` for each rule and for the defaults when `[tests]` is absent
- [X] T005 [P] Add `RejectionRule { status: Vec<u16> (one or a list, reuse signin's `de_statuses`), body_contains: Option<String>, reason: RejectionReason }` with `RejectionReason = model_not_found | model_not_available | type_not_supported` and a `matches(status, body)` like `RefusedRule::matches` in `crates/nullrouter-registry/src/schema/rejection.rs`; add `rejections: Vec<RejectionRule>` (`[[rejections]]`, default empty) to the plugin schema and `ProviderEntity` in `crates/nullrouter-registry/src/schema/plugin.rs`
- [X] T006 Gate `[[rejections]]` in `crates/nullrouter-registry/src/validate/gate.rs`: refuse a status of 408 or 429 (`408 and 429 are never a rejection`) or outside 400–499 (`only 400–499 can be a rejection`), and an unknown reason (`unknown reason "x"`), at path `rejections[i].status` / `.reason`; gate fixtures and tests in `crates/nullrouter-registry/tests/gate/` and `crates/nullrouter-registry/tests/gate.rs`
- [X] T007 [P] Record a SHA-256 digest of each provider's plugin source bytes at load (bundled plugins hash their embedded text) and expose `Registry::plugin_digest(id) -> Option<&str>` (`sha256:<hex>`) in `crates/nullrouter-registry/src/registry.rs` and `crates/nullrouter-registry/src/load.rs`; test in `crates/nullrouter-registry/tests/plugins.rs` that a one-byte change changes the digest and a reload of identical files keeps it

### Verdict core

- [X] T008 Define in `crates/nullrouter-engine/src/verdict/mod.rs`: `Pair { provider, account, model }` (account `-` for no-auth providers; model = `Candidate::upstream_id`), `State { Pass, Broken, Unknown }`, `Rejection { ModelNotFound, ModelNotAvailable, TypeNotSupported, Plugin(String) }`, `Source { Test, Retest, ComboTest, Operator }`, `Basis { secret: Option<String>, signed_in_at: Option<String>, plugin: String }`, and `Verdict { state, reason (redacted, ≤ 500 chars), rejection, source, at, record, step, next, basis, note }` exactly as data-model.md § Verdict, all serde to the `verdicts.jsonl` line shape shown there
- [X] T009 Implement `verdict::Board` in `crates/nullrouter-engine/src/verdict/mod.rs`: an `ArcSwap<HashMap<Pair, Verdict>>` with `get`, `is_broken(provider, account, model)`, `set`, `clear`, `list(filter)`; hold it on `Engine` in `crates/nullrouter-engine/src/state.rs` (not in the swapped `EngineState`, so a reload keeps it)
- [X] T010 Add `Target::Verdicts` (`routing/verdicts.jsonl`, mode 0600) to `crates/nullrouter-engine/src/journal/writer.rs`, with the same sync cadence and disk-full health as `routing/warm.jsonl`
- [X] T011 Implement `crates/nullrouter-engine/src/verdict/store.rs`: render a set line and a `{"cleared":true,"why":…,"at":…}` line; replay the file at open (last line per pair wins, malformed lines skipped and counted); compact through the writer when the file holds more than 4× the live pairs, as `journal/state.rs` does; load it in `Engine::open` in `crates/nullrouter-engine/src/state.rs`
- [X] T012 [P] Store tests in `crates/nullrouter-engine/tests/verdict_store.rs`: set, clear, replay after restart, a killed writer mid-append loses at most the last line, compaction keeps every live pair (SC-005 first half)

**Checkpoint**: settings load, the board persists, nothing uses it yet.

---

## Phase 3: User Story 1 — The operator proves which models work (Priority: P1) 🎯 MVP

**Goal**: `nullrouter test` on a pair, a model, a unified model or `--all` makes real minimal calls and prints PASS / BROKEN / UNKNOWN with the reason.

**Independent Test**: mock providers answering success, model-not-found, not-available, unsupported type, a plugin-declared rejection, 429, 500 and a hang, for one pair of each type; each verdict matches (spec US1).

### Tests for User Story 1

- [ ] T013 [P] [US1] Table test of `verdict::judge` in `crates/nullrouter-engine/tests/verdict_judge.rs`: for each row of research R3 (core list per status and phrase; a bare 404 with no model wording → UNKNOWN; 408, 429, 5xx, no status, stall, break → UNKNOWN; a 401 and a `[[signin.refused]]` match → account, not verdict; a plugin rule wins over the core list) and for a malformed success per type (SC-001)
- [ ] T014 [P] [US1] End-to-end single-pair tests in `crates/nullrouter-engine/tests/model_test.rs` using the testkit mock upstream: one pair of each type (text, embedding, tts, stt, image, video) answering success, each core rejection, a plugin rule, 429, 503 and a hang past a shortened timeout; assert the verdict, that exactly one account was called (no fallback), that the record is marked `test` with no prompt or output, and that the quota tally counted the call (US1 scenarios 1–8, SC-008)

### Implementation for User Story 1

- [ ] T015 [US1] Implement `verdict::judge(failure_or_answer, provider) -> Judged { state, rejection, reason }` in `crates/nullrouter-engine/src/verdict/judge.rs` per research R3: not-definitive first (no status, 408, 429, 5xx, auth as `attempt::token_rejected` / `[[signin.refused]]` decide it), then the plugin's `rejections`, then the core list (status 400/403/404/405/422 and a lower-cased message containing `model` plus a phrase from R3's table; `model_not_available` from 403 or with its phrases), else UNKNOWN with status and message; reasons through the redactor and cut to 500 chars. Make `token_rejected` `pub(crate)` in `crates/nullrouter-engine/src/attempt.rs`
- [ ] T016 [US1] Add `pin: Option<Pin { provider, account }>` and `test: Option<TestTag { run, source }>` to `TextRequest` in `crates/nullrouter-engine/src/attempt.rs`; in `crates/nullrouter-engine/src/plan.rs` keep only the pinned account's steps when `pin` is set; in `crates/nullrouter-engine/src/route.rs` skip `learn` for test requests and let `source = operator` tests bypass priority 0 and the reserve floor, but not an account resting after a rate limit: that pair is skipped with `rate-limited until <time>` (research R1, spec Assumptions)
- [ ] T017 [US1] Add `test: Option<TestMark { run, source }>` to `RequestRecord` (test records carry no agent and are tagged `test`, FR-021) in `crates/nullrouter-engine/src/records.rs`; keep no generated output for test records (research R9); extend the record JSON in `crates/nullrouter-engine/src/journal/records.rs`
- [ ] T018 [US1] Build the minimal request per type in `crates/nullrouter-engine/src/tests/bodies.rs` per research R2: text `"hi"` with `max_tokens` 1024, no stream; embedding input `"test"`; tts text `"test"` with the model's first declared voice; stt a 250 ms 16 kHz mono silent WAV built in memory; image prompt `"a red dot"`, smallest declared size, one image; video prompt `"a red dot"`, shortest duration and smallest size; untyped models as text. Each with its PASS check from the R2 table (stt: a `text` field, empty allowed)
- [ ] T019 [US1] Implement `tests::run_pair(engine, pair, source, run) -> TestResult` in `crates/nullrouter-engine/src/tests/mod.rs`: build the request (T018), run `Engine::reply` under `tokio::time::timeout(settings.tests.timeout[type])` cancelling the request's token on expiry (UNKNOWN `timeout after <d>`), follow a video job to `completed` and its first content bytes, judge (T015), store the verdict with its basis (current secret digest, `signed_in_at`, plugin digest) on the board, and return `{ pair, state, reason, rejection?, ms, ttft_ms?, record, skipped? }`. An account that is disabled or can't serve gives `skipped` with the reason and no call (spec edge cases); an auth failure marks the account and leaves the verdict (FR-009). When two tests of one pair overlap, both run and are recorded and the one that finishes last is kept (spec edge case)
- [ ] T020 [US1] Implement pair expansion in `crates/nullrouter-engine/src/tests/mod.rs`: `provider/model [--account]` → that model on one or every account of its provider, including a model a passthrough provider's plugin doesn't list (spec edge case); a unified model → each member on each account that serves it; `all` → every pair any loaded unified model reaches, de-duplicated (FR-004, clarify Q1); plus the per-type call counts for `test.plan`; and a semaphore of `settings.tests.concurrency` shared with retests (research R10)
- [ ] T021 [US1] Add the socket ops `test.plan` and streamed `test.run` (one `{"event":"result",…}` line per pair, then the `done` summary; a closed connection cancels calls not yet sent) to `crates/nullrouter-server/src/operator.rs` per contracts/operator-socket.md, with a streaming variant of `operator::call` for the CLI
- [ ] T022 [US1] Add `nullrouter test <target> [--account NAME] [--all] [--yes] [--json]` in `crates/nullrouter-cli/src/cmd/test.rs` (registered in `crates/nullrouter-cli/src/cmd/mod.rs` and `main.rs`): call `test.plan`, print `N billed calls: …. Continue? [y/N]` when N > 1 without `--yes`, then print each result line as it arrives and the summary, exactly as contracts/cli.md § `nullrouter test`; no server → `no running server; start it with nullrouter serve`, exit 1, no call
- [ ] T023 [US1] CLI tests in `crates/nullrouter-cli/tests/model_test.rs` against a test server with mock upstreams: output lines, confirmation prompt and `--yes`, `--json`, no-server message, Ctrl-C (closed socket) leaves finished results saved, and a single-pair test against a hanging mock returns within its timeout + 5 s (SC-004)

**Checkpoint**: US1 complete; verdicts exist but don't steer routing yet.

---

## Phase 4: User Story 2 — Routing stops only at a definitive rejection (Priority: P1)

**Goal**: BROKEN pairs are skipped for every target without an attempt; nothing else changes.

**Independent Test**: one pair BROKEN by a test; direct, unified and combo requests never reach it and are served by the other account; the record names the skip (spec US2).

### Tests for User Story 2

- [ ] T024 [P] [US2] Routing tests in `crates/nullrouter-engine/tests/verdict_routing.rs`: US2 scenarios 1–5 (BROKEN on A, untested on B → B serves and the record has a `skipped` attempt `BROKEN since …`; N on A still served; UNKNOWN routes; every pair BROKEN → informational error listing verdicts with zero upstream calls; a client request meeting a rejection doesn't change any verdict) (SC-002)
- [ ] T025 [P] [US2] Model-list test in `crates/nullrouter-server/tests/models.rs`: a target whose every pair is BROKEN is still listed in all four styles (clarify Q2)

### Implementation for User Story 2

- [ ] T026 [US2] Add `ErrorClass::Broken` in `crates/nullrouter-engine/src/records.rs`; pass the board's snapshot into `plan::plan` in `crates/nullrouter-engine/src/plan.rs` and turn a step whose pair is BROKEN into `Step::Skip` with class `Broken` and reason `BROKEN since <time>: <reason>`; update every `plan` caller (`crates/nullrouter-engine/src/attempt.rs`, `crates/nullrouter-engine/src/route.rs`, server views)
- [ ] T027 [US2] In `walk` (`crates/nullrouter-engine/src/attempt.rs`), when every step is a `Broken` skip, fail before any upstream call with slice 003's informational error whose tried list carries each pair's verdict and reason (FR-011)
- [ ] T028 [P] [US2] Extend `crates/nullrouter-engine/benches/engine.rs` with a plan over a unified model of 4 members × 4 accounts with 0 and 1000 verdicts on the board (research R16)

**Checkpoint**: US1 + US2 make tests pay off in routing.

---

## Phase 5: User Story 3 — UNKNOWN settles by itself, on the operator's terms (Priority: P1)

**Goal**: UNKNOWN retests on the schedule; BROKEN only if turned on; retests wait while the account can't serve or take cold work.

**Independent Test**: simulated clock, a mock answering 503 three times then 200 → retests at ~1, 5, 30 min, then PASS, then none (spec US3).

### Tests for User Story 3

- [ ] T029 [P] [US3] Simulated-clock tests in `crates/nullrouter-engine/tests/verdict_retest.rs`: the 1/5/30 min/6 h schedule within 10 % (SC-003); settling stops retests; a new operator UNKNOWN restarts at step 0; BROKEN never retested with `broken_retest = "off"`, retested every interval when on, operator-set BROKEN never; a retest waits while the account is disabled, needs sign-in, is refreshing, is rate-limited or has a window at its reserve floor, and runs once it can serve (clarify Q4); a changed schedule applies to the next retest without restart; after a restart overdue retests start `n × 10 s` apart; two overlapping tests of one pair keep the later result; and a token refresh due while retests fill the test limit still runs on time (research R8, analyze C1)

### Implementation for User Story 3

- [ ] T030 [US3] Implement the retest task in `crates/nullrouter-engine/src/tests/retest.rs` per research R8, spawned by `serve` in `crates/nullrouter-server/src/serve.rs` beside the maintenance task with a child cancellation token, and never taking a `maintenance::MAX_JOBS` slot: a timer over the board's `next` times; `due` from `next` (UNKNOWN: `at + schedule[step]`, then the last step repeating; BROKEN from a test: every `broken_retest` when on; never for `Operator`); held (not due) while the account can't serve or couldn't take cold work, using the account state and the routing view's floor data; a due pair runs `tests::run_pair` with `Source::Retest` under the shared test semaphore. Leave room for combo results (T049)
- [ ] T031 [US3] Compute and store `step` and `next` on every verdict change in `crates/nullrouter-engine/src/verdict/mod.rs`; spread overdue retests at start (`n × 10 s`); expose `waiting: <reason>` for `verdicts.list` without storing it (data-model.md: "`waiting` is computed for display, never stored")

**Checkpoint**: verdicts settle without the operator.

---

## Phase 6: User Story 5 — Clients ask for a combo by name (Priority: P1)

**Goal**: `[[combo]]` in `config.toml`, checked at load, listed to clients, routed as an ordered fallback chain.

**Independent Test**: combo `coder` = [`sonnet`, `fallback-chain`], `fallback-chain` = [`gpt`, `glm`]; with every `sonnet` and `gpt` account failing transiently, `glm` answers and the record shows each member tried (spec US5).

### Tests for User Story 5

- [ ] T032 [P] [US5] Registry tests in `crates/nullrouter-registry/tests/combos.rs`: each load error of contracts/config-and-plugins.md with its exact `config.toml:L:C path: rule` (clash with a unified model, clash with a combo, empty, unknown member, cycle printed `a → b → a`, kind conflict); a combo dropped when its unified model was dropped by a skipped plugin; a plugin with a `combo` table refused with `unknown field "combo"`; `resolve` returns `Resolution::Combo`; flattening is depth-first and de-duplicated
- [ ] T033 [P] [US5] Walk tests in `crates/nullrouter-engine/tests/combo_walk.rs`: US5 scenarios 1–6 (first member answers; moves on only after the member's own retries and fallbacks; never after output; a `fallback = false` request error ends the combo; an all-BROKEN member skipped without an attempt; a repeated unified model not retried); records carry `combo` and each attempt's `member` path (SC-006)
- [ ] T034 [P] [US5] Extend `crates/nullrouter-server/tests/models.rs`: each combo listed next to unified models in all four styles with its members' type (US5 scenario 7)

### Implementation for User Story 5

- [ ] T035 [US5] Add `ComboDecl { name, members: Vec<String> }` (`[[combo]]`) to `OperatorConfig` in `crates/nullrouter-registry/src/schema/config.rs`
- [ ] T036 [US5] Implement combo loading in `crates/nullrouter-registry/src/combos.rs` per research R12 and data-model.md § Combo (`name`: "non-empty, no `/`, unique across unified models and combos"; `members`: "non-empty; each a unified model or combo; no cycle; kinds agree"): load after unified models, report errors at `combo[i]…`, drop combos that need a dropped unified model (reported as `dropped combo X: needs unified model Y (dropped)`), derive `kind`, compute `flat` (depth-first, each unified model once); store `combos` and a name index in `crates/nullrouter-registry/src/registry.rs`
- [ ] T037 [US5] Add `Resolution::Combo(&Combo)` in `crates/nullrouter-registry/src/resolve.rs` and a combo case in `crates/nullrouter-registry/benches/resolve.rs`
- [ ] T038 [US5] Walk combos in `crates/nullrouter-engine/src/attempt.rs` per research R13: for `Resolution::Combo`, iterate `flat`; give each unified model its own `plan` and `route::decide` when its turn comes; move on only when its walk failed with `fallback = true` and `after_output == false`; return a `fallback = false` failure at once; skip a member whose plan is all `Broken` without an attempt; the final informational error lists every member tried. Add `RequestRecord.combo` and `Attempt.member` (`a › b › unified`) in `crates/nullrouter-engine/src/records.rs`
- [ ] T039 [US5] List combos in `crates/nullrouter-server/src/models.rs` (`listed`) next to unified models with the combo's kind; client requests naming a combo reach the engine unchanged in `crates/nullrouter-server/src/text.rs` and `crates/nullrouter-server/src/media.rs`
- [ ] T040 [US5] Add `nullrouter combos [NAME] [--json]` in `crates/nullrouter-cli/src/cmd/combos.rs` and a combos view in `crates/nullrouter-server/src/views/combos.rs` (CLI reads go through views, spec 008); `resolve NAME` prints the same tree for a combo; `check` lists combo errors and dropped combos (`crates/nullrouter-cli/src/cmd/check.rs`, `crates/nullrouter-server/src/views/check.rs`); output exactly as contracts/cli.md § `nullrouter combos`, including `no combos; declare one with [[combo]] in config.toml`
- [ ] T041 [P] [US5] Extend `crates/nullrouter-engine/benches/engine.rs` with a 3-level combo plan (research R16)

**Checkpoint**: all P1 stories done.

---

## Phase 7: User Story 4 — The operator keeps the last word on verdicts (Priority: P2)

**Goal**: list, clear and mark verdicts; settings from the CLI; reset on secret, sign-in or plugin change.

**Independent Test**: mark a pair BROKEN, restart: still BROKEN, shown as operator-set; replace the key: untested; update the plugin: that provider's pairs untested (spec US4).

### Tests for User Story 4

- [ ] T042 [P] [US4] Basis tests in `crates/nullrouter-engine/tests/verdict_store.rs`: a changed `--env` secret, a new sign-in (`signed_in_at` changes) and a changed plugin digest reset only the affected pairs with a `cleared` line; a token refresh doesn't; a removed account or provider drops its pairs (SC-005 second half, US4 scenarios 5–7)
- [ ] T043 [P] [US4] CLI tests in `crates/nullrouter-cli/tests/verdicts.rs` and goldens in `crates/nullrouter-cli/tests/read_golden.rs`: `verdicts` list and filters, `clear` (and the error on an untested pair), `mark --note` shown as `set by the operator: <note>`, `settings` show and each setter with `applied` / `saved; applies at next start`, invalid values print the rule and exit 1 (US4 scenarios 1–4)

### Implementation for User Story 4

- [ ] T044 [US4] Compute each account's current basis in `crates/nullrouter-engine/src/verdict/mod.rs` per research R7 (secret: SHA-256 of `install-id ‖ 0x00 ‖ key` as `sha256:<hex>`, `None` for sign-in and no-auth; `signed_in_at` from `tokens.rs`; plugin from `Registry::plugin_digest`), and drop mismatching verdicts with a `cleared` line (`account changed` / `plugin changed`) at open, after every reload (`Engine::reload_blocking`) and after a token swap in `crates/nullrouter-engine/src/state.rs`
- [ ] T045 [US4] Add the socket ops `verdicts.list` (with `waiting`) and `verdicts.set` (`broken` with optional note, or `clear`; clearing an untested pair → `{"ok":false,"error":"no verdict for …"}`) in `crates/nullrouter-server/src/operator.rs` per contracts/operator-socket.md; a verdicts view in `crates/nullrouter-server/src/views/verdicts.rs`
- [ ] T046 [US4] Add `nullrouter verdicts [--provider] [--account] [--model] [--state]`, `verdicts clear`, `verdicts mark [--note]` and `verdicts settings …` in `crates/nullrouter-cli/src/cmd/verdicts.rs`, exactly as contracts/cli.md § `nullrouter verdicts`; with no server, `clear`/`mark` append to `routing/verdicts.jsonl` and print `saved; applies at next start`; settings write `config.toml` atomically then ask for a reload, as `routing window` does (`crates/nullrouter-cli/src/cmd/routing.rs`)
- [ ] T047 [US4] Show each member's verdict per account in `unified NAME` (`crates/nullrouter-server/src/views/unified.rs`, `crates/nullrouter-cli/src/cmd/unified.rs`) and add `records list --test/--no-test` and the `test`, `combo`, `member` fields to `records get` (`crates/nullrouter-cli/src/cmd/records.rs`, `crates/nullrouter-server/src/views/records.rs`, `records.list` filter in `crates/nullrouter-engine/src/journal/records.rs`); update the affected goldens in the same commit

**Checkpoint**: the operator can override and trust verdicts.

---

## Phase 8: User Story 6 — The operator tests a combo as a client would (Priority: P2)

**Goal**: `nullrouter test <combo>` makes one call through the combo and shows the combo verdict, the member that answered, and each member tried, nested.

**Independent Test**: a 3-level combo whose first two leaves are BROKEN and UNKNOWN and whose third answers → PASS, answered by the third, nested output (spec US6).

### Tests for User Story 6

- [ ] T048 [P] [US6] Server test in `crates/nullrouter-server/tests/combo_test.rs`: US6 scenarios 1–4 (first member answers, no other call; UNKNOWN when any failure isn't definitive; BROKEN when every member is BROKEN or definitively rejected; an attempt's PASS or rejection updates its pair, a 503 doesn't and starts no retest, clarify Q3), the nested `ComboResult` for 3 levels (SC-007), and scenario 5 on a simulated clock: an UNKNOWN combo result is kept, survives a restart, is retested on the schedule until PASS, and is cleared when the combo's definition changes (FR-030)

### Implementation for User Story 6

- [ ] T049 [US6] Implement `tests::run_combo` in `crates/nullrouter-engine/src/tests/combo.rs` per research R14: one tagged request (no pin) of the combo's kind through the combo; judge each attempt from the record; set PASS or BROKEN pairs with `Source::ComboTest`, leave others; derive the combo verdict and build `ComboResult { combo, state, answered_by?, tried: [ { member, kind, state, reason, attempts, tried? } ] }` from the attempts' `member` paths. Keep the result on the board under `combo:<name>` and journal it in `routing/verdicts.jsonl` as data-model.md § Combo result (with the `definition` digest; a changed definition clears it at load). Add UNKNOWN combo results to the retest task (T030): due on the `retest` schedule, held while the combo's first unified model has no account that can serve, run with source `retest` (FR-030, analyze D1)
- [ ] T050 [US6] Route `test.run` / `test.plan` with a combo target to `run_combo` (1 call of the combo's kind) in `crates/nullrouter-server/src/operator.rs`, and print the nested combo output of contracts/cli.md in `crates/nullrouter-cli/src/cmd/test.rs`, with UNKNOWN attempts marked `(not saved)`; list combo results under the pair table in `nullrouter verdicts` and in `verdicts.list` (contracts/cli.md, contracts/operator-socket.md); goldens in `crates/nullrouter-cli/tests/read_golden.rs`

**Checkpoint**: every story done.

---

## Phase 9: Polish & Cross-Cutting

- [ ] T051 [P] Extend the secrets sentinel in `crates/nullrouter-server/tests/secrets.rs`: no secret, test prompt or generated output in test records, `test.run`/`verdicts.list` answers or `routing/verdicts.jsonl` (SC-008)
- [ ] T052 [P] Docs: `docs/operator-config.md` gains "Model tests", "Verdicts", "Combos" and the `[tests]` reference, and `routing/verdicts.jsonl` in the file list and modes; `docs/plugins.md` gains `[[rejections]]`; `check` warns `verdicts not being kept` when the journal can't write (`crates/nullrouter-server/src/views/check.rs`)
- [ ] T053 [P] Opt-in live check in `crates/nullrouter-engine/tests/live.rs` (`NR_LIVE=1`, `.nr-live/` home): one real model of each type the operator holds an account for; every result PASS or a recognisable UNKNOWN (SC-010). Bills real calls; the user runs it
- [ ] T054 Bench baseline: the user runs `cargo bench -p nullrouter-engine --bench engine` and `-p nullrouter-registry --bench resolve` locally before and after; record in `specs/011-model-tests-combos/bench-baseline.md` (SC-009, target within 5 %). CI doesn't compare benches, so the branch is not merged until this file holds both runs (constitution, Performance gate)
- [ ] T055 Security review with the security-auditor agent over the verdict basis, `[[rejections]]` gate and test records (ask the user first)
- [ ] T056 Walk `specs/011-model-tests-combos/quickstart.md` § Manual scenario (user)

---

## Dependencies & Execution Order

- **Setup (T001–T002)** → **Foundational (T003–T012)** → stories.
- **US1 (T013–T023)** first: the judge, pins, bodies and runner are used by US3, US4 and US6.
- **US2 (T024–T028)** needs only the board (Phase 2); can start in parallel with US1's
  T015–T023 once T009 is done, but T024's "a test made it BROKEN" path needs T019.
- **US3 (T029–T031)** needs US1 (`run_pair`).
- **US5 (T032–T041)** needs only Phase 2 for the registry parts (T032, T035–T037); T038 needs
  US2's `Broken` skips (T026) for "an all-BROKEN member is skipped".
- **US4 (T042–T047)** needs US1 (verdicts exist) and US3 (`waiting`).
- **US6 (T048–T050)** needs US1, US2, US3 (the retest task) and US5.
- **Polish** last.

Shared files, edit in small localized changes and re-read before editing: `attempt.rs` (T015,
T016, T027, T038), `plan.rs` (T016, T026), `records.rs` (T017, T026, T038), `operator.rs`
(T021, T045, T050), `engine.rs` bench (T028, T041), `models.rs` test (T025, T034).

### Parallel opportunities

- Phase 2: T005 and T007 alongside T003–T004; T012 once T011 lands.
- US1: T013 and T014 together (test files), then T015 and T018 in parallel.
- US2 and US5's registry half (T032, T035–T037) can run beside US1.
- Polish: T051–T053 in parallel.

## Implementation Strategy

**MVP**: Phases 1–4 (US1 + US2): the operator tests models, and BROKEN pairs leave routing. Push
for CI there. Then US3 (retests) and US5 (combos), the other P1 stories; then US4 and US6.

One push per phase group, with the user's OK; each push cancels the previous CI run, so group
commits. If the slice proves too large, the clean split is US1–US4 (model tests) and US5–US6
(combos), as plan.md notes.
