# Scope brief: latency slice 1, phases and live view

Shaped 2026-10-07 with /shape-spec in `.worktrees/012` (while a parallel session planned 012), then
saved in `.worktrees/013`, branch `013-latency-phases-live`, cut from `009-dashboard` at `9696394`.
Slices 004, 010, 011 and 012 run in their own worktrees; this slice takes 013. **Before
`/speckit-specify`, run `/speckit-constitution` to amend Principle VIII** (row 20).

## Core map (2026-10-07)

| Capability | Status | Evidence |
|---|---|---|
| Unified models, plugin-declared providers | shipped | 002 |
| Request execution, retry, fallback | shipped | 003 |
| Non-text model types | shipped | 003 `crates/nullrouter-server/src/media.rs` |
| Clean upstream for optimizers | shipped | 003 harness runs (headroom) |
| Account sign-in, quota polling | shipped | 005 |
| Routing decision (warm cache, per agent, amortization, priority) | shipped | 006 |
| Records and read model | shipped | 008 |
| Dashboard pages | partial | 009 built; the user's look review is open |
| Usage totals; latency median and p95 per agent and provider | in progress | 010, T009 of 52 |
| Model tests, combos, nested combos | in progress | 011, implement next |
| Harness adapters, Claude Code connection | in progress | 004 |
| Quota fit, outside use, leak detection | in progress | 012, clarified (`1dbb7d4`), plan running |
| Latency phases and live view | absent → this slice | Records hold only `ttft_ms`/`total_ms` (`crates/nullrouter-engine/src/records.rs`); no in-flight view (010 Out of scope: "live state … later") |
| Connection settings tied to phases | absent → this slice | Plugins declare `timeout_ms`/`stall_timeout_ms` per endpoint with env fallbacks (`upstream.rs`); no operator override, proxy, HTTP/2 or retry settings |
| Latency trends, per unified model, bottleneck call | absent → latency slice 2 | 008/009/010 defer to "the latency slice"; needs 010's summary code |

`main` holds 002, 003 and 005 only; 006–009 sit on a stacked branch that hasn't been merged.

## Playback (confirmed)

