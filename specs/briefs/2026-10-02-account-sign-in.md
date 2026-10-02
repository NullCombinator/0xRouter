# Scope brief: account sign-in (slice 005)

Shaped with `/shape-spec` on 2026-10-02. The user chose to shape 005 next, over shaping 006
(routing) first or implementing 004 first. In `/speckit-clarify` and `/speckit-plan`, an
answer that contradicts a confirmed row below means stop and revisit this brief. Don't
accept it.

## Core map (at `49a1fe2`, plus the uncommitted zerorouter→nullrouter rename)

| Capability | Status | Evidence |
|---|---|---|
| Provider plugins as data, validation gate, reload | shipped | slice 002 (69/69), `nullrouter-registry` |
| Unified models as routing targets | shipped | 002 `resolve.rs`; 003 fallback stays inside one unified model |
| Four client API styles and translation | shipped | 003, `nullrouter-wire`, `styles/bundled/` |
| Request execution, streaming, cancellation | shipped | 003 `attempt.rs`, `relay.rs` |
| Retry, fallback, stay-warm, informational errors | shipped | 003 `f94e005`, `8ea14f1` |
| Non-text model types | shipped, live check pending | 003 `1fce197`; T087 open |
| Mid-stream continuation and restart | shipped, live check pending | 003 `fc2213b`; T096 open |
| Latency and usage records | partial | in memory only (003 `records.rs`) → persistence in 006 |
| Community provider set (fit or refuse) | shipped | 003 `5304802` |
| Access keys, agent identity | shipped | 003 `keys.rs`, `auth.rs` |
| Harness adapters (hermes, WASM sandbox, review) | specified, not built | 004 spec, plan and tasks, 0/98 |
| Account sign-in (OAuth), xai, grok-cli, quota polling | absent | → **this slice (005)** |
| Routing decision (cache-aware, per-agent isolation, windowed amortization) | absent, design approved 2026-09-28 | → 006 |
| Persistent request history | absent | → 006 |
| Combos, model tests, dashboard, community sign-in | absent | → later |

