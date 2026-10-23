# Tasks: Functional Dashboard (014)

**Input**: `specs/014-functional-dashboard/` (spec.md, plan.md, research.md, data-model.md,
contracts/, quickstart.md). **Tests**: requested by the success criteria (SC-002 to SC-008), so each
story has its test tasks.

**Rule for every "move" task**: behaviour-preserving. The command keeps its syntax, output, exit
code and prompts. The logic moves into `crates/nullrouter-server/src/ops/<area>.rs` and the CLI file
keeps clap parsing and printing. The existing tests in `crates/nullrouter-cli/tests/<area>.rs` must
pass unchanged; never edit a golden file to make a move pass.

## Phase 1: Setup

- [X] T001 Create `crates/nullrouter-server/src/ops/mod.rs` and declare `pub mod ops;` in `crates/nullrouter-server/src/lib.rs`. Define `Ctx { home: OperatorHome, engine: Option<Arc<Engine>> }`, `Done { status: Status, text: String, once: Option<Once> }`, `Status::{Applied, SavedNotApplied { reason: String }, Started { task: TaskId }}`, `Once { label: String, value: SecretString }`, and `OpError { text: String, exit: u8, field: Option<&'static str> }` with exit codes 1 invalid, 3 refused by gate, 4 no server, 5 sign-in ended (data-model.md § Operation).
- [X] T002 Move `apply()` from `crates/nullrouter-cli/src/lib.rs:17-32` to `ops::apply(&Ctx)`. With `ctx.engine` set, it reloads in-process (the same call the operator socket's `reload` op makes). Without it, it sends `{"op":"reload"}` over the socket as today. Keep its texts ("saved; applies at next start", the refusal case). The CLI's `apply` calls it.
- [X] T003 Move the config text editor `edit` (`crates/nullrouter-cli/src/cmd/connection.rs:251`) and its helpers (`quoted`, the temp-file-plus-rename write, the "the edit would not load" check) to `crates/nullrouter-server/src/ops/config_text.rs`, with no behaviour change. CLI callers import it from there.

## Phase 2: Foundational (blocks every story)

- [X] T004 [P] Add a Tasks table in `crates/nullrouter-engine/src/tasks.rs` and expose it on `Engine`. Fields per data-model.md § Task: id `tk_` + 16 random base32 characters; kind ∈ {test, signin, adapter_review, adapter_build, adapter_rebuild, catalogue_install, proxy_fixed, quota_poll}; subject; started/ended; progress lines; result running|done{text}|failed{text}; input for sign-in. Keep the newest 200 tasks, with at most 4 running. A fifth start returns `busy: 4 tasks are running; try again when one ends`. Unit tests cover the bound, the limit and the transitions.
- [X] T005 [P] Add the form layer in `crates/nullrouter-dashboard/src/forms.rs`:
  - The form token is `hex(HMAC-SHA256(process_key, session_cookie_value))[..32]`, with HMAC built on `sha2` (RFC 2104, 64-byte block) and `process_key` 32 bytes from `getrandom`.
  - `check_post(headers, form)` verifies, in order: the session cookie, then Origin equals the dashboard origin (or, if Origin is absent, Referer does), then `_t` in constant time. Failures map as contracts/dashboard-forms.md § Every form says.
  - A `_back` sanitizer accepts only a known dashboard page path, otherwise `/endpoint`.
  - Unit tests include the RFC 4231 HMAC test vector 2.
- [X] T006 [P] Add a flash store in `crates/nullrouter-dashboard/src/forms.rs` (data-model.md § Flash): a 128-bit random base32 id bound to its session; kinds done/once/refused (refused carries kept_fields, never a write-only field); expires 5 minutes after creation; consumed on first read; at most 1,000 entries, oldest dropped. Tests: another session can't read a flash; the second read is empty.
- [X] T007 [P] Add the change log in `crates/nullrouter-dashboard/src/log.rs`: `<home>/dashboard-log.jsonl`, mode 0600, lines `{"at","op","subject","result","text"}` with result ∈ {done, saved_not_applied, started, refused}. Rewrite the file to the newest 1,000 lines once it reaches 1,200. Add a reader that returns entries newest first. Tests: the bound, 0600, order.
- [X] T008 Add form components in `crates/nullrouter-dashboard/src/components.rs`:
  - `form(action, back, token, body)`, which adds the hidden `_t` and `_back` fields;
  - `field`, `select`, `switch`, and `secret_field` (`type="password" autocomplete="off"`, never given a value);
  - `notice(flash)`, and `once_box(label, value)` with the text "This is the only time it is shown.";
  - `confirm_window(title, loss_text, form)`, with a Cancel link that closes the window;
  - `task_panel(task)`, which adds `<meta http-equiv="refresh" content="3">` while the task runs.
  Styles go in `crates/nullrouter-dashboard/style/dashboard.css`, using only `var(--token)` values. New tokens go in `style/tokens.toml` with ref/9router source lines (style-guide tests).
- [X] T009 Add the `/do/<area>/<op>` dispatch in `crates/nullrouter-dashboard/src/changes.rs` and route it in `crates/nullrouter-dashboard/src/lib.rs` (POST only; GET gives 405).
  - Each request runs, in order: `forms::check_post`, the parse into a typed op, the `ops` call under the dashboard guard, the change-log line, then `303` with the flash per contracts/dashboard-forms.md § Every form.
  - Started tasks redirect with `?task=<id>`.
  - Make the page reader in `crates/nullrouter-dashboard/src/lib.rs` consume `done`/`task` query parameters and pass them to frame rendering.
  - Rename the session cookie to `nrd_session_<port>` (R10) in `crates/nullrouter-dashboard/src/access.rs`, keeping sign-in working.
- [X] T010 Add `ops::describe_loss(op, subject) -> String` in `crates/nullrouter-server/src/ops/mod.rs` for every irreversible op in spec FR-009 (account remove, key revoke, plugin uninstall, proxy remove, adapter remove, unified/combo remove, adapter clear, private endpoints on). Example: "clients using key work-laptop stop working at once". Unit-test that each op has a text.
- [X] T011 Test the foundation in `crates/nullrouter-dashboard/tests/forms.rs` (SC-005), using a no-op test route:
  - a post without `_t`, with a wrong `_t`, or with a foreign Origin gives 403 and writes no log line;
  - a GET to `/do/...` gives 405;
  - a good post gives 303 with `done`;
  - reloading the 303 target repeats nothing.

## Phase 3: User Story 1 — Accounts and client keys (P1) 🎯 MVP

**Goal**: add, sign in, order, enable, make exclusive and remove accounts; issue, revoke, tag and bind keys, all from the browser. **Independent test**: spec US1.

- [X] T012 [US1] Move the `accounts add|priority|disable|enable|exclusive|remove` logic from `crates/nullrouter-cli/src/cmd/accounts.rs` to `crates/nullrouter-server/src/ops/accounts.rs`. Secrets arrive as `SecretString` (stdin or `--env` resolution stays in the CLI). `crates/nullrouter-cli/tests/accounts_keys.rs` passes unchanged.
- [X] T013 [US1] Move the `keys issue|revoke|tag|set-break|set-adapter` logic from `crates/nullrouter-cli/src/cmd/keys.rs` to `crates/nullrouter-server/src/ops/keys.rs`. `issue` returns the new key in `Done.once`. `crates/nullrouter-cli/tests/accounts_keys.rs` passes unchanged.
- [X] T014 [US1] Move sign-in to `crates/nullrouter-server/src/ops/accounts.rs::signin`, which runs as a task (T004). The engine flow writes the token entry and the account exactly as `crates/nullrouter-cli/src/signin.rs` does today, and the CLI follows the task and prints as today.
  - **Loopback redirect**: the listener's success response becomes a `303` to `<dashboard>/providers?window=provider&id=<p>&task=<id>` when the sign-in was started from the dashboard; otherwise it keeps the CLI page.
  - **Code page**: the task waits on a paste channel fed by `accounts.signin_paste`.
  - **Device flow**: the task's `input` carries code and link.
  - The terms warning is required via `accept_terms`, just as `--accept-terms-risk` works.
  - `crates/nullrouter-cli/tests/signin.rs` passes unchanged.
- [X] T015 [US1] Build the Providers forms in `crates/nullrouter-dashboard/src/pages/providers.rs`: the provider window gets "Add account" (name, secret_field, order) and "Sign in" (for OAuth providers; it opens a confirm window with the terms warning when the provider declares one). Account rows get priority, enable/disable and exclusive controls, plus Remove through `confirm_window` with `describe_loss`. The sign-in task panel shows the paste field or the device code.
- [X] T016 [US1] Build the Endpoint & Key forms in `crates/nullrouter-dashboard/src/pages/endpoint.rs`: Issue key (name, harness tag, break, adapter), with the result in `once_box`. Key cards get tag, break and adapter controls, plus Revoke through the confirm window. Remove the disabled hints these replace.
- [X] T017 [P] [US1] Add form handlers for every US1 op id in contracts/dashboard-forms.md to `crates/nullrouter-dashboard/src/changes.rs` (accounts.add, signin, signin_paste, priority, enable, disable, exclusive, remove; keys.issue, revoke, tag, set_break, set_adapter). Write-only fields go straight into `SecretString`.
- [X] T018 [US1] Test US1 in `crates/nullrouter-dashboard/tests/accounts_keys_forms.rs`:
  - each US1 op through the browser matches the CLI on a twin home (same files, same `keys list --json` / `accounts list --json`);
  - a duplicate account shows the CLI's refusal next to the name field with the other fields kept;
  - a new key appears once and never after reload;
  - a pasted marker secret is found in no response body and no file but the secret store (SC-004 for US1).

## Phase 4: User Story 2 — Routing settings (P1)

**Goal**: behaviour, overrides, window, connection, proxies, quota interval, poll and acks from the browser. **Independent test**: spec US2.

- [X] T019 [P] [US2] Move `behaviour set-break` (`crates/nullrouter-cli/src/cmd/behaviour.rs`) and `routing set|unset|set-plugin|unset-plugin|window` (`crates/nullrouter-cli/src/cmd/routing.rs`) to `crates/nullrouter-server/src/ops/routing.rs`. `crates/nullrouter-cli/tests/routing.rs` passes unchanged.
- [X] T020 [P] [US2] Move `connection set|unset` (`crates/nullrouter-cli/src/cmd/connection.rs`) to `crates/nullrouter-server/src/ops/connection.rs`. Its tests pass unchanged.
- [X] T021 [P] [US2] Move `proxy add|remove|use|clear|fixed` (`crates/nullrouter-cli/src/cmd/proxy.rs`) to `crates/nullrouter-server/src/ops/proxy.rs`. `fixed` runs as a task. The password is a `SecretString`. `crates/nullrouter-cli/tests/proxy.rs` passes unchanged.
- [X] T022 [P] [US2] Move `quota interval|poll|ack` (`crates/nullrouter-cli/src/cmd/quota.rs`) to `crates/nullrouter-server/src/ops/quota.rs`. `poll` runs as a task in-process. `prune` and `forget` stay CLI-only (spec FR-001). `crates/nullrouter-cli/tests/quota.rs` passes unchanged.
- [X] T023 [US2] Add the US2 forms to pages:
  - Settings (`pages/settings.rs`): default break behaviour, amortization window, test settings (`verdicts settings`);
  - provider and account windows (`pages/providers.rs`): routing overrides, plugin meters, connection settings;
  - Proxy Pools (`pages/proxy_pools.rs`): add with a write-only password, use, clear, remove (confirm), fixed (task);
  - Quota Tracker (`pages/quota.rs`): poll interval, poll now (task), alert ack.
  Each refusal shows next to its field.
- [X] T024 [P] [US2] Add handlers for every US2 op id in `crates/nullrouter-dashboard/src/changes.rs`.
- [X] T025 [US2] Test US2 in `crates/nullrouter-dashboard/tests/settings_forms.rs`:
  - browser equals CLI on twin homes for each US2 op;
  - an out-of-range override shows the CLI's text;
  - a server that refuses the reload yields "saved but not applied" with the reason;
  - a proxy password marker never appears in responses.

## Phase 5: User Story 3 — Unified models, combos and tests (P2)

**Goal**: declare targets and run tests from the browser; new CLI twins. **Independent test**: spec US3.

- [X] T026 [US3] Add `crates/nullrouter-server/src/ops/targets.rs` with `unified add|set|remove` and `combos add|set|remove`. They edit `[[unified_model]]` / `[[combo]]` in `config.toml` through `ops::config_text`. Members are checked as the loader checks them. Removing a target another combo uses is refused with `<name>: combo <c> uses it; remove it from the combo first`. Output texts follow contracts/operator-cli.md.
- [X] T027 [US3] Add the CLI subcommands `unified add|set|remove` (`crates/nullrouter-cli/src/cmd/unified.rs`) and `combos add|set|remove` (`crates/nullrouter-cli/src/cmd/combos.rs`), calling T026, with the bare `unified [name]` / `combos [name]` reads unchanged. Tests go in `crates/nullrouter-cli/tests/unified.rs` and `combos.rs`.
- [X] T028 [P] [US3] Move `test` (`crates/nullrouter-cli/src/cmd/test.rs`) and `verdicts mark|clear|settings` (`crates/nullrouter-cli/src/cmd/verdicts.rs`) to `crates/nullrouter-server/src/ops/model_tests.rs`. `test` plans, then runs as a task whose progress lines are the CLI's streamed lines. The CLI's `[y/N]` question text comes from `ops`. `crates/nullrouter-cli/tests/model_test.rs` and `verdicts.rs` pass unchanged.
- [X] T029 [US3] Build the Combo page forms in `crates/nullrouter-dashboard/src/pages/combo.rs`: declare/edit/remove unified models and combos (members one per line; remove via confirm), and Test buttons per target plus Test All. When the plan bills more than one call, a confirm window asks the CLI's question. The task panel shows progress and the verdict. Add a Test button and verdict mark/clear to the provider window's model rows in `pages/providers.rs`.
- [X] T030 [P] [US3] Add handlers for unified.*, combos.*, test.run, verdicts.* in `crates/nullrouter-dashboard/src/changes.rs`.
- [X] T031 [US3] Test US3 in `crates/nullrouter-dashboard/tests/targets_forms.rs`:
  - browser equals CLI for the target ops;
  - an unknown member shows the refusal;
  - a two-call test asks to confirm first;
  - a started test shows "started", and after it ends the verdict matches `verdicts --json` (mock upstream).

## Phase 6: User Story 4 — Plugins, custom providers and adapters (P2)

**Goal**: plugin and adapter lifecycle from the browser; the visibility, custom-provider and private-endpoint twins. **Independent test**: spec US4.

- [X] T032 [US4] Move `plugins install|uninstall` (`crates/nullrouter-cli/src/cmd/plugins.rs`) to `crates/nullrouter-server/src/ops/plugins.rs`. Add `private_endpoints(on)` there, editing `allow_private_endpoints` in `config.toml` with the confirm text "Plugins may then reach this machine and your local network." Add the CLI `plugins private-endpoints on|off [--yes]`. `crates/nullrouter-cli/tests/plugins.rs` passes unchanged, and new cases are added.
- [X] T033 [US4] Add provider visibility:
  - Registry and config: `[provider.<id>] enabled` (default true) and `hidden` (default false) in the operator config schema (`crates/nullrouter-registry`).
  - Routing: the engine's candidate filter skips disabled providers, as it skips disabled accounts (`crates/nullrouter-engine`).
  - Reads: model lists and the views' provider list leave hidden ones out, and the side panel still lists them.
  - Ops and CLI: `ops::plugins::{enable, disable, hide, show}` plus CLI `providers enable|disable|hide|show <id>` (`crates/nullrouter-cli/src/cmd/providers.rs`), with outputs per contracts/operator-cli.md.
  - Tests: routing skips a disabled provider; a hidden provider is absent from `/v1/models`.
- [X] T034 [US4] Add the custom compatible provider:
  - `crates/nullrouter-registry/src/compatible.rs` builds a schema-2 plugin TOML from (id, name, style ∈ {openai-chat, anthropic-messages}, base_url, models), following data-model.md § Custom compatible provider: id "lowercase letters, digits and dashes, up to 32 characters, not an existing provider", 1 to 200 model ids, one text section, API-key auth in the style's header, a static model list.
  - `ops::plugins::add_compatible` runs `check_user_plugin` with the operator's `allow_private`, then writes `plugins/<id>.toml` and applies. A private address while the setting is off gives the refusal text in contracts/operator-cli.md (exit 3).
  - Add the CLI `providers add-compatible`.
  - Tests cover openai and anthropic styles, a taken id, a bad URL and a private URL.
- [X] T035 [P] [US4] Move `adapters install|review|build|approve|reject|clear|rebuild|remove|review-settings` (`crates/nullrouter-cli/src/cmd/adapters.rs`), `catalogue install` (`cmd/catalogue.rs`) and `alerts ack` (`cmd/alerts.rs`) to `crates/nullrouter-server/src/ops/adapters.rs`. Review, build, rebuild and catalogue install run as tasks. The `[y/N]` texts for clear and remove come from `ops`. `crates/nullrouter-cli/tests/adapter_output.rs` and the adapters crate's lifecycle tests pass unchanged.
- [X] T036 [US4] Make the side panels and settings windows work:
  - Providers settings window (`pages/providers.rs`): Show on page / Enabled switches, Install, Uninstall (confirm), and the "Add OpenAI Compatible" / "Add Anthropic Compatible" windows (id, name, base URL, models one per line), showing the private-endpoint refusal with a link to Settings.
  - Settings (`pages/settings.rs`): the private-endpoints switch, with a confirm window when turning it on.
  - Endpoint & Key adapter settings window (`pages/endpoint.rs`): install from path, install from catalogue (task), review settings, per-version review/build/approve/reject/clear (confirm)/remove (confirm), and alert ack.
- [X] T037 [P] [US4] Add handlers for plugins.*, providers.*, adapters.*, catalogue.install and alerts.ack in `crates/nullrouter-dashboard/src/changes.rs`.
- [ ] T038 [US4] Test US4 in `crates/nullrouter-dashboard/tests/plugins_adapters_forms.rs`:
  - browser equals CLI for each US4 op that needs no network (catalogue install uses the existing catalogue mock);
  - a custom provider created in the browser serves a request through the mock upstream;
  - uninstall and adapter remove need confirmation.

## Phase 7: User Story 5 — Console Log (P3)

- [X] T039 [US5] Build the Console Log page in `crates/nullrouter-dashboard/src/pages/console_log.rs`: entries newest first from `log.rs` (local time with zone, op, subject, result, text). The empty state reads "No changes made from the dashboard yet." Drop the page's placeholder hint.
- [X] T040 [US5] Add the CLI `nullrouter dashboard log [--limit N] [--json]` (`crates/nullrouter-cli/src/cmd/dashboard.rs`), default limit 50, with lines `<local time>  <op>  <subject>  <result>[: text]`. It reads the same file through a reader shared in `nullrouter_server::views` (move the reader from T007 there if needed so the CLI doesn't depend on the dashboard).
- [X] T041 [US5] Test US5 in `crates/nullrouter-dashboard/tests/console_log.rs`: three changes, one refused, appear in order on the page and in `dashboard log --json`; no entry contains a pasted marker secret.

## Phase 8: Polish & cross-cutting

- [X] T042 Add the parity suite `crates/nullrouter-cli/tests/dashboard_parity.rs` (SC-002, SC-003). It drives every op id in contracts/dashboard-forms.md once through `/do/...` and once through its CLI twin on twin homes. It compares the homes' files, excluding timestamps, ids and `dashboard-log.jsonl`, and for each scripted refusal compares the texts. It prints one `op: browser == cli` line per op.
- [X] T043 Add a secret sweep in `crates/nullrouter-dashboard/tests/secret_sweep.rs` (SC-004): post marker secrets into every write-only field (account secret, proxy password, sign-in paste code), crawl every page and window, and read every home file except the secret stores. No marker may appear anywhere else.
- [ ] T044 Add a confirm sweep in `crates/nullrouter-dashboard/tests/confirm.rs` (SC-007). For every irreversible op, the button leads to the confirm window, Cancel leaves the home byte-identical, and Confirm applies the change.
- [X] T045 Extend `crates/nullrouter-dashboard/tests/style_guide.rs` (SC-008): no `<script`, and the CSP keeps no `script-src` and keeps `form-action 'self'`, on every page including windows, confirm windows and task panels.
- [ ] T046 Add a bench in `crates/nullrouter-server/benches/` or extend the 013 overhead bench (SC-006): router overhead p95 with a loop posting dashboard changes as fast as possible, against the 013 baseline. Record the result in `specs/014-functional-dashboard/bench-baseline.md`.
- [X] T047 [P] Update the docs:
  - `docs/operator-config.md`: the new commands, `[provider.<id>] enabled|hidden`, and private endpoints;
  - `docs/dashboard/` (or the 009 docs): making changes, confirm windows, Console Log, the form-token note.
- [ ] T048 Run a security review of the branch (security-auditor) over the forms layer, flash, change log, sign-in hand-back and custom-provider writes, and re-check finding L1 (the shared loopback cookie host, accepted 2026-10-06 for a read-only dashboard) now that the cookie opens the change forms; update the "What the cookie reaches" paragraph in `docs/operator-config.md` to match. Record it in `specs/014-functional-dashboard/security-review.md` and fix every High and Medium.
- [ ] T049 Run quickstart.md §§ 1–4 on a scratch home and record the results in `specs/014-functional-dashboard/operator-runs.md`. § 5 (real OAuth sign-in) joins the user's whole-system live run.

## Dependencies

- Setup (T001–T003), then Foundational (T004–T011), then the stories.
- US1 (T012–T018) is the MVP. US2 depends only on the foundation. US3 needs accounts (US1) to test end to end but not to build. US4 depends on the foundation. US5 depends on T007 and T009.
- Polish (T042–T049) comes after all stories.

## Parallel examples

- Foundation: T004, T005, T006 and T007 touch separate files.
- US2: T019, T020, T021 and T022 move different command files.
- Handlers (T017, T024, T030, T037) are separate sections of `changes.rs`. Run them in parallel only when split into per-area submodules (`changes/accounts.rs` …).

## Implementation strategy

1. MVP = Setup + Foundation + US1. A browser can add an account and issue a key.
2. Then US2. The daily operator work is complete.
3. Then US3 and US4 in either order, then US5.
4. Polish, the security review, and the operator runs, then merge. After that comes the user's live run, then the first release.
