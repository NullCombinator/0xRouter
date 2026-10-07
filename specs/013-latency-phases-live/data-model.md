# Data Model: Latency Slice 1, Phases and Live View

All times are milliseconds from the request's arrival, measured on a monotonic clock (FR-015),
as `f64`, like today's `started`/`ended`/`ttft_ms`/`total_ms`.

## AttemptTiming (new, on each `Attempt` in a request record)

`records.rs`: `Attempt.timing: Option<AttemptTiming>`. Records written before this slice don't
have it, and views show their phases as `not recorded`.

| Field | Type | Meaning |
|---|---|---|
| `retry_wait_ms` | `Option<f64>` | deliberate wait before this same-account retry (R1, clarify Q4) |
| `refresh_ms` | `Option<f64>` | sign-in token refresh before this attempt; reported inside connect (R6) |
| `connected` | `Option<f64>` | connection ready; `None` = reused or never got that far |
| `connection` | `new` \| `reused` \| `none` | `none` when the attempt ended before a connection (skipped, refused at build) |
| `http` | `Option<"1.1" \| "2">` | from the response |
| `proxy` | `Option<String>` | the proxy's **name** only |
| `headers` | `Option<f64>` | response headers arrived |
| `first_output` | `Option<f64>` | first model output from the provider (R17) |
| `merged_wait` | `bool` | headers and first output came together (R13) |
| `upstream_done` | `Option<f64>` | last byte from the provider |
| `blocked_ms` | `f64` | time the engine waited on the client channel during this attempt (R4) |
| `closing_ms` | `Option<f64>` | the serving attempt only: wait for the record's close line, inside delivery |
| `timeout` | `Option<TimeoutHit>` | the timeout that ended the attempt |

`TimeoutHit { which: connect|headers|first_token|stall, ms: u64, source: Source }`

`Source { by: operator|plugin|built_in, level: model|provider|endpoint|env|default }`

Invariants:
- The marks are non-decreasing in this order: `started ≤ connected ≤ headers ≤ first_output ≤
  upstream_done ≤ ended`, for those present.
- `connection == reused` ⇒ `connected == None`.
- A skipped attempt has `timing` with every mark `None`.

## Phases (derived, never stored)

`phases::of(&RequestRecord) -> Vec<AttemptPhases>` (engine, `crates/nullrouter-engine/src/phases.rs`).

`AttemptPhases { n, phases: [PhaseValue; 7], merged_wait, ended_in: Option<Phase>, slowest: Option<(Phase, f64)> }`

`Phase = router_overhead | retry_wait | connect | headers | first_token | generation | delivery`.
When `merged_wait` is set, `headers` and `first_token` show as one `waiting_for_provider` value.

`PhaseValue = Ms(f64) | NotApplicable | NotRecorded | InProgress(f64)`

The formulas are in research R1. Validation rules:
- For a completed request, the sum over attempts of every `Ms` equals `total_ms` within 1 ms
  (FR-009).
- For the attempt that produced the request's first output, the phases up to and including
  `first_token`, summed over it and every earlier attempt, equal `ttft_ms` within 1 ms.
- The request-level router overhead is the first non-skipped attempt's router overhead. Slice
  010's summary calls this function (FR-010).

Side of a phase (the list column and SC-010):

| Phase | Side |
|---|---|
| router overhead | 0router |
| retry wait | retry |
| connect | network |
| headers, first token, waiting for provider, generation | provider |
| delivery | client |

## AttemptClock and LiveEntry (in memory only)

`AttemptClock`: one per running attempt, shared as `Arc` by the attempt loop, the connector layer
(through a task-local) and the live table. Atomic slots for each `AttemptTiming` mark, plus the
attempt's effective `connect_timeout`, which the connector layer reads (R3). `end_attempt`
turns it into an `AttemptTiming`.

`LiveEntry`: one per request in flight, in `engine.live` (`Mutex<HashMap<String, LiveEntry>>`).

| Field | Source |
|---|---|
| `id`, `arrived` (`Instant`), `agent`, `target` | the request |
| `attempt: Option<(n, provider, account, model, Arc<AttemptClock>)>` | set at `start_attempt` |
| `finished: Vec<AttemptPhases>` | earlier attempts' phases, appended at `end_attempt` |

It is inserted at request arrival, before the first `await`, and removed in `end_request`. It
holds no content, headers or secrets (FR-019).

## Effective connection settings (resolved per attempt)

`Effective { connect, headers, first_token: Option, stall: Duration each with Source; reuse: bool;
http: Negotiate | Http1Only; retry: RetryPolicy with Source; proxy: Option<ProxyRef> with level }`

They are resolved from the engine snapshot at `candidate()` time, so an in-flight request keeps
them (FR-031). Precedence:
- **Timeouts**: operator model → operator provider → plugin model → plugin endpoint → built-in
  (env).
- **Retry**: operator status → operator `all` → plugin status → core default.
- **Reuse and HTTP/2**: operator provider → plugin (`http2 = false`) → on / negotiate.
- **Proxy**: account → provider → all → none.

## Proxy

`proxies.toml` (mode 0600) holds `[[proxy]] { name, url, username?, password? }`. The password is
a string or `{ env = "VAR" }`.

Validation:
- `name` matches `[a-z0-9][a-z0-9_-]{0,31}` and is unique; `none` is reserved.
- `url` scheme is `http`, `https` or `socks5`, with a host and port.
- The URL itself has no credentials.

`ProxyState`, in `routing/proxies.json` (mode 0600):

```text
in_use ──probe fails after a connect error──▶ paused {since, reason}
paused ──`proxy fixed` + probe succeeds────▶ in_use
paused ──definition or assignment changed──▶ in_use (re-evaluated on next use)
```

The file holds names, times and reasons only, never addresses or credentials.

## Settings in `config.toml` and `accounts.toml` (additions)

See [contracts/config-files.md](contracts/config-files.md). The entities are:
- `ConnectionSettings` (four timeouts, reuse, http2, proxy) at the all, provider and model
  levels. All doesn't take timeouts; the model level takes timeouts only.
- `RetrySettings` per provider: `all` and per-status `{retries, delay_ms}`.
- `Account.proxy: Option<name | "none">`.

## Plugin schema (additions)

- `Endpoint.connect_timeout_ms: Option<u64>`
- `Endpoint.first_token_timeout_ms: Option<u64>`
- `Model.timeouts: Option<{connect_ms, headers_ms, first_token_ms, stall_ms}>` (`schema/model.rs`)
- `Transport.http2: Option<bool>`: only `false` changes anything
- `Endpoint.retry` entries are capped: `retries ≤ 5`, `delay_ms ≤ 30000`
- Any `proxy`-like key is refused with the operator-only message (R10).
