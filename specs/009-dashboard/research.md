# Research: Dashboard (slice 009)

Decisions taken in planning. Each one lists what was chosen, why, and what was rejected. The
brief's ledger (`specs/briefs/2026-10-05-dashboard.md`) is the authority. Nothing here
contradicts a confirmed row; where a decision touches one, the row is named.

Spec 007 (`specs/briefs/superseded/007-dashboard/research.md`) is the starting point. Its R2,
R3, R5 to R8, R10 and R11 carry over, adjusted where noted. Its R1, R9 and R12 were delivered by
slice 008 (the read model: `nullrouter_server::views`, `fetch_in_process`, the newest-first
records read with the segment index). Its R4 and R13 are replaced below.

## R1. Pages render the read model's views; nothing is computed twice

**Decision**: Every fact on a page comes from a `views::*` value built by the same function the
CLI's `--json` prints, fetched in process with `views::run_in_process` (slice 008). A page
module only arranges and formats those values. Page → views:

| Page | Views (CLI twin) |
|---|---|
| Endpoint & Key | `keys` (`keys list`), `check` (endpoint URL, notices with subject `endpoint`) |
| Providers | `providers`, `model` per model, `accounts` (`accounts list --long`), `plugins` (`plugins list --community`), `check` (subject `providers`) |
| Quota Tracker | `accounts`, `quota`, `routing`, `check` (subject `quota`) |
| Usage | `records` (`records list --limit 50 [--before]`), `record` (`records show`), `check` (subject `usage`) |
| Settings | `behaviour` (`behaviour show`), `check` (home, subject `settings`), `dashboard` (`dashboard status`) |
| Combo | `check` (subject `combo`) |
| Console Log, Proxy Pools | none |
| Housekeeping panel (every page) | `check` (all notices), `accounts` (accounts needing action) |

**Rationale**: The slice fails if a page disagrees with the CLI (brief row 9). One builder makes
agreement structural; the CLI-agreement tests then cover transport and HTML rendering.

**Alternative rejected**: page-specific queries straight into the engine. Every one would be a
second rendering of a fact that can drift.

## R2. One new crate, `nullrouter-dashboard` (carried from 007 R2)

**Decision**: `crates/nullrouter-dashboard` depends on `nullrouter-server` (views, operator
handle) and `nullrouter-engine`. `nullrouter serve` starts it beside the client listener and the
operator socket. Dependency order: cli → dashboard → server → engine → registry.

**Rationale**: HTML, cookies, fonts, icons and CSS stay out of the client request surface and its
build.

## R3. Server-rendered HTML with `maud`, no scripts (carried from 007 R3)

**Decision**: `maud` (compile-time templates, escaping by default). No `<script>` anywhere, and the
Content-Security-Policy has no `script-src`, so an injected script would not run. Narrowing and
paging are `GET` forms and links. The token prompt is a `POST` form. (Brief row 3; FR-004.)

**Alternatives rejected**: `askama` (works, but splits markup from the Rust that feeds it),
`minijinja` (runtime templates: a typo becomes a page error), Leptos or Dioxus with hydration
(ships code to the browser).

## R4. Windows, panels and filters without scripts

**Decision**:
- **Windows** (a provider's detail, a record's detail) are addressed: `/providers/<id>` and
  `/usage/records/<id>` render the underlying page with the window open on top of it, styled as
  9router's modal. Its close button is a link back to the page with the same query. A reload keeps
  the window open, and the back button closes it.
- **The housekeeping panel** is addressed the same way: `?notices` on any page renders the page
  with the panel open; the round button is a link that toggles it.
- **Side panels** (Client adapters, Provider plugins) are always rendered. Their collapse chevron
  is a `<details>` element, which needs no script.
- **Filters**: the Providers search, the kind filter, Quota Tracker's provider and account
  filters, and "Expiring first" are `GET` forms whose values become query parameters. The kind
  filter is a list of links, one per kind, each with its count.

**Rationale**: The spec's assumption (each window has an address) gives reload and back-button
behaviour for free, and keeps every window's content built only when it is open: a record window
reads one record, not fifty.

**Alternatives rejected**:
- CSS `:target` windows rendered inline for every row: each Usage page would build 50
  `records show` values that the operator never opens.
