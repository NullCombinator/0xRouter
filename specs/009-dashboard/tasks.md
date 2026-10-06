---

description: "Task list for slice 009, dashboard (slice 1 of two)"
---

# Tasks: Dashboard

**Input**: Design documents from `specs/009-dashboard/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/dashboard-http.md,
contracts/cli.md, contracts/plugin-logo.md, contracts/style-guide.md, quickstart.md. Scope brief:
`specs/briefs/2026-10-05-dashboard.md`.

**Tests**: requested by the spec (SC-001 to SC-012, each story's Independent Test), so each story
has test tasks. Write them first and see them fail.

**Build**: `export CARGO_HOME=$PWD/.cargo-home`; one crate at a time with `-j 2`; never chain a
workspace test and clippy. Check `git branch --show-current` is `009-dashboard` before each
commit. CLI goldens (`crates/nullrouter-cli/tests/golden/`) change only in the commit that changes
the read, re-blessed with `NR_BLESS=1 cargo test -p nullrouter-cli --test read_golden`.

**Gate (closed)**: security Low L1 (research R7) was decided by the user on 2026-10-06: accept and
document. The default listen address stays `127.0.0.1:20130` on every OS; T025 writes the
cookie-scope note.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an unfinished task)
- **[Story]**: US1 access, US2 Quota Tracker, US3 Usage, US4 Providers, US5 Endpoint & Key and
  Settings, US6 notices and not-built entries, US7 look, US8 plugin logos

---

## Phase 1: Setup

- [X] T001 Create `crates/nullrouter-dashboard/` (`Cargo.toml` with `maud`, `jiff`, `subtle`, `axum`, `tokio`, `serde_json`, `sha2`, `base64`, `getrandom`, and path deps `nullrouter-server`, `nullrouter-engine`, `nullrouter-registry`; `src/lib.rs` with a doc comment pointing to `specs/009-dashboard/contracts/dashboard-http.md`), add it to the workspace `members` in `Cargo.toml`, and add `maud`, `jiff`, `subtle` to `[workspace.dependencies]`
- [X] T002 [P] Add the `nullrouter-dashboard` row to the workspace table in `CLAUDE.md` ("Read-only web dashboard served by `serve`: access, pages, style tokens, assets"), and correct the `plugins/bundled/` row from five providers to seven
- [X] T003 [P] Add a `dashboard()` fixture home to `crates/nullrouter-engine/src/testkit/homes.rs` (pinned clock, fixed ids), built on `full()`, that has: a signed-in polled account, a sign-in account whose refresh failed (needs sign-in), an estimated account, a pay-as-you-go account, an account in a cooldown, a disabled account, a target with two accounts, two agent keys (one revoked, one with its own break behaviour) and one never used, records covering warm, cold, overflow, a failed attempt with fallback, a record with changes, an unfinished record, a record whose usage the provider did not report, a record cut short, a period with records not kept, and at least one notice of every `check` kind (pending conflict, declined, withheld credential, skipped plugin, dropped unified model, limits note, unmetered window, routing warning, sign-in error, tokens without account, account without tokens, a bad file mode). The logo-ignored note and the dashboard-not-listening warning don't exist yet; T022 and T062 add them to this home

---

## Phase 2: Foundational (blocking)

**⚠️ CRITICAL**: no page work before T016.

### `check` notices and the endpoint (research R5, R6; carries 007 T011)

- [X] T004 Write `crates/nullrouter-server/src/views/check.rs` unit tests: for the `dashboard()` home, `notices` has one entry per line `check` prints after `unified models:`, in print order, each `{"level","subject","text"}` with `level` in `error|warning|note` and `subject` in `endpoint|providers|combo|usage|quota|settings` per the research R5 table; a skipped plugin's indented error lines are joined to its `skipped:` line with `\n`; a notice kind not in the table gets `settings`
- [X] T005 Move the line building from `crates/nullrouter-cli/src/cmd/check.rs` into `views::check` as `notices`, keep every existing JSON field, and make the CLI print `home:`, then `endpoint:` (T007), `providers:`, `unified models:`, then each `notices[].text`. Text output for the existing goldens must be byte-identical except the new `endpoint:` line
- [X] T006 Add the `server.status` op to `crates/nullrouter-server/src/operator.rs`: `{"ok":true,"client_listen":"…","dashboard":{"enabled":bool,"listen":"…","serving":bool,"error":"…"|null}}`, with `client_listen` the address `serve` actually bound (pass it into the engine or server state at startup in `crates/nullrouter-cli/src/cmd/serve.rs`); `dashboard` is `{"enabled":false,…}` until T012 wires the listener
- [X] T007 Add `endpoint` and `endpoint_source` to `views::check` (`check::NEEDS` gains `server.status`): from `client_listen` with `"server"`, else `config.toml [server] listen` with `"config"`; `0.0.0.0` shown as `127.0.0.1`, `::` as `[::1]`; always `http://<host:port>/v1`. Text: `endpoint: <url>`, plus ` (configured; no server running)` for `config`. Unit tests for both sources and both unspecified hosts
- [X] T008 Re-bless the CLI goldens (`check` gains `endpoint:` and `notices`) in one commit, and check the diff contains nothing else

