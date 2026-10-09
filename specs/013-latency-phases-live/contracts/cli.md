# Contract: CLI

**The command names are proposals for the operator to review before `/speckit-tasks`.**

Every mutating command writes its file atomically and asks the running `serve` to reload (the
existing pattern). It prints `applied` when `serve` acknowledges, or `saved; applies at next
start` when no `serve` is running. An invalid value is refused with a message that names the
field, and nothing is written (FR-032).

## `nullrouter live [--json]` (US2)

This command lists the requests in flight. Without `--json` it redraws once a second until
Ctrl-C. When stdout isn't a terminal, it prints one snapshot per second instead. It exits 1
with `no server is running` when `serve` isn't up.

```text
in flight 3 · 14:02:07 CEST                                    (Ctrl-C to quit)
paused proxies: eu-exit (unreachable since 13:58:12)

AGENT    TARGET        ATTEMPT                     PHASE                  SINCE ARRIVAL  FINISHED
alice    sonnet        1 kiro/work                 headers 2.1 s          2.3 s          overhead 4 ms
ci       gpt-5         2 openrouter/main           generation 11.4 s      19.0 s         #1 failed in headers 6.0 s (timeout) · retry wait 2.0 s · connect 41 ms · first token 3.2 s
bob      opus          —                           router overhead 1 ms   1 ms
```

With nothing in flight, it prints `nothing in flight`, and `--json` gives `{"in_flight":[]}`.

`--json` prints a single snapshot and exits:

```json
{"as_of":"2026-10-07T14:02:07+02:00","paused_proxies":[{"name":"eu-exit","since":"…","reason":"…"}],
 "in_flight":[{"id":"rq_…","agent":"alice","target":"sonnet","since_arrival_ms":2310.4,
   "attempt":{"n":1,"provider":"kiro","account":"work","model":"claude-sonnet-4.5",
              "phase":"headers","in_phase_ms":2104.0,"proxy":null},
   "finished":[{"n":1,"phases":{"router_overhead":4.1}}]}]}
```

Phase names in JSON: `router_overhead`, `retry_wait`, `connect`, `headers`, `first_token`,
`waiting_for_provider`, `generation`, `delivery`.

## `nullrouter records` (US1, clarify Q5)

`list` adds a column `SLOWEST` showing the request's longest phase over all its attempts, its
time and its side:

```text
ID          ARRIVED   AGENT  TARGET  OUTCOME    TTFT    TOTAL    SLOWEST
rq_01J…     14:01:55  ci     gpt-5   succeeded  9.3 s   41.0 s   generation 30.1 s (provider)
rq_01J…     14:01:40  alice  sonnet  succeeded  0.8 s   1.2 s    delivery 0.3 s (client)
rq_01J…     14:00:02  bob    opus    succeeded  —       —        not recorded
```

An in-flight request shows its current phase with `…`, e.g. `headers 2.1 s… (provider)`.

`show <id>` adds a phase table per attempt:

```text
attempt 1  openrouter/main  new connection · HTTP/2 · proxy eu-exit       failed in headers (timeout: headers 6000 ms, operator per provider)
  router overhead     4.1 ms
  connect            41.0 ms   (includes token refresh 0 ms)
  headers          6000.2 ms
  first token      not applicable
  generation       not applicable
  delivery         not applicable
attempt 2  openrouter/main  reused · HTTP/2                              ok
  retry wait       2000.0 ms
  router overhead     0.3 ms
  connect          not applicable
  waiting for provider 3205.5 ms
  generation      31740.5 ms
  delivery           12.3 ms   (of which record close 1.1 ms)
total 41000.6 ms = sum of phases
```

`--json` carries `timing` (the marks) and `phases` per attempt. See [record.md](record.md).

## `nullrouter connection` (US3, US5, US6)

```text
nullrouter connection show [<provider>] [--json]
nullrouter connection set <provider> [--model <model>] <key> <value>
nullrouter connection unset <provider> [--model <model>] <key>
```

Keys:

| Key | Values | Levels |
|---|---|---|
| `connect-timeout`, `header-timeout`, `first-token-timeout`, `stall-timeout` | duration (`500ms`, `30s`, `5m`) or `off` (first token only) | provider, model |
| `reuse` | `on` \| `off` | provider |
| `http2` | `on` \| `off` | provider |
| `retries` | `0`–`5` | provider (all statuses) |
| `retries.<status>` | `0`–`5` | provider |
| `retry-wait`, `retry-wait.<status>` | duration ≤ `30s` | provider |

The header timeout counts from the attempt's start, connect included, as today (R12).

`show` lists each value with its source:

```text
openrouter
  connect timeout      60 s      built-in (FETCH_CONNECT_TIMEOUT_MS default)
  header timeout       10 s      operator, provider
  first-token timeout  off       built-in
  stall timeout        360 s     built-in
  reuse                on        built-in
  http2                negotiate built-in
  retries              503: 3 after 2 s (built-in) · 429: 1 after 2 s (built-in)
  proxy                eu-exit   operator, all providers   [PAUSED since 13:58:12]
  model anthropic/claude-opus-4.1
    first-token timeout  300 s   operator, model
  accounts
    main    proxy eu-exit (all providers)
    backup  proxy none (account)
```

## `nullrouter proxy` (US4)

```text
nullrouter proxy add <name> <url> [--username <u>]      # password from stdin, or --password-env VAR
nullrouter proxy list [--json]                          # name, scheme://host:port, user set?, state, used by
nullrouter proxy remove <name>                          # refused while assigned anywhere; names the assignments
nullrouter proxy use <name|none> --all | --provider <p> | --account <p>/<n>
nullrouter proxy clear --all | --provider <p> | --account <p>/<n>     # remove the assignment at that level
nullrouter proxy fixed <name>                           # probe now; resume if reachable (clarify Q1)
```

- Passwords are never read from argv and never printed. `list` shows `user ✓` or `—`.
- `proxy fixed` prints `eu-exit reachable; traffic resumed` or
  `eu-exit still unreachable: <reason>` (exit 1).

## `nullrouter check` (FR-028, FR-034)

New items:
- `error: proxy eu-exit is paused (unreachable since …); fix it, then run nullrouter proxy fixed eu-exit`
  (exit 1, as for other errors).
- `note: connection settings for unknown provider "xx" are unused`.
- `note: account kiro/old is assigned proxy "gone", which isn't defined`. This is refused at
  write time, so it can only come from a hand edit.

## `nullrouter accounts list`

It adds a column `PROXY`: the effective proxy name and its level, or `—`.
