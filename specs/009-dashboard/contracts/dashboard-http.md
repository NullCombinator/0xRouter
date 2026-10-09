# Contract: Dashboard HTTP surface

Served on `[dashboard] listen` (default `127.0.0.1:20130`), only while `nullrouter serve` runs.
Every route is `GET` except `POST /signin`. No route changes 0router's state (FR-013). There are
no scripts anywhere (FR-004).

## Every response

- Headers (research R8): `Content-Security-Policy: default-src 'none'; style-src 'self';
  font-src 'self'; img-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'`,
  `X-Frame-Options: DENY`, `Referrer-Policy: same-origin`, `X-Content-Type-Options: nosniff`,
  `Cross-Origin-Resource-Policy: same-origin`, `Cache-Control: no-store`. Assets and logos:
  `Cache-Control: public, max-age=86400, immutable` under a content-hashed path.
- A `Host` other than the dashboard's own (`127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>`,
  or the configured loopback host) → `421`, plain text, no page.
- Any method other than `GET`, except `POST /signin` → `405`.
- `lang="en"`, UTF-8.

## Access

| Request | No token issued | Token issued, no or wrong cookie | Valid cookie |
|---|---|---|---|
| `GET /assets/<hash>/<file>` | 200 | 200 | 200 |
| `GET /logos/<hash>/<id>.png` | 303 → `/signin` | 303 → `/signin` | 200 |
| `GET /signin` | 200: "No dashboard token yet. Run `nullrouter dashboard token`." | 200: the token form | 303 → `/` |
| `POST /signin` (`token=…`, form-encoded) | 200: the no-token page | right: 303 → `next` or `/`, `Set-Cookie`. Wrong: after the delay, 401 with the form and "That token is not the current one." | 303 → `/` |
| any other route | 303 → `/signin` | 303 → `/signin?next=<path and query>` | the page |

- `next` must be a path on this dashboard (starts with `/`, not `//`); anything else becomes `/`.
- `POST /signin` with an `Origin` that isn't this dashboard's → 403.
- `Set-Cookie: nr_dashboard=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000`.
- There is no sign-out route: signing out is issuing a new token or clearing the browser's data.
- Logos need the cookie, as pages do: a user plugin's logo shows which plugins this home loads
  (FR-008). The browser sends the SameSite=Strict cookie when a page loads its own images.
  Assets (style, font, icons) carry no 0router data and need no cookie.

## Frame (every page after sign-in)

- **Sidebar** (9router's, 288 px, translucent): "0Router Proxy" and the version
  `nullrouter --version` prints; Endpoint & Key, Providers, Combo, Usage, Quota Tracker; heading
  "System": Proxy Pools, Console Log, Settings. The current page is marked.
- **Header**: page icon, title, subtitle (from the mockup), and
  `as of 15:04:05 CEST (Europe/Berlin) · reload to refresh`.
- **Background**: the constant grid.
- **Housekeeping button**: round, bottom right, on every page; a link to the same page with
  `notices` added to the query (or removed, when open).
- **Housekeeping panel** (`?notices`): every `check` notice (`notices[].text`, grouped by level)
  and every account needing action (from `accounts`), then the chat box, disabled, "Not built
  yet." With none: "No notices."

## Routes

| Route | Views | Query |
|---|---|---|
| `/` | 303 → `/endpoint` | |
| `/endpoint` | `check` (endpoint, subject `endpoint`), `keys` | `notices` |
| `/providers` | `providers`, `accounts`, `plugins`, `check` (subject `providers`) | `q` (search), `notices` |
| `/providers/<id>` | as `/providers`, plus `model` for each of the provider's models; the window open | `kind`, `q`, `notices` |
| `/combo` | `check` (subject `combo`) | `notices` |
| `/usage` | `records` (`limit=50`, `before`), `check` (subject `usage`) | `before`, `notices` |
| `/usage/records/<id>` | as `/usage`, plus `record` (`records show <id>`); the window open | `before`, `notices` |
| `/quota` | `accounts`, `quota`, `routing`, `check` (subject `quota`) | `provider`, `account`, `order=expiring`, `notices` |
| `/proxy-pools`, `/console-log` | none | `notices` |
| `/settings` | `behaviour`, `check` (home, subject `settings`), `dashboard` | `notices` |

### Endpoint & Key

- "API Endpoint" card: `check.endpoint`, as selectable text. When `endpoint_source` is `config`,
  the card adds "(configured; no server running)", as `check` does. The copy button is not drawn
  (it needs a script).
- Slot: "Agent traffic" (the landscape), "Arrives with the next dashboard slice."
- Agent cards, one per key, as `keys list` shows them: name, id, `…last4`, created, revoked (with
  time), break behaviour (`default` when none), last used (`never` when none). Each card has a
  slot for "requests today". The "Add Agent" control is disabled with
  "In the CLI: `nullrouter keys issue <name>`".
- Side panel "Client adapters": "Client-side adapters are not built yet." No rows.
- Empty: "No agent keys. Run `nullrouter keys issue <name>`."

### Providers

- Disabled "Add Anthropic Compatible", "Add OpenAI Compatible", and "Test All" per section, with
  the hints in research R15.
- One section per `providers` category, named as 9router names it, and "Custom Providers" for the
  operator's own plugins. Each card: logo (`/logos/…`) or text icon (the provider's alias, as
  9router draws it), id, and its accounts: `N connections` with the state of any that are not
  active (`cooling`, `needs sign-in`, `disabled`), from `accounts`; `No connections` when none.
