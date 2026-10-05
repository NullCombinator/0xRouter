# Operator configuration

The operator's state lives in one directory, `$NULLROUTER_HOME` (default `~/.0router`):

```text
$NULLROUTER_HOME
├── config.toml     # unified models, per-provider settings, plugin decisions, server settings
├── accounts.toml   # provider accounts and their secrets (mode 0600)
├── tokens.toml     # signed-in accounts' tokens and states (mode 0600)
├── tokens.lock     # lock for tokens.toml writers (mode 0600)
├── install-id      # this installation's random id (mode 0600)
├── keys.toml       # agent key digests (mode 0600)
├── plugins/        # user and installed community plugins (*.toml, top level only), see plugins.md
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
nullrouter keys list                          # id, name, …last4, created, revoked
nullrouter keys revoke claude-code-laptop     # by name or id
```

The key's id is the agent's identity: provider session ids are derived from it, and
records name it.

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
```

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
nullrouter records show rq_01JAB3…                     # the decision table and every attempt
nullrouter records prune --before 2026-09-01            # prints how many were removed
nullrouter records forget --account anthropic/max       # or --agent KEY
```

`records list` filters by `--provider`, `--account P/N`, `--agent KEY`, `--model` (the target
the client named, its unified model, or the model that served it), `--reason` (a placement
reason: `warm`, `cold_by_deficit`, `moved_for_capacity`, `left_pay_as_you_go`, `overflow`,
`last_resort`, `retry`, `fallback`) and `--since`. All `records` commands work without a
server. With a server running, a request still in flight shows `in progress`; without one,
a record that never closed shows `interrupted`.

`records forget --agent` also drops that agent's cache fingerprints, so its next request is
cold. `prune` and `forget` take the journal lock and exit 1 if it isn't free within 10 s.
Error bodies sent to clients carry the record id, so a failure can be looked up.

### Durability

The routing state (which account each agent is warm on, and the deficits) is kept in
`routing/` next to the records, so both survive a restart.

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
`records not kept since T (disk full): N requests`.

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

Each member's upstream id is resolved once, at load. Rules:

- every member model must be declared by its provider, unless that provider is
  passthrough;
- a provider appears at most once per unified model;
- typed members must agree on `kind` (untyped members never conflict).

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
    `quota interval`, `routing set`/`unset`/`window`, `plugins install`/`uninstall`) writes its file atomically and then asks the running
    server to reload over the operator socket. It prints `applied` when the server
    acknowledged, or `saved; applies at next start` when no server is running. A hand
    edit applies at the next start or the next such reload. If the reload loads a unified
    model whose members' limits differ, the command prints the note on stderr.

`nullrouter check` prints the load report:
- provider counts;
- pending and declined conflicts;
- withheld credentials;
- skipped plugins;
- dropped unified models;
- notes for unified models whose members differ in `context_length` or `max_output_tokens`
  (`note: unified model sonnet: members differ in context_length: kiro 200000, openrouter 128000`;
  the model still loads, and `resolve` prints the same note);
- quota windows a provider reports that no `[[routing.window]]` meter names (paced in their own
  unit);
- pay-as-you-go accounts with no price, and an `explicit` cache mode on a provider none of whose
  endpoints speaks a style with cache markers;
- file modes of the sign-in, quota, record and routing files;
- with a server running, whether records are being kept (disk full).

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
```

| Check | Sends | Prints |
|---|---|---|
| `signin_anthropic` (L1) | one Messages request, `max_tokens` 5, per anthropic sign-in account | `SERVED` with the answer and usage, or `REFUSED` with the status and the provider's text for `[[signin.refused]]`. Note whether sign-in showed the code page ("paste the code") or fell back to loopback: the token store doesn't record it. |
| `signin_grok_cli` (L3) | three streamed Responses requests through one grok-cli account: (a) every `[identity]` header, (b) the fixed-value headers only, (c) every header and a body with an `item_reference` and foreign item ids | `PASSED`/`FAILED` per variant, with the identity header names sent and the error text |
| `token_lifetimes` (L4) | one refresh per sign-in account, saved like any refresh | the stored and the fresh `expires_in`, whether the refresh token `ROTATED`, and a hint when the lifetime is under 2 × `refresh_lead` |
| `live_routing_matches_polls` (L7) | per polled account: one quota poll, one tiny request, a second poll | for each window, the routing view's `remaining_now` beside the poll's figure, and how far it fell after the request beside the cost its meter charged. Any window where the provider charged more than 1% of capacity differently is listed under `METER CORRECTIONS NEEDED`, for a dated fix to the bundled plugin's `[[routing.window]]`. |
| `quota` (L2, L5) | one quota read per account with `[quota]` (the fallback only when the primary yields no window), and `GET api.x.ai/v1/models` per xai account kind | the raw answer (truncated) next to the extracted windows, and the `x-ratelimit-*` headers xai returned |

A provider with no account in the home is skipped with a message, except for `live_routing_matches_polls`: it fails when no polled account was checked, and the failure names the home it read and the kinds it needs (an anthropic, grok-cli, opencode-go or opencode-zen account that is enabled and answers its quota poll). The checks send their
requests directly rather than through the attempt loop, so a refusal doesn't take the
account out of service. Run them while no server uses the same home: the checks refresh
tokens, and two processes refreshing one rotating token can sign the account out.
