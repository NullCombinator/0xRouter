# Parity audit: slice 005 (T094)

Audited 2026-10-03 against `ref/9router` at `39e36d3`, using the `/rust-parity-audit` protocol.
The request path is `chat.js:253` `checkAndRefreshToken` → `chatCore.js:420-462` (refresh on
401/403, then one retry). The background path is `backgroundTokenRefresh.js`. The sign-in path is
the dashboard's `api/oauth/[provider]/[action]` and `OAuthModal.js`. The quota path is the
dashboard's `api/usage/[connectionId]` → `usage.js` `getUsageForProvider`, which is not on the
request path: per the code-review-graph, only that route and unit tests call it. Accepted
deviations are R19 (plus R8–R11 and Clarifications Q1, Q6) and the deviations asserted in
`tests/parity_oauth.rs`, `tests/parity_usage.rs` and `tests/no_cloaking.rs`. None of them is
repeated below.

| Rust | 9router |
|---|---|
| `crates/nullrouter-engine/src/signin/mod.rs`, `pkce.rs`, `loopback.rs` | `src/lib/oauth/providers/{claude,xai}.js`, `services/xai.js` (discovery), `utils/pkce.js`, `utils/server.js`, `providerHelpers.js` (JWT email) |
| `signin/device_code.rs` | `src/lib/oauth/providers/grok-cli.js`, `api/oauth/[provider]/[action]/route.js:520-557`, `OAuthModal.js:160-190` |
| `signin/refresh.rs`, `classify.rs`, `dedup.rs` | `open-sse/services/tokenRefresh.js`, `tokenRefresh/{providers,dedup}.js`, `oauthCredentialManager.js`, `src/sse/services/{tokenRefresh,backgroundTokenRefresh}.js` |
| `quota/extract.rs`, `grpc_web.rs`, `mod.rs` | `open-sse/services/usage/{claude,grok-cli,grokCliQuotaFrame,opencode-go,opencode-zen,shared}.js`; evidence: `tests/parity_usage.rs` against the generated oracle |
| `quota/poll.rs` | `open-sse/services/usage.js`, `src/app/api/usage/[connectionId]/route.js` |
| `quota/history.rs`, `tally.rs` | none: 9router keeps no poll history or traffic tally. Checked against R15 only |

## Checked and matching

- PKCE: verifier of 32 bytes (anthropic) or 96 bytes (xai), base64url; S256 challenge; 32-byte
  state. The xai authorize URL carries the same parameters plus `nonce` (16 random bytes, hex),
  `plan` and `referrer`, encoded like `encodeURIComponent`.
- Code exchange: anthropic sends a JSON body with `code`, `state`, `grant_type`, `client_id`,
  `redirect_uri`, `code_verifier`, and splits a pasted `code#state`. xai sends a form body with
  the same fields minus `state`. A token response without `refresh_token` keeps the old one
  (`providers.js:131`).
- Device grant: `client_id`, `scope`, `referrer=grok-build`; the poll sends `grant_type`,
  `device_code`, `client_id`. `authorization_pending` and `slow_down` keep waiting;
  `expired_token` and `access_denied` end the sign-in.
- Discovery: when either discovered endpoint is unusable, both declared URLs are used, as in
  `services/xai.js:52-76`.
- Refresh: the token URL, body encoding and exactly `grant_type`, `refresh_token`, `client_id`
  match. grok-cli refreshes like xai.
- Background lead: `max(plugin lead, 30 min)`. The half-lifetime ceiling and the default hour
  for a missing `expires_in` are R9 and the `parity_oauth.rs` note.
- Dedup: one in-flight refresh shared by every waiter. The per-account key and the absence of
  the 10 s result cache are documented in `dedup.rs`. A late waiter sees the new token in the
  cell.
- JWT payload: read only with exactly three parts, and never verified.
- gRPC-web decoder: frames, trailer bit, bare-message fallback, fixed32/fixed64 ratio
  (absent = 0), `Timestamp` reset and the 100 cap all match; 16 frames agree with the oracle.
- `parse_reset` (`auto`): seconds below 1e12, else milliseconds, also for numeric strings;
  `0`, `null` and `""` give no reset.
- grok-cli billing windows, alternatives and order, including first-bag-wins for credits; the
  claude `five_hour`, `seven_day`, `seven_day_*` and `limits[]` windows; opencode `rolling`,
  `weekly` and `monthly` all match, bar the listed deviations.
- R15: the history line format (`v`, `at`, `ok`, `windows`, per-model `tally`), mode 0600,
  the 10 s tally checkpoint, and "input without cache" in the tally all match.

## Findings