- Side panel "Provider plugins": "Bundled · N" and "Installed from community · N", each plugin
  with its source, schema, model count and state as `plugins list` shows it; then "Community · not
  installed: N available. In the CLI: `nullrouter plugins list --community`." Switches disabled.
- Window (`/providers/<id>`): the provider's accounts as `accounts list --long` shows them; the
  kind filter (one link per kind the plugins declare, with this provider's count; `All` first);
  the models of the chosen kind, each with what `model <provider> <model>` shows. Slot: "last
  response", for slice 2.

### Usage

- Slots: the period filter, the five stat cards, and the topology graph.
- "Recent Requests" beside the graph slot: the newest 10 of the same read, model, tokens in and
  out, and when.
- "Requests" table: "Newest first, 50 at a time · same as `nullrouter records list --limit 50`".
  Columns: When, Agent, Model → placed on, Why, TTFT, Total, Result, in the CLI's words.
  "Older" links to `?before=<last id>`. No total count.
- A row links to `/usage/records/<id>`: the window shows what `records show` shows: each attempt
  (account, outcome, reason, latency), the stay-warm decision, the changes, and usage.
- Empty: "No request records yet."

### Quota Tracker

- Filters: provider, account, "Expiring first" (`order=expiring`: windows sorted by reset time).
- One card per account: provider, name, priority, sign-in status and since when, cooldowns with
  time left, needs-sign-in with the command, email and tier where `accounts list --long` shows
  them; each quota window with quota left, source, last poll and reset; each target the account
  serves with pace, share and deficit (`routing`), and the amortization window in the header.
- Footer: "Showing N accounts · polled quota is read from the provider, estimated quota is counted
  from your requests."
- Empty: "No accounts. Run `nullrouter accounts add <provider> <name>`."

### Settings

- "Routing": `behaviour show`'s settings, each with "(default)" where it is.
- "Local mode": where the data lives (`check.home`).
- "Dashboard": `dashboard status` (on or off, address, serving or why not, token issued) and
  "Change it with `nullrouter dashboard token`."

### Combo, Console Log, Proxy Pools

The texts in research R15. Combo then lists its notices.

## Notices on pages

Each page shows the notices whose `subject` is its own, above its content, in `check`'s words,
styled by level.

## Errors

- A failed build → 500 with the frame and "This page could not be built: <reason>. Other pages
  and client requests are unaffected."
- Busy (2 builds running, a third waited 5 s) → 503, "Busy; reload."
- A record or provider id that doesn't exist → 404 inside the frame, with the CLI's message.

## Times

Every instant is a `<time datetime="<RFC 3339 UTC>">` showing local time. Durations are shown as
the CLI shows them.

## Never on a page

Secrets beyond the last four characters the CLI shows, OAuth client secrets, the dashboard token,
prompt or response text (FR-043, FR-044).
