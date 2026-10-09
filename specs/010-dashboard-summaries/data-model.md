# Data model: Dashboard summaries and landscape

What this slice adds to the operator home, the engine and the read model. Spec 009's
[data model](../009-dashboard/data-model.md) is unchanged except where this file says so.
Decisions are in [research.md](research.md) (R-numbers).

## Operator home

### `keys.toml`: one optional field (R7)

```toml
schema = 1

[[key]]
id = "ak_7f3k2m9q"
name = "claude-code"
digest = "sha256:…"
last4 = "9f2a"
created = "2026-10-01T09:12:00Z"
harness = "claude-code"        # NEW, optional; absent when the key has no tag
```

| Field | Type | Rule |
|---|---|---|
| `harness` | string, optional | trimmed, 1 to 32 Unicode scalar values, no control characters (`keys::check_harness`). Written only when set; `keys tag --clear` removes the line. |

There is no schema bump. A file without tags is unchanged. A binary older than this slice refuses
a file with a tag (`deny_unknown_fields`); see R7.

No other home file changes. The summaries read the journal (`records/*.jsonl`) and write nothing.

## Engine: `journal::summary` (R3 to R6)

### Window

| Field | Type | Meaning |
|---|---|---|
| `from` | `Option<SystemTime>` | inclusive start; `None` for All |
| `to` | `SystemTime` | exclusive end, the read time `at` |

Segments read: every `records/YYYY-MM-DD.jsonl` whose UTC day lies between `day_of(from)` and
`day_of(to)`, both included (all segments when `from` is `None`).

### `Totals`

| Field | Type | Meaning |
|---|---|---|
| `requests` | u64 | records with `from <= arrived < to` and an agent or not (every request the router took) |
| `in_flight` | u64 | of those, still unfinished (`in_progress`) |
| `not_reported` | u64 | finished, with no `usage` |
| `input` | u64 | uncached input tokens (R4) |
| `cached` | u64 | cache-read tokens |
| `output` | u64 | output tokens |
| `cost_usd` | f64 | Est. Cost of the priced records |
| `unpriced` | map reason → u64 | finished records with tokens left out of the cost: `no_price`, `account_gone`, `no_output_price` |
| `agents` | map key id → u64 | requests per agent key |
| `providers` | map provider id → u64 | requests with a non-skipped attempt on the provider |

Totals add up across segments, which is what makes the segment cache possible (R3).

### `SegmentTotals` cache entry (server only)

| Field | Type | Meaning |
|---|---|---|
| key | (path, inode, length, reload generation) | any change recomputes the entry |
| value | `Totals` for the whole UTC day | used only when the day lies wholly inside the window |

It lives in the server's engine state beside the segment index. It is dropped for a segment that
is pruned or forgotten (inode change) and wholesale on reload (generation change). There is no
size limit beyond one entry per segment file: 60 days of history means 60 small entries.

### `Latency`

| Field | Type | Meaning |
|---|---|---|
| `agents` | map key id → `AgentLatency` | rows for agents with requests in the window |
| `providers` | map provider id → `ProviderLatency` | rows for providers with a non-skipped attempt in the window |

`AgentLatency`: `requests` u64; `overhead` `Pct?`; `ttft` `Pct?`; `last` `Last?`.

`ProviderLatency`: `requests` u64; `own_ttft` `Pct?`; `agents` map key id → u64; `last` `Last?`.

`Pct`: `p50` f64 ms, `p95` f64 ms, `n` u64 (values counted). It is null when no request has a
value.

`Last`: `result` (`resolved` | `failed`), `at` (RFC 3339), and for a provider, `status`
(the HTTP status of a failed attempt, if any).

### Derived per record (R6)

| Name | Rule |
|---|---|
| first attempt | first attempt whose `kind` isn't `skipped` |
| router overhead | first attempt's `started` (ms from arrival) |
| first-token attempt | last non-skipped attempt with `started <= ttft_ms` |
| provider's own TTFT | `ttft_ms − first_token_attempt.started` |
| serving account | `served_by.{provider, account}` |

