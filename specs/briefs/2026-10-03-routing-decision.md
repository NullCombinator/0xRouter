# Scope brief: routing decision and persistent request history (slice 006)

Shaped with `/shape-spec` on 2026-10-03. The user chose to shape 006 next, over building 004
first or a close-out slice. In `/speckit-clarify` and `/speckit-plan`, an answer that
contradicts a confirmed row below means stop and revisit this brief. Don't accept it.

## Core map (at `7cd82d9`, branch `005-account-sign-in`)

| Capability | Status | Evidence |
|---|---|---|
| Provider plugins as data, validation gate, reload | shipped (no `nullrouter reload` command) | slice 002, `nullrouter-registry` |
| Unified models as routing targets | shipped | 002 `resolve.rs` |
| Four client API styles and translation | shipped | 003, `nullrouter-wire` |
| Request execution, streaming, cancellation | shipped | 003 `attempt.rs`, `relay.rs` |
| Retry, fallback, informational errors | shipped | 003 `f94e005` |
| Non-text types, mid-stream continuation | shipped, live checks open (T087, T096) | 003 |
| Access keys, agent identity | shipped | 003 `keys.rs` |
| Account sign-in, xai, grok-cli | shipped, live checks open (T045, T056, T078, T097) | 005, 96/100 |
| Quota polling, poll history with token tally on disk | shipped | 005 `quota/history.rs` |
| Cache-aware routing | partial: last-serving account per (agent, target), no cache lifetime, in memory | `plan.rs` `WarmMap` |
| Per-agent isolation | partial: the warm map is keyed per agent | same |
| Windowed amortization | absent; design approved 2026-09-28; unified members tried in declared order | `plan.rs` |
| Request records, latency | partial: in memory only | 003 `records.rs` |
| Quota fit, leak detection | absent | → 007 |
| Harness adapters (hermes, WASM) | specified, 0/98 built | 004 |
| Model tests, combos, dashboard, community sign-in | absent | → later |

Open threads: `main` holds none of 003–005 (branch `005-account-sign-in` carries all of it);
005 live checks T045, T056, T078, T097; 003 live checks T060, T087, T096.

## Why now

005 delivered the inputs routing needs: real subscription windows, polled quota and per-poll
token tallies. The routing design is approved and recorded, and the slice order (005 → 006)
is recorded. Today a unified model is an ordered fallback list, the widest gap between
0router and init.md.

## Playback (confirmed)

Several agents can share one model backed by several accounts: subscriptions and
pay-as-you-go keys, on one provider or across providers. 0router remembers, per agent, which
account already holds each prompt's cache, and keeps that agent there. It moves the agent
only when the account hits a limit, or to get off a paid key once a subscription is free.
Work that has nothing cached anywhere goes to the subscription furthest behind its pace for
the current window, so quota doesn't expire unused. Request-capped accounts get the large
requests. Accounts without a quota report are paced from 0router's own count and labelled
"estimated". Paid keys are only overflow, chosen by priority and current price. You set
priorities and see each account's pace, share and backlog from the CLI. Every request, and
why it went where it did, survives restarts until you prune it. A simulated week plus a live
check prove it.

## Ledger