- `<details>` for windows: no backdrop, and it can't be closed by a link.

## R5. A `subject` and the exact line for every `check` notice (carries 007 T011; FR-024)

**Decision**: `views::check` gains a `notices` array. Each entry is
`{"level": "error"|"warning"|"note", "subject": <page>, "text": <the line check prints>}`, in the
order `check` prints them. The line building moves from `nullrouter-cli/src/cmd/check.rs` into the
view, and the CLI's text output prints `notices[].text` after its header lines, so its output is
unchanged byte for byte. Existing JSON fields stay.

| Notice (`check` line) | Level | Subject |
|---|---|---|
| `conflict pending: …`, `declined: …`, `credential WITHHELD: …` | warning | `providers` |
| `skipped: …` (with its indented errors) | error | `providers` |
| `note: logo ignored: …` (R10; its own `logos_ignored` list, printed right after the `skipped:` lines) | note | `providers` |
| `dropped unified model …`, `note: unified model … members differ …` (limits notes) | error / note | `combo` |
| `warning: records not kept since …` | warning | `usage` |
| `note: … reports window …` (unmetered), `warning:` routing warnings, sign-in `error:` lines, tokens without an account, accounts without tokens | as printed | `quota` |
| file mode of `accounts.toml`, `tokens.toml`, quota and routing files | as printed | `quota` |
| file mode of `keys.toml` | as printed | `endpoint` |
| file mode of `config.toml`, `dashboard.toml`, any other file; `warning: dashboard not listening …` | as printed | `settings` |

The subjects are page ids: `endpoint`, `providers`, `combo`, `usage`, `quota`, `settings`. A
notice whose subject isn't decided by the table goes to `settings` (FR-024's last sentence). A
test lists every `println!` the old `check.rs` had and fails if a line kind has no row here.

**Rationale**: one place builds the words, so the panel, the page and `check` can't disagree.

**Alternative rejected**: a `subject` field added to each object in the existing lists. The
string-only lists (`withheld`, `notes`, `routing_warnings`) have no object to put it on, and the
dashboard would still need the CLI's line-building to get the words.

## R6. Endpoint URL and "where the data lives": both in `check` (FR-020)

**Finding**: no CLI read shows the client endpoint today. `check` shows the home.

**Decision**: `check` gains `"endpoint": "http://127.0.0.1:20129/v1"` and the text line
`endpoint: http://127.0.0.1:20129/v1` after `home:`. With a server running it is the address the
server actually listens on (a new `server.status` operator op, R8; `serve --listen` can differ
from the file). Without a server it is `config.toml [server] listen`, followed by
` (configured; no server running)`. An unspecified host (`0.0.0.0`, `::`) is shown as
`127.0.0.1` (`[::1]` for `::`), because the dashboard is read on this machine. The base path is
`/v1`, which the Chat Completions, Messages and Responses styles share. The Gemini style's
`/v1beta` is not shown, because the mockup shows one URL.

**Rationale**: `check` is an existing read (the spec says to add the fact to one), already asks
the running server for live facts, and already shows the home.

**Alternative rejected**: put it in `dashboard status` only. That is a new command, and the
endpoint isn't a dashboard fact.

## R7. Dashboard token: digest stored, cookie carried (carried from 007 R6)

**Decision**:
- `nullrouter dashboard token` makes 32 random bytes, prints `nrd_<base64url>` once, and stores
  only the SHA-256 digest and the issue time in `dashboard.toml` (0600, atomic write).
- `POST /signin` compares digests in constant time (`subtle`) and sets
  `nr_dashboard=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000` (400 days, the
  browser cap: "asked once", brief row 4).
- Every request rehashes the cookie and compares it with the current digest from the engine
  snapshot. A new token signs every browser out on its next load. No session table.
- Wrong tokens: a global delay, 1 s doubling to 30 s, reset by a success (FR-010).

