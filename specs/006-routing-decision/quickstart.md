# Quickstart: validating slice 006

Formats: [contracts/operator-cli.md](contracts/operator-cli.md),
[contracts/routing-schema.md](contracts/routing-schema.md),
[contracts/record-journal.md](contracts/record-journal.md). States and fields:
[data-model.md](data-model.md).

```bash
export CARGO_HOME=$PWD/.cargo-home
alias nr='cargo run -q -p nullrouter-cli --'
```

Run one crate at a time. Never chain a workspace test with clippy (build budget: 2 jobs,
`nice`).

## 1. Automated suite (mock providers that report cache reads and quota)

```bash
nice cargo test -p nullrouter-registry -j 2     # [routing] schema, gate, limits note
nice cargo test -p nullrouter-engine -j 2       # decision core, journals, integration
nice cargo test -p nullrouter-server -j 2       # end to end with several mock accounts
nice cargo test -p nullrouter-cli -j 2          # routing, records offline, prune, forget
```

Expected: all pass. Notable tests and what they prove:

| Test | Proves |
|---|---|
| `routing::fingerprint` (unit) | longest prefix wins; cache modes; per-model key; hashes only |
| `routing_warm` | warm stays; only the four move reasons move it; each is recorded (US1, SC-002) |
| `routing_cold` | shares follow weights; admission, priority 0, floor; epoch-aligned reset (US2) |
| `routing_overflow` | pay-as-you-go only when no subscription can serve; schedule switch; last resort; error lists why-not (US3, SC-003) |
| `routing_state` | warm state and deficits survive restart (US5 scenarios 3–4) |
| `records_journal` | killed child keeps every finished record; in-flight → interrupted; torn line; disk full keeps serving (US5, FR-037a) |
| `estimate` | between-poll estimate equals the mock's figure at every poll (SC-008) |
| `fallback` (rerun with several accounts) | 0 injected failures reach the client when another account could serve (SC-010) |
| `usage_records` | recorded usage equals provider-reported usage (SC-011) |
| `secrets` (extended) | zero secrets and zero planted prompt text in journals, routing files, CLI output (SC-012) |

## 2. The simulated week

```bash
nice cargo test -p nullrouter-engine --release --test sim_week -j 2 -- --nocapture
```

Expected: the run takes under 2 minutes, passes SC-001 to SC-006, and prints one row per
account window: target share, actual share, and remaining at reset. Run it twice. The output is
identical (fixed seed). The restart case also prints `post-restart placements identical: yes`.

## 3. Harnesses (more than one client)

```bash
NR_HARNESS=1 nice cargo test -p nullrouter-server --test harness -j 2
```

The Python OpenAI SDK (chat) and Claude Code (messages) run warm, cold and overflow scenarios
against three mock subscription accounts and one pay-as-you-go account. The Node SDK runs the
standing smoke. Expected: zero client errors, and the mock reports cache reads on every warm
request (SC-009).

## 4. By hand, with mock accounts

```bash
export NULLROUTER_HOME=$(mktemp -d)
nr serve &                                        # with the testkit mock config from the harness
nr routing sonnet                                 # pace, share, deficit, polled/estimated per account
nr accounts priority anthropic pro 0              # "applied"; pro shows "cold work off"
nr records list --limit 5
nr records show <id>                              # decision table and placement reasons
kill -9 %1; nr records list --limit 5             # works without a server; in-flight → interrupted
nr serve & nr routing sonnet                      # same deficits as before the kill
nr check                                          # limits note for a unified model with mixed context sizes
```

## 5. Live check L7 (opt-in, the operator's real accounts)

```bash
export NULLROUTER_HOME=$PWD/.nr-live
nr quota poll anthropic max && nr routing --json > before.json
# send one tiny request through each polled account, then:
nr routing --json > after.json
nr quota poll anthropic max && nr routing --json > polled.json
```

Expected: in `before.json` and `polled.json`, each polled account's `remaining_now` equals its
poll's `remaining`. In `after.json`, it is lower by the request's metered `cost_since_poll`
(SC-007). The full steps are in `docs/operator-config.md` under "Live checks".

## 6. Bench

```bash
nice cargo bench -p nullrouter-engine -j 2 -- 'route|journal|ttfb/routed'
```

Expected: `ttfb/routed` p95 is within 5 ms of slice 005's `ttfb/direct` on the same machine
(SC-013). Record the results in `bench-baseline.md`.
