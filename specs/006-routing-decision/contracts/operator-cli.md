# Contract: Operator CLI and settings (slice 006 additions)

Extends [slice 005's operator CLI contract](../../005-account-sign-in/contracts/operator-cli.md).
File formats on disk: [record-journal.md](record-journal.md). Plugin data:
[routing-schema.md](routing-schema.md).

## Settings files

```toml
# config.toml
[routing]
amortization = "5h"                 # default (Clarifications Q4)

[routing.amortization_for]
"sonnet" = "1h"                     # unified model
"anthropic/claude-opus-4-1" = "24h" # direct target
```

`accounts.toml` gains `priority` and `[account.routing]` per account
([routing-schema.md § Account overrides](routing-schema.md#account-overrides-accountstoml)).

## Commands

| Command | Effect | Server needed |
|---|---|---|
| `routing [target] [--json]` | the routing view (below) | yes (exit 4 otherwise) |
| `routing set <provider> <name> <key>=<value>…` | sets account overrides: `cache_lifetime`, `reserve`, `price.input`…, `window.<name>.capacity`/`length`/`reserve` | no; applies at once if running |
| `routing unset <provider> <name> <key>…` | removes overrides | no |
| `routing window [<target>] <duration\|default>` | sets the default or a target's amortization length | no |
| `accounts priority <provider> <name> <n>` | sets priority (0 = never cold work) | no |
| `accounts list` | adds a `priority` column | no |
| `records list [--provider P] [--account P/N] [--agent KEY] [--model U] [--reason R] [--since DATE] [--limit N] [--json]` | newest first, from disk | no |
| `records show <id> [--json]` | one record with its decision table | no |
| `records prune --before DATE` | deletes records that arrived before DATE; prints the count | no (takes the journal lock) |
| `records forget (--account P/N \| --agent KEY)` | deletes that account's or agent's records; `--agent` also drops its fingerprints | no |
| `check` | adds the unified-model limits notes, routing warnings (R5, R16) and journal health | no |
| `resolve <unified>` | adds the limits note | no |

Each mutating command writes its file atomically and asks a running server to reload. It prints
`applied`, or `saved; applies at next start` when no server is running (slice 005). An unknown
key or bad value exits 1 with the gate's message.

## `routing` (text)

```text
amortization 5h (05:00–10:00 UTC, 2h 48m left) · records kept, last sync 0.4 s ago

sonnet                              subscription tier
  account          source     pace  share  deficit    priority  cache  windows
  anthropic/max    polled      1.42  61%    +91.2k     1         5m     5-hour 5.6M/9.0M wtok · floor 5% · rst 10:00 | weekly 72.9M/90.0M wtok · floor 5% · rst Thu 09:00
  anthropic/pro    polled      0.88  39%    -91.2k     1         1h     5-hour 1.4M/4.5M wtok · floor 10% · rst 08:40 | weekly 19.8M/45.0M wtok · floor 10% · rst Mon 02:00
  opencode-go/main estimated   1.05  —      0          0         5m     rolling 4.2M/6.0M wtok · floor 5% · cold work off (priority 0)
                                    pay-as-you-go tier
  openrouter/main  payg        —     100%   0          1         5m     price now 3.00/Mtok in
```

Each window shows its remaining amount over its capacity in its unit (`wtok` weighted tokens or
`req` requests), its reserve floor and its reset time. `cache` is the cache lifetime in effect
(the account's override, else the plugin's). Deficits are in tokens (research R8).

Warnings appear on their own lines:
- `anthropic/max: polled 14:20, last poll failed 14:31 (timeout), estimate running (stale)`;
- `opencode-go/main: window rolling capacity assumed`;
- `records not kept since 09:01 (disk full): 214 requests`.

`--json` emits the same data with the exact numbers: per account `cache_lifetime`; per window
`unit`, `remaining_at_poll`, `cost_since_poll`, `remaining_now`, `capacity`, `reserve`,
`resets_at`; and the journal health.

## `records show` (text)

```text
rq_01J…  2026-10-04 09:12:03  agent claude-laptop  anthropic-messages  sonnet  succeeded  2.21 s (ttft 640 ms)
decision  cold · size 18.4k · amortization 05:00–10:00
  #  account          tier          eligible  pace  share  deficit   price
  0  anthropic/max    subscription  yes       1.42  61%    +91.2k
  1  anthropic/pro    subscription  yes       0.88  39%    -91.2k
  2  openrouter/main  payg          yes                             3.00
attempts
  1  anthropic/max  cold_by_deficit (rank 0)  ok  2.21 s  in 18,210 · out 512 · cache w 18,100
```

A warm request shows `decision  warm on anthropic/max · prefix 41.2k · idle 38 s · stayed`. A
move shows `moved: reserve_floor on anthropic/max`.

## Operator socket (added and changed ops)

| Op | Request | Answer |
|---|---|---|
| `routing.view` | `{target?}` | the routing view data |
| `routing.health` | — | journal health (also part of `routing.view`) |
| `records.list`, `records.get` | unchanged shape | served from the journal plus in-flight records |
| `records.forget` | `{account?: "P/N", agent?: KEY}` | the server drops that agent's fingerprints (or that account's fingerprints and ledger entries) from memory and `routing/warm.jsonl`; the CLI then rewrites the record segments |

## Exit codes

Unchanged set. `routing` exits 4 without a server. `records prune` and `records forget` exit 1 if
the journal lock can't be taken within 10 s.
