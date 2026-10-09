# Operator configuration

The operator's state lives in one directory, `$NULLROUTER_HOME` (default `~/.0router`):

```text
$NULLROUTER_HOME
├── config.toml     # unified models, combos, test settings, per-provider settings, plugin decisions, server settings
├── accounts.toml   # provider accounts and their secrets (mode 0600)
├── tokens.toml     # signed-in accounts' tokens and states (mode 0600)
├── tokens.lock     # lock for tokens.toml writers (mode 0600)
├── install-id      # this installation's random id (mode 0600)
├── keys.toml       # agent key digests (mode 0600)
├── dashboard.toml  # the dashboard token's digest and issue time (mode 0600)
├── proxies.toml    # named proxies and their credentials (mode 0600)
├── plugins/        # user and installed community plugins (*.toml, top level only), see plugins.md
├── routing/
│   └── proxies.json    # paused proxies: names, times and reasons, no addresses (mode 0600)
└── run/
    └── operator.sock   # the running server's operator socket (mode 0600)
```

Nothing here is required to load. A missing directory or file means no unified models,
default settings, no user plugins, no accounts and no keys. Write `accounts.toml` and
`keys.toml` with the CLI rather than by hand.

Full reference: [`specs/002-provider-model-registry/contracts/operator-config.md`](../specs/002-provider-model-registry/contracts/operator-config.md)
and slice 003's [`operator-cli.md`](../specs/003-request-pipeline/contracts/operator-cli.md),
with slice 005's additions in [`operator-cli.md`](../specs/005-account-sign-in/contracts/operator-cli.md).

## Running the server

```bash
nullrouter accounts add anthropic main        # paste the API key on stdin
nullrouter keys issue claude-code-laptop      # prints the agent key once
nullrouter serve                              # foreground; logs to stderr, redacted
```

`serve` listens on `127.0.0.1:20129` unless `--listen` or `config.toml` says otherwise:

```toml
[server]
listen = "127.0.0.1:20129"
```

It refuses to start if `accounts.toml`, `tokens.toml` or `keys.toml` is readable by group or others, and
names the file and the `chmod` that fixes it. Clients point their base URL at the server
and send the agent key wherever their API style carries one (`Authorization: Bearer`,
`x-api-key`, `x-goog-api-key`, …); see [api-styles.md](api-styles.md).

## Accounts

An account is one secret for one provider. A provider can have several; they are tried in
`order`, and an account that is rate-limited or failing cools down while the others serve.

```bash
nullrouter accounts add openrouter main                    # secret from stdin, never argv
nullrouter accounts add openrouter ci --env OPENROUTER_KEY # read from the environment at load
nullrouter accounts add openrouter backup --order 1
nullrouter accounts list                                   # name, order, …last4 or env:VAR, state
nullrouter accounts disable openrouter backup              # or enable, remove
```

```toml
# accounts.toml
schema = 1
[[account]]
provider = "anthropic"
name = "main"
secret = "sk-ant-…"              # or secret = { env = "ANTHROPIC_KEY_MAIN" }
order = 0
```

A secret is sent only to its own provider's endpoints, and only in that provider's auth
header. Logs, records and error bodies never carry it.

