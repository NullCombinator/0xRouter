# Contract: Files and plugin schema

## `config.toml` additions (operator)

```toml
[connection]                      # all providers
proxy = "eu-exit"                 # or "none"

[provider.openrouter.connection]
connect_timeout_ms = 5000
header_timeout_ms = 10000
first_token_timeout_ms = 60000     # 0 = off
stall_timeout_ms = 360000
reuse = true
http2 = false                      # HTTP/1.1 only
proxy = "none"                     # beats [connection].proxy

[provider.openrouter.model."anthropic/claude-opus-4.1".connection]
first_token_timeout_ms = 300000    # timeouts only at model level

[provider.openrouter.retry]
all = { retries = 2, delay_ms = 1000 }
"503" = { retries = 3, delay_ms = 2000 }
```

Validation:
- Timeouts are 1–3 600 000 ms; first token may also be 0 (off).
- `retries` is 0–5 and `delay_ms` is 0–30 000.
- A status key is a 3-digit HTTP status.
- A `proxy` must name a proxy in `proxies.toml`, or be `"none"`.
- Settings for an unknown provider or model load, and `check` notes them as unused (FR-034).

## `accounts.toml` addition

```toml
[[account]]
provider = "kiro"
name = "work"
kind = "signin"
proxy = "us-exit"                  # or "none"; absent = inherit provider / all
```

## `proxies.toml` (new, mode 0600)

```toml
schema = 1
[[proxy]]
name = "eu-exit"
url = "socks5://10.0.0.5:1080"
username = "router"
password = { env = "EU_EXIT_PASSWORD" }   # or a string; written by `proxy add` from stdin
```

`check` warns when the file isn't 0600, as it does for `accounts.toml`.

## `routing/proxies.json` (new, mode 0600, written by `serve`)

```json
{"paused": {"eu-exit": {"since": "2026-10-07T13:58:12+02:00", "reason": "connect to proxy refused"}}}
```

## Plugin schema additions (`schema = 2`, data only)

```toml
[transport]
http2 = false                       # only `false` has an effect: the provider doesn't speak HTTP/2

[[endpoints]]
timeout_ms = 30000                  # exists: header timeout
stall_timeout_ms = 360000           # exists
connect_timeout_ms = 10000          # new
first_token_timeout_ms = 120000     # new
retry = { "429" = { retries = 2, delay_ms = 2000 } }   # exists; now capped at 5 / 30000

[[models]]
id = "slow-reasoner"
timeouts = { first_token_ms = 600000, stall_ms = 600000 }   # new: connect_ms, headers_ms, first_token_ms, stall_ms
```

Refused at validation, at any depth: `proxy`, `proxy_url`, `https_proxy`, `no_proxy`, with
`plugins can't declare a proxy; proxies are operator-only` (FR-026, R10).
