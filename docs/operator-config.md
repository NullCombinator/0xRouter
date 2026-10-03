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

*Arrives with slice 005's quota stories (US4, US5); this section is a preview of the
contract.*

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

## Request records

The running server keeps a record of each request: agent, target, the provider and
account that served it, every attempt with its outcome, time to first token and total,
usage, and anything left out when translating across styles.

```bash
nullrouter records list --limit 20            # newest first; --provider, --model, --json
nullrouter records show rq_01JAB3…           # the full record with attempts
```

Records live in the server's memory. `records` needs a running server and exits 4 without
one. Error bodies sent to clients carry the record id, so a failure can be looked up.

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
    `plugins install`/`uninstall`) writes its file atomically and then asks the running
    server to reload over the operator socket. It prints `applied` when the server
    acknowledged, or `saved; applies at next start` when no server is running. A hand
    edit applies at the next start or the next such reload.

`nullrouter check` prints the load report:
- provider counts;
- pending and declined conflicts;
- withheld credentials;
- skipped plugins;
- dropped unified models.

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
```

| Check | Sends | Prints |
|---|---|---|
| `signin_anthropic` (L1) | one Messages request, `max_tokens` 5, per anthropic sign-in account | `SERVED` with the answer and usage, or `REFUSED` with the status and the provider's text for `[[signin.refused]]`. Note whether sign-in showed the code page ("paste the code") or fell back to loopback: the token store doesn't record it. |
| `signin_grok_cli` (L3) | three streamed Responses requests through one grok-cli account: (a) every `[identity]` header, (b) the fixed-value headers only, (c) every header and a body with an `item_reference` and foreign item ids | `PASSED`/`FAILED` per variant, with the identity header names sent and the error text |
| `token_lifetimes` (L4) | one refresh per sign-in account, saved like any refresh | the stored and the fresh `expires_in`, whether the refresh token `ROTATED`, and a hint when the lifetime is under 2 × `refresh_lead` |
| `quota` (L2, L5) | one quota read per account with `[quota]` (the fallback only when the primary yields no window), and `GET api.x.ai/v1/models` per xai account kind | the raw answer (truncated) next to the extracted windows, and the `x-ratelimit-*` headers xai returned |

A provider with no account in the home is skipped with a message. The checks send their
requests directly rather than through the attempt loop, so a refusal doesn't take the
account out of service. Run them while no server uses the same home: the checks refresh
tokens, and two processes refreshing one rotating token can sign the account out.
