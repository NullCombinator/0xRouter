# Data Model: Account Sign-In (slice 005)

Extends [slice 003's data model](../003-request-pipeline/data-model.md). Research items are
cited as R-numbers from [research.md](research.md).

## Provider account (extended)

| Field | Type | Notes |
|---|---|---|
| `provider` | provider id | unchanged |
| `name` | `[a-z0-9_-]{1,32}` | unique per provider (unchanged from slice 003) |
| `kind` | `key` \| `signin` | new; schema-1 files load as `key` |
| `secret` | secret source | `key` accounts only |
| `order` | integer | unchanged |
| `disabled` | bool | unchanged |
| `poll_interval` | duration, optional | default 10 min, floor 2 min (R11) |
| `hosts` | set of hosts | `key`: as slice 003; `signin`: kept with the tokens (R5) |

Identity: `(provider, name)`. Signing in under an existing `(provider, name)` replaces that
account's tokens and returns it to `active`; the CLI says it replaced them.

## Sign-in credentials (`tokens.toml`, new)

One entry per `signin` account. Held only by the core.

| Field | Type | Notes |
|---|---|---|
| `provider`, `name` | | joins the account |
| `access_token` | secret | |
| `refresh_token` | secret, optional | some providers may omit it |
| `expires_at` | RFC 3339 | from `expires_in` at receipt |
| `scope` | string | as granted |
| `claims` | `{email?, user_id?, tier?}` | read from the id token or profile; not secret, shown in `accounts list --long` |
| `hosts` | set of hosts | endpoint + `[signin]` + `[quota]` hosts at sign-in |
| `signed_in_at`, `last_refresh_at` | RFC 3339 | |
| `state` | see below | persisted only for `needs_sign_in` and `refused` |
| `state_since`, `state_reason` | RFC 3339, string | provider's error code or message, redacted |

Validation: file mode 0600 or `serve` refuses to start (as `accounts.toml`); an entry with no
matching `signin` account is ignored and reported by `nullrouter check`.

## Account state machine (R10)

```text
             sign-in ok
   (new) ─────────────────► active ◄──────────────┐
                              │  ▲                │ refresh ok
              transient fail  │  │ refresh ok     │
                              ▼  │                │
                          refreshing ─────────────┘
                              │
               permanent fail │            permanent fail (from active, on use)
                              ▼
                        needs_sign_in ──── sign-in again ───► active

   active ── fresh token refused / refusal rule ──► refused ── sign-in again or `enable` ──► active
   any ── `disable` ──► disabled ── `enable` ──► previous state
```

Serving: only `active`. Every other state is a recorded skip with its reason (FR-016).
`key` accounts use only `active` and `disabled`, plus slice 003's cooldowns.

## Sign-in declaration (plugin `[signin]`, new)

| Field | Notes |
|---|---|
| `flow` | `device_code` \| `pkce` |
| `client_id` | public client id |
| `scopes` | list |
| `authorize_url`, `token_url`, `device_url`, `discovery_url` | per flow; discovery results must stay on the declared hosts |
| `redirect` | list of `{ uri, kind = "loopback" \| "code_page" }`, tried in order |
| `params` | extra fixed parameters for authorize/device requests (closed key set) |
| `body` | `form` \| `json` for token requests |
| `verifier_bytes` | 32–96 |
| `refresh_lead` | duration |
| `auth` | `{ header, scheme }` used for requests from sign-in accounts |
| `profile` | optional `{ url, headers, email, user_id, tier }` post-sign-in read |
| `refused` | error rules (status + body text) that mark the account `refused` |
| `terms_warning` | bool; prints the R4 warning (anthropic only) |

Full format: [contracts/signin-quota-schema.md](contracts/signin-quota-schema.md).

## Identity declaration (plugin `[identity]`, new)

Headers sent on requests, polls and model-list calls of sign-in accounts. Values are fixed
strings or one of the core placeholders in R7 (spec Clarifications Q5).

## Quota declaration (plugin `[quota]`, new)

| Field | Notes |
|---|---|
| `accounts` | `signin` \| `key` \| `any`: which account kinds it applies to |
| `request` | `{ url, method, headers, body = "none" \| "grpc_web_empty" }`, plus optional `fallback` request |
| `windows` | list of window rules (R12) |
| `decoder` | `json` (default) \| `grpc_web_ratio` |

## Quota window (runtime)

| Field | Type |
|---|---|
| `name` | string, from the rule's name template |
| `unit` | `percent` \| `credits` \| `requests` \| `tokens` |
| `used`, `limit`, `remaining` | numbers, each optional; percent windows have `limit = 100` |
| `resets_at` | RFC 3339, optional |

## Quota poll (history entry, `quota/<provider>/<account>.jsonl`)

| Field | Type | Notes |
|---|---|---|
| `v` | 1 | format version |
| `at` | RFC 3339 | poll time |
| `ok` | bool | |
| `error` | `{class, reason}` | when `ok = false`; previous windows stay shown (FR-021) |
| `windows` | list of quota windows | as reported |
| `tally` | traffic tally | since the previous entry |

## Traffic tally

Per account and poll interval, keyed by upstream model id:

| Field | Type |
|---|---|
| `requests` | count of attempts sent through the account |
| `requests_usage_unreported` | attempts whose provider reported no usage |
| `input`, `output`, `cache_read`, `cache_write` | sums of provider-reported tokens |

Names and categories match the approved quota meter (token weights `input`, `output`,
`cache_read`, `cache_write`; per-model multiplier), so slice 006 fits without migration
(FR-026). Running tally checkpoint: `quota/<provider>/<account>.tally.json` (R15).

## Attempt and record (extended)

- `ErrorClass` gains `NeedsSignIn`, `Refused` and `TokenRefreshing`, used for skipped attempts
  of out-of-service accounts.
- An attempt notes provider-forced parameters (R8) next to dropped fields.
- The informational error's `Tried` lines name the account and the command to sign it in again.

## Installation id

`$NULLROUTER_HOME/install-id`: a random UUID written once (mode 0600), used only for
`{install.id}` (R7).
