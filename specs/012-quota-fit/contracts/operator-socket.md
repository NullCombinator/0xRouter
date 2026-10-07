# Contract: Operator socket

Additions to `crates/nullrouter-server/src/operator.rs`'s ops. Same framing as today (one JSON
object per line, `{"ok":true,…}` or `{"ok":false,"error":…}`).

| Request | Response |
|---|---|
| `{"op":"routing.view","target"?}` | as today; each account gains `meter`, `outside_use`, `fit_note` (below) |
| `{"op":"quota.outside","provider"?,"account"?,"since"?,"limit"?}` | `{"ok":true,"entries":[OutsideEntry]}` |
| `{"op":"quota.alerts"}` | `{"ok":true,"alerts":[Alert]}` (unacknowledged only) |
| `{"op":"quota.ack","id"?,"provider"?,"account"?}` | `{"ok":true,"acknowledged":N}`; no `id` means all (optionally narrowed) |
| `{"op":"quota.refit","provider","account"?}` | `{"ok":true}`: rejoins a split account, or restarts the account's epochs |
| `{"op":"reload"}` | as today; also rebuilds the meters in effect from the new overrides and exclusive-use declarations |

## `WindowMeterView`

```json
{
  "window": "5-hour",
  "epoch": "2026-10-07T10:00:00.000Z",
  "split": null,
  "numbers": [
    {
      "number": "capacity",
      "declared": 9000000,
      "account_override": null,
      "plugin_override": null,
      "fit": {"value": 13800000, "low": 13100000, "high": 14600000},
      "in_use": 13800000,
      "source": "fit",
      "state": "fitted",
      "since": "2026-10-08T09:10:00.000Z",
      "progress": {"intervals": 640, "half_width": 0.052},
      "partner": null,
      "reason": null
    }
  ],
  "set_aside": {"reset": 3, "usage_unreported": 12, "exhausted": 0}
}
```

- `state`: `learning`, `fitted`, `relearning`, `restarted`, `not_separable`, `yardstick`,
  `not_reported`.
- `since`: for `fitted`, `relearning` and `restarted`.
- `partner`: for `not_separable`.
- `reason`: for `relearning` and `restarted`.
- `split`: `{"since": T, "reason": "weight.output 2.1× the pooled value"}`, or null.
- `fit` is null while there are no rows. A number in a `learning` state still has a `fit` range.

## `outside_use` (per account, summary)

`{"intervals_7d": 3, "last": OutsideEntry|null, "steady": {"rate_per_hour": 0.2, "since": T}|null}`

## `fit_note`

Present only for an unfitted account: `"provider reports no quota"` or `"pay-as-you-go"`.

## `OutsideEntry`, `Alert`

As in [state-files.md](state-files.md); `Alert` adds `text`.

Nothing in these responses carries a secret (FR-031, SC-010): they hold account names, window
names, numbers and times only.
