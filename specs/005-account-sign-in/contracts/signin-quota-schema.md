# Contract: Plugin sections for sign-in, identity, quota and live models

Extends [provider schema 2](../../003-request-pipeline/contracts/provider-schema-v2.md). In this
slice these sections are accepted only in bundled plugins; a community plugin declaring any of
them is refused whole with "not supported by this core: account sign-in" (or "quota").

General rules:
- `deny_unknown_fields` everywhere. Closed sets for `flow`, `redirect.kind`, `body`, `decoder`,
  extractor tools and placeholders.
- Every URL passes the SSRF checks. Hosts must be among the plugin's endpoint hosts or hosts
  declared in `[signin]` / `[quota]`; the account's tokens are bound to that set.
- No field holds a secret. Values that look like secrets are refused.

## `[signin]`

```toml
[signin]
flow = "pkce"                                   # pkce | device_code
client_id = "b1a00492-073a-47ea-816f-4c329264a828"
scopes = ["openid", "profile", "email", "offline_access", "grok-cli:access", "api:access"]
discovery_url = "https://auth.x.ai/.well-known/openid-configuration"   # optional
authorize_url = "https://auth.x.ai/oauth2/authorize"                   # fallback when discovery fails
token_url = "https://auth.x.ai/oauth2/token"
redirect = [{ uri = "http://127.0.0.1:56121/callback", kind = "loopback" }]
params = { plan = "generic", referrer = "cli-proxy-api", nonce = "{random.hex16}" }
body = "form"                                   # form | json
verifier_bytes = 96
refresh_lead = "5m"
auth = { header = "Authorization", scheme = "bearer" }
terms_warning = false

[signin.profile]                                # optional post-sign-in read
url = "https://cli-chat-proxy.grok.com/v1/user"
email = "email | id_token.email | id_token.preferred_username"
user_id = "userId | principalId"
tier = "subscriptionTier"

[[signin.refused]]                              # marks the account `refused`
status = [400, 403]
body_contains = "only authorized for use with Claude Code"
```

- `device_code` flow uses `device_url` and `token_url`; `authorize_url`, `redirect` and
  `verifier_bytes` are refused there.
- `params` keys are a closed set: `plan`, `referrer`, `code`, `audience`, `prompt`, `nonce`.
  Values are fixed strings or `{random.hex16}`.
- `redirect.kind = "code_page"` allows pasting a bare code or `code#state`.

## `[identity]`

```toml
[identity.headers]
User-Agent = "grok-shell/0.2.99 (linux; x86_64)"
x-grok-client-identifier = "grok-shell"
x-grok-client-version = "0.2.99"
x-grok-session-id = "{session.id}"
x-grok-req-id = "{request.id}"
```

Applied to every request, poll and live-model call of a sign-in account. Placeholders (closed
set, R7, spec Clarifications Q5): `{session.id}`, `{request.id}`, `{session.turn}`,
`{model.upstream}`, `{account.email}`, `{account.user_id}`, `{install.id}`. A value is either a
fixed string or exactly one placeholder. Security-floor header names (`authorization`,
`cookie`, `x-api-key`, …) are refused.

## Forced parameters (endpoint and model level)

```toml
[endpoints.text]
force = { store = false, "reasoning.summary" = "concise", include = ["reasoning.encrypted_content"] }

[[models]]
id = "grok-4.5-high"
upstream_id = "grok-4.5"
force = { "reasoning.effort" = "high" }
```

Keys are a closed set: `store`, `reasoning.summary`, `reasoning.effort`, `include` (appended,
deduplicated). Each forced parameter is noted in the request record.

## `[quota]`

```toml
[quota]
accounts = "signin"                             # signin | key | any
request = { url = "https://api.anthropic.com/api/oauth/usage", headers = { anthropic-beta = "oauth-2025-04-20" } }

[[quota.window]]
path = "five_hour"
name = "5-hour"
unit = "percent"
used = "utilization"
resets_at = "resets_at"

[[quota.window]]
path = "seven_day_*"                            # `*` binds {1}
name = "weekly {1}"
unit = "percent"
used = "utilization"
resets_at = "resets_at"

[[quota.window]]
path = "limits[*]"
where = { kind = "weekly_scoped" }
name = "weekly {scope.model.display_name|lower}"
unit = "percent"
used = "percent"
resets_at = "resets_at"
```

- Value paths may use `first_of` alternatives with `|` (`"billingPeriodEnd | billing_period_end | currentPeriod.end"`).
- `unwrap_val = true` reads protobuf-JSON `{ val = n }` numbers.
- A rule may set `used`, `limit`, `remaining` (at least one), `resets_at`, and
  `resets_format = "auto" | "epoch_s" | "epoch_ms" | "rfc3339"`.
- A window whose `used`/`remaining` doesn't resolve to a number is skipped, as in 9router.
- `[quota.fallback]` declares a second request used when the first yields no window, with
  `decoder = "grpc_web_ratio"` for grok-cli's credits RPC (window name and unit declared there).
- No `[quota]` section: the account shows "quota not reported".

## `[models_live]`

```toml
[models_live]
url = "https://cli-chat-proxy.grok.com/v1/models"
headers = { x-xai-token-auth = "xai-grok-cli", x-grok-client-mode = "headless" }
list = "data | models | results | ."
id = "id | model_id | modelId | slug | name"
name = "display_name | displayName | name"
context = "context_length | context_window"
max_output = "max_output_tokens"
type = "text"                                   # or a path; default text
refresh = "6h"
```

## Gate errors (added)

| Case | Message |
|---|---|
| sign-in/quota section in a community plugin | `not supported by this core: account sign-in` / `quota` |
| URL host outside the declared host set | `<field>: host <h> is not one of this provider's hosts` |
| unknown placeholder | `<field>: unknown placeholder {x}` |
| forced parameter outside the set | `force: <key> can't be forced` |
| secret-like value | `<field>: looks like a secret; plugins can't hold secrets` |
