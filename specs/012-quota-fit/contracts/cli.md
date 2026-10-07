# Contract: CLI

Additions to slice 006's `routing` command and slice 005's `quota` and `accounts` commands. Every
command takes `--json`. Exit codes as today: 0 ok, 2 refused input, 4 no running server (for
commands that need one).

## Overrides (FR-019, clarify Q5)

```bash
# per account: new keys beside window.<name>.capacity|length|reserve
nullrouter routing set anthropic max window.weekly.weight.output=15
nullrouter routing set anthropic max 'window.weekly.multiplier.claude-opus-*=1.5'
nullrouter routing unset anthropic max window.weekly.weight.output
nullrouter routing unset anthropic max 'window.weekly.multiplier.claude-opus-*'

# per plugin: weights and multipliers only, every account of the plugin
nullrouter routing set-plugin anthropic window.weekly.weight.output=15
nullrouter routing unset-plugin anthropic window.weekly.weight.output
```

- `weight.<class>`: `input`, `output`, `cache_read`, `cache_write`. Refused on a `requests`
  window, with the gate's wording.
- `multiplier.<glob>`: the glob must be one the plugin's meter declares for that window.
  Otherwise: `routing.window.weekly.model_multiplier: anthropic declares no glob "claude-opus-4*"
  for window weekly`.
- `set-plugin … window.<name>.capacity=…` is refused:
  `capacity is per account; use routing set <provider> <account>`.
- Values are checked by the plugin's rules (FR-019); the refusal text is the gate's text.
- Overriding `weight.input` on a percent window changes the yardstick: the fit's other numbers
  are relative to it, so they keep their meaning (research R3).

## The routing view (FR-028, FR-029, SC-009)

`nullrouter routing [target] [--json]` adds one meter block under each polled account. It always
shows, so one call answers SC-009:

```text
  anthropic/max    polled      1.42  61%    +91.2k     1         5m     5-hour 5.6M/9.0M wtok · …
    meter 5-hour   capacity      9.0M declared · fit 13.1M–14.6M · in use 13.8M fit since Tue 09:10
                   weight.input  1     yardstick
                   weight.output 5     declared · fit 14.2–15.9 · in use 15 override (account)
                   weight.cache_read 0.1  declared · learning 312 intervals ±40%
                   weight.cache_write 1.25 declared · not separable from weight.cache_read
                   multiplier.claude-opus-*  1.67 declared · learning 40 intervals ±120%
    meter weekly   capacity      90.0M declared · relearning since Tue 14:00 (provider rules changed)
    outside use    3 intervals this week (last Wed 02:10–02:30, 4% of weekly) · steady up to 0.2%/h (08–12) since Mon
    usage alert    01JB7… 4% of weekly used 02:10–02:30 Wed with no traffic from 0router (nullrouter quota ack 01JB7…)
  anthropic/team   polled      …
    meter 5-hour   split off since Wed 11:00: weight.output 2.1× the pooled value
  opencode-go/main estimated   …
    meter          not fitted: provider reports no quota
```

The line format is `name  declared · fit <95% range> · in use <value> <source> <state>`. When the
number in use is the declared one, `in use` is left out. Warnings (existing lines) add:

- `anthropic/max: provider rules changed around Tue 14:00 on weekly capacity, relearning`;
- `fit state not saved since 09:01 (disk full)`;
- `anthropic/max: 2 unacknowledged usage alerts (nullrouter quota alerts)`, in addition to the
  `usage alert` lines under the account, which give each alert's text (FR-027).

`--json`: each `AccountView` gains `meter` (`[WindowMeterView]`), `outside_use` (summary) and
`fit_note` (for an unfitted account). See [operator-socket.md](operator-socket.md) for the shape.

## Outside use and alerts (FR-021, FR-027)

```bash
nullrouter quota outside [provider [account]] [--since T] [--limit N]   # the outside-use list
nullrouter quota alerts                                                 # unacknowledged usage alerts
nullrouter quota ack <alert-id>|all [provider [account]]                # acknowledge
```

```text
anthropic/max            weekly   idle   Wed 02:10–02:30   4%
anthropic/max            5-hour   busy   Wed 15:40–15:50   3% beyond explained use
anthropic/max            weekly   steady 08–12 since Mon 08:00   0.2%/h
xai/main                 not polled: no outside-use detection
```

```text
01JB7… anthropic/max  Wed 02:20  4% of weekly used 02:10–02:30 with no traffic from 0router
01JB8… anthropic/max  Wed 16:00  3% of 5-hour used 15:40–15:50 beyond what 0router's traffic explains
```

`quota outside`, `quota alerts` and `quota ack` read and write files and work without a server.
With a server running, `ack` goes through the operator socket, so the server's view updates at
once.

## Exclusive use (FR-023)

```bash
nullrouter accounts exclusive anthropic max on     # used only through 0router
nullrouter accounts exclusive anthropic max off
```

- `on` for an account with no quota polls exits 2:
  `account xai/main: exclusive use needs quota polls; xai reports no quota for this account`.
- `accounts list` gains an `exclusive` column (`since <date>` or blank).

## check (FR-027)

`nullrouter check` adds one warning line per unacknowledged alert. Like every other warning, it
leaves the exit status alone (`check` exits 1 only on errors), because an alert changes nothing
on the account (FR-026):

```text
warn  anthropic/max: 4% of weekly used 02:10–02:30 Wed with no traffic from 0router (nullrouter quota ack 01JB7…)
```

## serve log (FR-016, FR-027)

One `tracing` line at `warn` per break and per alert, with stable fields:

```text
WARN quota.fit: provider rules changed provider=anthropic account=max window=weekly number=capacity around=2026-10-13T14:00:00Z replaced=13.8M
WARN quota.alert: unexplained use provider=anthropic account=max window=weekly start=… end=… amount=4% idle=true
```

Outside use on an account without an exclusive-use declaration logs nothing (FR-022).