An account has a `kind`: `key` (an API key, as above) or `signin` (a subscription account
signed in through the provider's own sign-in). `accounts.toml` is schema 2. A schema 1 file
still loads, and the next save rewrites it as schema 2 with every account as `kind = "key"`.

```toml
# accounts.toml
schema = 2
[[account]]
provider = "grok-cli"
name = "work"
kind = "signin"                  # no secret here; the tokens live in tokens.toml
order = 0

[[account]]
provider = "opencode-go"
name = "main"
kind = "key"
secret = { env = "OPENCODE_GO_KEY" }
```

## Signing in a subscription account

anthropic (Claude Pro/Max), xai and grok-cli (SuperGrok) accounts sign in with the
provider's own sign-in page instead of an API key:

```bash
nullrouter accounts signin grok-cli work
nullrouter accounts signin xai main --paste       # don't open a browser
nullrouter accounts signin anthropic max
```

The command runs the flow the provider's plugin declares, writes the account and its
tokens, and tells a running server to apply them (`applied`). Signing in under an existing
name replaces that account's tokens.

**Device code** (grok-cli): open the printed page on any device and approve. Nothing needs
to run on the device you approve from.

```text
Open this page on any device and approve:
  https://accounts.x.ai/device?user_code=WXYZ-1234
Code: WXYZ-1234 (expires in 15 min)
Waiting for approval… done.
grok-cli/work: signed in as a…@example.com (SuperGrok); applied
```

**Browser with paste-back** (xai, anthropic): the link is printed every time. On a machine
with a display the browser opens too, unless you pass `--paste` or `--no-browser`. This
works headless, for example over SSH:

- **xai**: the browser ends on a `http://127.0.0.1:…/callback` address. On another device
  that page doesn't load. Copy its full address from the address bar and paste it. The
  code is in the address.
- **anthropic**: Anthropic's page shows a code (`code#state`). Paste that.

```text
Open this link in a browser (on any device):
  https://auth.x.ai/oauth2/authorize?…
If the browser ends on a page that doesn't load, copy its full address and paste it here.
Paste the address or code: _
xai/main: signed in as a…@example.com; applied
```

A pasted address must carry the `state` 0router sent. Browser flows give up after 10
minutes; a device code ends when the provider says it expires.

**anthropic terms warning.** Before the link, every anthropic sign-in prints this warning,
including a repeat sign-in of the same account:

```text
Anthropic's terms limit the use of Claude Pro/Max subscriptions outside Anthropic's own
apps. Anthropic may refuse these requests or act on your account. You carry that risk.
Continue? [y/N]
```

Anything but `y` or `yes` ends the sign-in with nothing written. `--accept-terms-risk`
answers yes for scripted use, and the warning is still printed. Nothing is stored, so you
are asked every time. 0router sends a signed-in anthropic account's requests with the
plugin's declared headers and the body as your client sent it. It doesn't disguise them.
If Anthropic refuses them, the account is marked "refused by provider".

**Exit 5.** A sign-in that is denied, expires, is abandoned (Ctrl-C, or `n` at the
warning) or fails ends with exit code 5. Nothing is saved:

```text
xai/main: sign-in ended: access_denied; nothing saved
```

### Tokens

`tokens.toml` holds each signed-in account's access and refresh tokens and their expiry.
It also holds the token response's scope, the email, user id and plan the account signed
in as, when it signed in and last refreshed, the hosts the tokens are bound to, and any
state that must survive a restart. It must be mode 0600, like `accounts.toml`. The server
(refreshing) and the CLI (signing in, removing) both write it, each under an exclusive
lock on `tokens.lock`. A rotated refresh token is written before the old one is dropped.
Don't edit it by hand.

A token is sent only to the hosts the provider's plugin declared when the account signed
in: its endpoint, sign-in, quota and live-model hosts. If a replacing plugin names a new
host, the account is skipped with that host in the reason until you sign it in again.
Redirects are never followed. Tokens never appear in logs, records, errors, CLI output or
the operator socket. Listings show `…last4`.

`install-id` is a random id made once per installation. A plugin may send it as an
identity header in place of the official client's machine id.

### Account states

```bash
nullrouter accounts list                  # adds kind and sign-in state
nullrouter accounts list xai --long       # adds email, plan tier, token expiry, last refresh
```

```text
provider   name   kind    order  secret     state
anthropic  max    signin  0      …h3Kq      active
anthropic  api    key     1      …9fA2      active
xai        main   signin  0      …Zt1c      needs sign-in since 2026-10-03 14:02 (invalid_grant)
grok-cli   work   signin  0      …p0Lm      refreshing (token expired 40 s ago, retrying)
  → run: nullrouter accounts signin xai main
```

| State | Meaning | Serves | Ends when |
|---|---|---|---|
| `active` | usable | yes | — |
| `refreshing` | the token expired and the last refresh failed for a passing reason (timeout, 5xx, 429); retrying with backoff | no | a refresh succeeds |
| `needs sign-in` | the provider rejected the refresh for good (`invalid_grant`, …), or the account has no tokens | no | you run `accounts signin` again |
| `refused by provider` | the provider refused the account's requests because of how they were sent | no | you sign in again, or run `accounts enable` to retry |
| `disabled` | your choice | no | `accounts enable` |

0router refreshes a token shortly before it expires. While the old token is still valid, a
failed refresh is only retried and the account keeps serving. `needs sign-in` and
`refused by provider` are kept in `tokens.toml` with their time and reason. Requests skip an
account that can't serve and try the next. The request record names the account and the
command to run, for example `needs sign-in: run nullrouter accounts signin xai main`.
`accounts remove` also deletes the account's tokens.

## Quota

0router polls each account's quota where the provider's plugin declares how (anthropic,
grok-cli, opencode-go, opencode-zen), every 10 minutes by default, and keeps every poll
with a tally of the traffic sent in between.

```bash
nullrouter quota                                   # current windows per account
nullrouter quota history grok-cli work --since 2026-10-01 --limit 50 --json
nullrouter quota poll anthropic max                # poll now (needs a running server)
nullrouter quota interval grok-cli work 15m        # or `default`; floor 2 min
nullrouter quota prune --before 2026-09-01 [provider [name]]
nullrouter quota forget grok-cli work              # delete one account's history
```

```text
anthropic/max            polled 14:20 (3 min ago, every 10 min)
  5-hour                 62% left   resets 16:00
  weekly                 81% left   resets Thu 09:00
xai/main                 quota not reported
```

Every value belongs to the poll time shown on its account line. History lives in
`quota/<provider>/<account>.jsonl`, with a tally checkpoint beside it
(`<account>.tally.json`), both mode 0600. Nothing is dropped automatically: history stays
until you prune it or forget the account. `accounts remove` keeps it.

## Agent keys

Every client request carries an agent key. 0router stores only a digest and the last four
characters, so a lost key can't be shown again: issue a new one.

```bash
nullrouter keys issue claude-code-laptop      # prints the key (0r-…) once
nullrouter keys list                          # id, name, harness, …last4, created, revoked
nullrouter keys revoke claude-code-laptop     # by name or id
nullrouter keys issue codex-ci --harness codex-cli   # issue with a harness tag
nullrouter keys tag codex-ci claude-code      # set or replace the tag, by name or id
nullrouter keys tag codex-ci --clear          # remove it
```

The key's id is the agent's identity: provider session ids are derived from it, and
records name it.

The harness tag is a label for you: the dashboard shows it as a badge on the key and
`keys list` shows it in the `HARNESS` column (`-` when none). It is 1 to 32 characters with no
control characters, it changes nothing about routing, and a refused tag exits 1 and issues or
changes nothing. `keys tag` works on revoked keys too, never touches the secret, and unknown
keys exit 2.

**Downgrading.** A `keys.toml` that holds a tagged key is refused by a binary from before the
tag existed. Run `nullrouter keys tag <key> --clear` for each tagged key before going back.

## The dashboard

`serve` also serves a read-only web dashboard on its own port, `127.0.0.1:20130`. It shows the
endpoint, agent keys, providers, quota, routing and request records on pages that read the same
facts the CLI prints; it changes nothing, so every change stays in the CLI. It runs in the
`serve` process, on its own listener, and a fault in it never slows or breaks a client request.

```toml
# config.toml
[dashboard]
enabled = true                  # false: `serve` opens no dashboard port
listen = "127.0.0.1:20130"      # a loopback address only: 127.0.0.0/8, ::1 or localhost
```

A `listen` that isn't loopback is refused at load (`config.toml:L:C dashboard.listen: must be a
loopback address; network binding is not supported`). The setting applies at the next `serve`
start; a reload doesn't rebind the port. If the port is taken, `serve` starts anyway, the clients
are served, and `nullrouter dashboard status` and `nullrouter check` say why the dashboard isn't
listening.

### Signing in

Until you issue a token, the dashboard's only page tells you to run the command that does:

```bash
nullrouter dashboard token     # prints nrd_… once; a running server is told to reload
nullrouter dashboard status    # on or off, the address, whether it is listening, when the token was issued
```

Open `http://127.0.0.1:20130` and enter the token once. The browser keeps a cookie
(`nr_dashboard`, `HttpOnly`, `SameSite=Strict`) for 400 days, the longest a browser allows, so it
doesn't ask again. 0router stores only the token's SHA-256 digest in `dashboard.toml` (mode
0600), so a lost token can't be shown again: issue a new one. Issuing a token signs out every
browser that holds the previous one. There is no sign-out page; clearing the browser's cookies
does the same for one browser.

A wrong token is refused after a delay that starts at 1 second and doubles to 30 seconds with
each wrong token, and a right one resets it. The delay is shared, not per browser. Other limits:
a request whose `Host` isn't the dashboard's own gets `421` (DNS rebinding), a sign-in posted
from another site gets `403`, and every method but `GET` gets `405` (the sign-in post is the one
exception).

### What the cookie reaches

Browsers scope a cookie by host name, not by port. The dashboard cookie is therefore also sent to
any other web server you run on `127.0.0.1` and visit in the same browser. This is a known and
accepted limit: a process running as your own user can already read `~/.0router`, and the only
thing the cookie opens is a read-only view that shows no secret and no prompt. (Security review
finding L1, accepted on 2026-10-06.) To give the dashboard its own cookie host on Linux, where
every `127.0.0.0/8` address is loopback, set `listen = "127.0.0.2:20130"` and open that address.
The default stays `127.0.0.1:20130` on every system.

## When a stream breaks

When a provider's stream fails after some output has reached the client, 0router either
**restarts** the answer on the next target (the client sees a note and the new answer), or
ends the stream with an **error event** in the client's style.

```toml
[pipeline]
break_behaviour = "restart"   # restart | error_event
```

```bash
nullrouter behaviour set-break error_event              # the operator default
nullrouter keys set-break claude-code-laptop restart    # per key; `default` clears it
nullrouter behaviour show                               # break_behaviour  restart  (default)
```

`behaviour show` reads `config.toml` only and works without a server. It prints each setting
and marks the ones still at their default; `--json` gives
`{"break_behaviour":{"value":"restart","default":true}}`. A `config.toml` that doesn't load
prints the same error `check` gives, exit 1.

## Private endpoints

Plugin URLs may not point at loopback, private, link-local or metadata addresses,
`localhost`, `*.local` or `*.internal`, and the address a host resolves to is checked again
at request time. To route to a local model server, opt in:

```toml
allow_private_endpoints = true
```

## Routing

When a target has several accounts, 0router decides which one serves each request:

- **Warm**: an agent whose prompt prefix is still cached on an account stays on that account,
  whatever the other numbers say. It moves only when that account can't serve (rate limited,
  at its reserve floor, out of service) or to leave pay-as-you-go for a subscription. The
  record names the reason.
