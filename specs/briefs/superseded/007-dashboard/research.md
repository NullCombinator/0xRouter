# Research: Dashboard (slice 007)

Decisions taken in planning. Each one lists what was chosen, why, and what was rejected. The
brief's ledger (specs/briefs/2026-10-05-dashboard-v1-superseded.md) is the authority. Nothing here contradicts a
confirmed row; where a decision touches one, the row is named.

## R1. One view layer shared by the CLI and the dashboard

**Decision**: Move the code that builds each CLI read command's `--json` value out of
`nullrouter-cli` into a new module, `nullrouter_server::views`. Both the CLI and the dashboard
call it. The CLI prints the value as JSON or as its current text, and the dashboard renders the
same value as HTML. The views:

| View | Builds what this CLI command shows | Live input (operator op) |
|---|---|---|
| `views::accounts` | `accounts list --long` | `accounts.state` |
| `views::quota` | `quota` | `quota.list` |
| `views::routing` | `routing [target]` | `routing.view` |
| `views::records` | `records list` (filters, limit, `before` cursor) | `records.list` |
| `views::record` | `records show` | `records.get` |
| `views::providers` | `providers [--capability]` | none (registry snapshot) |
| `views::model` | `model <provider> <model>` | none |
| `views::plugins` | `plugins list --community` | none |
| `views::unified` | `unified` (new, FR-016a) | none |
| `views::keys` | `keys list` | none (`keys.toml`) |
| `views::behaviour` | `behaviour show` (new, FR-016a) | none (`config.toml`) |
| `views::check` | `check` | `routing.health` |

A view is a function of `(home files, registry snapshot, live answer)`. The CLI gets the live
answer over the operator socket. The dashboard, in the same process, calls
`operator::handle(engine, req)` directly, which is the function the socket calls. The two paths
differ only in transport.

**Rationale**: The slice fails if a page disagrees with the CLI (brief row 9). Two independent
renderings of the same facts will drift. One builder makes the agreement structural, and SC-001
then tests the transport and the HTML rendering, which are what's left. The CLI's text output is
unchanged: its existing tests pin it.

**Alternatives rejected**:
- The dashboard runs the CLI binary and parses its `--json`: a process per page, and a dependency
  on the binary's path. Rejected.
- The dashboard has its own builders, with tests comparing them to the CLI: twice the code, and
  every CLI change needs a matching dashboard change. Rejected.
- Put the views in `nullrouter-engine`: they read the operator socket's answers, which the server
  crate defines. The CLI already depends on the server crate. Rejected.

## R2. A new crate, `nullrouter-dashboard`

**Decision**: The dashboard is `crates/nullrouter-dashboard`, which depends on
`nullrouter-server` (views, operator handle) and `nullrouter-engine`. `nullrouter serve` (CLI)
starts it next to the client listener and the operator socket. The dependency graph stays acyclic:
cli → dashboard → server → engine → registry.

**Rationale**: The client request surface (`nullrouter-server`) should not gain HTML, cookies,
fonts and CSS. A separate crate also keeps the dashboard's dependencies (the template engine, the
time-zone crate) out of the request path's build.

**Alternative rejected**: a `dashboard` module inside `nullrouter-server`. It's simpler, but
it mixes the operator-facing surface with the client-facing one. Rejected.

## R3. Server-rendered HTML with `maud`, no JavaScript

**Decision**: Pages are built with `maud` (compile-time HTML macros, escaping by default). There
is no `<script>` anywhere. The Content-Security-Policy has no `script-src` at all, so the browser
refuses scripts even if one were injected. Filters and paging are plain `GET` forms and links.
The token prompt is a plain `POST` form. (Brief rows 3, 4; FR-003a.)

**Rationale**: `maud` checks templates at compile time, has no template files to find at run
time, escapes every interpolated value (record fields come from clients and providers), and
needs no runtime. Rust only (constitution, Architecture Constraints).

**Alternatives rejected**:
- `askama`: compile-time too, but its templates live in separate files with their own syntax.
  Either would work. `maud` keeps a component's markup next to the Rust that feeds it.
- `minijinja`: runtime templates. A typo becomes a page error instead of a build error. Rejected.
- Leptos/Dioxus SSR with WASM hydration: ships application code to the browser. Ruled out by the
  user (row 4: "no WASM").

## R4. The style guide: one TOML token file with sources, one CSS file built only from it

**Decision**:
- `docs/dashboard/style-guide.md` is the human-readable guide. It covers the palette, type,
  spacing scale, radii, shadows, and each component (button, card, badge, table, input,
  navigation, sidebar, empty state) with its 9router source and a short description.
- `crates/nullrouter-dashboard/style/tokens.toml` is its machine-readable half. Each token has a
  `value` and a `source` (`ref/9router/src/app/globals.css:22`, or a component file and the
  Tailwind class it came from, such as `shared/components/Card.js:40 rounded-[10px]`).
- `style/dashboard.css` uses only `var(--token)` references and the literal `0`.
- `tokens.css`, the `:root` block, is generated from `tokens.toml` by a test that fails when the
  two differ (the same pattern as the parity fixtures, never hand-edited).