| # | Tag | Claim | Source |
|---|---|---|---|
| 1 | U | Shape 006 next | picked "006 routing" |
| 2 | C✓ | Outcome: agents stay on warm accounts; cold work → subscription furthest behind pace; PAYG only overflow; every request and why it went where survives restart | "Yes, that's it" |
| 3 | U | Fails if a warm session is moved for balancing, not capacity | picked "Warm cache broken" |
| 4 | U | Fails if a subscription window resets with quota left while work went to PAYG or another account (cold work only, see row 9) | picked "Quota expires unused" |
| 5 | U | Fails if the operator can't tell from the CLI or record why a request went where | picked "Can't see why" |
| 6 | U | Fails if a restart or crash loses records or routing state (warm sessions, deficits) | picked "History lost" |
| 7 | M 2026-09-28 | Objectives: minimize token waste, then spread load; no forecasting | approved design |
| 8 | M 2026-09-28, C✓ | Strict warm, shared prefix (system prompt + tools) included; cold = no warm prefix anywhere or idle past cache lifetime | "Yes, strict as on 09-28" |
| 9 | C✓ | Exception: a warm session on PAYG moves back once a subscription can serve | "Strict, except leave PAYG" |
| 10 | M 2026-09-28 | Pace π, weight r × g(π) × priority, share, deficit placement; reserve floor; short windows admission-only; deficits reset at amortization-window boundary | approved design |
| 11 | M 2026-09-28 | PAYG overflow tier by priority ÷ price in effect now, from a plugin-declared price schedule | 09-28 direction |
| 12 | M 2026-09-28 | Between polls, 0router subtracts its own counted traffic; each poll corrects | 09-28 decision (3) |
| 13 | C✓ | Balancing unit is the account | "Yes, per account" |
| 14 | C✓ | Direct provider/model targets with several accounts are spread the same way | "Yes, direct too" |
| 15 | C✓ | No quota report → paced from own count vs plugin-declared limits, shown "estimated"; neither → PAYG | "Yes" |
| 16 | C✓ | Plugin declares cache lifetime; operator overrides per account | "Yes" |
| 17 | C✓ | Warm tracking by prompt-prefix fingerprints (hashes, never content) per agent | "Prefix, per agent" |
| 18 | C✓ | Request-limited windows compared using the size of the request being placed | "Yes, in 006" |
| 19 | C✓ | CLI priority per account (0 = never for cold work) and a routing view (pace, share, deficit, polled/estimated) | "Yes, CLI" |
| 20 | C✓ | Records (attempts, latency, usage, routing reason) on disk; kept until the operator prunes | outcome + "Keep until pruned" |
| 21 | C✓ | Evidence: simulated-clock week replay over mock accounts + opt-in live check of the routing view against polls | "Simulation + live smoke" |
| 22 | C✓ | One slice | "One slice" |
| 23 | C✓ | Out: fitted weights, leak and outside-use detection → 007 | "Yes, fit in 007" |
| 24 | C✓ | Out: automatic responsiveness → later; manual priority in 006 | "Manual in 006" |
| 25 | C✓ | Out: dashboard → later | "Yes, CLI only" |
| 26 | M 2026-09-28, C✓ | No runtime check of per-member limits; a CLI note when a unified model's members' limits differ, in 006 | "No check; note in 006" |
| 27 | M 2026-09-27 | Standing failure signals; tested with more than one harness | shape-spec 003 |
| 28 | M 2026-10-02 | Minimum friction: client's request forwarded as received | 005 analyze Q6 |
| 29 | K | init.md: cache-aware routing, per-agent isolation, amortization over a configurable window; plugins declare, user overrides | init.md "What it is" |
| 30 | K | Principle I: plugins are data only and never see secrets | constitution |

## P notes for research.md

- Today's stay-warm is `WarmMap` (`crates/nullrouter-engine/src/plan.rs`), keyed (agent,
  target) → last-serving account, in memory, with no expiry. 006 replaces it with
  per-agent prefix fingerprints that persist.