- **Cold**: work that is cached nowhere is spread over the subscription accounts. Each
  account's share follows its **pace**: how much of its quota is left against how much time
  its windows have left. A running **deficit** per account corrects for what each one actually
  received, over an **amortization window** (5 hours by default).
- **Overflow**: pay-as-you-go accounts serve only when no subscription can. They share that
  work by priority divided by the price in effect now, so a cheaper account takes more. An
  account with no declared price counts as price 1, and `check` warns about it.
- **Last resort**: a subscription held back only by its reserve floor serves after
  pay-as-you-go has failed, rather than failing the client.

Every decision is recorded with the full candidate table (see [Request records](#request-records)).

### Priority

```bash
nullrouter accounts priority anthropic max 2     # twice the cold work of a priority-1 account
nullrouter accounts priority opencode-go main 0  # never cold work, last resort or fallback
```

Priority multiplies an account's share of cold work: any number of 0 or more, default 1.
Priority 0 means the account takes no cold work at all, and is never a fallback for it; an
agent that is already warm on it still stays. `accounts list` shows the priority column.

### Account overrides

A plugin declares its accounts' quota windows, cache lifetime and prices. Override them per
account when yours differ (a bigger plan, a negotiated price):

```bash
nullrouter routing set anthropic max cache_lifetime=1h reserve=10%
nullrouter routing set anthropic max window.5-hour.capacity=12000000
nullrouter routing set openrouter main price.input=3 price.output=15
nullrouter routing unset anthropic max window.5-hour      # or one field: window.5-hour.capacity
```

They are saved in `accounts.toml`:

```toml
[[account]]
provider = "anthropic"
name = "max"
kind = "signin"
priority = 2

[account.routing]
cache_lifetime = "1h"                            # > 0, at most 24h
reserve = "10%"                                  # 0–50%, every window; default 5%
window."5-hour" = { capacity = 12_000_000 }      # also length, reserve
# price = { input = 3.0, output = 15.0 }         # per million tokens; replaces the plugin's schedule
```

The values are checked by the same rules as the plugin's (see `docs/plugins.md`, `[routing]`).
A window name that matches none of the plugin's windows is refused.

### Amortization window

```toml
# config.toml
[routing]
amortization = "5h"                  # the default

[routing.amortization_for]
"sonnet" = "1h"                      # a unified model
"anthropic/claude-opus-4-1" = "24h"  # a direct target
```

```bash
nullrouter routing window 2h            # the default
nullrouter routing window sonnet 1h     # one target
nullrouter routing window sonnet default
```

Windows are aligned to the Unix epoch, so a 5-hour window runs 00:00–05:00, 05:00–10:00 and so
on, in UTC. Deficits start from zero in each window. A shorter window corrects faster; a longer
one evens out bursts over more time.

### The routing view

`nullrouter routing [target] [--json]` shows what the next decision would see. It needs a
running server and exits 4 without one.

```text
amortization 5h (05:00–10:00 UTC, 2h 48m left) · records kept, last sync 0.4 s ago

sonnet                              subscription tier
  account          source     pace  share  deficit    priority  cache  windows
  anthropic/max    polled      1.42  61%    +91.2k     1         5m     5-hour 5.6M/9.0M wtok · floor 5% · rst 10:00 | weekly 72.9M/90.0M wtok · floor 5% · rst Thu 09:00
  anthropic/pro    polled      0.88  39%    -91.2k     1         1h     5-hour 1.4M/4.5M wtok · floor 10% · rst 08:40
  opencode-go/main estimated   1.05  —      0          0         5m     rolling 4.2M/6.0M wtok · floor 5% · cold work off (priority 0)
                                    pay-as-you-go tier
  openrouter/main  payg        —     100%   0          1         5m     price now 3.00/Mtok in
```

| Column | Meaning |
|---|---|
| `source` | `polled`: the provider reports quota. `estimated`: no report, so 0router counts traffic against the plugin's declared limits. `payg`: neither, so the account is pay-as-you-go. `(pending first poll)`: no poll yet, so the account is treated as on pace. `(stale)`: the last poll failed, so the estimate keeps running from the last good one. |
| `pace` | above 1, the account has quota to spare for the time left; below 1, it is running short |
| `share` | its share of the next cold work |
| `deficit` | tokens it is owed (+) or has had beyond its share (−) in this amortization window |
| `cache` | the cache lifetime in effect: the account's override, else the plugin's |
| `windows` | per window, what is left over its capacity (`wtok` weighted tokens, `req` requests), the reserve floor, and the reset time |

Between polls, what is left is the last poll less the traffic 0router sent since, costed by
the window's meter. `--json` gives the exact numbers per window: `remaining_at_poll`,
`cost_since_poll`, `remaining_now`, `capacity`, `reserve` and `resets_at`.

Warnings appear on their own lines, for example
`opencode-go/main: window rolling capacity assumed` when no capacity is declared anywhere for
that window, or `records not kept since 09:01 (disk full): 214 requests`.

## Request records

0router keeps a record of each request: agent, target, the routing decision with every
candidate, every attempt with its account, placement reason and outcome, time to first token
and total, usage, and anything left out when translating across styles. Records are written to
`records/YYYY-MM-DD.jsonl` (one file per UTC day of arrival) and kept until you remove them.
No record holds prompt text, a header value or a secret.

```bash
nullrouter records list --limit 20                      # newest first, read from disk
nullrouter records list --account anthropic/max --reason overflow --since 2026-10-04
nullrouter records list --limit 20 --before rq_01JAB3…  # the next page, older than that record
nullrouter records show rq_01JAB3…                     # the decision table and every attempt
nullrouter records prune --before 2026-09-01            # prints how many were removed
nullrouter records forget --account anthropic/max       # or --agent KEY
```

`records list` filters by `--provider`, `--account P/N`, `--agent KEY`, `--model` (the target
the client named, its unified model, or the model that served it), `--reason` (a placement
reason: `warm`, `cold_by_deficit`, `moved_for_capacity`, `left_pay_as_you_go`, `overflow`,
`last_resort`, `retry`, `fallback`) and `--since`. `--before ID` lists only records older than
that one, after the other filters and before `--limit`, so passing each page's last id as the
next `--before` walks the whole journal; an id that names no record is `no record ID`, exit 1.
Paging back reads only the segment holding the cursor and the older ones, and a long-running
server keeps an in-memory index of each segment it pages through. All `records` commands work without a
server. With a server running, a request still in flight shows `in progress`; without one,
a record that never closed shows `interrupted`.

`records forget --agent` also drops that agent's cache fingerprints, so its next request is
cold. `prune` and `forget` take the journal lock and exit 1 if it isn't free within 10 s.
Error bodies sent to clients carry the record id, so a failure can be looked up.

### Usage and latency

Two read-only commands summarise the records. Both ask a running server, or read the journal
in their own process when none is running, and both give the same numbers.

```bash
nullrouter usage                      # today, local midnight to now
nullrouter usage --period 7d          # today, 24h, 7d, 30d, 60d or all
nullrouter latency                    # the last 24 hours, per agent and per provider
```

`usage` prints requests (with how many are in flight and how many reported no usage), input
tokens (uncached), cached tokens, output tokens, an estimated cost, and requests per agent and
per provider. Add `--json` for exact integers. The dashboard's Usage page shows the same
figures for the same period.

**Est. Cost** is an estimate, not billing. It prices each record with the prices declared today
for its provider and account, at the rate in effect when the request arrived; earlier price
changes are not tracked. Cached tokens are priced at the cache rate when one is declared, and at
the input rate when none is. A request is left out of the cost, and counted under "not priced"
with its reason, when its account has no price, when it has output tokens but the price has no
output rate, or when it reported no usage.

The per-agent rows can add up to less than the total: a request refused before a key matched
counts in the total but belongs to no agent. A request that fell back counts once for each
provider it tried, so the per-provider rows can add up to more.

`latency` lists, for each agent, the router's own overhead and the time to first token, and for
each provider its own wait for a first token (measured from the start of its attempt, so a
fallback's earlier tries don't count), as p50 / p95, with the last response and, for a provider,
requests by agent. `none` means no request in the window had that value; it is never `0 ms`.

### Durability

The routing state (which account each agent is warm on, and the deficits) and the model
verdicts (`routing/verdicts.jsonl`, see [Model tests](#model-tests)) are kept in `routing/` next
to the records, so all of them survive a restart.

| Event | Records | Warm state and deficits |
|---|---|---|
| clean shutdown | nothing lost | nothing lost |
| 0router crash | nothing lost; requests in flight become `interrupted` | nothing lost for finished requests |
| power loss or OS crash | at most about the last second | at most about the last second |
| disk full | see below | kept in memory, written again when space returns |

A client gets the last byte of its answer only after the record's final line is written.

**When the disk is full**, serving continues. Up to 10,000 pending lines are held in memory
and written when space returns (retried every 5 s); records beyond that are not kept, only
counted. The server logs a warning when writing first fails, every minute while it fails, and
when it resumes. `nullrouter routing` and `nullrouter check` show
`records not kept since T (disk full): N requests`; `check` adds `verdicts not being kept since
T`, as verdict changes go through the same writer and are held and retried the same way.

Files under `records/` and `routing/` are mode 0600, the directories 0700, and `check` warns
about any other mode.

## Addressing models

Clients name a target in one of two ways:

- **`provider/model`**: a direct target. `provider` is an id or alias (`kr`, `kiro`), and
  the model part may contain further `/` (`openrouter/meta-llama/llama-3`).
- **A bare name**: a **unified model** you declared. A bare name never falls back to
  a provider's model, so `claude-sonnet-4.5` alone is not found unless you declared it.

```bash
nullrouter resolve kr/claude-sonnet-4-5 --json
nullrouter model kr claude-sonnet-4-5          # what the provider declares about it
nullrouter providers --capability tts
```

## Declaring a unified model

A unified model is one name backed by one or more provider models, listed in order.

```toml
schema = 1

[[unified_model]]
name = "sonnet"                  # non-empty, no "/"
kind = "llm"                     # optional; members must not conflict with it
members = [
  { provider = "kr",         model = "claude-sonnet-4-5" },           # id or alias
  { provider = "openrouter", model = "anthropic/claude-sonnet-4.5" },
]
```

Check it:

```bash
nullrouter resolve sonnet
# unified sonnet:
#   0. kiro claude-sonnet-4-5 → upstream claude-sonnet-4.5
#   1. openrouter anthropic/claude-sonnet-4.5 → upstream anthropic/claude-sonnet-4.5
```

```bash
nullrouter unified              # every unified model, then `dropped unified model …` lines
nullrouter unified sonnet       # one; exit 2 if it isn't loaded
```

`unified` prints the members and limits notes as `resolve` does, and lists the models a
skipped plugin made the server drop. `unified NAME --json` prints exactly what `resolve NAME
--json` prints. With none declared it says `no unified models; declare one with
[[unified_model]] in config.toml`.

Each member's upstream id is resolved once, at load. Rules:

- every member model must be declared by its provider, unless that provider is
  passthrough;
- a provider appears at most once per unified model;
- typed members must agree on `kind` (untyped members never conflict).

## Phases in records

Each attempt of a record carries the marks the router took and the phases derived from them.
`records show ID` prints a table per attempt:

```text
attempt 2  openrouter/main  reused · HTTP/2                              ok
  retry wait       2000.0 ms
  router overhead     0.3 ms
  waiting for provider 3205.5 ms
  generation      31740.5 ms
  delivery           12.3 ms
total 41000.6 ms = sum of phases
```

- **Router overhead** is 0router's own work before sending. A deliberate wait before a
  same-account retry is **retry wait**, and a sign-in token refresh counts inside **connect**;
  neither is overhead.
- **Connect**, **headers** and **first token** are the network and the provider's wait. A phase
  that didn't happen reads `not applicable`. The phases add up to the total.
- The attempt line names the connection (`new connection` or `reused`), the HTTP version and
  the proxy, and a timeout names which one fired, its value and who set it.
- `records list` has a `SLOWEST` column: the request's longest phase and whose side it is on.
- `records show --json` carries `timing` (the marks) and `phases` per attempt.

Latency never changes routing: no provider, account or placement depends on a measured phase.

## The live view

```bash
nullrouter live           # redraws once a second until Ctrl-C; a snapshot per second if piped
nullrouter live --json    # one snapshot: {"as_of": …, "paused_proxies": […], "in_flight": […]}
```

It lists the requests in flight with their agent, target, current attempt, current phase and
time in it, and what earlier attempts of the same request did. A finished request leaves the
list at once. It needs a running server and exits 1 with `no server is running` otherwise. A slow
`live` client never slows a request.

## Connection settings

Timeouts, connection reuse, HTTP/2 and same-account retries are set per provider in
`config.toml`; timeouts also per model:

```toml
[provider.openrouter.connection]
connect_timeout_ms = 5000          # 1-3 600 000
header_timeout_ms = 10000          # from the attempt's start, connect included
first_token_timeout_ms = 60000     # 0 = off, which is the default
stall_timeout_ms = 360000          # time with no data at all, so keepalives keep a stream alive
reuse = true                       # false: a new connection for every request
http2 = false                      # false: HTTP/1.1 only; true: negotiate (the default)

[provider.openrouter.model."anthropic/claude-opus-4.1".connection]
first_token_timeout_ms = 300000    # timeouts only at model level

[provider.openrouter.retry]
all = { retries = 2, delay_ms = 1000 }
"503" = { retries = 3, delay_ms = 2000 }
```

or with the CLI, which refuses a value the config would refuse and writes nothing then:

```bash
nullrouter connection show [PROVIDER] [--json]     # every value with the level that set it
nullrouter connection set openrouter header-timeout 10s
nullrouter connection set openrouter --model anthropic/claude-opus-4.1 first-token-timeout 5m
nullrouter connection set openrouter first-token-timeout off
nullrouter connection set openrouter reuse off     # reuse and http2 take on|off
nullrouter connection set openrouter retries.503 3        # then:
nullrouter connection set openrouter retry-wait.503 2s    # a wait needs its retry count first
nullrouter connection unset openrouter header-timeout
```

**Timeouts.** For each timeout the first value set wins: operator per model, operator per
provider, plugin per model, plugin per provider, then the built-in default (60 s connect and
headers, 360 s stall, first token off; `FETCH_CONNECT_TIMEOUT_MS` and `STREAM_STALL_TIMEOUT_MS`
still override the defaults). A timeout is a failed attempt: it is classified, retried and
failed over like another transport failure. A thinking stream is never cut: thinking output is
output. Settings apply from the next request, with no restart.

**Reuse and HTTP/2.** `http2 = true` only lets the connection negotiate; it never forces HTTP/2
against a server that doesn't offer it. A plugin may declare `http2 = false` for a provider
that doesn't speak it; the operator's value wins. One endpoint without HTTP/2 puts the whole
provider on HTTP/1.1.

**Retries.** A rule is `{ retries, delay_ms }` with 0-5 retries and a wait of 0-30 000 ms,
under `all` or a three-digit status. For a failure the first rule that applies wins: operator
status, operator `all`, plugin status, then the built-in table. On a 429 a `retry-after` of 5 s
or less replaces the configured wait. The wait is recorded as the attempt's retry wait.

## Proxies

A proxy is named in `proxies.toml` and assigned to all providers, one provider or one account.
The account's assignment wins over the provider's, which wins over the all-providers one;
`none` is an answer and stops the search. Plugins can't declare a proxy.

```bash
nullrouter proxy add eu-exit http://10.0.0.5:3128 --username me   # password on stdin, or --password-env VAR
nullrouter proxy list                                   # name, address without credentials, user ✓, state, used by
nullrouter proxy use eu-exit --all                      # or --provider P, or --account P/N
nullrouter proxy use none --account anthropic/work      # this account goes direct
nullrouter proxy clear --provider anthropic             # remove the assignment at that level
nullrouter proxy remove eu-exit                         # refused while assigned; names where
nullrouter proxy fixed eu-exit                          # probe now; resume traffic if it answers
```

The schemes are `http`, `https` and `socks5`. The password is never read from an argument and
never printed, and it appears in no record, live view, dashboard page, log line or error; with
`--password-env` only the variable's name is stored. Everything sent for an account goes
through its proxy: requests, token refreshes, quota polls and job polls.

**Pause.** When a proxy can't be reached, 0router probes it at once. If the probe fails too, the
proxy is **paused** (`routing/proxies.json`) and everything behind it is skipped as
`proxy NAME paused`, with no direct fallback and no cooldown for the account. The pause
survives a restart. It ends when you run `nullrouter proxy fixed NAME` and the proxy answers,
or when a reload finds the proxy's definition or an assignment to it changed. `nullrouter
accounts list` has a `PROXY` column, `connection show` and `live` show a pause, and `check`
reports it as an error (exit 1) and reports an assignment naming an undefined proxy as a note.

## Combos

A combo is an ordered fallback chain of unified models or other combos. Clients ask for it by
name, as for a unified model, and it is listed next to unified models in every style's model
list. Only `config.toml` declares combos; a plugin can't.

```toml
[[combo]]
name = "coder"                         # non-empty, no "/", unique among unified models and combos
members = ["sonnet", "fallback-chain"] # unified models or combos, tried in order

[[combo]]
name = "fallback-chain"
members = ["gpt", "glm"]
```

A request for `coder` tries `sonnet` with its own retries, accounts and fallbacks, and moves to
the next member only once those are used up. It never moves on once the client has received
part of an answer, and a request error that doesn't fall back (a bad parameter, say) ends the
whole combo. A unified model reached twice through nested combos is tried once. The request's
record names the combo, and each attempt its path (`coder › fallback-chain › gpt`).

At load a combo may not clash with a unified model's or another combo's name, contain itself,
be empty, name an unknown member, or mix members of different kinds. These are errors like
unified-model errors. A combo that needs a unified model dropped at startup is dropped too.

```bash
nullrouter combos               # every combo with its members, then `dropped combo …` lines
nullrouter combos coder         # one, nested combos expanded; exit 2 if it isn't loaded
nullrouter resolve coder        # the same tree
```

## Model tests

A test proves that a model works on one account: one real, minimal call through 0router (a
two-letter prompt for text; the smallest request of the model's type for embedding, speech,
transcription, image and video). It is billed, recorded, counted against quota and paced like
any call. Tests run only when you ask, and need a running server.

```bash
nullrouter test anthropic/claude-sonnet-4-5               # every account that has the model
nullrouter test anthropic/claude-sonnet-4-5 --account max
nullrouter test sonnet                                     # each member on each account that serves it
nullrouter test coder                                      # one call through the combo
nullrouter test --all                                      # every pair a unified model or combo reaches
```

Before more than one call it prints the count by type and asks `Continue? [y/N]`; `--yes`
skips the question. Each pair prints one line as it finishes:

```text
PASS     anthropic/max        claude-sonnet-4-5          1.8 s (first output 0.9 s)
BROKEN   anthropic/max        claude-opus-4-1            model not available: 403: …
UNKNOWN  openrouter/main      anthropic/claude-sonnet-4.5  503: upstream overloaded; retest in 1 min
SKIPPED  xai/main             grok-4                     account needs sign-in: …
```

Ctrl-C stops the calls not yet sent; finished results stay saved. A combo test makes one call
through the combo as a client would and prints which member answered and every member it
tried, nested under its combo. Its attempts that answered or were definitively rejected update
their pairs; any other failure is shown as `(not saved)`.

### Verdicts

- **PASS**: the provider returned a usable result of the model's type.
- **BROKEN**: a definitive rejection: the model doesn't exist, isn't available to this account,
  or doesn't support the request type (0router's own list, plus a plugin's `[[rejections]]`).
  Routing skips the model on that account only, and a request whose every pair is BROKEN fails
  at once, naming them. Model lists still show it.
- **UNKNOWN**: anything else: rate limits, server errors, timeouts, broken connections, empty
  or malformed answers. Routing is unchanged.

An untested model routes normally. A rejected key or an expired sign-in marks the account (see
[Account states](#account-states)), never its models. A verdict returns to untested when the
account's key or sign-in changes, its plugin changes, or the account or provider is removed;
that is checked at start, at every reload and when a token is replaced.

```bash
nullrouter verdicts                                      # every verdict, then the combo results
nullrouter verdicts --provider anthropic --state broken  # also --account, --model
nullrouter verdicts mark anthropic max claude-opus-4-1 --note "not on our plan"
nullrouter verdicts clear anthropic max claude-opus-4-1  # back to untested
```

`mark` sets BROKEN with source `operator`; it is never retested. Both go through the running
server, or, with none, are written to `routing/verdicts.jsonl` and apply at the next start.
`unified NAME` shows each member's verdicts per account, and `records list --test` / `--no-test`
picks test calls; a test record carries no agent, no prompt and no output.

### Retests

An UNKNOWN verdict is retested on its own, by default after about 1 minute, 5 minutes, 30
minutes and then every 6 hours, until it gives PASS or BROKEN. A BROKEN from a test is retested
only when `broken_retest` is on. A retest waits while its account can't serve (disabled, needs
sign-in, rate-limited) or a quota window is at its reserve floor; `verdicts` shows why in the
`next` column. An UNKNOWN combo result is retested the same way, waiting while no account of
its first unified model can serve. A combo's result never steers routing, and it is cleared
when the combo's members change.

### `[tests]`

```toml
[tests]
retest = ["1m", "5m", "30m", "6h"]     # after an UNKNOWN; 1–10 steps, each ≥ 30s, never shorter; the last repeats
broken_retest = "off"                  # "on" (every 24h) or an interval ≥ 1h
concurrency = 4                        # test calls at once, retests included (1–32)

[tests.timeout]                        # each 5s–30m
text = "30s"
embedding = "30s"
tts = "30s"
stt = "30s"
image = "5m"
video = "5m"
```

```bash
nullrouter verdicts settings                       # each value beside its default
nullrouter verdicts settings retest 2m,10m         # or `default`
nullrouter verdicts settings broken-retest on      # or `off`, or an interval
nullrouter verdicts settings timeout image 10m
nullrouter verdicts settings concurrency 2
```

## Per-provider settings

```toml
[provider.openai]
allow_uncatalogued_models = false   # default true
```

With the default, `openai/brand-new-model` is forwarded unchanged even though openai does
not declare it. With `false`, it is a not-found error. Passthrough providers accept any
model either way.

## Replacing a bundled provider

A user plugin whose `id` matches a bundled provider stays **pending**: the bundled
provider remains active and `check` reports the conflict. Decide in `config.toml`:

```toml
[plugin_decisions]
gemini-cli = "replace"   # the user plugin becomes active; its settings above still apply
kiro       = "decline"   # bundled kiro stays; the user file is reported as declined
```

## Errors, startup, and reload

Every error is reported as `file:line:col path: rule`:

```text
config.toml:4:1 unified_model[0].members[1].provider: unknown provider "xx"
```

- **At startup**:
  - an invalid `config.toml` is fatal;
  - an invalid user plugin is skipped and reported, and everything else loads;
  - a unified model that needs a skipped plugin is dropped and reported.
- **On reload**:
  - the whole new state is built and validated first;
  - any error rejects the reload, and the previous state keeps serving;
  - in-flight requests keep the snapshot they started with;
  - nothing watches the files. Each mutating command (`accounts`, `keys`, `behaviour`,
    `quota interval`, `routing set`/`unset`/`window`, `connection set`/`unset`, `proxy add`/`remove`/`use`/`clear`/`fixed`,
    `verdicts settings`, `plugins install`/`uninstall`) writes its file atomically and then asks the running
    server to reload over the operator socket. It prints `applied` when the server
    acknowledged, or `saved; applies at next start` when no server is running. A hand
    edit applies at the next start or the next such reload. If the reload loads a unified
    model whose members' limits differ, the command prints the note on stderr.

`nullrouter check` prints the load report:
- provider counts;
- pending and declined conflicts;
- withheld credentials;
- skipped plugins;
- dropped unified models and combos, and combo load errors;
- notes for unified models whose members differ in `context_length` or `max_output_tokens`
  (`note: unified model sonnet: members differ in context_length: kiro 200000, openrouter 128000`;
  the model still loads, and `resolve` prints the same note);
- quota windows a provider reports that no `[[routing.window]]` meter names (paced in their own
  unit);
- pay-as-you-go accounts with no price, and an `explicit` cache mode on a provider none of whose
  endpoints speaks a style with cache markers;
- file modes of the sign-in, quota, record and routing files;
- with a server running, whether records and verdicts are being kept (disk full).

A note or warning doesn't change the exit code; a skipped plugin, a dropped unified model or a
file `serve` refuses to start with exits 1.

## Live checks (opt-in)

The live checks send a few tiny requests with your real accounts and print what the
providers answered. They never run in CI and print no tokens. Keep their accounts in a
separate home (`.nr-live/` is git-ignored), then point both the CLI and the tests at it:

```bash
export NULLROUTER_HOME=$PWD/.nr-live
nullrouter accounts signin anthropic max      # add --paste over SSH
nullrouter accounts signin grok-cli work
nullrouter accounts signin xai main           # optional: L2 and L4 for xai
nullrouter accounts add opencode-go main      # optional: key accounts with [quota]

NR_LIVE=1 cargo test -p nullrouter-engine --test live -- signin_anthropic signin_grok_cli --nocapture
NR_LIVE=1 cargo test -p nullrouter-engine --test live -- token_lifetimes --nocapture
NR_LIVE=1 cargo test -p nullrouter-engine --test live -- quota --nocapture
NR_LIVE=1 cargo test -p nullrouter-engine --test live -- live_routing_matches_polls --nocapture
NR_LIVE=1 cargo test -p nullrouter-engine --test live -- model_tests --nocapture   # NR_LIVE_VIDEO=1 adds video
```

| Check | Sends | Prints |
|---|---|---|
| `signin_anthropic` (L1) | one Messages request, `max_tokens` 5, per anthropic sign-in account | `SERVED` with the answer and usage, or `REFUSED` with the status and the provider's text for `[[signin.refused]]`. Note whether sign-in showed the code page ("paste the code") or fell back to loopback: the token store doesn't record it. |
| `signin_grok_cli` (L3) | three streamed Responses requests through one grok-cli account: (a) every `[identity]` header, (b) the fixed-value headers only, (c) every header and a body with an `item_reference` and foreign item ids | `PASSED`/`FAILED` per variant, with the identity header names sent and the error text |
| `token_lifetimes` (L4) | one refresh per sign-in account, saved like any refresh | the stored and the fresh `expires_in`, whether the refresh token `ROTATED`, and a hint when the lifetime is under 2 × `refresh_lead` |
| `model_tests` (spec 011) | one model test per type you hold an account for (text, embeddings, image, speech, transcription; video with `NR_LIVE_VIDEO=1`), on the first account that can serve it, kept like any test's verdict | one line per test: `PASS`, `BROKEN` or `UNKNOWN` with the reason. It fails on a BROKEN, or when nothing passed; an UNKNOWN is for you to recognise (a rate limit, an overloaded provider). |
| `live_routing_matches_polls` (L7) | per polled account: one quota poll, one tiny request, a second poll | for each window, the routing view's `remaining_now` beside the poll's figure, and how far it fell after the request beside the cost its meter charged. Any window where the provider charged more than 1% of capacity differently is listed under `METER CORRECTIONS NEEDED`, for a dated fix to the bundled plugin's `[[routing.window]]`. |
| `quota` (L2, L5) | one quota read per account with `[quota]` (the fallback only when the primary yields no window), and `GET api.x.ai/v1/models` per xai account kind | the raw answer (truncated) next to the extracted windows, and the `x-ratelimit-*` headers xai returned |

A provider with no account in the home is skipped with a message, except for `live_routing_matches_polls`: it fails when no polled account was checked, and the failure names the home it read and the kinds it needs (an anthropic, grok-cli, opencode-go or opencode-zen account that is enabled and answers its quota poll). The checks send their
requests directly rather than through the attempt loop, so a refusal doesn't take the
account out of service. Run them while no server uses the same home: the checks refresh
tokens, and two processes refreshing one rotating token can sign the account out.
