# Contract: record journal and routing state files (slice 006)

The on-disk format the CLI reads without a server. Research R11 and R12.

## Files

| Path | Mode | Contents |
|---|---|---|
| `records/YYYY-MM-DD.jsonl` | 0600 (dir 0700) | request record lines, by UTC day of arrival |
| `records.lock` | 0600 | advisory lock: the writer per batch, and prune or forget for a rewrite |
| `routing/warm.jsonl` | 0600 (dir 0700) | warm fingerprint upserts |
| `routing/ledger.jsonl` | 0600 | deficit ledger lines |
| `routing/salt` | 0600 | 32 random bytes |

Every line is one JSON object with `"v":1` and a type `t`. A line that doesn't parse is skipped
by readers. Recovery truncates a torn final line.

## Record lines

```jsonl
{"v":1,"t":"open","id":"rq_01J…","arrived":"2026-10-04T09:12:03.120Z","agent":"key_7f…","style":"anthropic-messages","op":"generate","type":"text","target":"sonnet"}
{"v":1,"t":"decision","id":"rq_01J…","decision":{"kind":"cold","at":"…","amortization_window":{"start":"2026-10-04T05:00:00Z","length":"5h"},"size_tokens":18400,"candidates":[{"provider":"anthropic","account":"max","model":"claude-sonnet-4-5","tier":"subscription","eligible":true,"quota_source":"polled","pace":1.42,"rate":5210.0,"priority":1,"weight":7398.2,"share":0.61,"deficit_before":91200},{"provider":"anthropic","account":"pro","model":"claude-sonnet-4-5","tier":"subscription","eligible":true,"quota_source":"polled","pace":0.88,"rate":5300.0,"priority":1,"weight":4664.0,"share":0.39,"deficit_before":-91200},{"provider":"openrouter","account":"main","model":"anthropic/claude-sonnet-4.5","tier":"payg","eligible":true,"quota_source":"pay-as-you-go","priority":1,"price_now":3.0}],"order":[0,1,2]}}
{"v":1,"t":"attempt","id":"rq_01J…","attempt":{"n":1,"provider":"anthropic","account":"max","model":"claude-sonnet-4-5","kind":"initial","placement":{"reason":"cold_by_deficit","rank":0},"started":0.4,"ended":2210.7,"outcome":{"state":"ok"},"usage":{"input":18210,"output":512,"cache_read":0,"cache_write":18100,"input_semantics":"excludes_cache","estimated":false},"dropped":[],"forced":[]}}
{"v":1,"t":"close","id":"rq_01J…","outcome":"succeeded","served_by":{"provider":"anthropic","account":"max","model":"claude-sonnet-4-5"},"ttft_ms":640.2,"total_ms":2211.0,"usage":{…},"break_handling":{"kind":"none"}}
```

- `open` and `close` are acknowledged by the writer before the request continues (`open`) or
  before the client's last byte is sent (`close`).
- Folding: start from `open`, append each `attempt` in `n` order, set `decision`, then apply
  `close`. Later duplicates of a line type replace earlier ones.
- After a crash, recovery writes `{"t":"close","id":…,"outcome":"interrupted","recovered_at":…}`.
- No prompt content, header value or secret ever appears. `dropped` is paths only (slice 003).
  Error reasons are redacted (slice 005).

## Routing state lines

```jsonl
{"v":1,"t":"warm","agent":"key_7f…","hash":"9c1e…(32 hex)","provider":"anthropic","account":"max","model":"claude-sonnet-4-5","prefix_tokens":18100,"last_used":"2026-10-04T09:12:05.331Z"}
{"v":1,"t":"ledger","target":"sonnet","tier":"subscription","window_start":"2026-10-04T05:00:00Z","deficits":{"anthropic/max":-8800,"anthropic/pro":8800},"at":"2026-10-04T09:12:05.331Z"}
```

Load: the last `warm` line per identity wins, with expired entries dropped. The last `ledger`
line per `(target, tier)` wins if its `window_start` is the current window's. Ledger deficits are
in tokens (input + cache read + cache write + output, unweighted; research R8).

Wire names: `tier` uses the short form `payg`; `quota_source` uses `pay-as-you-go`, the label the
routing view shows. Both name the same account kind.

New files (a daily segment, a compacted routing file after its rename) are followed by an
`fsync` of their parent directory, so a power loss can't lose a whole file.

## Durability

| Event | Records | Warm and ledger |
|---|---|---|
| clean shutdown | nothing lost | nothing lost |
| 0router crash | nothing lost; in-flight requests become `interrupted` | nothing lost for requests whose `close` was acked |
| power loss or OS crash | at most about the last 1 s | at most about the last 1 s |
| disk full | up to 10,000 pending lines are held and written when space returns; records beyond that are not kept, and are counted and reported; serving continues | in memory only while full; written again when space returns |
