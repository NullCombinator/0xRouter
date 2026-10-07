# Contract: State files

All files are under `$NULLROUTER_HOME`, mode 0600 in directories of mode 0700, and written
through `crate::files`. None is ever sent anywhere or read by a plugin (FR-030, FR-031).

## `quota/fit/<provider>.json` (atomic replace)

```json
{
  "v": 1,
  "windows": {
    "5-hour": {
      "epoch": "2026-10-07T10:00:00.000Z",
      "meter_hash": "sha256:…",
      "restarted": {"at": "2026-10-07T10:00:00.000Z", "reason": "plugin_meter_changed"},
      "numbers": {
        "weight.output": {"state": "fitted", "since": "2026-10-08T09:10:00.000Z"},
        "capacity@max": {"state": "fitted", "since": "2026-10-08T07:40:00.000Z"}
      },
      "splits": {"team": {"since": "2026-10-09T11:00:00.000Z", "reason": "weight.output 2.1× the pooled value"}},
      "breaks": [
        {"at": "2026-10-13T14:00:00.000Z", "detected_at": "2026-10-13T17:20:00.000Z",
         "number": "capacity@max", "replaced": 13800000}
      ],
      "prior": null
    }
  }
}
```

- Per-account numbers are keyed `<number>@<account>`.
- `prior`: `{"through": T, "params": [...], "mean": [...], "information": [[...]]}` for rows
  pruned from inside the epoch (research R11).
- **Load**: a missing file starts every window at the first poll. A window whose declared meter
  hashes differently restarts (FR-017). A file that doesn't parse is renamed to
  `<provider>.json.bad-<time>`, the fit restarts, and `check` warns. It is never silently
  overwritten.
- **Removal**: `plugins remove <provider>` deletes it (Edge Cases).

## `quota/<provider>/<account>.outside.jsonl` (append-only)

One JSON object per line. Written through the history file's lock and writer (slice 005), so it
interleaves safely with `quota prune`.

```json
{"v":1,"kind":"entry","id":"01JB7…","window":"weekly","type":"idle","start":"…","end":"…","amount":4.0,"unit":"percent","found_at":"…"}
{"v":1,"kind":"entry","id":"01JB9…","window":"weekly","type":"steady","start":"…","rate_per_hour":0.2,"unit":"percent","found_at":"…"}
{"v":1,"kind":"alert","id":"01JB8…","entry":"01JB7…","raised_at":"…"}
{"v":1,"kind":"ack","alert":"01JB8…","at":"…"}
{"v":1,"kind":"reclassified","entry":"01JBA…","at":"…","reason":"break"}
```

- `reclassified`: a provisional busy entry that turned out to be a rule change (research R6).
  Readers drop the entry.
- `quota forget <p> <a>` deletes it with the history. `quota prune --before T` drops lines whose
  `start` is before T, except unacknowledged alerts and their entries.

## `accounts.toml`

```toml
[[account]]
provider = "anthropic"
name = "max"
kind = "signin"
exclusive_use = "2026-10-07T12:00:00Z"   # FR-023; absent = not declared

[account.routing]
window."5-hour" = { capacity = 12_000_000, token_weights = { output = 15.0 }, model_multiplier = { "claude-opus-*" = 1.5 } }
```

## `config.toml`

```toml
[provider.anthropic.meter."5-hour"]
token_weights = { output = 15.0 }
model_multiplier = { "claude-opus-*" = 1.5 }
```

Errors are reported as today: `config.toml:L:C provider.anthropic.meter."5-hour".capacity: capacity
is per account; set it with routing set <provider> <account>`.