**Residual risk, still open (security Low L1, raised in 007 and not yet judged by the user)**:
browsers scope cookies by host, not port, so another web server on `127.0.0.1` that the browser
visits receives the cookie. A process of the same OS user can already read `~/.0router`; a
process of another OS user could replay it. Mitigation: `docs/operator-config.md` documents it
and `[dashboard] listen = "127.0.0.2:20130"` on Linux, where every `127/8` address is loopback
and the cookie host differs. The user decides whether that is acceptable or whether the
dashboard should move the default to `127.0.0.2` on Linux.

**Alternatives rejected**: HTTP Basic auth (browsers forget it at restart: breaks "asked once"),
a session table (state for no gain), the token in the URL (leaks through history).

## R8. Bound to this machine, isolated from client traffic (carried from 007 R7, R8)

**Decision**:
- `config.toml [dashboard] enabled = true, listen = "127.0.0.1:20130"`. A non-loopback `listen` is
  a load error naming the rule (FR-002).
- Its own `TcpListener`, task and axum router. A bind failure is logged and kept; `serve` keeps
  serving clients. A new operator op, `server.status`, answers
  `{client_listen, dashboard: {enabled, listen, serving, error}}` for `check` (R6) and
  `dashboard status` (contracts/cli.md).
- Bounded work: at most 2 page builds at once (others wait up to 5 s, then 503), each with a 10 s
  timeout, each in its own task so a panic is that page's 500. Reads run on the blocking pool
  (`run_in_process` already does). No engine lock is held across an await; the snapshot is an
  `ArcSwap` load.
- Headers on every response: the CSP (`default-src 'none'; style-src 'self'; font-src 'self';
  img-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'`),
  `X-Frame-Options: DENY`, `Referrer-Policy: same-origin`, `X-Content-Type-Options: nosniff`,
  `Cache-Control: no-store` (assets: immutable, content-hashed paths),
  `Cross-Origin-Resource-Policy: same-origin`. A `Host` that isn't the dashboard's loopback
  address and port gets 421 (DNS rebinding). `POST /signin` needs `Origin` absent or its own.
  Amended by the security review (security-review.md H1): the policy was `no-referrer`, under
  which a browser's form post sends `Origin: null`, so no browser could sign in.

## R9. "Last used" on keys: from the journal, through the server when it runs (FR-029a)

**Definition** (spec): the arrival time of the newest record whose agent is the key's id, or
`never`.

**Decision**:
- The segment index (`journal::index`, slice 008) gains, per segment, the newest arrival per
  agent: one more field read from each `open` line it already parses.
- A new operator op, `keys.last_used`, answers `{"<key id>": "<RFC 3339>" | null}` from the
  server's cached index: segments newest first, stopping once every key is found or the oldest
  segment is read.
- `views::keys` takes that answer when a server runs. Without one, it reads the journal the same
  way in its own process, so `keys list` with no server costs one pass over the segments it needs.
- A Criterion bench, `keys_last_used_100k`, measures the cold read (no server, one key never
  used) and the warm one (server index) on a 100,000-record journal. Target: warm under 5 ms,
  cold under 1 s.

**Rationale**: one definition, two transports, like every other view. The server's index is
already in memory and kept current, so the page costs almost nothing. The cold CLI path costs no
more than `records list` with a filter that matches nothing.

**Alternatives rejected**:
- An in-memory "last used" map updated by the request path: it is lost at restart and would
  disagree with the records after `records forget` or `prune`.
- A new persisted file: a second record of something the journal already records.

## R10. Plugin logos (FR-040 to FR-042, FR-040a)