| # | File | Finding | Severity | Recommendation |
|---|---|---|---|---|
| 1 | `signin/mod.rs:96-114`, `device_code.rs:75`, `registry/src/schema/signin.rs:130` ↔ `providers/grok-cli.js:174-178, 193-196` | 9router's grok-cli device request and token polls send `User-Agent: grok-pager/0.2.93 grok-shell/0.2.93 (linux; x86_64)`. 0router's discovery, device, token and refresh calls send no `User-Agent` at all: reqwest sets none, and `upstream::client` adds none. `[signin]` has no `headers` field, so a plugin can't declare one. If `auth.x.ai` refuses UA-less calls, fixing it takes a code change, not a data change. | Medium | Add an optional fixed-value `[signin] headers` table, gate-checked like `[signin.profile] headers`, sent on discovery, device, token and refresh calls. Declare grok-cli's UA there, and cover it in live check L4. |
| 2 | `plugins/bundled/grok-cli.toml:33`, `signin/mod.rs:268-280` ↔ `providers/grok-cli.js:240-244` | grok-cli email order: 0router reads the profile `email` first, then the id token. 9router reads the id token first (`email`, `preferred_username`), then the access token's JWT claims, then the profile. The access-token fallback is absent; only the `sub` fallback is listed in R19. This feeds `x-email` on grok-cli requests. | Low | Reorder to `id_token.email \| id_token.preferred_username \| email` (data only). Record the access-token fallback as not ported. |
| 3 | `grok-cli.toml:32` ↔ `providers/grok-cli.js:226-230` | 9router's profile read also sends `User-Agent` and `x-grok-client-version`. | Low | Declare both in `[signin.profile] headers` (data only). |
| 4 | `device_code.rs:127, 136` ↔ `OAuthModal.js:178-184` | 9router caps `slow_down` at a 30 s interval and keeps polling on any error other than `expired_token`/`access_denied`. 0router adds 5 s without a cap (RFC 8628), and ends the sign-in on any non-transient rejection or a non-JSON 2xx. | Low | Accept (RFC behaviour), or record it in R19. |
| 5 | `loopback.rs:99` ↔ `utils/server.js:26` | 9router answers both `/callback` and `/auth/callback`; 0router answers only the declared path. | Low | Accept. |
| 6 | `signin/mod.rs:336-360`, `refresh.rs:115` ↔ `services/xai.js:52-76, 155-156` | 9router caches discovery for the process and refreshes at the discovered token URL. 0router fetches discovery at each sign-in and refreshes at the declared `token_url`. The two URLs are the same today. | Low | Accept. |
| 7 | `signin/mod.rs:200-203` ↔ `tokenRefresh/providers.js:21-26` | 0router also reads an object `error.code`/`error.type`. 9router's xai refresh matches `invalid_grant`/`invalid_request` anywhere in the error text. Either way, a 400/401/403 is permanent in 0router (R10). | Low | Accept. |
| 8 | `refresh.rs:47, 88-91` ↔ `chat.js:253` → `oauthCredentialManager.js:45-50` | Use-time refresh fires within 30 s of expiry. 9router's request path refreshes within the provider lead: 5 min for xai and grok-cli, 4 h for claude. R9 states 0router's rule, but R19's table doesn't list it. | Low | Add a row to R19. |
| 9 | `refresh.rs:250-258` (called from `attempt.rs:839-843`) ↔ `chatCore.js:421-434` | On rejection, 9router refreshes on 401 and on *any* 403, with up to 3 refresh attempts (`refreshWithRetry`, 1 s and 2 s apart). 0router makes one deduplicated refresh, and on a 403 only when the auth-rejection rule matches. R9 states 0router's rule, but R19 doesn't list it. | Low | Add a row to R19. |
| 10 | `quota/grpc_web.rs:29-30` ↔ `usage/grok-cli.js:334` | The gRPC fallback's `used` is rounded to two decimals; 9router rounds to an integer. The oracle's only case is 35, so the test doesn't show the difference. | Low | Keep it (closer to SC-006) and record it next to the `parity_usage.rs` deviations. |
| 11 | `quota/poll.rs:377` ↔ `usage/grok-cli.js:371-373`, `api/usage/[connectionId]/route.js:175-183` | A poll refreshes the token only on 401. 9router's usage route also force-refreshes on a grok-cli 403 ("authentication expired"). | Low | Accept, or refresh on 403 for sign-in polls too. |
| 12 | `quota/poll.rs:426-445` ↔ `usage/grok-cli.js:311-317`, `usage/claude.js:69-73` | `[identity]` headers go on every quota source: anthropic usage (`User-Agent`, `X-App`), the grok-cli billing read (session, request and agent ids), and the gRPC fallback to `grok.com`. 9router sends only the auth and content headers to `grok.com` and claude usage. Q3 permits copying the identity, but the deviation isn't recorded. | Low | Record it, or apply `[identity]` only to the primary quota source. |
| 13 | `plugins/bundled/opencode-{go,zen}.toml` `[quota].request` ↔ `usage/opencode-go.js:38-40` | 9router sends `Accept: application/json`; 0router's opencode polls don't. | Low | Declare it (data only). |
| 14 | `quota/extract.rs:204-208` ↔ `usage/grok-cli.js:272-279` | A credits bag with only `remaining > 0` (no total) gives a remaining-only window; 9router draws "0 used of <remaining>". Only the `balance: 0` case is listed. | Low | Record it with the `credits-bag-balance-only` deviation. |
| 15 | `quota/extract.rs:74-77` ↔ `usage/claude.js:106-126` | When `seven_day_<m>` and a `limits[]` entry name the same model, 0router keeps the first (`seven_day_*`); 9router's `limits[]` entry overwrites it. No fixture covers this. | Low | Accept, or move the `limits[*]` rule first in `anthropic.toml`. |
| 16 | `quota/extract.rs:281-289` ↔ `usage/shared.js:35` | A reset string that is neither RFC 3339 nor `YYYY-MM-DD` (e.g. an HTTP date) gives no reset; 9router's `new Date(s)` parses it. | Low | Accept. |

