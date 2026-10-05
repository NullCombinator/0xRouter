# Parity audit: slice 006 (T096)

Audited 2026-10-05 against `ref/9router` at `39e36d3`, using the `/rust-parity-audit` protocol.
The request path is `chat.js` `handleSingleModelChat` (the account loop, `chat.js:233-338`) →
`auth.js` `getProviderCredentials` / `markAccountUnavailable` / `clearAccountError` →
`chatCore.js` (executor call, 401/403 refresh at `:420-465`, error result at `:467-490`), and for a
combo `combo.js` `handleComboChat` (`:280-382`). Callers were traced with the code-review-graph:
`markAccountUnavailable` is called from `handleSingleModelChat` (`chat.js:327`) on the chat path,
and `checkFallbackError` is reached from there and from `handleComboChat`. 0router's callers and
blast radius come from `code-review-graph-0router` at `f3e7be8`.

Not repeated below: D-006-1..3 (`tests/parity/deviations.toml`), R26 and slice 003's audit
(`resetsAtMs` absent, the rate-limit-first retry budget, the 4xx no-fallback rule, informational
errors in place of the last upstream error), and slice 005's R9/R10/R19 rows (use-time refresh
margin, one refresh on 401 or a matching 403, a refused account).

| Rust | 9router |
|---|---|
| `crates/nullrouter-engine/src/plan.rs` (`plan`, `member`, `endpoints`) | `chat.js:166-232` (combo expansion), `combo.js` `getComboModelsFromData` / `handleComboChat` model list, `auth.js` `getProviderCredentials` (the connection filter) |
| `crates/nullrouter-engine/src/attempt.rs` `walk`, `candidate` | `chat.js:233-338` (account loop), `chatCore.js:467-490`, `auth.js` `markAccountUnavailable`, `clearAccountError`, `combo.js:300-382` |
| `crates/nullrouter-engine/src/attempt.rs` `token_rejected`, `refusal` | `chatCore.js:420-465`; audited in slice 005 (R9, R10), not again here |

## Checked and matching

- One try per account per request: `excludeConnectionIds` ↔ each placement slot is walked once.
  A same-account retry is 0router's own R7 budget, not a second selection.
- A verdict without fallback ends the request with the provider's error at once
  (`chat.js:337`, `combo.js:337-341`); a fallback verdict moves to the next account, then the
  next member.
- A resting account is skipped before any call (`isModelLockActive`) and shown as a skip with
  its rest time; when every candidate rests, the client gets a 503 with the earliest end as
  `retry-after` (`chat.js:237-243`).
- A success clears the model's rest and resets the backoff level when no other rest is active
  (`clearAccountError`); `Cooldowns::succeed` does the same.
- A transport failure (no headers in time, a reset connection) counts as a 502 and is
  transient (30 s); a client that leaves ends the request as 499 and falls back nowhere.
- Out-of-service sign-in accounts are named in the error with the command that brings them
  back; 9router has no such state.
- A disabled account is left out of the plan, as `isActive: true` filters it in
  `getProviderConnections`.
- 0router's own rule that keeps this from being a gap: accounts held only by a reserve floor, and
  the account a moved request left, are appended after the usable ones (`place.rs:153-155`),
  cooling accounts last. What `walk` reports as "not tried" is only priority 0, a refusing
  short window and an out-of-service account (finding 5).

## Findings

| # | File | Finding | Severity | Recommendation |
|---|---|---|---|---|
| 1 | `plan.rs:171`, `plan.rs:77` ↔ `chat.js:244-247` | A direct target whose provider has no enabled account is a 503 in 0router and a 404 ("No active credentials for provider") in 9router. Inside a combo 9router also ends in 503 (`combo.js:367-368`), so only the direct case differs. A client reading 404 as "model not found" and not retrying sees a different outcome. The message names the fix (`accounts add`), which 9router's does not. | Low | Record as a deviation: 503 is retryable once the operator adds an account, and the message is actionable. |
| 2 | `attempt.rs:965` ↔ `auth.js:240` | 9router never rests a no-auth provider: `markAccountUnavailable` returns "no fallback" for `noauth`. 0router rests a candidate with no account like any other, under the key `(provider, "", model)`. One transient failure then holds the model for 30 s (2 min on 401–404), and a direct request meanwhile fails with "cooling down" without trying upstream. Only the 9 community plugins with `no_auth = true` are affected; none of the bundled five is. | Low | Accept and record (rest is the same rule for every candidate), or skip `cooldowns.fail` when `c.account` is `None`. |
| 3 | `attempt.rs:954-964` ↔ `executors/base.js:111-122` | The same-account budget is fixed by the first failure. 9router keeps one counter per URL but looks the limit up by the current status, so a 500 followed by a 502 gets 3 retries there and 1 here, and a 502 followed by a 504 gets 2 there and 3 here. Only a flapping upstream shows it. | Low | Accept; recompute the budget per failure only if live use shows it. |
| 4 | `attempt.rs:644-676` ↔ `combo.js:342-349` | Before falling to the next combo model on a 502/503/504 whose cooldown is 5 s or less, 9router waits that long. 0router moves on at once after its own retries (R7), which already wait 2–3 s each. | Low | Accept. |
| 5 | `attempt.rs:693-696`, `place.rs` ↔ `auth.js:85-98` | An account with priority 0 or a refusing short window is never tried, even when every other candidate failed. 9router tries every active, unlocked account. This is the spec's rule (priority 0 means never for cold work; a short window only admits or refuses), shown as "not tried: …" in the error, but it is not in `deviations.toml`. | Low | Add it as D-006-4 next to D-006-1..3. |

Not audited: the TTS `provider/model/voice` split at the top of `walk` (`attempt.rs:604-625`) and
the unknown-provider and unknown-model statuses in `plan`. Neither is on the retry and fallback
path this audit was asked to cover, and I did not compare them with the JS.

No Critical or High findings, so there is nothing to fix and no new test. Finding 5 wants a line
in `tests/parity/deviations.toml`, and finding 1 a line in R26.

## Resolution (2026-10-05)

| # | Status | Test or record |
|---|---|---|
| 1 | Accepted, to be recorded in R26. | — |
| 2 | Accepted. | — |
| 3 | Accepted. | — |
| 4 | Accepted. | — |
| 5 | Open: add D-006-4 to `deviations.toml` and assert it in `routing_cold.rs`. | — |