**Decision**:
- **Schema**: an optional top-level `logo = "<file>.png"` in a schema 2 plugin. The value is a
  bare file name (no `/`, `\`, or `..`; else a validation error on the field). The file is looked
  up in the `logos/` directory beside the plugin: `plugins/bundled/logos/`,
  `plugins/community/logos/` (both embedded at build by `nullrouter-registry/build.rs`, as the
  TOML files are), and `<home>/plugins/logos/` for user and installed plugins. `plugins install`
  copies a community plugin's logo there, and `plugins uninstall` removes it.
- **Check at load** (the core reads; the plugin does nothing): at most 64 KiB; the PNG signature;
  an `IHDR` chunk first, with width and height each 1 to 256. No decoding, so no image library
  in the core. A failure keeps the plugin loaded and adds an entry to the load report's own
  `logos_ignored` list (not the typed limits `notes`): `{id, reason}`, with reasons
  `file not found: logos/<file>`, `not a PNG`, `<n> KiB, over 64 KiB` and
  `<w> × <h> px, over 256 px` (contracts/plugin-logo.md). `check` prints each as
  `note: logo ignored: <plugin id>: <reason>` right after the `skipped:` lines, with subject
  `providers`, and its JSON gains `logos_ignored`.
- **Serving**: `GET /logos/<content hash>/<provider id>.png`, behind the dashboard cookie like a
  page (FR-008: a user plugin's logo shows which plugins this home loads), from the registry
  snapshot's bytes (kept in memory,
  at most 64 KiB each), `Content-Type: image/png`, `nosniff`, and the CSP's `img-src 'self'`. No
  SVG is accepted (FR-042).
- **Shipping (FR-040a)**: the generator (`tools/gen-bundled/generate.mjs`) copies
  `ref/9router/public/providers/<id>.png` to `plugins/{bundled,community}/logos/<id>.png` and
  writes `logo = "<id>.png"` into the generated community plugins. The seven hand-maintained
  bundled plugins get the line by hand (six; `opencode-zen` has no logo). Of 121 plugins, 119 have a file of the same id in 9router
  (`opencode-zen` and `ollama-search` don't, so they show text icons). Five files fail the check
  today: `crush.png` (2700 × 1392, 790 KB), `nebius.png`, `reka.png` and `siliconflow.png` (JPEGs
  named `.png`), and `kimchi.svg` (SVG). They are converted once to PNGs within the limits with
  ImageMagick and committed as overrides in `tools/gen-bundled/seeds/logos/`. The generator uses
  an override when one exists and stops with an error when a copied logo would fail the check.
- **Licence**: 9router's code is MIT. `plugins/LOGOS.md` records each logo's source path and
  that it names its provider only. Provider marks belong to their owners.

**Rationale**: the plugin stays data. The core does the reading and checking. A bad logo can't
stop a plugin. The generator stays dependency-free (no image library in Node).

**Alternatives rejected**: decode and re-encode every logo in the core (an image library on the
load path, for a check the header already answers); a `logo` path relative to anywhere (path
traversal surface); base64 inside the TOML (bloats the plugin and the validation gate refuses long
opaque strings by design).

## R11. Fonts and icons served from the binary (007 R5, extended)

**Decision**: Inter (SIL OFL 1.1), one variable woff2, Latin subset, about 100 KB. Icons: a fixed
set of Material Symbols Outlined glyphs (Apache 2.0), embedded as inline SVG paths, not the
3.8 MB icon font the mockups load. The set is every icon the mockups use (about 45: `api`, `dns`,
`layers`, `bar_chart`, `data_usage`, `lan`, `terminal`, `settings`, `schedule`, `search`,
`expand_more`, `extension`, `close`, `lock`, `key`, `smart_toy`, `history`, `route`, `folder`,
`monitor`, `chat`, `hub`, …), kept in `crates/nullrouter-dashboard/assets/icons/*.svg`, with a
test that every icon a page names exists. Both licence texts go in `assets/LICENSES/`.

**Rationale**: nothing is fetched from outside the machine (FR-015, SC-010), and a 45-icon set is
a few KB.

## R12. The style guide (replaces 007 R4)

**Decision**: `docs/dashboard/style-guide.md` (the draft written for 007) becomes the guide for
this slice, and `crates/nullrouter-dashboard/style/tokens.toml` its machine-readable half, as
007's contract described: each token has a `value` and a `source` (`ref/9router` file and line,
and the Tailwind class when it came from one). `dashboard.css` uses only `var(--token)`,
keywords, `0`, and percentages in `width`/`flex` layout; `tokens.css` is generated from `tokens.toml` and tested. Light theme only.

What changes from 007:
- The guide's "Not taken: page structure, navigation entries" is removed. The sidebar (the
  translucent 288 px `.bg-vibrancy` panel), its entries and grouping ("System"), the page header,
  the modal, and the side panel are taken from 9router (brief rows 5, 6).
- The constant background grid (`.landing-grid`, `globals.css` about lines 464 to 471) is in, on
  every page (brief row 5). It is no longer "open".
- New components to extract: sidebar and its entries, page header with the "as of" line, modal
  window, side panel, round floating button and its panel, provider card, quota card, key card,
  slot.
- A **slot** has no 9router precedent. It is built only from existing tokens: the card style with
  a dashed border in `border`, muted text, and the label "Arrives with the next dashboard slice".

**Tests** (SC-007): the three from 007 (CSS uses only tokens; every source line exists in
`ref/9router` and holds the value; `tokens.css` matches). The source test runs where
`ref/9router` is present (locally and in cloud sessions), not in GitHub Actions.

**SC-008** is the user's side-by-side judgement against 9router in light mode and the mockups
(`quickstart.md` step 8).

## R13. Times in this machine's zone (carried from 007 R10)

`jiff` shows each instant in the machine's zone; the header reads
`as of 15:04:05 CEST (Europe/Berlin) · reload to refresh`. Each `<time>` carries the CLI's
RFC 3339 UTC value in `datetime`, and the tests compare instants (FR-023).

## R14. One consistent state per page (carried from 007 R11)

A page build takes one engine snapshot and passes it to every view; the home files are read once
per build; the "as of" time is the snapshot's. In tests, "the same moment" means a fixed test
home with no traffic; values that change by themselves (cooldown left) are compared within the
seconds between the two reads.

## R15. Not-built entries and disabled controls (replaces 007 R13; FR-014, FR-037)

| Entry or control | What it says |
|---|---|
| Combo | "Combos are not built yet. Declare unified models today with `[[unified_model]]` in config.toml and list them with `nullrouter unified`." Then the `combo` notices. |
| Console Log | "The console log is not built yet. Request records are on the Usage page." (link) |
| Proxy Pools | "Proxy pools are not built yet. 0router has no proxy pool feature today." |
| Add Anthropic / OpenAI Compatible | "Not built yet. Add a provider with a plugin file; see docs/plugins.md." |
| Test All | "Not built yet: model tests come with their own slice." |
| plugin enable, disable, hide | "Not built yet. Every loaded plugin is active." |
| plugin install, uninstall | "In the CLI: `nullrouter plugins install <id>`" / `uninstall <id>` |
| Add Agent | "In the CLI: `nullrouter keys issue <name>`" |
| housekeeping chat box | "Not built yet." |

A disabled control is a `<button disabled>` (or a non-link `<span>`) with the hint beside it, so
there is nothing to submit. The access test checks that no route except `POST /signin` accepts a
non-`GET` method.

## R16. Testing

- **Unit**: page rendering per view (every scalar leaf of the view JSON that the page shows appears
  escaped), `check` notices and subjects, the logo check, the token, cookie, Host and CSP checks.
- **Integration** (`crates/nullrouter-dashboard/tests/`): one fixture home that triggers every
  status in User Stories 2 to 6 and every `check` notice kind; a real `serve`, the real CLI, and
  pages fetched with `reqwest`. Covers SC-001 to SC-004, SC-006 (the secrets sentinel from slice
  005, extended to every response), SC-010 (a mock that fails any outbound fetch), SC-011.
- **Twin test** (SC-002): each page module declares the views it reads; a test fails if a page
  renders a value not found in those views' JSON.
- **Isolation** (SC-005): 1,000 client requests against a mock provider while 4 workers load pages
  and a fault-injection feature makes builds panic or sleep; p95 and failures compared with the
  dashboard off. Opt-in (`--ignored`), local.
- **Benches** (local only): `pages` (each page on a home with 50 accounts, 20 unified models and
  100,000 records; SC-009) and `keys_last_used_100k` (R9).
- **CI**: everything except the style-source test and the benches runs in GitHub Actions.

## Brief rows touched

Rows 2 and 3 (R3, R4, R15), 4 (R7, R8), 5 and 6 (R11, R12), 7 (R15), 9 (R1, R5, R6, R9), 10
(R13, R14), 11 and 12 (R4, R15), 13, 14 and 18 (R4, R15), 15 (R10), 16 (R4), 17 (R4, R15), 20
(R12 slots), 21 (R7, R8, R10), 22 (R5, R7, R8). No decision above contradicts a confirmed row.