- Fingerprint boundaries: decide which prefixes to hash per style (Anthropic
  `cache_control` breakpoints; OpenAI's automatic prefix caching; Gemini implicit cache).
  Only hashes are stored (row 17).
- Percent windows need token weights to make sustainable rates comparable across
  accounts. Use the quota-meter shape from 005 (unit; input/output/cache_read/cache_write
  weights) and the 005 tally; the fit itself is 007.
- Concurrent cold placements can race to the same largest deficit; debit tentatively at
  placement and settle on completion (Claude's call).
- Crash durability (row 6) vs. hot-path cost: pick the write and fsync cadence from a
  bench. 005's JSONL + advisory lock + 0600 history is the starting pattern.
- π cap (e.g. 10), deficit clamp bound, tie order (higher share, then fixed order) and
  the reserve floor default are technical; the floor is plugin-declared with an operator
  override (row 29).
- 9router reference: `getProviderCredentials` (`src/sse/services/auth.js`, fill-first /
  round-robin with a sticky limit) and `open-sse/services/combo.js`. Neither paces by
  quota; they inform only tie-breaking and equal-state behaviour. No parity target for
  the placement rule itself.

## Final command

```
/speckit-specify Routing decision and persistent request history: a target backed by several accounts spends subscription quota before it expires, keeps warm caches, and remembers every decision. Several agents can share one target backed by several accounts (subscriptions and pay-as-you-go keys, on one provider or across providers). The account, not the provider, is the unit being balanced, and direct provider/model targets with several accounts are spread the same way as unified models. Objectives, in order: waste as few tokens as possible, then spread load across accounts. Decisions use only the present state, never forecasts. Warm first: 0router keeps, per agent, fingerprints (hashes, never content) of prompt prefixes and which account holds each one cached. A request whose prefix that agent has warm on an account stays there, the shared system prompt and tools included. A warm request moves only when its account can't serve it (a rate limit or the reserve floor), or to leave a pay-as-you-go account once a subscription can serve. Each provider's plugin declares how long its prompt cache lives, and the operator can override it per account; past that idle time the prefix is cold. Cold work (no warm prefix on any account, or idle past the cache lifetime) is placed by pace over a configurable amortization window. A quota window's pace is the share of its quota left divided by the share of its time left. An account's weight is its sustainable rate times its capped pace times the operator's priority, and shares follow the weights. Each account keeps a deficit of work owed against its share, cold work goes to the largest deficit, and deficits reset at each amortization-window boundary. Short windows such as per-minute limits only admit or refuse. Windows that count requests are compared with token windows using the size of the request being placed, so large requests favour request-limited accounts. Between polls, 0router subtracts its own counted traffic from the last polled quota, and each poll corrects the estimate. An account whose provider reports no quota is paced from 0router's own count against limits its plugin declares and is shown as "estimated"; an account with neither is pay-as-you-go. Pay-as-you-go accounts are overflow only, used when no subscription can serve, and chosen by priority divided by the price in effect now, from a price schedule the plugin declares. The operator sets a priority per account from the CLI (0 means never for cold work), and a CLI routing view shows each account's pace, share, deficit, and whether its quota is polled or estimated. Every request record (attempts, latency, usage and the reason for each placement) is kept on disk and survives restarts and crashes, as do warm fingerprints and deficits; nothing is dropped until the operator prunes. When the operator creates or updates a unified model whose members' limits (such as context size) differ, the CLI notes it. The slice fails if: a warm session is moved for balancing rather than capacity; a subscription window resets with quota left while cold work went to pay-as-you-go or another account; the operator can't tell from the CLI or the record why a request went where it did; a restart or crash loses records, warm state or deficits; a standard client breaks on 0router; a single 429, 5xx or timeout reaches the client when another account or provider could serve; usage numbers are missing or wrong; a secret appears in a log, record, error or anything a plugin sees. Evidence: a simulated-clock test replays a week of traffic over mock accounts with real-shaped windows and checks that cold work met each window's target share, no warm session moved for balancing, pay-as-you-go served only overflow, and a restart mid-run lost nothing; an opt-in live check on the operator's real accounts shows the routing view matching the polls. Tests use more than one client harness. Constraints: plugins are data, never code, and never see secrets; plugins declare and the operator overrides; the client's request is forwarded as received; there is no runtime check that a request fits each member's limits, so a rare misfit fails and ordinary retry and fallback move on. Out of scope: fitted quota weights, leak detection and outside-use detection → slice 007; automatic responsiveness (0router lowering a slow account's share by itself) → later; a dashboard → later. Scope brief: specs/briefs/2026-10-03-routing-decision.md
```

## Trace

| Sentence | Rows |
|---|---|
| Purpose; several agents, several accounts, cross-provider | 1, 2 |
| Account is the unit; direct targets too | 13, 14 |
| Objectives; no forecasts | 7 |
| Prefix fingerprints per agent, hashes only; shared prefix warm | 8, 17, 29 |
| Warm moves only on capacity, or off PAYG | 3, 8, 9 |
| Cache lifetime declared, overridable | 16, 29 |
| Cold definition; pace, weight, share, deficit; configurable window; reset | 8, 10, 29 |
| Short windows admit or refuse | 10 |
| Request-limited conversion | 18 |
| Between-poll estimate | 12 |
| Estimated pacing; neither → PAYG | 15 |
| PAYG overflow, price schedule | 2, 11 |
| CLI priority and routing view | 5, 19 |
| Records, warm state and deficits on disk; prune | 5, 6, 20 |
| Unified-model limits note | 26 |
| Failure list | 3–6, 27 |
| Evidence; more than one harness | 21, 27 |
| Constraints | 26, 28, 29, 30 |
| Out of scope | 23, 24, 25 |