### Pricing (R5)

`Rates { input: f64, output: Option<f64>, cache_read: Option<f64>, cache_write: Option<f64> }`,
per million tokens, USD, from `price::entry_at(spec, arrived)`.

```text
uncached_input = input normalized to exclude cache_read and cache_write
cost = uncached_input · in
     + cache_read  · (cache_read  rate or in)
     + cache_write · (cache_write rate or in)
     + output      · out            (no out rate and output > 0 → unpriced: no_output_price)
     / 1e6
```

## Read model views

### `usage` view (`nullrouter usage --json`)

Args: `period` (`today` | `24h` | `7d` | `30d` | `60d` | `all`, default `today`), `at` (RFC 3339,
optional).

```json
{
  "period": "today",
  "at": "2026-10-06T14:02:11Z",
  "from": "2026-10-05T22:00:00Z",
  "to": "2026-10-06T14:02:11Z",
  "zone": "Europe/Berlin",
  "requests": 214,
  "in_flight": 1,
  "not_reported": 3,
  "tokens": { "input": 202000, "cached": 410000, "output": 89000 },
  "cost": {
    "usd": 1.8412,
    "label": "Estimated, not actual billing",
    "note": "Priced with today's declared prices; earlier price changes are not tracked.",
    "unpriced": { "requests": 12, "no_price": 9, "account_gone": 0, "no_output_price": 3 }
  },
  "agents":    [ { "id": "ak_7f3k2m9q", "name": "claude-code", "requests": 150 } ],
  "providers": [ { "id": "anthropic", "requests": 120 } ],
  "warnings": []
}
```

`agents` lists every key with requests in the window (name from `keys.toml`, or null for a deleted
key); `providers` lists every provider with requests. Both are sorted by requests, descending,
then id. `warnings` carries the "records were not kept" warning `records list` prints, when the
window touches such a period.

### `latency` view (`nullrouter latency --json`)

Args: `at` (optional).

```json
{
  "window": "last 24 h",
  "at": "2026-10-06T14:02:11Z",
  "from": "2026-10-05T14:02:11Z",
  "to": "2026-10-06T14:02:11Z",
  "agents": [
    { "id": "ak_7f3k2m9q", "name": "claude-code", "requests": 612,
      "overhead": { "p50": 5, "p95": 9, "n": 612 },
      "ttft": { "p50": 420, "p95": 1910, "n": 598 },
      "last": { "result": "resolved", "at": "2026-10-06T14:01:58Z" } }
  ],
  "providers": [
    { "id": "anthropic", "requests": 542,
      "own_ttft": { "p50": 410, "p95": 1900, "n": 530 },
      "agents": [ { "id": "ak_7f3k2m9q", "requests": 512 } ],
      "last": { "result": "resolved", "at": "2026-10-06T14:01:58Z" } }
  ],
  "warnings": []
}
```

### `keys` view: one field (R7)

Each row gains `"harness": "claude-code"` or `null`. Its position in the list is also the agent's
colour index (R8).

## Operator ops (server)

| Op | Args | Answer |
|---|---|---|
| `usage.totals` (new) | `from` (RFC 3339 or null), `to` | `Totals` as JSON, cache and ring included |
| `latency.summary` (new) | `from`, `to` | `Latency` as JSON, ring included |

Both are added to `views::request` (`views/mod.rs:106`) and to the protocol table in
`operator.rs`'s doc comment. Names (keys, accounts) are joined in the view, not in the op.

## Dashboard

| Page | Views read (new in bold) | Fills |
|---|---|---|
| Endpoint & Key | keys, check, **latency**, **usage(today)**, accounts | landscape; "requests today"; harness badge |
| Providers | (009's) + **latency** | window "last response" |
| Usage | records, record, check, **usage(period)**, **latency** | period filter, five cards, topology graph |

The style tokens gain `[token.agent-1]` to `[token.agent-7]` (R8) and `[component.gauge]`,
`[component.pipe]` and `[component.topology]` (R9, R10), each naming its 9router source.
