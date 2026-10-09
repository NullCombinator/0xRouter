# Contract: Request record additions

Each `attempts[]` item gains `timing`, which is stored and journaled, and views add `phases`,
which is derived. A record written before this slice has no `timing`; views show
`"phases": "not_recorded"`.

```json
{
  "n": 2, "provider": "openrouter", "account": "main", "model": "openai/gpt-5",
  "kind": "same_account_retry", "started": 6042.3, "ended": 41000.6, "outcome": {"ok": {}},
  "timing": {
    "retry_wait_ms": 2000.0,
    "refresh_ms": null,
    "connected": null,
    "connection": "reused",
    "http": "2",
    "proxy": "eu-exit",
    "headers": 6043.1,
    "first_output": 9247.8,
    "merged_wait": true,
    "upstream_done": 40988.3,
    "blocked_ms": 0.0,
    "closing_ms": 1.1,
    "timeout": null
  },
  "phases": {
    "router_overhead": 0.3,
    "retry_wait": 2000.0,
    "connect": "not_applicable",
    "waiting_for_provider": 3205.5,
    "generation": 31740.5,
    "delivery": 12.3,
    "ended_in": null
  }
}
```

- `phases` values are a number of ms, `"not_applicable"`, `"not_recorded"`, or
  `{"in_progress": ms}` (views of in-flight requests only).
- `ended_in` names the phase a failed or cancelled attempt ended in. It is `null` for a success.
- When `merged_wait` is false, `phases` has `headers` and `first_token` instead of
  `waiting_for_provider`.
- `timeout`, when set: `{"which":"headers","ms":6000,"source":{"by":"operator","level":"provider"}}`.
- `proxy` holds a name only. Addresses and credentials never appear (FR-027).
- The record-level `ttft_ms` and `total_ms` are unchanged. The phases add up to them (FR-009).

The journal stores `timing` in the attempt's existing end-of-attempt update, so it adds no extra
journal lines.