- Tailwind classes are resolved to values with Tailwind v4's default scale (spacing unit
  0.25rem, `rounded-lg` 0.5rem, `text-sm` 0.875rem/1.25rem). That scale is recorded once in the
  guide with its source (the Tailwind v4 theme, the version `ref/9router/package.json` pins).
- Light palette only (FR-034). The `.dark` block of `globals.css` is not extracted.

**Tests** (SC-005):
1. Every declaration value in `dashboard.css` is `var(--x)`, a keyword, or `0`, and every `--x`
   is in `tokens.toml`.
2. Every `source` in `tokens.toml` points to a file and line that exist in `ref/9router` and
   contain the value or the class. This test runs where `ref/9router` is present: locally and in
   cloud sessions.
3. `tokens.css` equals the file generated from `tokens.toml`.

**SC-006** (judged the same side by side) is the user's call. `quickstart.md` step 7 sets up the
comparison: 9router's dashboard and 0router's open together in light mode.

**Alternatives rejected**:
- Copy 9router's compiled Tailwind CSS: it carries 9router's whole page structure and dark
  theme, and no value traces to a source. Rejected.
- Use Tailwind in 0router: a Node build step. Rejected (Rust only).

## R5. Fonts and icons served from the binary

**Decision**: Embed Inter (SIL OFL 1.1) as one variable woff2 with the Latin subset, about
100 KB, with `include_bytes!`. Icons are a fixed set of Material Symbols Outlined glyphs (Apache
2.0), embedded as inline SVG paths: about 15 icons (accounts, quota, routing, records, models,
keys, warning, check, schedule, lock and a few more), not the multi-megabyte icon font. Both
licence texts go in `crates/nullrouter-dashboard/assets/LICENSES/`. (FR-011, SC-008.)

**Rationale**: 9router loads both from Google (`ref/9router/src/app/layout.js:1`). The dashboard
must render offline and fetch nothing from outside this machine.

**Alternative rejected**: the system font stack only. It looks visibly different from 9router,
which risks failing SC-006. It stays as the CSS fallback.

## R6. Dashboard token: stored as a digest, carried in a cookie

**Decision**:
- `nullrouter dashboard token` makes 32 random bytes (`getrandom`), prints them once as
  `nrd_<base64url>`, and stores only the SHA-256 digest in `dashboard.toml` (mode 0600, written
  atomically). This is the same treatment `keys issue` gives agent keys (FR-005).
- **Sign-in**: the browser `POST`s the token to `/signin`. The server compares digests in
  constant time (`subtle`) and answers with `Set-Cookie: nr_dashboard=<token>; Path=/; HttpOnly;
  SameSite=Strict; Max-Age=34560000`. That is 400 days, the browser cap, so "asks once" holds
  (row 7, FR-007).
- **Every request** recomputes the digest of the cookie's value and compares it with the current
  `dashboard.toml` digest, read through the engine's reload path. Issuing a new token changes the
  digest, so every old cookie stops working on its next request. No session state is kept.
- **Wrong tokens**: each failure adds a delay before the answer, starting at 1 s and doubling to
  a 30 s cap. The delay resets after a success. It is global, not per address, because every
  caller is local. (FR-008.)

**Residual risk, documented**: browsers scope cookies by host, not port. Another web server on
`127.0.0.1` that the operator's browser visits would receive the cookie. Such a server is a
local process. A local process of the same OS user can already read `~/.0router`. One of another
OS user could replay the cookie. Mitigation: `docs/operator-config.md` documents this and the
option `[dashboard] listen = "127.0.0.2:…"` on Linux, where every 127/8 address is loopback and
the cookie host differs from other local servers. This is raised as security Low **L1** for the user to
judge; it is not silently accepted.

**Alternatives rejected**:
- HTTP Basic auth: scoped per origin, so no cross-port leak, but browsers forget it at restart,
  which breaks "asks once" (row 7). Rejected.
- A server-side session table: adds state to persist and expire, for no gain over the digest
  check. Rejected.
- The token in the URL: it leaks through history and the `Referer` header. Rejected.

## R7. Browser-side protections

**Decision**: every dashboard response carries:
- `Content-Security-Policy: default-src 'none'; style-src 'self'; font-src 'self'; img-src 'self';
  form-action 'self'; frame-ancestors 'none'; base-uri 'none'`
- `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`,
  `Cache-Control: no-store` (pages carry account data), `Cross-Origin-Resource-Policy:
  same-origin`.

Requests whose `Host` is not `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>` are refused
with 421. This defeats DNS rebinding, where another website points its name at 127.0.0.1 to read
pages. `POST /signin` also needs `Origin` to be absent or the dashboard's own. There are no other
write routes (FR-010). (FR-009.)

## R8. Bound to this machine, isolated from client traffic

**Decision**:
- `config.toml` gains `[dashboard] enabled = true, listen = "127.0.0.1:20130"`. A `listen`
  address that isn't loopback is a load error naming the rule, because network binding is out of
  scope (row 11, FR-002).