Open threads: 003 operator-run live checks T060, T087 and T096 (004's T001 waits on them);
the zerorouter→nullrouter rename is uncommitted.

## Why now

Windowed amortization (006) spreads work across subscription windows. With only API-key
accounts there are no windows to amortize. 005 gives 006 real subscription accounts, along
with poll and traffic history to fit against. 005 doesn't depend on 004.

## Playback (confirmed)

You sign in a subscription account from the CLI: a Grok account (xai), a Grok Build account
(grok-cli) or a Claude Pro/Max account. Signing in also works over SSH with no browser. For
Claude, the CLI warns you once that Anthropic's terms carry a risk. After that, the account
serves requests like any API-key account, with retry, fallback and records. You can add
several accounts per provider. 0router keeps tokens fresh, so a client never fails because a
token expired. If an account can't be refreshed, it is taken out of service, other accounts
carry on, and you are told exactly which account needs signing in again. 0router polls quota
on a slow schedule, even when idle, for every account whose provider reports it. The CLI
shows each account's quota and when it was last polled. Each poll is kept with a tally of the
tokens 0router sent, so routing in 006 has data from day one. Quota doesn't steer requests
yet.

## Ledger

| # | Tag | Claim | Source |
|---|---|---|---|
| 1 | U | Shape 005 next | picked "005 account sign-in" |
| 2 | U | Sign in once from the CLI; the account then serves like an API-key account (retry, fallback, records); tokens are refreshed unnoticed; xai and grok-cli work end to end; quota is visible from the CLI | picked "Same, plus quota visible" |
| 3 | C✓ | Quota = what the provider reports (remaining %, window reset), polled regularly, with poll history kept locally. Between-poll estimates, fit and leak detection → 006 | "Polled now, history kept" |
| 4 | U | Fails if: token expiry hits a client; re-sign-in is needed without saying which account; quota shown is wrong or stale (no last-poll time); sign-in can't complete headless | all four picked |
| 5 | C✓ | Generic core sign-in (browser, device code, refresh) and quota polling; each chosen provider's plugin declares the specifics; only chosen providers may use them in 005 | "Generic core, chosen-only" |
| 6 | C✓ | Claude Pro/Max subscription sign-in is in, with a one-time terms-risk warning; its windows are polled | "In 005, with a warning" |
| 7 | C✓ | A refresh that fails for good takes the account out, fallback continues, and the account is named in the accounts list, the record and the error | "Yes" |
| 8 | C✓ | Several signed-in accounts per provider | "Yes" |
| 9 | C✓ | Polling covers API-key accounts with a quota endpoint too (opencode-go, opencode-zen) | "Yes" |
| 10 | C✓ | Out: quota-aware placement → 006 (a 0% account is still tried once) | "Yes, routing in 006" |
| 11 | C✓ | Out: grok-cli bulk import → later | "Yes, later" |
| 12 | C✓ | Out: sign-in for other providers (Codex, Gemini CLI, Copilot, Kiro, Cursor…) → the later community sign-in slice | "Yes, later" |
| 13 | C✓ | Out: web or dashboard sign-in → later | "Yes, CLI only" |
| 14 | C✓ | xai shows "quota not reported" if research finds no endpoint | "Yes" |
| 15 | C✓ | Each poll interval keeps a per-account tally of 0router's input, output, cache-read and cache-write tokens; full request history stays in 006 | "Yes, keep the tally" |
| 16 | C✓ | Plugins may declare the official client identity the provider expects | "Yes, declared in plugin" |
| 17 | C✓ | Steady slow polls, even when idle; cadence adjustable per account | "Yes, steady slow polls" |
| 18 | C✓ | Signed-in tokens stored like API keys (0600, host-bound, never shown in full) | "Same as API keys" |
| 19 | M 2026-09-28 | Order: 005 sign-in, 006 routing and persistent history | slice renumbering |
| 20 | M 2026-09-27 | xai: text, image, video; grok-cli: text, plus more if its live model list shows it | "grok = both" |
| 21 | M 2026-09-27 | Not text-first; standing failure signals (client breaks, a single hiccup reaches the client, wrong usage numbers, secret leak); tested with more than one harness | shape-spec 003 |
| 22 | M 2026-09-28 | Provider plugins are data only | renumbering decision |
| 23 | K | Principle I: plugins never get secrets; the core does all sending and injects secrets only at execution | constitution v3.0.1 |

## P notes for research.md

- xai: OAuth discovery URL plus a static fallback (`ref/9router/src/lib/oauth/providers/xai.js`),
  authorization code with PKCE on a fixed loopback port. The headless path needs a
  paste-the-redirect fallback; check what xai allows.
- grok-cli: device code flow at auth.x.ai, inference at `cli-chat-proxy.grok.com`. The access
  token lasts about 40–45 min, so refresh ahead of expiry, not only on 401
  (`providers/grok-cli.js`, `open-sse/executors/grok-cli.js`).
- Claude subscription: authorization code with PKCE; the code may carry state after `#`
  (`providers/claude.js`).
- Concurrent refreshes of one account are deduplicated (`open-sse/services/tokenRefresh/dedup.js`).
- The bundled OAuth client secrets stay in the core credentials table
  (`crates/nullrouter-registry/src/credentials`), bound to their hosts (slice 002 FR-012).
- Quota parsers: `open-sse/services/usage/{claude,grok-cli,grokCliQuotaFrame,opencode-go,opencode-zen}.js`.
  9router has no xai quota reader.
- Record and tally data shapes should match the approved quota meter (unit, token weights
  input/output/cache_read/cache_write) so 006 can fit without migration.

## Final command

```
/speckit-specify Account sign-in: subscription accounts become ordinary 0router accounts. The operator signs a subscription account in from the CLI for three chosen providers: anthropic (Claude Pro/Max), xai (Grok account) and grok-cli (Grok Build). Sign-in can be completed on a machine with no browser, such as an SSH session. Before an anthropic subscription sign-in completes, the CLI warns once that Anthropic's terms limit subscription use outside its own apps and that the operator carries that risk. A signed-in account then serves requests like an API-key account, with the same retry, fallback and request records, and a provider can have several signed-in accounts. xai serves text, image and video; grok-cli serves text, plus any other model type its live model list shows. 0router keeps tokens fresh on its own, so no client request fails because a token expired. When an account can no longer be refreshed, it is taken out of service, other accounts and providers carry on, and the operator is told which account needs signing in again: in the accounts list, in the request record and in the informational error. Quota is visible. For every chosen-provider account whose provider reports quota, signed-in or API-key (opencode-go and opencode-zen included), 0router polls on a steady slow schedule, also while idle, adjustable per account. The CLI shows the remaining quota, the window reset time and when it was last polled; an account whose provider reports no quota shows "quota not reported". Each poll is kept locally, with a tally of the input, output, cache-read and cache-write tokens 0router sent through that account since the previous poll, so the routing slice learns from real data from its first day. The core is generic and the specifics are data: the core provides the sign-in flows (browser, device code, token refresh) and quota polling, and each provider's plugin declares its sign-in endpoints, scopes, quota endpoint, and the client identity its endpoints expect. In this slice, only the chosen providers may use them. The slice fails if: a standard client breaks on 0router; a single 429, 5xx or timeout reaches the client when another account or provider could serve; a token expiry makes a client request fail; an account needs signing in again and the operator isn't told which one; a shown quota doesn't match what the provider reports or doesn't say when it was polled; sign-in can't be completed without a browser; usage numbers are missing or wrong; a secret appears in a log, record, error, or anything a plugin sees. Tests use more than one client harness. Constraints: plugins are data, never code, and never hold secrets; the core does all sending and injects secrets only at execution; signed-in tokens are stored and protected like API-key secrets (refused if readable by others, bound to their provider's hosts, never shown in full); every model type stays first-class. Out of scope: quota-aware routing (an account at 0% is still tried until retry and fallback move on), between-poll quota estimates, fitted quota weights, leak detection, and persistent request history → slice 006; sign-in for community plugins and for other providers (Codex, Gemini CLI, Copilot, Kiro, Cursor and others) → later; grok-cli bulk import → later; sign-in from a web page or dashboard → later. Scope brief: specs/briefs/2026-10-02-account-sign-in.md
```

## Trace

| Sentence | Rows |
|---|---|
| Subscription accounts become ordinary accounts; CLI sign-in for anthropic, xai, grok-cli | 2, 6, 19, 20 |
| Completed with no browser | 4 |
| Anthropic terms warning | 6 |
| Serves like API-key accounts; several per provider | 2, 8 |
| xai: text, image, video; grok-cli: text and more | 20 |
| Tokens kept fresh; no expiry failures | 2, 4 |
| Refresh fails → out of service, fallback, named in 3 places | 7 |
| Quota polled for every account that reports it, including API-key accounts; steady, idle, adjustable | 3, 9, 17 |
| Shows remaining, reset, last poll; "not reported" | 3, 4, 14 |
| Poll history and token tally | 3, 15 |
| Generic core; plugin declares endpoints, scopes, quota, client identity; chosen-only | 5, 16, 22 |
| Failure list | 4, 21 |
| More than one harness | 21 |
| Constraints | 18, 21, 22, 23 |
| Out of scope (006) | 3, 10, 15, 19 |
| Out of scope (later) | 5, 11, 12, 13 |