### Config and home files

- [X] T009 [P] Add `[dashboard]` to `OperatorConfig` in `crates/nullrouter-registry/src/schema/config.rs`: `enabled` (bool, default `true`) and `listen` (default `"127.0.0.1:20130"`); "The host must be a loopback address (`127.0.0.0/8`, `::1`, `localhost`). Anything else is a load error: `config.toml:L:C dashboard.listen: must be a loopback address; network binding is not supported`." Unit tests for the default, each loopback form, and the refusal
- [X] T010 [P] Add `dashboard.toml` load and save to `crates/nullrouter-engine/src/files.rs`: `schema = 1`, `token_digest` ("hex string, 64 chars", "SHA-256 of the full token text (`nrd_…`). Absent until the first `dashboard token`."), `issued` (RFC 3339 UTC); "Mode 0600, written atomically"; loaded into the engine snapshot so a reload picks up a new digest. Unit tests: round trip, mode, missing file (no token)
- [X] T011 Add the `dashboard.toml` file-mode finding to `views::check` (subject `settings`, as `keys.toml`'s is checked), with a unit test

### Dashboard crate skeleton

- [X] T012 Create `crates/nullrouter-dashboard/src/lib.rs`: `spawn(engine, settings, version) -> DashboardHandle` binding its own `TcpListener` and axum router in its own task; a bind failure is logged and kept in the handle; `DashboardHandle::status()` feeds `server.status`'s `dashboard` object. Wire it in `crates/nullrouter-cli/src/cmd/serve.rs` after the client listener, skipped when `enabled = false`
- [X] T013 [P] Create `crates/nullrouter-dashboard/src/guard.rs`: a semaphore of 2 build permits (a third waits up to 5 s, then 503 "Busy; reload."), a 10 s timeout per build, each build in its own task so a panic becomes that page's 500 ("This page could not be built: <reason>. Other pages and client requests are unaffected."). Unit tests for busy, timeout, panic
- [X] T014 [P] Create `crates/nullrouter-dashboard/src/time.rs` with `jiff`: an instant renders as `<time datetime="<RFC 3339 UTC>">` with local `YYYY-MM-DD HH:MM:SS`; the header line `as of HH:MM:SS <abbr> (<zone name>) · reload to refresh`. Unit tests with a fixed `TZ`, including a DST change
- [X] T015 [P] Create `crates/nullrouter-dashboard/src/headers.rs`: the response headers of contracts/dashboard-http.md "Every response" (CSP, `X-Frame-Options`, `Referrer-Policy`, `nosniff`, `Cross-Origin-Resource-Policy`, `Cache-Control: no-store`; assets `public, max-age=86400, immutable`), the `Host` check (421) and the method check (405 for any non-`GET` except `POST /signin`). Unit tests
- [X] T016 Create `crates/nullrouter-dashboard/src/page.rs`: a page build takes one engine snapshot, its "as of" instant, and the views it needs through `views::run_in_process` (research R1, R14); each page module declares its view list as a `const`, used by the twin test (T070). Unit test: a reload landing between two view fetches of one build leaves every view reporting the first snapshot's generation (FR-025)

---

## Phase 3: User Story 1 — Open the dashboard once, and only on this machine (Priority: P1) 🎯 MVP

**Goal**: the token gate, `dashboard token` and `dashboard status`.

**Independent Test**: spec US1; quickstart steps 1 and 2.

### Tests for User Story 1

- [X] T017 [P] [US1] Write `crates/nullrouter-dashboard/tests/access.rs` against a real `serve` on a temp home: every route gives only the no-token page before a token exists; after one, every route redirects to `/signin?next=…` without a cookie; the right token sets `nr_dashboard=…; Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000` and opens pages; a wrong one gets 401 after the delay and the delay doubles; a new token signs the old cookie out; `next` outside the dashboard becomes `/`; `/logos/…` without a valid cookie redirects like a page, while `/assets/…` is served; a foreign `Origin` on `POST /signin` gets 403; a foreign `Host` gets 421; `no_write_routes`: every route with every non-`GET` method gets 405 except `POST /signin`
- [ ] T018 [P] [US1] Write `crates/nullrouter-cli/tests/dashboard.rs`: `dashboard token` prints `nrd_` plus 43 base64url characters once and its `--json` shape; `dashboard.toml` holds only the digest, mode 0600; `dashboard status` prints each state of contracts/cli.md's table (serving, bind failed with the port taken, off, no server) and `token: none; run nullrouter dashboard token`; exit 0 in each
- [ ] T019 [P] [US1] Write `crates/nullrouter-dashboard/tests/bind.rs`: with the dashboard port taken, `serve` starts and serves a client request, `dashboard status` says not listening, `check` prints `warning: dashboard not listening: <addr>: <reason>`; with `enabled = false`, the port is closed; the bound address is loopback (`local_addr().ip().is_loopback()`), and a connect to the dashboard port on a non-loopback address of this host is refused (skipped with a message when the host has none) (FR-002, SC-004)

### Implementation for User Story 1

- [ ] T020 [US1] Create `crates/nullrouter-server/src/views/dashboard.rs`: the `dashboard` view of data-model.md from `server.status` and `dashboard.toml` (`server`, `serving` null with no server, `error`, `token_issued`); never the token
- [ ] T021 [US1] Create `crates/nullrouter-cli/src/cmd/dashboard.rs` with `token` (32 random bytes → `nrd_<base64url>`, digest and `issued` to `dashboard.toml`, then reload a running server and print `applied` / `saved; applies at next start`; the "dashboard is off" line when disabled) and `status` (contracts/cli.md), registered in `crates/nullrouter-cli/src/main.rs`
- [ ] T022 [US1] Add the `warning: dashboard not listening: <addr>: <reason>` notice (subject `settings`) to `views::check` from `server.status`, and add a bound dashboard port to the `dashboard()` fixture's serve so the warning occurs there (T003)
- [ ] T023 [US1] Create `crates/nullrouter-dashboard/src/access.rs`: cookie check by rehashing and constant-time compare (`subtle`) against the snapshot's digest; the global wrong-token delay (1 s × 2ⁿ, cap 30 s, reset by a success); `next` sanitising; `Origin` check
- [ ] T024 [US1] Create `crates/nullrouter-dashboard/src/pages/signin.rs`: `GET /signin` (no-token page naming `nullrouter dashboard token`, or the token form), `POST /signin`; route every other path through `access.rs` in `lib.rs`
- [ ] T025 [US1] Document the dashboard in `docs/operator-config.md`: `[dashboard]`, `dashboard token`/`status`, signing in, the 400-day cookie, and the L1 cookie-scope note with the user's decision

**Checkpoint**: the gate works; pages are empty frames.

---

## Phase 4: User Story 7 (part 1) — Style tokens and the frame

Every page needs the frame. The style suite and the full component set finish in Phase 10.

- [ ] T026 [US7] Revise `docs/dashboard/style-guide.md` per research R12: drop "Not taken: page structure, navigation entries"; add the sidebar (`.bg-vibrancy`, 288 px), entries, "System" heading, page header, modal, side panel, round floating button and panel, background grid on every page, slot and disabled-hint (from existing tokens only); move 007 FR references to 009's; replace the "Open" section with the decisions
- [ ] T027 [US7] Create `crates/nullrouter-dashboard/style/tokens.toml` per contracts/style-guide.md (`schema = 1`, `tailwind = …`, `[token.*]` with `value`, `source`, optional `class`; `[component.*]` with `uses`, `source`), covering every value the guide lists
- [ ] T028 [US7] Generate `crates/nullrouter-dashboard/style/tokens.css` from `tokens.toml` with a `cargo test` that writes it under `NR_BLESS=1` and fails on any difference otherwise
- [ ] T029 [P] [US7] Add Inter (variable woff2, Latin subset) and its OFL text to `crates/nullrouter-dashboard/assets/`, and the Material Symbols Outlined SVGs for every icon the mockups use (`grep -o 'class="i[^"]*">[a-z_]*' docs/dashboard/mockups/*.html`) to `assets/icons/`, with the Apache 2.0 text in `assets/LICENSES/`; serve them at `/assets/<hash>/<file>` from `include_bytes!`
- [ ] T030 [US7] Create `crates/nullrouter-dashboard/src/components.rs` (card, badge by status per contracts/style-guide.md "Status → badge", button incl. disabled with hint, table, modal, side panel with `<details>` chevron, slot, empty state, notice line by level) and the matching rules in `style/dashboard.css` using only `var(--token)`, keywords, `0`, and percentages in `width`/`flex`
- [ ] T031 [US7] Create `crates/nullrouter-dashboard/src/frame.rs`: sidebar ("0Router Proxy", the version `nullrouter --version` prints, the eight entries under contracts/dashboard-http.md "Frame", the current entry marked), header (icon, title, subtitle, "as of" line), background grid, the round housekeeping button linking to the same path with `notices` toggled, and the page's own notices (subject = page id) above its content

**Checkpoint**: every route renders the frame for a signed-in browser.

---

## Phase 5: User Story 2 — Quota Tracker (Priority: P1)

**Goal**: one card per account with sign-in, quota and routing state.

**Independent Test**: spec US2.

### Tests for User Story 2

- [ ] T032 [P] [US2] Write `crates/nullrouter-dashboard/tests/agreement.rs` with a helper that starts `serve` on the `dashboard()` home, signs in, fetches a page, runs a CLI command with `--json` against the same server, and checks that every scalar the CLI JSON holds for the facts the page shows appears in the page (times compared as instants via `datetime`, live durations within the elapsed seconds); then a Quota Tracker case against `accounts list --long`, `quota` and `routing`, and the narrowed case `?provider=…&account=…` against `quota <provider> <name>`
- [ ] T033 [P] [US2] Add Quota Tracker cases to `crates/nullrouter-dashboard/tests/pages.rs`: the needs-sign-in card names `nullrouter accounts login <provider> <name>`; pending-first-poll and stale use `quota`'s words; pay-as-you-go has no bar; `order=expiring` sorts windows by reset; empty home names `nullrouter accounts add <provider> <name>`

### Implementation for User Story 2

- [ ] T034 [US2] Create `crates/nullrouter-dashboard/src/pages/quota.rs` per contracts/dashboard-http.md "Quota Tracker" (views `accounts`, `quota`, `routing`, `check`): filters as a `GET` form, one card per account, quota windows with bars, each target's pace, share and deficit, the amortization window in the header, the footer line

**Checkpoint**: MVP: access plus Quota Tracker.

---

## Phase 6: User Story 3 — Usage (Priority: P2)

**Goal**: the Requests table and the record window.

**Independent Test**: spec US3.

### Tests for User Story 3

- [ ] T035 [P] [US3] Add Usage cases to `crates/nullrouter-dashboard/tests/agreement.rs`: page 1 against `records list --limit 50`, "Older" against `--before <last id>`, `/usage/records/<id>` against `records show <id>` (attempts, stay-warm decision, changes, usage), the unfinished record, the record with no reported usage and the record cut short in `records`' words (never as zero), the records-not-kept warning
- [ ] T036 [P] [US3] Add Usage cases to `crates/nullrouter-dashboard/tests/pages.rs`: the three slots (period filter, stat cards, topology graph) show no digits; "Recent Requests" lists the newest 10; a missing record id gives 404 in the frame with the CLI's message; on an empty home the page says "No request records yet."

### Implementation for User Story 3

- [ ] T037 [US3] Create `crates/nullrouter-dashboard/src/pages/usage.rs` per contracts/dashboard-http.md "Usage": slots, "Recent Requests", the Requests table in the CLI's words, "Older" paging, rows linking to `/usage/records/<id>`, and the window (views `records` with `limit=50`, `before`; `record`)

---

## Phase 7: User Story 4 — Providers (Priority: P2)

**Goal**: provider cards by category, the provider window with the kind filter, the plugins panel.

**Independent Test**: spec US4 (logos come in Phase 12).

### Tests for User Story 4

- [ ] T038 [P] [US4] Add Providers cases to `crates/nullrouter-dashboard/tests/agreement.rs`: sections and cards against `providers` (category, source) and `accounts list --long`; `/providers/<id>` accounts against `accounts list --long`, each model against `model <provider> <model>`; the plugins panel against `plugins list --community`
- [ ] T039 [P] [US4] Add Providers cases to `crates/nullrouter-dashboard/tests/pages.rs`: the kind filter lists exactly the kinds the loaded plugins declare (no "decision" while none does), each with this provider's count, and `?kind=` lists exactly those models; every disabled control shows the research R15 hint and has no form; a provider with no accounts says "No connections"

### Implementation for User Story 4

- [ ] T040 [US4] Create `crates/nullrouter-dashboard/src/pages/providers.rs` per contracts/dashboard-http.md "Providers": disabled "Add … Compatible" and "Test All", one section per `providers` category named as 9router names it plus Custom Providers, cards with the text icon and account states, `q` search, the "Provider plugins" side panel with disabled switches, and the `/providers/<id>` window with accounts, the kind filter, the models, and the "last response" slot

---

## Phase 8: User Story 6 — Notices and not-built entries (Priority: P2)

**Goal**: the housekeeping panel, notices on their pages, Combo, Console Log, Proxy Pools.

**Independent Test**: spec US6; quickstart step 4.

### Tests for User Story 6

- [ ] T041 [P] [US6] Write `crates/nullrouter-dashboard/tests/notices.rs` on the `dashboard()` home: (its logo and dashboard-port cases pass once T022 and T062 have extended the fixture) `?notices` on any page lists every `check --json` `notices[].text` and every account needing sign-in or cooling down in `accounts list`'s words; each notice appears on the page its `subject` names; with no notices the panel says "No notices."; the chat box is disabled with "Not built yet."
- [ ] T042 [P] [US6] Add not-built cases to `crates/nullrouter-dashboard/tests/pages.rs`: Combo's, Console Log's and Proxy Pools' texts per research R15; Combo lists its `combo` notices and no unified model list

### Implementation for User Story 6

- [ ] T043 [US6] Add the housekeeping panel to `crates/nullrouter-dashboard/src/frame.rs` (views `check`, `accounts`; notices grouped by level, then accounts needing action, then the disabled chat box)
- [ ] T044 [P] [US6] Create `crates/nullrouter-dashboard/src/pages/combo.rs`, `pages/console_log.rs`, `pages/proxy_pools.rs` per research R15 (Console Log links to `/usage`)

---

## Phase 9: User Story 5 — Endpoint & Key and Settings (Priority: P3)

**Goal**: the endpoint URL, key cards with "last used", the adapters panel, and Settings.

**Independent Test**: spec US5.

### Tests for User Story 5

- [ ] T045 [P] [US5] Write `crates/nullrouter-engine` unit tests in `src/journal/index.rs` for the per-agent newest arrival: refresh after appends updates it; a replaced segment (prune, forget) recomputes it; a key in no segment has none
- [ ] T046 [P] [US5] Add `keys list` cases to `crates/nullrouter-cli/tests/accounts_keys.rs`: `last_used` equals the `arrived` of `records list --agent <key id> --limit 1`, with and without a server; `never` / `null` for an unused key; after `records forget --agent <key id>` it is `never`
- [ ] T047 [P] [US5] Add Endpoint & Key and Settings cases to `crates/nullrouter-dashboard/tests/agreement.rs`: the URL against `check --json` `endpoint` (and the "configured; no server running" text cannot occur on a served page), key cards against `keys list`, Settings against `behaviour show`, `check` (`home`) and `dashboard status`; no full key or token in either page; on an empty home Endpoint & Key says "No agent keys. Run `nullrouter keys issue <name>`."

### Implementation for User Story 5

- [ ] T048 [US5] Add the newest arrival per agent to the segment index in `crates/nullrouter-engine/src/journal/index.rs` (read from each `open` line's `agent` and `arrived`, which the index already parses)
- [ ] T049 [US5] Add `last_used(home) -> BTreeMap<String, SystemTime>` to `crates/nullrouter-engine/src/journal/records.rs`: segments newest first, stopping once every key in `keys.toml` is found or the oldest segment is read; through the index when cached
- [ ] T050 [US5] Add the `keys.last_used` op to `crates/nullrouter-server/src/operator.rs` (`{"ok":true,"last_used":{"<key id>":"<RFC 3339>"|null}}`), running the journal read in `tokio::task::spawn_blocking` as `records.get` does, and `last_used` to `views::keys` (`NEEDS` gains `keys.last_used`; without a server it calls T049 itself)
- [ ] T051 [US5] Append the last-used column to `keys list` text in `crates/nullrouter-cli/src/cmd/keys.rs` (RFC 3339 or `never`) and re-bless the goldens in one commit
- [ ] T052 [P] [US5] Add the `keys_last_used_100k` Criterion bench in `crates/nullrouter-engine/benches/keys_last_used_100k.rs` (registered in `Cargo.toml`; reuse `records_page.rs`'s journal builder): warm (index cached) and cold (one key never used); targets warm under 5 ms, cold under 1 s; record the baseline in `specs/009-dashboard/bench-baseline.md`, and confirm `records_page` didn't regress
- [ ] T053 [US5] Create `crates/nullrouter-dashboard/src/pages/endpoint.rs` per contracts/dashboard-http.md "Endpoint & Key": the API Endpoint card, the "Agent traffic" slot, key cards with last used and a "requests today" slot, disabled "Add Agent" with its hint, the "Client adapters" side panel, the empty state
- [ ] T054 [US5] Create `crates/nullrouter-dashboard/src/pages/settings.rs` per contracts/dashboard-http.md "Settings": Routing (`behaviour show`), Local mode (`check.home`), Dashboard (`dashboard status`, and "Change it with `nullrouter dashboard token`")

---

## Phase 10: User Story 7 (part 2) — The dashboard looks like 9router (Priority: P2)

**Goal**: every style value traced and enforced.

**Independent Test**: spec US7; quickstart step 8.

- [ ] T055 [P] [US7] Write `crates/nullrouter-dashboard/tests/style_guide.rs`: (1) every declaration in `dashboard.css` is `var(--x)`, a keyword, `0` or a `width`/`flex` percentage, every `--x` is a token, every component class is a `[component.*]` using only its tokens; (2) every `source` line exists in `ref/9router` and contains the value or class, and none is inside `.dark { … }` (skipped with a message when `ref/9router` is absent); (3) `tokens.css` matches `tokens.toml`
- [ ] T056 [P] [US7] Write `crates/nullrouter-dashboard/tests/offline.rs`: every page's HTML references only `/assets/…`, `/logos/…` and dashboard routes; no `<script>`; no `http(s)://` URL in `src`, `href` (except the endpoint text), or CSS `url()`
- [ ] T057 [US7] Complete the components the pages use (provider card, quota card, key card, request row) in `components.rs` and `dashboard.css`, each a `[component.*]` in `tokens.toml` naming the 9router component it composes, until T055 passes. Add to `tests/pages.rs`: every status in contracts/style-guide.md "Status → badge" renders with its badge variant; a 200-character account, model and key name is shown in full or truncated with the full name in a `title`, and two names differing only at the end stay distinguishable
- [ ] T058 [US7] Compare side by side as quickstart step 8 says (9router light mode, the mockups, this dashboard) and write the differences found and fixed to `specs/009-dashboard/look-review.md`; the user's judgement of SC-008 is recorded there

---

## Phase 11: User Story 8 — A plugin ships its logo (Priority: P3)

**Goal**: the logo field, the core's check, shipped logos, and their display.

**Independent Test**: spec US8; quickstart step 6.

### Tests for User Story 8

- [ ] T059 [P] [US8] Write logo-check unit tests in `crates/nullrouter-registry/src/logo.rs`: a valid PNG passes; "At most 65,536 bytes"; a JPEG named `.png` → `not a PNG`; 257 × 10 → `257 × 10 px, over 256 px`; missing → `file not found: logos/<file>`; `logo = "../x.png"`, `"a/b.png"`, `"x.svg"` fail validation on the field
- [ ] T060 [P] [US8] Write `crates/nullrouter-dashboard/tests/logos.rs`: a user plugin with each bad logo loads and serves a request, its card shows the text icon, and `check` prints `note: logo ignored: <id>: <reason>` with subject `providers`; a good logo is served at `/logos/<hash>/<id>.png` with `Content-Type: image/png` and `nosniff`; `plugins install`/`uninstall` copy and remove a community logo

### Implementation for User Story 8

- [ ] T061 [US8] With the `plugin-system-designer` agent, add the optional `logo` field to the schema 2 plugin in `crates/nullrouter-registry/src/schema/plugin.rs` ("A bare file name ending in `.png`: no `/`, `\` or `..`") and create `crates/nullrouter-registry/src/logo.rs` with the four checks of contracts/plugin-logo.md, reading only the first 33 bytes and the length, without decoding
- [ ] T062 [US8] Embed `plugins/bundled/logos/*.png` and `plugins/community/logos/*.png` in `crates/nullrouter-registry/build.rs` (a missing directory embeds an empty table), resolve `<home>/plugins/logos/` for user plugins in `load.rs`, keep passed logos' bytes in the registry snapshot, and add failures to the load report as their own list, `logos_ignored: [{id, reason}]` (not the typed `notes`, which are limits notes); `views::check` adds `logos_ignored` to its JSON and turns each into a `note: logo ignored: <id>: <reason>` notice with subject `providers`, printed right after the `skipped:` lines (research R5). Add a plugin with a bad logo to the `dashboard()` fixture (T003)
- [ ] T063 [US8] Make `plugins install <id>` copy the logo to `<home>/plugins/logos/` and `plugins uninstall <id>` remove it, in `crates/nullrouter-cli/src/cmd/plugins.rs`
- [ ] T064 [US8] Convert the five overrides once with ImageMagick (`convert <in> -resize 256x256\> -strip png:<out>`; `kimchi.svg` rasterised at 256 px) into `tools/gen-bundled/seeds/logos/{crush,nebius,reka,siliconflow,kimchi}.png`, each checked against the four limits
- [ ] T065 [US8] Extend `tools/gen-bundled/generate.mjs`: copy `ref/9router/public/providers/<id>.png` (or the override) to `plugins/{bundled,community}/logos/<id>.png`, apply the four checks and exit with an error naming a failing file, write `logo = "<id>.png"` into each generated community plugin, and write `plugins/LOGOS.md` (one row per logo with its source path or override at the ref SHA, the ImageMagick command, and "Each logo names its provider only; marks belong to their owners. 9router is MIT-licensed.")
- [ ] T066 [US8] Run the generator and commit its output on its own, naming the ref SHA; add `logo = "<id>.png"` by hand to the bundled plugins that have one (`anthropic`, `elevenlabs`, `grok-cli`, `opencode-go`, `openrouter`, `xai`; `opencode-zen` has none) in `plugins/bundled/*.toml`
- [ ] T067 [US8] Serve `/logos/<hash>/<id>.png` in `crates/nullrouter-dashboard/src/lib.rs` from the snapshot, behind the same cookie check as pages (FR-008), and show the logo on the provider card and window in `pages/providers.rs` (text icon when `None`)
- [ ] T068 [US8] Document the `logo` field, its directory, limits and failure note in `docs/plugins.md`

---

## Phase 12: Polish & cross-cutting gates

- [ ] T069 [P] Write `crates/nullrouter-dashboard/tests/secrets.rs`: extend the slice 005/006 sentinel (`crates/nullrouter-server/tests/secrets.rs`) to fetch every route on the `dashboard()` home with sentinel provider keys, sign-in tokens, agent keys, the dashboard token and a sentinel prompt, and scan every response body and header, the logs and the home (SC-006)
- [ ] T070 [P] Write `crates/nullrouter-dashboard/tests/twins.rs`: for each page, render it from its declared views (T016), and fail if any text node or attribute value that varies with the home's data is not found in those views' JSON (SC-002)
- [ ] T071 Write `crates/nullrouter-dashboard/tests/isolation.rs` (`#[ignore]`, release): 1,000 client requests to a mock provider while 4 workers load pages, and again with a test-only fault feature making every build panic or sleep 20 s; compare client p95 and failures with `enabled = false` (SC-005: ≤ 1 ms p95, 0 extra failures)
- [ ] T072 [P] Add the `pages` Criterion bench in `crates/nullrouter-dashboard/benches/pages.rs`: each page on a home with 50 accounts, 20 unified models and 100,000 records; each under 1 s (SC-009); baseline in `specs/009-dashboard/bench-baseline.md`
- [ ] T073 Run a security review of the dashboard (token storage, cookie, Host/Origin/CSP, the 405 rule, logo serving, bounded work) with the `security-auditor` agent, and write `specs/009-dashboard/security-review.md` with each finding and its resolution
- [ ] T074 [P] Update `init.md`'s dashboard line and `docs/dashboard/9router-inventory.md` "Decisions I need from you" to point at this spec where they are now decided
- [ ] T075 Run quickstart.md steps 1 to 9 by hand and tick each; record anything that differed

---

## Dependencies & Execution Order

- **Setup (T001–T003)** first.
- **Foundational (T004–T016)** blocks everything. Within it: T004 before T005; T006 before T007;
  T005 and T007 before T008; T012 before T013–T016 are wired.
- **US1 (T017–T025)** next: every page needs access. L1 is decided: accepted and documented (T025).
- **US7 part 1 (T026–T031)** before any page: the frame.
- **US2 (T032–T034)** is the MVP page. T032's helper is used by US3, US4 and US5 tests.
- **US3, US4, US6, US5** each need T031 and T032's helper, and are otherwise independent.
- **US7 part 2 (T055–T058)** after the pages exist.
- **US8 (T059–T068)** needs only Foundational for T059–T066; T067 needs US4's page.
- **Polish (T069–T075)** last; T070 needs every page.

## Parallel Opportunities

- Foundational: T009, T010, T013, T014, T015 together.
- US1: T017, T018, T019 together.
- After T032: US3 (T035–T037), US4 (T038–T040), US6 (T041–T044) and US5 (T045–T054) in parallel,
  each with its own page file.
- US8's T059–T066 alongside any page work.
- Polish: T069, T070, T072, T074 together.

### Example: User Story 5

```text
Together: T045 (index tests), T046 (keys list tests), T047 (page agreement)
Then:     T048 → T049 → T050 → T051, with T052 after T049
Then:     T053, T054
```

## Implementation Strategy

1. **MVP**: Setup, Foundational, US1, the frame (T026–T031), US2. Stop and check: the token gate
   and Quota Tracker agree with the CLI.
2. Add US3 (Usage), then US4 (Providers), then US6 (notices, not-built entries).
3. Add US5 (Endpoint & Key with last used; Settings).
4. Finish US7 (style suite, side-by-side review) and US8 (logos).
5. Polish gates. Each step leaves `serve` working and every CLI test passing.
