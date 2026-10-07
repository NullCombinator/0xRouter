# Contract: dashboard changes

What each page shows after this slice, and the CLI read each value must equal at the page's
`as of` time (FR-026, FR-031). Every page passes its `as_of` as the views' `at` (research R2).
Spec 009's [dashboard contract](../../009-dashboard/contracts/dashboard-http.md) (routes, access,
headers, frame) is unchanged.

## Endpoint & Key (`/`)

| Element | Value | Twin |
|---|---|---|
| Landscape heading | "Agent traffic · last 24 h · as of HH:MM:SS" | `latency` header |
| Agent node | key name, harness badge, revoked badge, colour (research R8) | `keys list` |
| Agent pipe gauge | overhead p50 (needle), p50/p95 printed, requests | `latency` agent row |
| Provider node | provider name, logo or text icon | `providers` |
| Provider pipe gauge | own TTFT p50 (needle), p50/p95 printed, requests, per-agent counts, last response | `latency` provider row |
| No traffic | nodes shown, "No requests in the last 24 hours" | `latency` (empty) |
| Key card "Requests today" | number | `usage --period today`, that agent's row (0 if absent) |
| Key card harness badge | tag, or nothing | `keys list` `HARNESS` |

Nodes listed (FR-020): every unrevoked key, every provider with an account, and any revoked key or
other provider with a `latency` row.

## Providers (`/providers`, window `?provider=<id>`)

| Element | Value | Twin |
|---|---|---|
| Window "Last response" | "resolved · <local time>", "failed · <local time> · <status>", or "None in the last 24 hours" | `latency` provider row `last` |

## Usage (`/usage?period=<p>`)

| Element | Value | Twin |
|---|---|---|
| Period filter | Today, 24h, 7D, 30D, 60D, All; the chosen one marked; a `GET` form | `usage --period` values |
| Card "Total Requests" | requests; small print "N in flight · N not reported" | `usage` `requests`, `in_flight`, `not_reported` |
| Card "Total Input Tokens" | input (uncached), the full number grouped as 9router prints it | `usage` `tokens.input` |
| Card "Cached Tokens" | cached | `usage` `tokens.cached` |
| Card "Output Tokens" | output | `usage` `tokens.output` |
| Card "Est. Cost" | `~$X.XX`, "Estimated, not actual billing", "N not priced" with reasons, the note | `usage` `cost` |
| Topology graph | router centre; each provider with requests in the period, its count; edge red when its last response failed, amber on the newest; label "last response … · last 24 h" | `usage` `providers`, `latency` `providers[].last` |
| Empty period | cards show 0, "No requests in this period" | `usage` |

- An unknown `period` shows Today with a notice "Unknown period; showing Today".
- The Requests table and record window (spec 009) are unchanged and ignore the period.
- No element shows a count of requests in flight now (clarify Q4); "in flight" on the requests card
  is the period's unfinished records, as `usage` prints it.

## Style

Every new element uses tokens from `style/tokens.toml` that name a 9router source (FR-030):
`agent-1` to `agent-7`, `component.gauge`, `component.pipe`, `component.topology`, and the
existing card, badge and filter components. No script, no animation, no external fetch.
