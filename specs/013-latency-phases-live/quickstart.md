# Quickstart: validate latency slice 1

The automated evidence runs in CI (research R16). It uses the engine's `testkit` scripted
upstream, which this slice extends with a `Step::Phased` that delays each phase. This
walk-through is the operator-level check for SC-010 and the live view against a real provider.
It uses a reasoning model for slow phases and small timeouts to force failures.

## Prerequisites

- `serve` running from this branch: `cargo run -p nullrouter-cli -- serve` (CI builds it; locally
  only with the user's OK, per the build budget).
- One provider account with a fast model and a reasoning model (below, `openrouter/main`), and
  one agent key: `nullrouter keys issue alice`.

## 1. Phases in records (US1)

1. Send one streamed request, then a second one straight after.
2. `nullrouter records list`: both rows show `SLOWEST`, usually `generation … (provider)`.
3. `nullrouter records show <first id>`: connect has a time and says `new connection`, and the
   phases add up to the total.
4. `nullrouter records show <second id>`: `reused`, and connect is `not applicable`.
5. Set `nullrouter connection set openrouter header-timeout 1ms` and send one request. `show`
   lists every attempt `failed in headers (timeout …)`, with `retry wait` between same-account
   retries if the timeout's class is retried. Afterwards, `unset` it.

## 2. Live view (US2)

1. Send a long request to the reasoning model (a hard puzzle with thinking on).
2. `nullrouter live`: the row moves from `router overhead` to `headers` to `first token`, with a
   time that climbs, and is gone one second after the answer ends.
3. `nullrouter live --json` prints one snapshot and exits.

## 3. Timeouts (US3)

1. `nullrouter connection set openrouter first-token-timeout 1s` prints `applied`.
2. Send a request to the reasoning model. The record shows attempt 1 `failed in first token
   (timeout: first_token 1000 ms, operator per provider)`, and the request falls over or fails as
   for a network error.
3. `nullrouter connection set openrouter --model <reasoning model> first-token-timeout 5m`. The
   reasoning model succeeds again while the fast model keeps 1 s, and
   `nullrouter connection show openrouter` lists the model line with source `operator, model`.

## 4. Proxy (US4)

1. Start a local proxy (any SOCKS5 or HTTP proxy on 127.0.0.1). Add it with
   `nullrouter proxy add local socks5://127.0.0.1:1080`, then `nullrouter proxy use local
   --account openrouter/main`.
2. Send a request. The record's attempt shows `proxy local`.
3. Stop the proxy and send a request. Expect:
   - the request fails, or falls over, with `proxy local paused`;
   - `nullrouter check` shows the error;
   - `live` lists the paused proxy;
   - `accounts list` shows no cooldown on `openrouter/main`.
4. Start the proxy again and run `nullrouter proxy fixed local`. It prints `traffic resumed`, and
   the next request succeeds.
5. `grep -r <proxy password> $NULLROUTER_HOME/records ~/.0router/*.log` finds nothing.

## 5. Reuse, HTTP/2, retries (US5, US6)

- `nullrouter connection set openrouter reuse off`: every new request shows `new connection`.
- `nullrouter connection set openrouter http2 off`: records show `HTTP/1.1`.
- `nullrouter connection set openrouter retries 1` and `retry-wait 500ms`, with the
  header-timeout trick from step 1.5: each request shows exactly one same-account retry with
  `retry wait 500 ms`.
