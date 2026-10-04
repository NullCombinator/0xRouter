# Contract: plugin `[routing]` section (slice 006)

Extends [slice 005's schema contract](../../005-account-sign-in/contracts/signin-quota-schema.md).
Every key is optional. An unknown key or value is refused by the validation gate. The section
holds no URL, header, credential or code, so bundled and community plugins may both declare it
(research R16).

## Example

```toml
[routing.cache]
mode = "explicit"          # explicit | automatic | none
lifetime = "5m"
# min_tokens = 1024        # automatic only

[[routing.window]]
name = "5-hour"            # matches a [quota] window name when the provider reports one
length = "5h"
unit = "weighted_tokens"   # weighted_tokens | requests
capacity = 9_000_000       # in unit; omitted = assumed from peers (shown as such)
token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }
model_multiplier = { "claude-opus-*" = 5.0 }
reserve = "5%"

[[routing.window]]
name = "weekly"
length = "7d"
unit = "weighted_tokens"
capacity = 90_000_000
token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }

[[routing.window]]         # an unreported window, estimated from 0router's own count
name = "daily requests"
length = "1d"
unit = "requests"
capacity = 1500
reset = "fixed"
anchor = "00:00+00:00"

[[routing.window]]         # admission only (length < 1h)
name = "per-minute"
length = "1m"
unit = "requests"
capacity = 50

[[routing.price]]          # first match wins; per million tokens
when = { days = ["mon", "tue", "wed", "thu", "fri"], from = "16:30", to = "00:30", offset = "+00:00" }
input = 0.135
output = 0.55
cache_read = 0.035

[[routing.price]]          # default
input = 0.27
output = 1.10
cache_read = 0.07
```

## Fields and gate rules

| Key | Type | Rule |
|---|---|---|
| `cache.mode` | enum | required when `[routing.cache]` is present |
| `cache.lifetime` | duration | > 0; ≤ 24h |
| `cache.min_tokens` | integer | ≥ 0; only with `automatic` |
| `window[].name` | string | unique within the plugin; 1–64 chars; may use `*` to match reported names (`weekly *`) |
| `window[].length` | duration | > 0 |
| `window[].unit` | enum | required |
| `window[].capacity` | number | > 0 |
| `window[].token_weights` | table | keys ⊆ `input`, `output`, `cache_read`, `cache_write`; values ≥ 0; only with `weighted_tokens` |
| `window[].model_multiplier` | glob → number | numbers > 0 |
| `window[].reserve` | percent | 0–50% |
| `window[].reset` | enum | only for a window no `[quota]` rule names; `fixed` needs `anchor` (`HH:MM±hh:mm`, or `D HH:MM±hh:mm` for monthly or weekly) |
| `price[].input`/`output`/`cache_read`/`cache_write` | number | ≥ 0; `input` required |
| `price[].when.days` | list | ⊆ `mon`…`sun` |
| `price[].when.from`/`to` | `HH:MM` | `to` before `from` wraps past midnight |
| `price[].when.offset` | `±hh:mm` | default `+00:00` |
| `price[]` | list | at most one entry without `when`, and it comes last |

`nullrouter check` also reports:
- a reported quota window with no matching meter: paced in its own unit, capacity assumed;
- a pay-as-you-go provider with no price;
- an `explicit` cache mode on a provider none of whose endpoints speaks a style with cache
  markers.

## Defaults when the section is absent

| Missing | Effect |
|---|---|
| `[routing.cache]` | mode `automatic`, lifetime 5m, min_tokens 1024 |
| `window[]` | reported windows are paced in their own unit, with capacity assumed; an account with no report is pay-as-you-go |
| `reserve` | 5% |
| `price[]` | price 1 (ranking only), with a `check` warning |

## Account overrides (`accounts.toml`)

```toml
[[account]]
provider = "anthropic"
name = "max"
kind = "signin"
priority = 2

[account.routing]
cache_lifetime = "1h"
reserve = "10%"
window."5-hour" = { capacity = 12_000_000 }
# price = { input = 3.0, output = 15.0 }   # flat price, replaces the schedule
```

The override keys are the meter keys above. Validation is the same. A window name that matches no
window is refused.
