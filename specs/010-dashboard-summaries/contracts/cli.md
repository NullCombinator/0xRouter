# Contract: CLI additions and changes

All commands take the global `--home` and `--json`. Exit codes follow the existing CLI: 0 ok,
1 error, 2 not found. JSON shapes are in [data-model.md](../data-model.md).

## `nullrouter usage [--period today|24h|7d|30d|60d|all]` (new)

Request and token totals for one period; `today` when no period is given. Read only.

```text
$ nullrouter usage --period 7d
period: 7d, 2026-09-29 14:02:11 → 2026-10-06 14:02:11 Europe/Berlin
requests          1,284   (1 in flight, 3 not reported)
input (uncached)  1.3M
cached            2.9M
output            611k
est. cost         ~$11.60   Estimated, not actual billing
                  12 requests not priced: 9 no price, 3 no output price
                  Priced with today's declared prices; earlier price changes are not tracked.

agent             requests
claude-code         1,020
codex                 264

provider          requests
anthropic             900
xai                   384
```

- Token counts print as `612k`, `1.3M` (one decimal from a million), and exact below 1,000. The
  JSON always gives exact integers.
- The cost line is always followed by the label. The "not priced" line appears only when the
  count is above 0. The note line always appears.
- An empty period prints `requests 0 (no requests in this period)` and no tables.
- When the window touches a period while records were not kept, `check`'s warning line follows
  the header, in `check`'s words.
- An unknown period exits 1: `unknown period "<p>"; use today, 24h, 7d, 30d, 60d or all`.
- With a server running, it asks `usage.totals`; without one, it reads the journal in its own
  process. Both give the same numbers for the same records.

## `nullrouter latency` (new)

The last 24 hours, per agent and per provider. Takes no period. Read only.

```text
$ nullrouter latency
last 24 h, 2026-10-05 14:02:11 → 2026-10-06 14:02:11 Europe/Berlin

agent            requests  overhead p50/p95  ttft p50/p95     last response
claude-code           612  5 ms / 9 ms       420 ms / 1.9 s   resolved 14:01:58
codex                  88  4 ms / 7 ms       700 ms / 1.5 s   resolved 13:40:02
ci-bot                  6  4 ms / 4 ms       none             failed 14:00:31

provider         requests  own ttft p50/p95  last response       by agent
anthropic             542  410 ms / 1.9 s    resolved 14:01:58   claude-code 512, hermes 30
elevenlabs              6  none              failed 14:00:31     ci-bot 6
```

- Times under one second print in ms, otherwise in seconds with one decimal. The JSON gives
  milliseconds as numbers.
- `none` means no request had that value in the window (FR-011). An agent or provider with no
  requests in the window isn't listed; with no requests at all, the command prints `no requests
  in the last 24 h`.
- `own ttft` is the provider's own wait (research R6), headed so it can't be read as the client's
  wait.
- The last-response time is local time, with the date when it isn't the read's date.
- Transports as for `usage`: `latency.summary` with a server, the journal without one.

## `nullrouter keys issue <name> [--break B] [--harness TEXT]` (changed)

`--harness` stores the tag (research R7). A refused tag exits 1 and issues no key:
`harness tag must be 1 to 32 characters with no control characters`. Output otherwise unchanged;
`--json` gains `harness`.

## `nullrouter keys tag <key> <TEXT>` / `nullrouter keys tag <key> --clear` (new)

Sets, replaces or removes the harness tag of an existing key, by name or id. Revoked keys too.

```text
$ nullrouter keys tag codex codex-cli
codex: harness codex-cli
applied
```

- `--clear` prints `codex: no harness`. Giving both a TEXT and `--clear` is a usage error (exit 1).
- Unknown key: exit 2, `no key "<key>"`, as `revoke` says.
- The key's secret, id, created time, revoked state and break behaviour are unchanged.
- `applied` or `saved; applies at next start`, as the other mutators print. `--json`:
  `{"id","name","harness","status"}`.

## `nullrouter keys list` (changed)

A `HARNESS` column after the name (`-` when none). `--json` rows gain `harness`. Every other column
and its order is unchanged.

## Operator socket (new ops)

| Request | Answer |
|---|---|
| `{"op":"usage.totals","from":"<RFC 3339>"\|null,"to":"<RFC 3339>"}` | `{"ok":true,"totals":{…}}` |
| `{"op":"latency.summary","from":"<RFC 3339>","to":"<RFC 3339>"}` | `{"ok":true,"latency":{…}}` |

## Unchanged

Every other command's text and `--json` output. Their existing tests must pass unchanged.
