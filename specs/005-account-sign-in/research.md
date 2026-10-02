# Research: Account Sign-In (slice 005)

Phase 0 output of `/speckit-plan`. Every item is a technical decision Claude made, per the
user's standing direction that technical choices are Claude's. Items that change something the
user can see are marked **(user-visible)**. R7 was confirmed by the user on 2026-10-02
(spec Clarifications Q5).

Sources:
- `ref/9router` at pin `39e36d3`, judged on the request path (chatCore) plus the OAuth and
  usage services that 9router's dashboard and background refresher call;
- the scope brief `specs/briefs/2026-10-02-account-sign-in.md` (ledger and P notes);
- spec Clarifications 2026-10-02 (Q1–Q4);
- the current 0router code at `2762c89`;
- the approved quota meter design (agentmemory, 2026-09-28).

No NEEDS CLARIFICATION remains. Live facts 9router can't settle are pinned to opt-in live checks
(R17); each has a fallback that keeps the slice shippable.

---

## R1. Dependencies

**Decision**: no new crates.

| Need | Covered by |
|---|---|
| PKCE verifier, challenge, state, nonce | `getrandom`, `sha2`, `base64` (engine already) |
| Loopback listener for browser sign-in | `tokio::net::TcpListener` + a hand-parsed `GET /callback?…` line |
| Opening a browser | `std::process::Command` on `xdg-open` / `open`, only when `$DISPLAY`/`$WAYLAND_DISPLAY` or macOS |
| JWT payload read (email, tier) | `base64` + `serde_json`, no signature check (display only, never trusted for auth) |
| File locking for token writes | `std::fs::File::lock` (stable since Rust 1.89) |
| Timestamps | `engine/src/clock.rs` (`rfc3339`, `parse_rfc3339`) |
| Form bodies | `url::form_urlencoded` (`url` is already a workspace dependency) |

**Rationale**: the work is HTTP calls the engine already makes, plus a few small protocol
pieces. An OAuth crate would bring its own HTTP client and redirect policy, and would get around
the engine's SSRF-checking, redirect-free client (`upstream.rs:46-91`).