Every request now records how long each step took: 0router's own work, connecting, waiting for the
provider, the first token, the rest of the answer, and delivery to your client. It records this for
every attempt, failed ones included. Steps that didn't happen show as "not applicable", never as
zero. A live CLI view shows the requests in progress: who sent them, where they went, which step
they're on and for how long. It shows no content. You can then act on what you see. Per provider,
you can set connect, header, first-token and stall timeouts, a proxy (yours only, never a plugin's),
connection reuse/HTTP2, and how retries and backoff behave. Changes apply from the next request
without a restart. Latency never steers routing by itself, and the constitution is amended first to
say so. Trends, per-unified-model figures, the bottleneck verdict and the dashboard come later.

## Ledger

| # | Tag | Claim | Source |
|---|---|---|---|
| 1 | U | 013 is latency slice 1: phases and the live view | "Latency 1: phases + live" (over one full latency slice after 010, and over holding off) |
| 2 | M | Six phases: router overhead, connect, headers, first token, generation, delivery; headers and first token may merge as "waiting for provider" | 2026-09-28 |
| 3 | M | Measured from real traffic; active tests only on request | 2026-09-28 |
| 4 | M | Live view of requests in progress: current phase plus finished-phase times | 2026-09-28 |
| 5 | M | Latency never steers routing; the operator acts by hand | 2026-09-27, reaffirmed 2026-10-07 |
| 6 | M | Connection settings tied to phases (reuse/HTTP2, proxy, timeouts, retry policy) | 2026-09-28 |
| 7 | K | Plugins are data and never see secrets | Constitution I |
| 8 | C✓ | Phase times per attempt, failed included, in `nullrouter records` | "Yes, that's it" (outcome) |
| 9 | C✓ | Live CLI view; refreshes until quit; `--json` gives one snapshot | "Yes, that's it"; "Yes" |
| 10 | C✓ | Connect, headers, first-token and stall timeouts: plugin default → operator per-provider override wins → built-in default | "Plugin default, you override" |
| 11 | C✓ | Phases that don't apply are "not applicable", never 0 ms | "Yes" |
| 12 | C✓ | Out: trends over time → latency 2 | "Yes, latency 2" |
| 13 | C✓ | Out: per-unified-model latency → latency 2 | "Yes, latency 2" |
| 14 | C✓ | Out: bottleneck call against the provider's history → latency 2 | "Yes, latency 2" |
| 15 | C✓ | Out: dashboard phases and live view → later | "Yes, later" |
| 16 | U | Connection reuse/HTTP2 and proxy per provider are in | "No, include them" |
| 17 | U | Retry policy settings (retry count, backoff) are in | "No, include it" |
| 18 | C✓ | Proxy is operator-only, per provider or for all; a plugin that declares one fails validation; proxy credentials are kept like account secrets | "You only, never a plugin" |
| 19 | C✓ | Reuse/HTTP2: plugin declares, operator overrides. Retry: plugins may declare same-account retries up to a validated maximum, operator overrides per provider, core default otherwise. ~~Plugins can't set retry~~, revised 2026-10-07 in specify Q2: four plugins already declare retries from 9router (VI parity) | "Split by who knows"; specify Q2 → A |
| 20 | C✓ | Constitution VIII amended before specify: measurements inform the operator; they don't change routing | "No; amend VIII first" |
| 21 | C✓ | Fails if measuring slows requests | failure pick |
| 22 | C✓ | Fails if phases don't add up to the total or are misattributed | failure pick |
| 23 | C✓ | Fails if the live view misses a request, shows the wrong phase, keeps an ended one, or lags | failure pick |
| 24 | C✓ | Fails if a timeout cuts a stream that was still progressing | failure pick |
| 25 | C✓ | First token = any model output, thinking included; keepalive pings don't count | "Yes, any model output" |
| 26 | C✓ | One definition: phases match 010's router overhead and first token | "Yes, one definition" |
| 27 | C✓ | Live view shows metadata only, never content | "Yes, metadata only" |
| 28 | C✓ | Settings apply from the next request without a restart; in-flight requests keep theirs | "Yes, next request" |
| 29 | C✓ | One slice; stories ordered phases → live → timeouts → proxy → reuse/HTTP2 → retry; the first two are a working MVP | "One slice, ordered stories" |
| 30 | K | Latency becomes something measured and visible, not something noticed | init.md "Why it exists" |
| 31 | C✓ | Out: proxy pools (0router deploying or testing proxies) → later | "Yes, out → later" |
| 32 | C✓ | An unreachable proxy: 0router tells the operator and pauses traffic through it until fixed; never sends direct; no account cooldown | "Announce user that proxy is not reachable, and pause for fix" (specify Q1) |

## P notes for research.md

- 9router: `open-sse/utils/proxyFetch.js` and `chatCore.js` route upstream calls through a per-connection
  proxy; `src/app/api/proxy-pools/` deploys and tests proxies (out, row 31). Timeouts come from
  `FETCH_CONNECT_TIMEOUT_MS` / `STREAM_STALL_TIMEOUT_MS` (`open-sse/config/runtimeConfig.js`) with
  `this.config.timeoutMs` per executor; retry from `RETRY_CONFIG`/`resolveRetryEntry` in
  `executors/base.js`. 9router has no per-phase timing and no live view: no oracle for rows 8–9.
- 0router today: header timeout `upstream.rs` (`timeout_ms` → env `FETCH_CONNECT_TIMEOUT_MS`), stall
  `upstream::stall_timeout` (`stall_timeout_ms` → env `STREAM_STALL_TIMEOUT_MS`), used in
  `attempt.rs`; account cooldown backoff constants in `classify.rs` (`BACKOFF_BASE_MS` 2 s,
  `BACKOFF_MAX_MS` 5 min); same-account retry budget per 003 FR-015; connection reuse always on per
  003 FR-021. `Attempt` already has `started`/`ended` (ms from arrival); phases extend it.
- Connect time is only visible per request if the HTTP client exposes it; on a reused connection
  connect is "not applicable" (row 11). Check what reqwest/hyper expose before promising separate
  connect vs headers on every request; row 2 allows merging headers and first token.
- Row 26: 010's definitions live in `journal/summary.rs` (`first_token_attempt`, `own_ttft`) on
  branch `010-dashboard-summaries`, not yet merged; 013 must reuse them, so coordinate with 010.
- Row 21 needs a criterion bench on the request path (router overhead with and without phase
  timing); baselines stay local.
- Settings follow the existing operator-file pattern: written atomically, then the running `serve`
  reloads (`docs/operator-config.md`), which gives row 28.
- Proxy credentials sit with account secrets (mode 0600) and must pass the same redaction tests as
  the dashboard and CLI secret scans (009 T069).

## Final command

```text
/speckit-specify Latency slice 1, phases and live view: latency becomes something the operator sees for each request and while it happens, and can act on, without latency ever changing routing by itself.

Every request's record holds, for each attempt including failed ones, the time spent in six phases: router overhead, connect, response headers, first token, generation, and delivery to the client. Where response headers and first token can't be told apart, they are shown as one "waiting for provider" phase. A phase that didn't happen for a request is shown as not applicable, never as zero. First token means the first model output of any kind, thinking included; keepalive pings don't count. Router overhead and first token use the same definitions as the latency view of slice 010, so a request's record and `nullrouter latency` never disagree. Phase times appear in `nullrouter records`. Latency is measured from real traffic only.

A live CLI view lists the requests in flight now: agent, target, provider and account, the phase each one is in and for how long, and the times of the phases already finished. It refreshes until the operator quits; `--json` prints one snapshot. It shows metadata only, never prompt or answer content.

Per provider, the operator can tune the connection settings tied to these phases:
- timeouts for connect, response headers, first token and stall: the plugin declares defaults, the operator's per-provider override wins, and a built-in default covers anything neither sets;
- a proxy, per provider or for all providers, set only by the operator: a plugin that declares a proxy fails validation, and proxy credentials are kept like account secrets and never shown;
- connection reuse and HTTP/2: the plugin declares what the provider supports, and the operator overrides per provider;
- the retry policy (same-account retry count and backoff): a core default that the operator overrides per provider; plugins can't set it, because each retry can spend the operator's quota.
A settings change applies from the next request without restarting `serve`; requests already in flight keep the settings they started with.

User stories in priority order: phases in records, live view, timeouts, proxy, connection reuse and HTTP/2, retry policy. The first two alone are a working result.

The slice fails if measuring makes requests measurably slower; if a record's phases don't add up to its total or are attributed to the wrong side; if the live view misses a request in flight, shows the wrong phase, keeps an ended one, or lags behind; or if a timeout cuts a stream that was still progressing, such as a reasoning model before its first visible text.

Constraints: latency never changes routing by itself; the operator acts through priority and these settings (constitution VIII, as amended before this spec). Plugins stay data and never see secrets (constitution I).

Out of scope:
- Latency trends over time → latency slice 2.
- Latency per unified model → latency slice 2.
- Judging a phase unusual against the provider's own history (the bottleneck call) → latency slice 2.
- Phases and the live view on the dashboard → later.
- Proxy pools (0router deploying or testing proxies for you) → later.

Scope brief: specs/briefs/2026-10-07-latency-phases-live.md
```

## Trace

| Command sentence | Rows |
|---|---|
| Purpose; latency never steers routing | 1, 5, 20, 30 |
| Six phases, per attempt, failed included | 2, 8 |
| "Waiting for provider" merge | 2 |
| Not applicable, never zero | 11 |
| First token = any output, pings excluded | 25 |
| Same definitions as 010 | 26 |
| In `nullrouter records` | 8 |
| Real traffic only | 3 |
| Live view contents, refresh, `--json`, metadata only | 4, 9, 27 |
| Timeouts | 6, 10 |
| Proxy | 16, 18, 7 |
| Reuse/HTTP2 | 16, 19 |
| Retry policy | 17, 19 |
| Applies from the next request | 28 |
| Story order, MVP | 29 |
| Failure signals | 21–24, 25 |
| Constraints | 5, 20, 7 |
| Out of scope | 12, 13, 14, 15, 31 |
