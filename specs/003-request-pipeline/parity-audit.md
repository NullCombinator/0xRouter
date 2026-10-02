# Parity audit: slice 003 (T141)

Audited 2026-10-02 against `ref/9router` on its request path (`chatCore` → `BaseExecutor.execute`
→ `markAccountUnavailable` / `clearAccountError`), with the `/rust-parity-audit` protocol.

| Rust | 9router |
|---|---|
| `crates/nullrouter-engine/src/classify.rs` | `open-sse/config/errorConfig.js` (`ERROR_RULES`, `BACKOFF_CONFIG`), `open-sse/services/accountFallback.js` (`checkFallbackError`, `getQuotaCooldown`), `open-sse/config/runtimeConfig.js` (`DEFAULT_RETRY_CONFIG`) |
| `crates/nullrouter-engine/src/cooldown.rs` | `src/sse/services/auth.js` (`markAccountUnavailable`, `clearAccountError`) |
| `crates/nullrouter-engine/src/attempt.rs`, `upstream.rs` (budgets, timeouts) | `open-sse/executors/base.js` (`tryRetry`, connect timer), `open-sse/handlers/chatCore/streamingHandler.js` (stall) |
| `crates/nullrouter-wire/src/codec/` | `open-sse/translator/*`; evidence: `tests/parity_translate.rs`, `tests/deviations.rs`, `tests/bundled.rs` against the generated oracle |
| `crates/nullrouter-wire/src/usage.rs` | `open-sse/utils/usageTracking.js`; evidence: `tests/usage.rs` (`usage_matches_the_9router_oracle`) |
| `crates/nullrouter-wire/src/estimate.rs` | `src/app/api/v1/messages/count_tokens/route.js` (`estimateAnthropicInputTokens`); evidence: `tests/estimate.rs` |

## Checked and matching

- Text rules first, in 9router's order, then 401/402/403/404 (2 min) and 429 (backoff); an
  unmatched 4xx returns the error without fallback or cooldown; anything else is transient
  (30 s).
- Backoff: `2000 · 2^(level−1)`, capped at 5 min, level capped at 15, kept per account; a
  success clears the model's rest and resets the level when no other rest is active.
- Same-account budgets 502 (3 × 3 s), 503 (3 × 2 s), 504 (2 × 3 s); network errors and
  connect timeouts count as 502; per-provider overrides replace the default entry.
- Header timeout 60 s and stall timeout 360 s, both with 9router's env overrides
  (`FETCH_CONNECT_TIMEOUT_MS`, `STREAM_STALL_TIMEOUT_MS`) and per-endpoint overrides.
  9router's `STREAM_FIRST_CHUNK_TIMEOUT_MS` is used only by the kiro executor, so 0router has
  no first-chunk timer.
- Estimator: system + tools + per-block message characters in UTF-16 units, `ceil(/4)`.

## Findings

| # | File | Finding | Severity | Resolution |
|---|---|---|---|---|
| 1 | `classify.rs` `budget` | 9router picks the same-URL retry budget by status before reading the body, so a 502/503/504 whose body says "overloaded" or "rate limit" gets 3 retries. 0router classifies it rate-limited first: 1 retry at the indicated wait (none if that wait is over 5 s). A 502/503 body with "no credentials" gets 0 retries instead of 3. | Medium | Follows R7's table, which puts rate-limit/overloaded text in the 1-retry row; recorded in R26. |
| 2 | `estimate.rs` `js_number` | A float of magnitude ≥ 1e21 prints in Rust's form, not JavaScript's exponent form, so its character count can differ by a few. No real request carries such a number. | Low | Accepted. |
| 3 | `cooldown.rs` | 9router's provider-reported reset (`resetsAtMs`, capped at 30 min) is absent. Only the codex, antigravity and github paths set it, and none of them is bundled or fits the core. | Low | Accepted. |

No Critical or High findings. The R26 deviations are accepted as recorded there.