### Against research.md

- R11's grok-cli row lists `GET …/v1/user?include=subscription` as a quota source. The bundled
  plugin never sends it. In 9router that read only names the plan and sets `subscriptionAccess`,
  which feeds the synthetic on-demand bar that R19 already drops. Fix R11's text.
- R9 describes the dedup as a `tokio::sync::Mutex<Option<Shared<…>>>` per account. The code
  uses one `std::sync::Mutex<HashMap<(provider, name), Shared<…>>>` with the work spawned as its
  own task. The behaviour is the same (and a dropped waiter can't cancel the refresh); only the
  text is stale.

No Critical or High findings. Finding 1 is Medium: before L4, add a sign-in headers field, or
confirm live that `auth.x.ai` accepts UA-less calls.

## Resolution (2026-10-03)

| # | Status | Test or record |
|---|---|---|
| 1 (Medium) | Fixed. Optional fixed-value `[signin] headers` (schema and gate, same rules as `[signin.profile] headers`), sent on discovery, device, token and refresh calls. grok-cli declares `User-Agent: grok-pager/0.2.93 grok-shell/0.2.93 (linux; x86_64)`. On refresh this is a recorded deviation (9router sends none). Live check L4 still to confirm `auth.x.ai`'s view. | `parity_oauth::grok_cli_device_grant_sends_9routers_bodies`, `parity_oauth::refresh_call_rotates_like_9router`, `parity_oauth::bundled_signin_declarations_match_9router`, `gate::signin_gate_rules`; R19 row |
| 2 | Kept and recorded: profile-first email; the access-token JWT fallback is not ported. | R19 row |
| 3 | Fixed (data): grok-cli's profile read sends 9router's `User-Agent` and `x-grok-client-version`. | `parity_oauth::grok_cli_device_grant_sends_9routers_bodies` (profile headers against the oracle) |
| 4, 5, 6, 7 | Accepted, as recommended. | — |
| 8 | Recorded. | R19 row (30 s use-time margin) |
| 9 | Recorded. | R19 row (one refresh on 401 or a matching 403) |
| 10 | Kept and recorded. | R19 row (two decimals) |
| 11 | Accepted: a poll refreshes on 401 only. | — |
| 12 | Kept and recorded. | R19 row (identity headers on every quota source, `grok.com` included) |
| 13 | Fixed (data): opencode-go and opencode-zen polls send `Accept: application/json`. | `parity_usage::quota_requests_match_the_research_table` |
| 14 | Recorded next to the `credits-bag-balance-only` deviation (no oracle case covers it). | `parity_usage.rs` deviation table |
| 15 | Accepted, not reordered: moving `limits[*]` first changes the reported window order, and no fixture covers the overlap. | — |
| 16 | Accepted. | — |
| R11 text | Fixed: the grok-cli row no longer lists `…/v1/user?include=subscription` as a quota source; R19 records that it is not read. | research.md R11, R19 |
| R9 text | Fixed: the dedup is one `std::sync::Mutex<HashMap<(provider, name), Shared<…>>>` with the refresh spawned as its own task. | research.md R9 |
