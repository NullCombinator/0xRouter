# Contract: Dashboard HTTP surface

Served on `[dashboard] listen` (default `127.0.0.1:20130`), only while `nullrouter serve` runs.
Every route is `GET` except `POST /signin`. No route changes 0router's state (FR-010).

## Every response

- Headers (R7): `Content-Security-Policy: default-src 'none'; style-src 'self'; font-src 'self';
  img-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'`,
  `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer`, `X-Content-Type-Options: nosniff`,
  `Cache-Control: no-store` (assets: `public, max-age=86400, immutable`, versioned by a content
  hash in the path), `Cross-Origin-Resource-Policy: same-origin`.
- `Host` other than `127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>` (or the configured
  loopback host) → `421 Misdirected Request`, plain text, no page.
- `lang="en"`, UTF-8. No `<script>` element anywhere.

## Access

| Request | No token issued | Token issued, no or wrong cookie | Valid cookie |
|---|---|---|---|
| `GET /assets/<hash>/<file>` | 200 | 200 | 200 |
| `GET /signin` | 200: "no token yet; run `nullrouter dashboard token`" | 200: token form | 303 → `/` |
| `POST /signin` (`token=…`, form-encoded) | 200: no-token page | right: 303 → `next` or `/`, `Set-Cookie`. Wrong: after the delay, 401 with the form and "That token is not the current one." | 303 → `/` |
| any other route | 303 → `/signin` | 303 → `/signin?next=<path>` | the page |

- `next` must be a path on this dashboard (starting with `/`, not `//`). Anything else is
  replaced with `/`.
- `POST /signin` with an `Origin` header that is not this dashboard's → 403.
- `Set-Cookie: nr_dashboard=<token>; Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000`.
- `POST /signout` is deliberately absent: signing out means issuing a new token or clearing the
  browser's cookies. (This is a view-only surface; a sign-out form would be its only other write.)

## Pages

Every page has the shared frame: the navigation (four pages, then "Not built yet": Model tests,
Combos), the page title, and the "as of" line (`as of 2026-10-05 15:04:05 CEST, Europe/Berlin`).
Page bodies render the view values from R1. Each fact keeps the CLI's words (FR-015).

| Route | Views | Query parameters |
|---|---|---|
| `/` → 303 `/accounts` | | |
| `/accounts` | `accounts`, `quota`, `check` (account-subject items) | `provider` |
| `/routing` | `routing`, `records` (page 1), `check` (journal and routing warnings) | `target`, and the record filters below |
| `/records` | `records` | `provider`, `account`, `agent`, `model`, `reason`, `since`, `before` (cursor: a record id) |
| `/records/<id>` | `record` | |
| `/models` | `providers`, `plugins`, `unified`, `check` (load-report items) | `capability` |
| `/models/<provider>/<model…>` | `model` | |
| `/keys` | `keys`, `behaviour`, `check` (`keys.toml`/`dashboard.toml` modes) | |
| `/not-built/model-tests`, `/not-built/combos` | none | |

- **Paging**: `/records` shows 50 records, newest first. "Older" links to
  `?before=<last id>` with the same filters. There is no total count, because counting would
  read everything.
- **Filters**: the same names and value forms as `records list` (a `since` date or RFC 3339
  time, UTC, as the CLI takes it). A filter value the CLI would refuse is shown as the CLI's
  error message, above an empty list.
- **Empty states** (FR-027): each page with nothing to show names the CLI command that adds it
  (`nullrouter accounts add …`, `nullrouter keys issue …`, `[[unified_model]]` in `config.toml`).
- **Errors**: a failed build → 500 with the frame and "This page could not be built: <reason>.
  Other pages and client requests are unaffected." No more than 2 builds at once. A third waits
  up to 5 s, then gets 503 "busy, reload".

## Times

Every instant is a `<time datetime="<RFC 3339 UTC>">` showing the machine's local time
(FR-017a). Durations (cooldown left, latency) are shown as the CLI shows them.

## Never on a page

Secrets beyond the last four characters the CLI shows, OAuth client secrets, the dashboard
token, prompt or response text (FR-028, FR-029).