**Alternatives**: `oauth2` crate (own client, generic error mapping we'd re-map anyway);
`open` crate (one function we can write in ten lines); `rusqlite` for poll history (R11
explains why JSONL suffices).

## R2. Where the work lives

**Decision**:

| Crate | Additions |
|---|---|
| `nullrouter-registry` | schema-2 sections `[signin]`, `[quota]`, `[identity]`, `[models_live]`; gate rules; fit rule "sign-in and quota only in bundled plugins" |
| `nullrouter-engine` | `signin/` (flows, refresh, classification), `tokens.rs` (token store), `quota/` (poller, extractor, history, tally), account states, maintenance task |
| `nullrouter-server` | operator ops for quota and sign-in state; start and stop the maintenance task |
| `nullrouter-cli` | `accounts signin`, richer `accounts list`, `quota` command |

The sign-in flows run in the CLI process, through engine code. The CLI is core, so "the core
does all sending" holds. The server never opens a browser or reads a terminal.

## R3. Sign-in flows **(user-visible)**

Three generic flows. Each provider's plugin picks one and declares its URLs and parameters.

**Device code** (RFC 8628), used by grok-cli:
- POST the device endpoint with `client_id`, `scope` and the declared extra parameters
  (grok-cli: `referrer=grok-build`).
- Show `verification_uri_complete` (or `verification_uri` plus `user_code`) and the expiry.
- Poll the token endpoint at `interval` (default 5 s). `authorization_pending` keeps waiting,
  `slow_down` adds 5 s, `expired_token` and `access_denied` end the sign-in with that reason.
- No PKCE. (9router sends a verifier only by accident of its routing, `route.js:281`.)

**Authorization code with PKCE**, used by xai and anthropic:
- Verifier 96 random bytes (xai) or 32 (anthropic) as declared; S256 challenge; random state.
- The CLI prints the authorize URL every time. On a machine with a display it also opens the
  browser.
- Completion, whichever comes first:
  - the loopback listener receives the redirect (when the declared redirect is loopback and the
    port is free);
  - the operator pastes into the CLI the full redirect URL, or the bare code, or `code#state`.
- The pasted URL's `state` must match. A bare code is accepted only for providers whose
  declared redirect shows a code page, so the state check can't be skipped silently.

**Headless use**: works for all three without a listener on the operator's device.
- grok-cli: device code needs nothing local.
- xai: the redirect goes to `http://127.0.0.1:56121/callback`. On another device the browser
  fails to load that address, and the operator copies it from the address bar and pastes it.
  The page doesn't need to load; the code is in the URL.
- anthropic: see R4.

**Rationale**: matches 9router's three providers. Paste-back of the full URL is the only
headless path that works for any loopback redirect, and 9router's dashboard uses the same
paste (`OAuthModal.js:448-451`).

**Timeouts**: device code ends at the provider's `expires_in`. Browser flows end after 10
minutes, or on Ctrl-C. Either way nothing is written (FR-006).

## R4. Anthropic subscription sign-in **(user-visible)**

**Decision**: PKCE against `https://claude.ai/oauth/authorize` with Claude Code's public client
id and scopes `org:create_api_key user:profile user:inference`, token endpoint
`https://api.anthropic.com/v1/oauth/token`, JSON bodies (`claude.js:29-43`). The redirect is
Anthropic's hosted code page (the one Claude Code itself uses, `…/oauth/code/callback`), with
`code=true`. That page shows `code#state`, which the operator pastes. The loopback redirect
`http://localhost:<port>/callback` is the declared fallback.

**Rationale**: the hosted page is the best headless experience: no address-bar copying. 9router
already splits `code#state` (`claude.js:20-27`) and sends `code=true`, which only makes sense
with that page, but uses localhost. Whether the public client id accepts the hosted redirect is
live check L1. If it doesn't, the plugin switches to the loopback redirect and R3's paste-back
applies. That's a data change, no code.

**Terms warning (FR-004)**: shown after the operator chooses `anthropic` and before the
authorize URL is printed. Text:

> Anthropic's terms limit the use of Claude Pro/Max subscriptions outside Anthropic's own
> apps. Anthropic may refuse these requests or act on your account. You carry that risk.
> Continue? [y/N]

Anything but `y`/`yes` ends the sign-in with nothing written. `--accept-terms-risk` answers it
for scripted use; it still prints the warning. Shown at every anthropic sign-in (Q2).

**No disguise (Q1)**: `claudeCloaking.js` is not ported. The anthropic `[signin]` section
declares only the auth placement and fixed headers (R7).

## R5. Account kinds and the token store **(user-visible)**

**Decision**:
- `accounts.toml` goes to schema 2. An account has `kind = "key"` (slice 003's account) or
  `kind = "signin"`. A sign-in account carries no secret in `accounts.toml`.
- Tokens live in a new file, `$NULLROUTER_HOME/tokens.toml`, mode 0600, refused otherwise.
  Per account: access token, refresh token, expiry, the token response's scope, id-token claims
  0router uses (email, user id, plan tier), `signed_in_at`, `last_refresh_at`, and the hosts
  the tokens are bound to.
- Schema-1 `accounts.toml` still loads and is rewritten as schema 2 on the next save (all its
  accounts become `kind = "key"`).

**Rationale**: tokens rotate (xai and grok-cli issue a new refresh token on every refresh,
roughly every 40 minutes). Rewriting the operator's account list that often would churn the file
the operator reads and edits. Two files keep each one simple. Both use `write_private`
(temp file, fsync, rename), so a crash never leaves a half-written file.

**Writers and locking**: the server (refresh) and the CLI (sign-in, remove) both write
`tokens.toml`. Every writer takes an exclusive `File::lock` on `tokens.lock`, re-reads, changes
its own entry, and writes. Rotation order (spec edge case): the new refresh token is written
before the old one is dropped from memory, so a crash mid-refresh leaves the newest token on
disk.

**Host binding (FR-031)**: at sign-in, the token's hosts are the provider's endpoint hosts plus
its `[signin]` and `[quota]` hosts. `accounts::release` refuses to hand the token to any other
host, as for API keys (`accounts.rs:270-279`).

**Display**: `…last4` of the access token, as for keys.

## R6. Serving with a sign-in account

**Decision**:
- `upstream::auth_header` uses the plugin's `[signin] auth` placement (anthropic: `Authorization:
  Bearer`, replacing `x-api-key`) when the account is a sign-in account, and adds the
  `[identity]` headers (R7).
- The access token is read from a live token cell on `Engine` (an `ArcSwap` per account), so a
  refresh never needs a full reload. Reading it in `Run::outgoing` costs one atomic load.
- Everything else (retry, fallback, stay-warm, records, informational errors) is slice 003's
  attempt loop, unchanged (FR-007).

**Model types (FR-008)**:
- xai: `[endpoints.text]` (openai-chat, `/v1/chat/completions`), `[endpoints.image]`
  (`/v1/images/generations`), `[endpoints.video]` (submit then poll `/v1/videos/{request_id}`,
  through slice 003's video job support). One plugin, three sections.
- grok-cli: `[endpoints.text]` (openai-responses, `force_stream`). Its live model list (R14)
  has no type field, so every listed model is text. If a future list carries a type, the
  extractor maps it; nothing else changes.

## R7. Client identity headers **(user-visible; confirmed by the user)**

**Facts**: grok-cli's executor sends, besides fixed headers, values that change per request or
per account (`executors/grok-cli.js:362-397`):

| Header | Value in 9router |
|---|---|
| `x-grok-session-id`, `x-grok-conv-id` | the client's own session id, else a random id per connection |
| `x-grok-req-id` | a random UUID per request |
| `x-grok-turn-idx` | the number of user messages, never decreasing per session |
| `x-grok-model-override` | the upstream model id |
| `x-email`, `x-userid` | the signed-in account's own email and user id |
| `x-grok-agent-id` | SHA-256 of the machine id, shaped as a UUID |

The spec's Assumptions currently say only fixed header values are declarable. That limit was
Claude's wording after Q3, not the user's answer.

**Decision** (user confirmed 2026-10-02, spec Clarifications Q5): a plugin may declare, besides fixed values, header values from a closed
set the core fills. Every value in the set is true information the core owns:

| Placeholder | Value |
|---|---|
| `{session.id}` | the agent's session id (slice 003), else a random id kept per agent |
| `{request.id}` | a fresh random id per request |
| `{session.turn}` | user turns in this request's conversation |
| `{model.upstream}` | the upstream model id |
| `{account.email}`, `{account.user_id}` | the signed-in account's own values, from its token or profile |
| `{install.id}` | a random id generated once per 0router installation, stored in `$NULLROUTER_HOME` |

Not in the set, so not declarable: hashes of the request body, ids derived to resemble another
machine or account, anything a plugin computes. `x-grok-agent-id` gets `{install.id}`: an
honest id of this installation, not a copy of the official CLI's device hash.

Live check L3 confirms which of these grok-cli actually needs; the plugin declares only those.

**Alternatives**: fixed headers only (may break grok-cli; unknown until L3); porting 9router's
values exactly, including the machine-id hash (crosses the Q1 line on invented device ids).

## R8. Provider-forced request parameters

**Facts**: grok-cli's executor forces `store=false`, `reasoning.summary="concise"`,
`include=["reasoning.encrypted_content"]`, maps effort suffixes (`grok-4.5-high`) to
`reasoning.effort`, keeps a 16-field allowlist, and rewrites Responses input items
(`grok-cli.js:161-237, 419-526`).

**Decision**:
- A plugin may declare `force` parameters on an endpoint, from a closed set of non-content
  fields: `store`, `reasoning.summary`, `reasoning.effort`, `include` (append). Per-model
  `force` covers the effort-suffixed model ids, together with `upstream_id`.
- Each forced parameter is noted in the request record, as dropped fields are (Constitution IV).
- The allowlist and the input rewrites are **not** ported. They drop or change conversation
  items (reasoning items, orphan tool outputs, other servers' item references). That is content,
  and removing another server's items is exactly the harness-coupling case slice 004's adapters
  exist for. Cross-style requests already reach grok-cli through 0router's own Responses
  encoder, which emits well-formed input.

**Risk**: a Codex CLI session carrying OpenAI-server item ids may be refused by grok-cli on a
same-style route. Live check L3 tests it. A refusal is a non-transient error, so another member
serves where one exists. The fix, if needed, is a 004 adapter, not this slice.

## R9. Token freshness (FR-010–FR-014)

**Decision**:
- **Proactive**: a maintenance task (R13) refreshes each sign-in account when `expires_at −
  lead` is reached. `lead` is the plugin's `refresh_lead` (grok-cli and xai 5 min, anthropic
  4 h as 9router declares), with a core floor of 2 min and a ceiling of half the token's
  lifetime. Refresh happens with or without traffic.
- **At use**: `Run::outgoing` checks expiry too. A token within 30 s of expiry triggers the
  same deduplicated refresh, and the request waits for it (bounded by the refresh timeout,
  15 s).
- **On rejection**: a 401, or a 403 whose body matches the plugin's auth-rejection rules, on a
  sign-in account triggers one deduplicated refresh, then one retry on the same account with
  the new token. The client sees only the answer. 9router does this for grok-cli and claude
  but not xai (its default executor has no xai refresher); 0router does it for all three.
- **Dedup (FR-012)**: one in-flight refresh per account, shared by every waiter (a
  `tokio::sync::Mutex<Option<Shared<…>>>` per account). Ports `dedup.js`, keyed per account
  instead of per old refresh token.
- **Rotation**: the new refresh token, or the old one when the response omits it
  (`providers.js:131`).
- **Restart (FR-014)**: tokens are read from `tokens.toml` at start. An access token already
  expired is refreshed before the account serves.
- **Redaction**: every refresh rebuilds the redactor with the new tokens plus the previous
  generation (so an old token echoed late is still masked) and swaps it into `Engine.redactor`.

## R10. Refresh failures and account states **(user-visible)**

**Classification** (ports and extends `tokenRefresh.js:45-54` and `providers.js:236-255`):

| Refresh outcome | Class |
|---|---|
| `invalid_grant`, `invalid_request`, `unauthorized_client`, `refresh_token_expired`, `refresh_token_reused`, `refresh_token_invalidated` | permanent |
| any other 400/401/403 from the token endpoint | permanent |
| timeout, connection failure, 5xx, 429 | transient |

**States**:

| State | Meaning | Serves? | Leaves the state when |
|---|---|---|---|
| `active` | usable | yes | — |
| `refreshing` | token expired, transient refresh failures, retrying with backoff (10 s, 30 s, 1 min, then every 2 min) | no; skipped as "token expired, refresh retrying" | a refresh succeeds |
| `needs sign-in` | permanent refresh failure | no | the operator signs the account in again |
| `refused by provider` | the provider refused a fresh token's request because of how it was sent (FR-004b) | no | the operator signs in again, or runs `accounts enable` to retry |
| `disabled` | operator choice | no | `accounts enable` |

`refused by provider` is set when, right after a successful refresh, the retried request is
rejected again with 401/403, or when a response matches the plugin's `refused` error rule (for
anthropic, the "only authorized for use with Claude Code" message family; exact text from live
check L1). A 403 that 9router's rules read as a model-access error stays an ordinary failure.

States `needs sign-in` and `refused by provider` are kept in `tokens.toml` with their time and
the provider's reason, so they survive a restart.

**Telling the operator (FR-016)**:
- `accounts list` state column: `needs sign-in since 2026-10-03 14:02 (invalid_grant)`, and a
  hint line with the command to run.
- Records: an out-of-service account is a `Skipped` attempt with class `NeedsSignIn` or
  `Refused` and the reason `needs sign-in: run nullrouter accounts signin xai main`.
  `plan::member` emits these skips instead of silently filtering, as it does for disabled
  accounts today.
- Informational error: the same `Tried` line, so the account and command appear in the
  message and in the structured details.
- A `warn` log line once per state change.

## R11. Quota polling **(user-visible)**

**Decision**:
- The maintenance task polls each account whose provider declares `[quota]`, on a per-account
  interval. Default 10 minutes for every provider; floor 2 minutes. 9router's dashboard polls
  every minute (Claude every 10) but only while a browser tab is open; 0router polls always,
  so it picks the slower rate for everyone.
- Jitter of ±10% spreads accounts out.
- A failed poll is retried once after 1 minute, then waits for the next interval (spec SC-007
  allows one retry). A 429 skips the retry. A 401 on a sign-in account refreshes and retries,
  as a request would.
- The interval is set per account: `nullrouter quota interval <provider> <name> 15m`.
- Polls use the same SSRF-checking, redirect-free client as requests, and the same token
  release rules (R5).

**Providers**:

| Provider | Endpoint | Windows |
|---|---|---|
| anthropic (sign-in only) | `GET https://api.anthropic.com/api/oauth/usage` with `anthropic-beta: oauth-2025-04-20` | `five_hour` → "5-hour", `seven_day` → "weekly", `seven_day_<model>` → "weekly <model>", `limits[]` with `kind = weekly_scoped` → "weekly <model>"; unit percent used |
| anthropic (API key) | none | "quota not reported" |
| grok-cli | `GET …/v1/billing?format=credits` and `GET …/v1/user?include=subscription`; fallback gRPC-web `POST https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig` | "monthly included", "on-demand", "prepaid", "weekly SuperGrok" (credits or percent, as reported) |
| opencode-go | `GET https://opencode.ai/zen/go/v1/usage` | `rolling`, `weekly`, `monthly`; percent used |
| opencode-zen | `GET https://opencode.ai/zen/v1/usage` | same |
| xai | none known (9router has no reader) | "quota not reported", unless live check L2 finds rate-limit headers on `GET /v1/models` |

**Units**: each window is shown in the provider's unit (percent, credits, requests). 0router
doesn't convert. Percent windows show "remaining" as `100 − used`.

**Poll cost**: all of these are billing or usage reads, not inference. None is known to spend
quota. A provider whose only report would spend quota is "quota not reported" (spec edge case).

## R12. The quota extractor (data, not code)

**Decision**: `[quota]` declares the request (URL, method, extra headers, body kind) and a list
of window rules. A window rule reads JSON paths with three tools only:

- a path with `*` over object keys or array items (`limits[*]`, `seven_day_*`);
- `first_of` alternatives, for fields 9router reads under several names (grok-cli's
  `billingPeriodEnd | billing_period_end | currentPeriod.end | …`);
- `{val: n}` unwrapping for protobuf-JSON numbers, as a flag.

Plus: a `where` equality filter (`kind = "weekly_scoped"`), a name template from matched parts,
the unit, which value is used and which is limit/remaining/percent, and the reset-time format
(`auto`: epoch seconds below 1e12, else milliseconds, else RFC 3339, as `parseResetTime`).

The grok-cli gRPC-web fallback uses a core decoder, `grpc_web_ratio`: read the first data
frame, then protobuf field 1 → nested field 1 (fixed32 float, or fixed64 double) as used ratio,
nested field 5 as a Timestamp. That's a closed, named core primitive the plugin selects, like
slice 003's primitives, because a protobuf decoder can't be data.

**Rationale**: covers all five readers without per-provider code. 9router's Claude legacy
org-usage path (`claude.js:152-199`) is not ported: it needs admin rights, returns an
unnormalised shape, and the OAuth path covers subscriptions.

## R13. Maintenance task

**Decision**: `serve` spawns one maintenance task next to the operator socket. It keeps a
timer queue of the next refresh time and next poll time per account, wakes for the earliest,
and runs due jobs with at most 4 at once. It stops on the existing shutdown `watch` channel
(`cli/src/cmd/serve.rs:41-64`), which now also cancels a `CancellationToken` passed to each
job. On reload it rebuilds the queue from the new account set, keeping last-run times.

Without a running server there are no refreshes and no polls. The CLI's `quota` shows the
stored history and says the server isn't running.

## R14. grok-cli live model list

**Decision**: `[models_live]` declares the list URL, extra headers (`x-xai-token-auth`,
`x-grok-client-mode`), and the extractor paths (list at `data | models | results` or the root,
id from `id | model_id | slug | name`, display name, context and output limits). The list is
fetched at server start and every 6 hours per provider (using the first active sign-in
account). Listed models join the static `[[models]]`; the static list is the fallback when the
live list fails (as 9router, `models/route.js:472-498`).

## R15. Poll history and traffic tally (FR-023–FR-026)

**Storage**: `$NULLROUTER_HOME/quota/<provider>/<account>.jsonl`, one JSON object per poll,
append-only, mode 0600 (the data isn't secret, but it describes the operator's use).

**Entry**:
- `v` (format version), `at` (RFC 3339), `ok` or `error` (class and reason);
- `windows`: name, unit, used, limit, remaining, resets_at, exactly as reported and parsed;
- `tally`: since the previous poll entry, per upstream model:
  `requests`, `requests_usage_unreported`, `input`, `output`, `cache_read`, `cache_write`.

**Why per model**: the approved quota meter computes cost as `model_multiplier × Σ(weight ×
tokens)`. 006 fits the multiplier per model only if the tally is split by model. The token
categories and names are the meter's.

**Counting**: the tally is fed from `Run::end_attempt` with each attempt's provider-reported
usage on that account (FR-024). Retries and fallbacks count on the account each attempt used.
An attempt with unreported usage counts in `requests_usage_unreported` and adds no tokens.

**Crash safety (FR-025)**: the running tally is checkpointed to
`quota/<provider>/<account>.tally.json` every 10 s and at shutdown, via `write_private`. A
crash loses at most 10 s of tally. A graceful restart loses nothing.

**Retention (Q4)**: kept until the operator prunes it: `nullrouter quota prune --before DATE
[provider [name]]` rewrites the files without older entries; `nullrouter quota forget
<provider> <name>` deletes an account's history. Removing an account stops its polls and keeps
its history.

**Size**: about 300 bytes per poll at 10-minute polls is about 16 MB per account per year,
less after gzip if 006 wants it. Acceptable for a local file, and reading the newest entries
only scans the file's tail.

**Alternatives**: SQLite (a new dependency and a schema to migrate when 006 adds request
history; 006 can import JSONL); per-attempt journal lines (exact on crash, but that's 006's
persistent request history in disguise).

## R16. Plugin gate and fit check

**Decision**:
- New schema-2 sections: `[signin]`, `[quota]`, `[identity]`, `[models_live]`
  ([contract](contracts/signin-quota-schema.md)). `deny_unknown_fields`, closed sets for flows,
  placeholders and extractor tools.
- Every URL in them passes the SSRF checks and must be on a host the plugin's endpoints,
  `[signin]` or `[quota]` declare. The token's bound hosts are computed from that set (R5).
- No secret field exists in these sections. A value that looks like a secret (the existing
  secret detector) is refused. anthropic, xai and grok-cli are public clients; no client secret
  is needed or stored.
- Fit check: `[signin]`, `[quota]`, `[identity]` placeholders and `[models_live]` are accepted
  only in bundled plugins. A community plugin declaring them is refused whole, naming sign-in
  or quota, as today (FR-029). Opening them up later is removing that one rule.
- The bundle grows to seven: anthropic (gains `[signin]`, `[quota]`), opencode-go and
  opencode-zen (gain `[quota]`), xai and grok-cli (new bundled files written by hand from the
  community ones, then removed from the community set).

## R17. Live checks

Opt-in (`NR_LIVE=1`) with the operator's real accounts, never in CI.

| Id | Question | Fallback if the answer is no |
|---|---|---|
| L1 | Does the anthropic client id accept the hosted code-page redirect? What does a subscription refusal look like? | Loopback redirect with paste-back; refusal rule from the observed body |
| L2 | Does `api.x.ai/v1/models` return rate-limit headers to a signed-in account? | xai shows "quota not reported" |
| L3 | Does cli-chat-proxy serve with the R7 headers (or with fixed headers only), and serve a Codex CLI session on a same-style route? | Declare what's needed; item-reference issues go to a 004 adapter |
| L4 | Real token lifetimes and rotation for each provider | Leads stay as declared |
| L5 | Each `[quota]` extractor against a real response | Fix the declared paths (data only) |

## R18. Test strategy

- **Mock identity provider** (in-process axum): device code (pending, slow_down, expired,
  denied), PKCE exchange with state check, refresh with rotation, configurable permanent and
  transient failures, tokens with lifetimes down to 2 s.
- **Mock providers** for anthropic, xai, grok-cli, opencode-go/zen serving requests and quota
  responses built from 9router's fixtures and parser cases.
- **Token-expiry soak (SC-003)**: 2 s tokens, 1 s lead, traffic and idle gaps across 20+
  lifetimes from two harnesses (Python and Node SDKs), zero expiry failures.
- **States (SC-005)**: permanent refresh failure and provider refusal, checked in the accounts
  list, records and informational errors.
- **Quota (SC-006–SC-008)**: extractor cases per provider; poll times; idle polling with a
  shortened interval; tally equals the sum of reported usage, per model.
- **Headless (SC-001)**: the CLI sign-in driven through stdin paste-back and device code, with
  no display variables set.
- **Secrets (SC-009)**: the slice 003 sentinel scan, extended with sentinel access tokens,
  refresh tokens, codes and verifiers, across logs, records, errors, CLI output, the operator
  socket and plugin-visible data.
- **Harnesses**: Python and Node SDKs and Claude Code against signed-in mock accounts.
- **Benchmarks**: the engine bench gains a sign-in account case (token cell load, identity
  headers, tally update). No regression against the slice 003 baseline.

## R19. Deliberate deviations from 9router (summary)

| 9router | 0router | Why |
|---|---|---|
| Claude cloaking (tools, decoys, billing header, fake ids) | not ported | Clarifications Q1; Constitution IV |
| grok-cli input rewrites and field allowlist | not ported; forced non-content parameters only | Constitution IV; 004 adapters own harness coupling |
| `x-grok-agent-id` = machine-id hash | `{install.id}` (R7) | no invented device ids (Q1 line) |
| Permanent refresh failures ignored | account taken out and named | FR-015, FR-016 |
| xai never refreshes on 401 | refresh and retry for all three | FR-011 |
| Anthropic loopback redirect only | hosted code page first (L1) | headless sign-in |
| Quota polled only while a dashboard tab is open | polled always, slower default | FR-018 |
| Claude legacy org-usage fallback | not ported | admin-only, unnormalised |
