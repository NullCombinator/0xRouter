# Contract: Provider plugin schema 2

Extends [slice 002's plugin schema](../../002-provider-model-registry/contracts/plugin-schema.md).
Every slice 002 registry key keeps its meaning. Schema 2 is used by the chosen five
(hand-maintained). Schema 1 stays valid for community plugins through the conversion below.

## Schema 1 → 2 conversion (community plugins)

| Schema 1 | Schema 2 |
|---|---|
| `[transport] base_url, format` | `[endpoints.text] url, wire` |
| `capabilities.<type>.endpoint` | `[endpoints.<type>]` |
| format `openai` / `claude` / `openai-responses` / `gemini` | wire `openai-chat` / `anthropic-messages` / `openai-responses` / `gemini` |
| capability `image_to_text` | `vision = true` on the text endpoint |
| any other format, quirk, hook, `executor_params`, OAuth | Unsupported (fit check) |

Schema 2 rejects the schema-1 execution keys (`transport`, `transports`,
`capabilities.*.endpoint`, `quirks`, `executor_params`, `auth.hooks`) so each fact has one
home. Slice 002's composed-transport view is derived from `endpoints`.

## Endpoints

```toml
[endpoints.text]                   # or [[endpoints.text]] for one entry per wire
url = "https://api.anthropic.com/v1/messages"
wire = "anthropic-messages"        # a loaded style id; OR inline body + response (not both)
method = "POST"
headers = { "anthropic-version" = "2023-06-01" }   # static, non-secret
timeout_ms = 60000                 # time to response headers
stall_timeout_ms = 360000
force_stream = false
vision = true
retry = { 429 = { retries = 1, delay_ms = 2000 } }  # overrides research R7 per status
models = []                        # optional: restrict this endpoint to these model ids

[endpoints.text.errors]
body = [{ when = { path_present = "error" }, message = "error.message", status = "error.code" }]
stream = [{ event = "error", message = "error.message", status_map = { overloaded_error = 529 } }]

[endpoints.text.token_count]       # only if the wire style defines count_tokens
url = "https://api.anthropic.com/v1/messages/count_tokens"

[endpoints.text.continuation]
method = "assistant_prefill"       # assistant_prefill | prefix_flag
trim_trailing_whitespace = true
unless = ["thinking_enabled", "tool_call_in_progress"]
models = ["claude-sonnet-4-20250514", "claude-opus-4-20250514"]   # or except_models
```

Inline endpoints (non-text, no wire):

```toml
[endpoints.stt]
url = "https://api.elevenlabs.io/v1/speech-to-text"
method = "POST"
encoding = "multipart"
body = { file = "{input.audio}", model_id = "{model.upstream_id}", language_code = "{input.language?}" }
response = { text = "text", language = "language_code" }
errors = { body = [{ when = { path_present = "detail" }, message = "detail.message" }] }
models = ["scribe_v2"]
```

URL rules:
- placeholders `{model}` and `{voice}` only, only in the path, percent-encoded, `.`/`..`
  rejected;
- no loopback, private, link-local or metadata host, `localhost`, `*.local` or `*.internal`
  unless `allow_private_endpoints = true` in the operator's `config.toml`;
- redirects are never followed.

Placeholders in `body` come from a fixed per-type set (`{input.*}`, `{model.upstream_id}`,
`{params.*}`, `{output.*}`); `{account.*}`, `{secret.*}` and anything unknown are rejected.

Per-model wires (replaces 9router's `supported_formats` and forced target format):

```toml
[[models]]
id = "kimi-k2"
wires = ["openai-chat"]            # endpoint choice: native pair first, then this order
```

## Session

```toml
[session]
header = "x-opencode-session"
derive = "ses_sha256_hex32"        # ses_sha256_hex32 | ses_time_base62; input is the agent id
```

## Forwarding and the security floor

```toml
[forwarding.to_upstream]
headers = [
  { name = "anthropic-beta", merge = "append_csv", from_styles = ["anthropic-messages"] },
  { name = "anthropic-version", merge = "replace", from_styles = ["anthropic-messages"] },
]

[forwarding.to_client]
headers = ["request-id", "retry-after", "anthropic-ratelimit-*"]
body = []                          # paths copied verbatim; native pairs only
```

The floor (never forwarded in either direction, whatever is declared):
- `authorization`, `proxy-authorization`, `x-api-key`, `api-key`, `x-goog-api-key`,
  `xi-api-key`, `cookie`, `set-cookie`, `set-cookie2`, `www-authenticate`,
  `proxy-authenticate`, `x-amz-security-token`, `x-auth-token`;
- any name matching slice 002's secret-name pattern;
- every loaded style's key carriers and every loaded provider's auth header;
- hop-by-hop and core-owned: `host`, `content-length`, `transfer-encoding`, `connection`,
  `keep-alive`, `proxy-connection`, `te`, `trailer`, `upgrade`, `content-encoding`,
  request `accept-encoding`; `x-0router-*` can't be overwritten.

Values containing any configured secret are dropped; values with CR or LF are rejected.

Gate behaviour:

| Declaration | Normal load | Strict (bundled, CI) |
|---|---|---|
| floor name in a forwarding list | diagnostic, entry stripped, plugin loads | error |
| wildcard that could match a floor name | warning | warning |
| bare `*` or prefix < 3 characters | error | error |
| secret-like body path | error | error |
| static header naming a credential | Invalid (slice 002) | Invalid |

## Fit check

Runs after the gate. Verdict `Fits` or `Unsupported`. Unsupported refuses the whole plugin:

```
plugins/qoder.toml: not supported by this core (0router 0.3.0, plugin schema 1-2)
  - plugins/qoder.toml:7:1 auth.kind = "oauth": account sign-in is not supported
  - plugins/qoder.toml:12:10 transport.format = "qoder": wire format "qoder" is not supported
  - plugins/qoder.toml:1:1 requires = "9router-executor:qoder": needs a provider-specific executor
No part of this plugin was loaded.
```

CLI exit code for this case: 3 (slice 002 uses 0 ok, 1 invalid, 2 usage).

Unsupported parts: OAuth, cookie or web-cookie auth (also alongside an API key); wires other
than the four; web search, web fetch and systemone sections; quirks, hooks,
`executor_params`, credential fallback, regions; media formats not implemented; any
`requires` name.

## Gate corpus additions

`tests/gate/invalid/providers/`: `endpoint-unknown-type`, `url-private-ip`,
`url-localhost`, `placeholder-in-host`, `wire-and-body`, `unknown-body-placeholder`,
`body-secret-key`, `forwarding-wildcard-bare`, `forwarding-bad-merge`,
`forwarding-body-secret-path`, `continuation-unknown-method`,
`token-count-without-count-style`, `error-rule-bad-status`, `schema2-with-transport`,
`model-type-without-endpoint`; strict-only: `forwarding-authorization`.

`tests/gate/unsupported/` with golden `.expected` messages: `oauth-auth`,
`cookie-category`, `kiro-format`, `quirk`, `hook`, `executor-requires`,
`web-search-section`, `systemone-section`, `schema-3`, `unknown-wire-style`, `mixed`.