- The dashboard has its own `TcpListener`, its own task, and its own axum router. Its startup
  failure (port in use) is logged and remembered. `serve` keeps serving clients, and `check`
  reports it through a new `dashboard.status` op.
- **Bounded work** (FR-012, FR-013): at most 2 page builds at once (a semaphore; others wait,
  then get 503 after 5 s), and each build times out after 10 s. A build runs in its own task, so
  a panic becomes that page's 500 and nothing else.
- Blocking reads (record segments, files) go through `spawn_blocking`, as `records.list` already
  does.
- No engine lock is held across an await. `engine.snapshot()` is an `ArcSwap` load, so a page
  read never blocks a request.
- The record list is paged at 50 per page (`?before=<id>`). No page ever loads all records.

**Rationale**: Same process, own port, isolated (row 14). Client tasks never wait on anything
the dashboard holds. The dashboard's CPU and disk use per page are capped.

**Alternative rejected**: a separate process (`nullrouter dashboard serve`). It would need its
own socket client and its own token checks. The user chose the same process (row 14).

## R9. Record reads at 100k records (SC-007)

**Finding**: `records::read` scans every journal segment and filters in memory. At 100k records
(about 1 KB each) one page would parse about 100 MB.

**Decision**: Give `records::read` a newest-first mode. It reads segments newest first and stops
once `limit` records after the `before` cursor match. The CLI's `records list --limit N` uses the
same path, so both get faster and stay identical. A Criterion bench (`records_page_100k`) pins
it. The constitution's performance gate covers the journal, which is on the request path's write
side; the read change doesn't touch writing.

## R10. Times in this machine's zone

**Decision**: use the `jiff` crate (reads `/etc/localtime` or `TZ`, bundles tzdb fallback) to
show every instant in the machine's zone, formatted `2026-10-05 15:04:05`. Each page's header
shows "as of 15:04:05 CEST (Europe/Berlin)". The underlying RFC 3339 UTC value goes in the
`datetime` attribute of each `<time>` element. Tests compare those attributes with the CLI's UTC
values as instants (FR-017a).

**Alternative rejected**: hand-rolled offset arithmetic on top of `engine::clock`, which can't
handle daylight-saving changes. Rejected.

## R11. One consistent state per page

**Decision**: a page build takes one `engine.snapshot()` (one generation) and passes it to every
view it calls. File reads that aren't part of the snapshot (`accounts.toml`, `tokens.toml`,
`keys.toml`) are read once per build. Live answers come from `operator::handle` within the same
build. The "as of" time is taken when the snapshot is taken. A reload during a build doesn't
affect it, because the snapshot is an `Arc` (FR-018).

**For the same moment, in tests**: SC-001's tests start a server against a fixed test home with
no traffic. They build the page, run the CLI's `--json` against the same server, and compare.
Live values that change on their own (cooldown time left, an in-flight record's age) are compared
within the seconds elapsed between the two reads.

## R12. New CLI commands

The commands are `nullrouter unified [name]`, `nullrouter behaviour show`, and `nullrouter
dashboard token` / `dashboard status`. They're defined in `contracts/cli.md`. `unified` reuses
`resolve`'s member and note JSON for each unified model. `behaviour show` prints every
`[pipeline]` setting the operator can set, with its value and whether it is the default.

## R13. Model tests and Combos entries

**Decision**: both are entries in the navigation, under a "Not built yet" heading. Each opens a
page that says the feature isn't built yet. The Model tests page names `nullrouter model
<provider> <model>` and `nullrouter resolve <name>` as the nearest thing today. The Combos page
names `nullrouter resolve <name>` (a unified model's ordered members are the nearest thing to a
fallback chain) (FR-025).

## R14. Testing

- **Unit**: each view (moved code, existing tests move with it), HTML rendering of each view (every
  scalar leaf of the view's JSON appears in the page, escaped), and the token, cookie, Host and
  CSP checks.
- **Integration** (`crates/nullrouter-dashboard/tests/`): one test home with every status from
  User Stories 1–5. A real `serve`, the real CLI binary, and pages fetched with `reqwest`
  (SC-001, SC-002, SC-004, SC-008: no request leaves 127.0.0.1, checked by a mock that fails any
  outbound fetch).
- **Isolation** (SC-003): a load test sends 1,000 client requests to a mock provider while 4
  workers fetch pages in a loop, and while a fault-injection feature makes every page build panic
  or sleep. It compares p95 and failure counts with the dashboard disabled. This is a test, not a
  Criterion bench, because it measures interference, not a function. Criterion benches cover
  page rendering and `records_page_100k`.
- **Secrets sentinel** (SC-004): the 005 sentinel is extended to scan every page and response
  from the integration suite.
- **CI**: everything except the style-guide source test (which needs `ref/9router`) runs in
  GitHub Actions. That test runs locally and in cloud sessions.

## Brief rows touched

Rows 3, 4 (R3), 7 (R6, R8), 8, 15 (R4), 9 (R1, R4), 10 (R11), 11 (R8), 13 (R13), 14 (R2, R8),
18 (R6, R7). No answer above contradicts a confirmed row.
