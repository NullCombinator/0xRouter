---

description: "Task list for 007-dashboard"
---

# Tasks: Dashboard

**Input**: Design documents from `specs/007-dashboard/`

**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md),
[data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: Included. The spec's failure conditions are test outcomes: CLI agreement (SC-001),
access (SC-002), isolation (SC-003), secrets (SC-004), style traceability (SC-005), offline
rendering (SC-008). Within each story, write the tests first and confirm they fail before
implementing.

**Organization**: Tasks are grouped by user story, so each story can be implemented and tested
on its own.

**Phase order**: US2 (sign-in, P1) comes before US1 (accounts page, P1), because no page can be
reached without the token gate (FR-006). The style guide's extraction is foundational, because
every page's components are built from it. US6's phase holds the enforcement tests and the
side-by-side review. Task IDs are in execution order.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependency on an incomplete task)
- **[Story]**: The user story the task belongs to (US1–US6)

## Path Conventions

Cargo workspace at the repo root ([plan § Project Structure](plan.md#project-structure)):

- `crates/nullrouter-dashboard/`: the new crate (pages, access, guard, style, assets)
- `crates/nullrouter-server/src/views/`: the shared view layer (R1)
- `crates/nullrouter-cli/`: new commands, text printers over the views, `serve` wiring
- `crates/nullrouter-engine/`: `dashboard.toml`, the newest-first record read
- `crates/nullrouter-registry/src/schema/config.rs`: `[dashboard]`

Build with `export CARGO_HOME=$PWD/.cargo-home`. Locally, run tests per crate with `-j 2`. Full
workspace runs belong to CI or cloud sessions.

---

## Phase 1: Setup (Shared Infrastructure)

- [ ] T001 Add `maud`, `jiff` and `subtle` to `[workspace.dependencies]`, and `nullrouter-dashboard = { path = "crates/nullrouter-dashboard" }`, in `Cargo.toml`
- [ ] T002 Create `crates/nullrouter-dashboard/Cargo.toml` (deps: axum, tokio, tokio-util, maud, jiff, subtle, sha2, serde, serde_json, toml, tracing, nullrouter-server, nullrouter-engine, nullrouter-registry; dev-deps: reqwest, tempfile, criterion, nullrouter-engine with `testkit`) and an empty `crates/nullrouter-dashboard/src/lib.rs` with `#![forbid(unsafe_code)]` via workspace lints
- [ ] T003 [P] Add `nullrouter-dashboard` to the workspace table in `CLAUDE.md` and to `crates/nullrouter-cli/Cargo.toml` dependencies

---

## Phase 2: Foundational (Blocking Prerequisites)

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

### Shared view layer (R1; no behaviour change, and the CLI's existing tests are the gate)

- [ ] T004 Create `crates/nullrouter-server/src/views/mod.rs`, with a `Live` trait that has two implementations: `Socket(&OperatorHome)` (calls `operator::call`) and `InProcess(&Arc<Engine>)` (calls `operator::handle`). Each view takes `(&OperatorHome, &EngineState snapshot or registry, &dyn Live)` and returns `serde_json::Value`
- [ ] T005 [P] Move the `accounts list --long` JSON builder (`shown`, `cooling`, the row `json!`, the needs-sign-in hints) from `crates/nullrouter-cli/src/cmd/accounts.rs` to `crates/nullrouter-server/src/views/accounts.rs`. The CLI's `print_list` prints the view
- [ ] T006 [P] Move the `quota` current-windows JSON builder from `crates/nullrouter-cli/src/cmd/quota.rs` and `crates/nullrouter-cli/src/quota_text.rs` (data part only) to `crates/nullrouter-server/src/views/quota.rs`
- [ ] T007 [P] Move the `routing [target]` view builder from `crates/nullrouter-cli/src/cmd/routing.rs` and `crates/nullrouter-cli/src/routing_text.rs` (data part only) to `crates/nullrouter-server/src/views/routing.rs`
- [ ] T008 [P] Move the `records list` and `records show` JSON builders (filter parsing, the "in flight" / "cut short" naming at `records.rs` "What a request with no `close` is called") from `crates/nullrouter-cli/src/cmd/records.rs` to `crates/nullrouter-server/src/views/records.rs`
- [ ] T009 [P] Move the `providers`, `model` and `plugins list --community` JSON builders from `crates/nullrouter-cli/src/cmd/{providers,model,plugins}.rs` to `crates/nullrouter-server/src/views/{providers,model,plugins}.rs`
- [ ] T010 [P] Move the `keys list` JSON builder (`id`, `name`, `key: "…last4"`, `created`, `revoked`, `break`) from `crates/nullrouter-cli/src/cmd/keys.rs` to `crates/nullrouter-server/src/views/keys.rs`
- [ ] T011 [P] Move the `check` JSON builder from `crates/nullrouter-cli/src/cmd/check.rs` to `crates/nullrouter-server/src/views/check.rs`. Give each warning, note and error a `subject` field (`accounts`, `routing`, `models`, `keys`) so pages can pick theirs (FR-019b). The CLI's text output is unchanged
- [ ] T012 Run `cargo test -p nullrouter-cli -j 2` and `cargo test -p nullrouter-server -j 2`. Every existing expected output passes unchanged (contracts/cli.md "Moved code")

### Home files and config

- [ ] T013 [P] Add `[dashboard]` to `OperatorConfig` in `crates/nullrouter-registry/src/schema/config.rs`: `enabled` (bool, default `true`) and `listen` (default `"127.0.0.1:20130"`). The host must be loopback (`127.0.0.0/8`, `::1`, `localhost`), else the load error `dashboard.listen: must be a loopback address; network binding is not supported`. Add unit tests for the default, the loopback forms and the refusal
- [ ] T014 [P] Add `dashboard.toml` load and save to `crates/nullrouter-engine/src/files.rs` (`schema = 1`, `token_digest` = 64-char hex SHA-256 of the full `nrd_…` text, `issued` RFC 3339 UTC). Mode 0600, atomic write (temporary file + rename). Load it into the engine snapshot so a reload picks up a new digest. Unit tests cover the round trip, the mode, and the missing file (no token)
- [ ] T015 [P] Add the `dashboard.toml` file-mode check, with subject `keys`, to the check view in `crates/nullrouter-server/src/views/check.rs`

### Dashboard skeleton

- [ ] T016 Implement `spawn(engine, settings) -> DashboardHandle` in `crates/nullrouter-dashboard/src/lib.rs`: bind its own `TcpListener` and run its own axum router in its own task. A bind failure is logged and stored as `bound: Err(reason)`, not returned (R8)
- [ ] T017 Implement the build guard in `crates/nullrouter-dashboard/src/guard.rs`: a semaphore of 2 permits (a waiter gets 503 "busy, reload" after 5 s), a 10 s timeout, and the build run in a spawned task so a panic becomes that page's 500 (contracts/dashboard-http.md "Errors")
- [ ] T018 Implement the per-build page snapshot in `crates/nullrouter-dashboard/src/frame.rs`: one `engine.snapshot()`, `as_of`, and `Live::InProcess` (R11). Add the shared frame (navigation with the four pages and "Not built yet": Model tests, Combos; the title; the "as of" line), `lang="en"`, and the security headers from R7 on every response
- [ ] T019 [P] Implement `crates/nullrouter-dashboard/src/time.rs` with `jiff`: render an RFC 3339 instant as `<time datetime="…Z">YYYY-MM-DD HH:MM:SS</time>` in the machine's zone, and the zone label `CEST, Europe/Berlin` (R10). Unit tests use a fixed `TZ` and cover a DST change
- [ ] T020 Add the `dashboard.status` op to `crates/nullrouter-server/src/operator.rs`, answering `{"ok":true,"enabled","listen","bound","error"?,"token_issued"?}` from a status cell the dashboard handle fills in
- [ ] T021 Start the dashboard from `crates/nullrouter-cli/src/cmd/serve.rs` next to the client listener and the operator socket when `[dashboard] enabled`, and stop it on shutdown with the others

### Style guide (R4, R5, contracts/style-guide.md)

- [ ] T022 Extract 9router's light look into `crates/nullrouter-dashboard/style/tokens.toml`:
  - `globals.css` `:root` colors, radii and shadows;
  - Inter and the type scale;
  - the Tailwind v4 spacing and radius scale entries the components use;
  - the palette colors used by the Badge variants (`ref/9router/src/shared/components/Badge.js:5-12`, sizes `:14-18`), Button, Card, Input, Select, Sidebar, Pagination, and the usage tables (`ref/9router/src/app/(dashboard)/dashboard/usage/components/UsageTable.js`).

  Every token has `value` and `source` (`file:line`, plus `class` when it came from a Tailwind class). Nothing comes from the `.dark` block. Add `[component.*]` entries listing the tokens each component uses
- [ ] T023 Write `docs/dashboard/style-guide.md`: palette, type, spacing, radii, shadows, components (button, card, badge, table, input, select, navigation, sidebar, empty state, page header), the status → badge table from contracts/style-guide.md, and "What is not taken from 9router" (layout, dark theme, JS-only effects)
- [ ] T024 Write the generator test `crates/nullrouter-dashboard/tests/style_guide.rs::tokens_css_is_generated`, which regenerates `style/tokens.css` from `tokens.toml` and fails on any difference, then generate `crates/nullrouter-dashboard/style/tokens.css`
- [ ] T025 [P] Embed Inter (Latin subset, variable woff2) as `crates/nullrouter-dashboard/assets/inter-latin.woff2`, and about 15 Material Symbols Outlined SVG paths in `crates/nullrouter-dashboard/assets/icons.rs`, with the OFL 1.1 and Apache 2.0 texts in `crates/nullrouter-dashboard/assets/LICENSES/`. Serve them from `/assets/<content-hash>/…` with `Cache-Control: public, max-age=86400, immutable`
- [ ] T026 Write `crates/nullrouter-dashboard/style/dashboard.css` using only `var(--token)`, keywords, `0`, and layout percentages. Write `crates/nullrouter-dashboard/src/components.rs` (maud): card, badge (variants default, success, warning, error, info), table, empty state (names a CLI command), filter form, pager, page header

**Checkpoint**: views are shared, the dashboard binds and renders an empty frame, and the style is in place. User stories can start.

---

## Phase 3: User Story 2 — Sign in to the dashboard once, and only on this machine (Priority: P1)

**Goal**: The CLI issues a token, the browser enters it once, and nothing opens without it.
Nothing is reachable from another machine or another site.

**Independent Test**: quickstart steps 1–2. `cargo test -p nullrouter-dashboard --test access`.

### Tests for User Story 2 ⚠️

- [ ] T027 [P] [US2] Write `crates/nullrouter-dashboard/tests/access.rs` covering:
  - no token issued: every route 303s to `/signin`, which names `nullrouter dashboard token`;
  - right token: `Set-Cookie: nr_dashboard=…; Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000`, then pages open;
  - wrong token: 401 after the delay, and two wrong tries take ≥ 1 s + 2 s;
  - a replaced token: the old cookie 303s to `/signin`;
  - a bad `next` (`//evil`, `http://…`) becomes `/`;
  - a foreign `Origin` on `POST /signin` gets 403;
  - a foreign `Host` gets 421;
  - every response carries the R7 headers, and there is no `<script` in any body;
  - `no_write_routes`: every route except `POST /signin` refuses `POST`/`PUT`/`DELETE`/`PATCH`
- [ ] T028 [P] [US2] Write `crates/nullrouter-cli/tests/dashboard.rs` covering:
  - `dashboard token` prints `nrd_` + 43 chars once and writes `dashboard.toml` mode 0600 holding only the digest;
  - `--json` has `token`, `issued`, `url`, `status`;
  - the token string is absent from the home directory afterwards;
  - `dashboard status` with no server, bound, bind failed (port taken), and `enabled = false`, worded as in contracts/cli.md
- [ ] T029 [P] [US2] Write `crates/nullrouter-dashboard/tests/loopback.rs`: with `listen = "0.0.0.0:…"` config load fails with the loopback rule; with the default, the socket's local address is loopback and a connection to the machine's non-loopback address is refused (skipped when the machine has none)

### Implementation for User Story 2

- [ ] T030 [US2] Implement `nullrouter dashboard token` and `dashboard status` in `crates/nullrouter-cli/src/cmd/dashboard.rs`, registered in `crates/nullrouter-cli/src/main.rs`. The token is 32 bytes from `getrandom`, base64url, prefixed `nrd_`. Store the digest via T014, then call `cmd::apply` (`applied` / `saved; applies at next start`)
- [ ] T031 [US2] Implement `crates/nullrouter-dashboard/src/access.rs`:
  - the Host check (421);
  - the cookie check: SHA-256 of the cookie value vs the snapshot's `token_digest`, with `subtle::ConstantTimeEq`;
  - the redirect to `/signin?next=` with `next` validation;
  - the global wrong-token delay (1 s × 2ⁿ, cap 30 s, reset on success);
  - the `Origin` check on `POST /signin`
- [ ] T032 [US2] Implement `crates/nullrouter-dashboard/src/pages/signin.rs`: the no-token page and the token form (a `POST` form; the field is `type="password"` with `autocomplete="current-password"` so password managers can keep it), and the "That token is not the current one." error
- [ ] T033 [US2] Add the dashboard lines to `check` (contracts/cli.md: `warning: dashboard not listening: <addr>: <reason>`, the listen-changed note, the `dashboard` JSON object) in `crates/nullrouter-server/src/views/check.rs` and its printer in `crates/nullrouter-cli/src/cmd/check.rs`

**Checkpoint**: an operator can issue a token, sign in once and see the empty frame. Nobody else can.

---

## Phase 4: User Story 1 — See accounts and quota at a glance (Priority: P1) 🎯 MVP

**Goal**: The accounts and quota page shows everything `accounts list --long` and `quota` show,
plus `check`'s account-related warnings.

**Independent Test**: `cargo test -p nullrouter-dashboard --test cli_agreement -- accounts`.

### Tests for User Story 1 ⚠️

- [ ] T034 [US1] Build the agreement fixture home in `crates/nullrouter-dashboard/tests/common/fixture.rs` using the engine testkit:
  - accounts: signed in; needs sign-in; refused; cooling on one model; disabled; priority 0; estimated; pay-as-you-go with no price; pending first poll; stale;
  - two agent keys, one revoked and one with its own break behaviour;
  - a unified model whose members differ in `context_length`, a dropped unified model, a pending plugin conflict, a skipped plugin;
  - records: warm, cold, overflow, failed then fallback, usage missing, in flight, cut short;
  - a disk-full period.

  Also add a helper that starts `serve` on free ports, signs in, fetches a page, and runs the CLI binary with `--json` against the same home
- [ ] T035 [US1] Write `crates/nullrouter-dashboard/tests/cli_agreement.rs::accounts`:
  - every scalar of `accounts list --long --json` and `quota --json` appears in `/accounts`, with times compared as instants via `datetime`;
  - each account-subject `check --json` item appears in `check`'s words;
  - each needs-sign-in account shows `nullrouter accounts signin <p> <n>`;
  - an empty home shows the `accounts add` empty state

### Implementation for User Story 1

- [ ] T036 [US1] Implement `crates/nullrouter-dashboard/src/pages/accounts.rs`. Per account: provider, name, kind, order, priority, the last four as the CLI shows them, state badge with since and reason, cooldowns per model with time left, email, tier, token expiry, last refresh. Per quota window: remaining, unit, source badge (polled / estimated / pay-as-you-go / pending first poll / stale), last poll, reset. Also the account-subject `check` items and the `provider` filter (FR-019a, FR-020)
- [ ] T037 [US1] Route `/` → 303 `/accounts` and `/accounts` through the guard and access layers in `crates/nullrouter-dashboard/src/lib.rs`

**Checkpoint**: MVP. A signed-in operator sees accounts and quota, and they match the CLI.

---

## Phase 5: User Story 3 — See where requests went and why (Priority: P2)

**Goal**: The routing view and paged, filterable request records, each record with its full
detail.

**Independent Test**: `cargo test -p nullrouter-dashboard --test cli_agreement -- routing records`.

### Tests for User Story 3 ⚠️

- [ ] T038 [P] [US3] Write `crates/nullrouter-engine/tests/records_page.rs`: the newest-first read with `limit` and `before` returns exactly what the full scan returns after the same filter, sort and truncate, over multiple segments and with in-flight records merged
- [ ] T039 [P] [US3] Write `crates/nullrouter-dashboard/tests/cli_agreement.rs::routing`: `/routing` vs `routing --json` (pace, share, deficit, priority, quota source, each window) and the routing and journal `check` items. `::records`: `/records` with each filter (provider, account, agent, model, reason, since) vs `records list --json` with the same filters. Paging via `before` vs `records list --before`. `/records/<id>` vs `records show --json` for each fixture record. "usage not reported", "in flight" and "cut short" appear in the CLI's words, and an invalid `since` shows the CLI's error

### Implementation for User Story 3

- [ ] T040 [US3] Add the newest-first read with early stop to `records::read` in `crates/nullrouter-engine/src/journal/records.rs` (segments newest first, stop at `limit` after `before`) and the `before` filter field (R9)
- [ ] T041 [US3] Add `before` to the `records.list` op in `crates/nullrouter-server/src/operator.rs` and `--before <ID>` to `records list` in `crates/nullrouter-cli/src/cmd/records.rs`
- [ ] T042 [P] [US3] Add the Criterion bench `records_page_100k` in `crates/nullrouter-engine/benches/` (100k records, 50 per page, newest page and a deep page). Record the baseline locally in `specs/007-dashboard/bench-baseline.md`
- [ ] T043 [US3] Implement `crates/nullrouter-dashboard/src/pages/routing.rs` (`/routing`: amortization window, per target and account the routing view, the routing and journal `check` items, the first page of records) and `crates/nullrouter-dashboard/src/pages/records.rs` (`/records`: filter form, 50 per page, "Older" link with `before`; `/records/<id>`: every attempt with account, outcome, class, reason, TTFT and total, the placement reason and its values, usage, dropped fields) (FR-021, FR-022, FR-026)

**Checkpoint**: the operator can tell from the dashboard why each request went where it did.

---

## Phase 6: User Story 4 — See models, providers and plugins (Priority: P2)

**Goal**: Providers and their model types, model details, plugins and their fit, unified models
with members and limits notes, the load report, and the "not built yet" entries.

**Independent Test**: `cargo test -p nullrouter-dashboard --test cli_agreement -- models` and
`cargo test -p nullrouter-cli --test unified`.

### Tests for User Story 4 ⚠️

- [ ] T044 [P] [US4] Write `crates/nullrouter-cli/tests/unified.rs`: `unified` text and `--json` per contracts/cli.md (members in order with upstream, limits notes, `dropped`), `unified <name>`, exit 2 with `resolve`'s message for an unknown name, and the empty-home message
- [ ] T045 [P] [US4] Write `crates/nullrouter-dashboard/tests/cli_agreement.rs::models`:
  - `/models` vs `providers --json`, `plugins list --community --json`, `unified --json`, and the models-subject `check --json` items;
  - `/models/<provider>/<model>` vs `model <provider> <model> --json` for each model of each active provider;
  - the `capability` filter vs `providers --capability`;
  - `/not-built/model-tests` and `/not-built/combos` say "isn't built yet" and name the commands from R13

### Implementation for User Story 4

- [ ] T046 [US4] Implement `crates/nullrouter-server/src/views/unified.rs` (data-model.md `unified` shape, reusing `resolve`'s member JSON and `note_json` from `crates/nullrouter-cli/src/cmd/resolve.rs`, moved into the view) and `nullrouter unified [NAME]` in `crates/nullrouter-cli/src/cmd/unified.rs`, registered in `crates/nullrouter-cli/src/main.rs`
- [ ] T047 [US4] Implement `crates/nullrouter-dashboard/src/pages/models.rs`: providers with model types and their models linking to `/models/<provider>/<model>`, plugins with state, community plugins with fit, unified models with kind and ordered members, dropped unified models, limits notes and the other models-subject `check` items. Also the model detail page (FR-023)
- [ ] T048 [P] [US4] Implement `crates/nullrouter-dashboard/src/pages/not_built.rs`: the Model tests and Combos pages per R13 (FR-025)

**Checkpoint**: everything configured is visible, and matches the CLI.

---

## Phase 7: User Story 5 — See agent keys and behaviour settings (Priority: P3)

**Goal**: Agent keys as `keys list` shows them, and the behaviour settings.

**Independent Test**: `cargo test -p nullrouter-dashboard --test cli_agreement -- keys`.

### Tests for User Story 5 ⚠️

- [ ] T049 [P] [US5] Extend `crates/nullrouter-cli/tests/accounts_keys.rs` with `behaviour show`: text `break_behaviour  restart  (default)`; after `behaviour set-break error_event`, `error_event` without `(default)`; and `--json` `{"break_behaviour":{"value","default"}}`
- [ ] T050 [P] [US5] Write `crates/nullrouter-dashboard/tests/cli_agreement.rs::keys`: `/keys` vs `keys list --json` and `behaviour show --json`, the keys-subject `check` items, revoked shown with its time, "default" for keys without their own behaviour with the operator default shown once, and no key text beyond `…last4`

### Implementation for User Story 5

- [ ] T051 [US5] Implement `crates/nullrouter-server/src/views/behaviour.rs` (one entry per `[pipeline]` setting the operator can set: `value`, `default`) and the `show` subcommand in `crates/nullrouter-cli/src/cmd/behaviour.rs`
- [ ] T052 [US5] Implement `crates/nullrouter-dashboard/src/pages/keys.rs` (FR-024) and the empty state naming `nullrouter keys issue <name>`

**Checkpoint**: all four pages are complete.

---

## Phase 8: User Story 6 — The dashboard looks like 9router (Priority: P2)

**Goal**: Every style value traces to 9router, and the components match 9router side by side.

**Independent Test**: `cargo test -p nullrouter-dashboard --test style_guide`, plus the user's
review (quickstart step 7).

### Tests for User Story 6 ⚠️

- [ ] T053 [P] [US6] Add `tokens_trace_to_9router` to `crates/nullrouter-dashboard/tests/style_guide.rs`. Every `source` line exists in `ref/9router` and contains the `value` or `class`, and no source line is inside `globals.css`'s `.dark { … }` block. The test is skipped with a message when `ref/9router` is absent (CI)
- [ ] T054 [P] [US6] Add `css_uses_only_tokens` to `crates/nullrouter-dashboard/tests/style_guide.rs`. Every declaration value in `dashboard.css` is `var(--x)` with `x` in `tokens.toml`, a keyword, `0`, or a layout percentage. Every component class is a `[component.*]` entry, using only that entry's tokens
- [ ] T055 [P] [US6] Add `status_badges_match_guide` to `crates/nullrouter-dashboard/tests/style_guide.rs`. Each status word on the fixture's pages uses the badge variant from contracts/style-guide.md's table (FR-032)

### Implementation for User Story 6

- [ ] T056 [US6] Fix whatever T053–T055 find in `crates/nullrouter-dashboard/style/` and `crates/nullrouter-dashboard/src/components.rs`
- [ ] T057 [US6] Capture screenshots of each component (card, badge, button, table, input, navigation) on 0router's pages and on 9router's dashboard in light mode with the `webapp-testing` skill, and put them side by side in `specs/007-dashboard/look-review.md` for the user's SC-006 judgment. Record the user's verdict and any fixes there

**Checkpoint**: the look is traceable and has been reviewed.

---

## Phase 9: Polish & Cross-Cutting Concerns

- [ ] T058 Write the isolation load test `crates/nullrouter-dashboard/tests/isolation.rs` (`#[ignore]`, run with `--release`). It sends 1,000 client requests to the testkit mock provider while 4 workers fetch pages in a loop, and again with a `fault-injection` cfg making every build panic or sleep 20 s. It asserts p95 within 1 ms of the dashboard-off run and 0 extra failures, and that a taken dashboard port still serves clients (SC-003, FR-012, FR-013)
- [ ] T059 [P] Extend the secrets sentinel `crates/nullrouter-server/tests/secrets.rs` (or a new `crates/nullrouter-dashboard/tests/secrets.rs` reusing its helpers) to scan every page and response of the fixture, the logs and the home directory for provider secrets beyond last four, OAuth client secrets, full agent keys, the dashboard token, and prompt text (SC-004)
- [ ] T060 [P] Write `crates/nullrouter-dashboard/tests/offline.rs`: every page's HTML and CSS references only `/assets/…` on the dashboard, with no `http(s)://` URL in `src`, `href` or `url()` (SC-008, FR-011)
- [ ] T061 [P] Add the Criterion bench `crates/nullrouter-dashboard/benches/pages.rs` (each page on a home with 50 accounts, 20 unified models, 100k records; target < 1 s, SC-007), and add the result to `specs/007-dashboard/bench-baseline.md`
- [ ] T062 [P] Document the dashboard in `docs/operator-config.md`: the `[dashboard]` table, `dashboard token/status`, signing in, `unified`, `behaviour show`, `records list --before`, and the cookie-scope note with the `127.0.0.2` option (R6, L1)
- [ ] T063 Run a security review of the dashboard (token storage, cookie, Host/Origin/CSP, no write routes, bounded work) with the `security-auditor` agent, and write `specs/007-dashboard/security-review.md`. L1 (cookie host scope) is listed for the user's decision
- [ ] T064 Run quickstart.md steps 1–8 on a throwaway home and mark each result in `specs/007-dashboard/quickstart.md`
- [ ] T065 Push and confirm CI (fmt, clippy `-D warnings`, tests) is green on `007-dashboard`

---

## Dependencies & Execution Order

- **Setup (T001–T003)** → **Foundational (T004–T026)** → user stories.
- Within Foundational: T004 before T005–T011; T012 after them; T016 before T017, T018 and T021;
  T022 before T023, T024 and T026.
- **US2 (T027–T033)** first: every page needs access.
- **US1 (T034–T037)** needs US2. T034's fixture is used by US3–US6 tests.
- **US3, US4, US5** each need US1's fixture and helper (T034), and are otherwise independent of
  each other.
- **US6** tests (T053–T055) need the components (T026), and T055 needs the pages. Run them after
  US5.
- **Polish** after all stories. T065 last.

### Parallel opportunities

- T005–T011 (different view files) after T004.
- T013, T014, T019, T025 alongside the view move.
- In US2: T027, T028, T029.
- After US1, US3/US4/US5 can be worked in parallel, each with its own page and view files.
- In Polish: T059–T062.

### Parallel example: Foundational view move

```text
Task: "Move accounts list builder → crates/nullrouter-server/src/views/accounts.rs" (T005)
Task: "Move quota builder → crates/nullrouter-server/src/views/quota.rs" (T006)
Task: "Move routing builder → crates/nullrouter-server/src/views/routing.rs" (T007)
Task: "Move records builders → crates/nullrouter-server/src/views/records.rs" (T008)
```

## Implementation Strategy

### MVP first

1. Phases 1–2 (views shared, skeleton, style).
2. US2 (access) and US1 (accounts page). **Stop and validate**: quickstart steps 1–3 for
   accounts.

### Incremental delivery

3. US3 (routing and requests) → US4 (models) → US5 (keys), each validated against the CLI.
4. US6 (look tests and the user's side-by-side review).
5. Polish: isolation, secrets, offline, benches, docs, security review, quickstart, CI.

### Notes

- The view move (T005–T011) must not change any CLI output. Treat a failing existing CLI test
  as a regression, not as an expected update.
- The criterion benches and the isolation test run locally (cloud timings don't compare).
