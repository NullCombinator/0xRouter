# Contract: Operator CLI and state files (slice 005 additions)

Extends [slice 003's operator CLI contract](../../003-request-pipeline/contracts/operator-cli.md).

## Files

| Path | Mode | Contents |
|---|---|---|
| `accounts.toml` | 0600 required | schema 2: accounts with `kind = "key" \| "signin"` |
| `tokens.toml` | 0600 required | sign-in tokens and persisted states (data-model) |
| `tokens.lock` | 0600 | advisory lock for `tokens.toml` writers |
| `install-id` | 0600 | random installation id |
| `quota/<provider>/<account>.jsonl` | 0600 | poll history |
| `quota/<provider>/<account>.tally.json` | 0600 | running tally checkpoint |

```toml
# accounts.toml
schema = 2
[[account]]
provider = "grok-cli"
name = "work"
kind = "signin"
order = 0
poll_interval = "15m"          # optional

[[account]]
provider = "opencode-go"
name = "main"
kind = "key"
secret = { env = "OPENCODE_GO_KEY" }
```

## Commands

| Command | Effect |
|---|---|
| `accounts signin <provider> <name> [--paste] [--no-browser] [--accept-terms-risk]` | runs the provider's flow; writes the account and tokens; applies at once |
| `accounts list [provider] [--long]` | adds `kind` and sign-in states; `--long` adds email, tier, token expiry, last refresh |
| `accounts enable <provider> <name>` | also clears `refused` (retries the account) |
| `accounts remove <provider> <name>` | also deletes its tokens; history is kept |
| `quota [provider [name]]` | current windows per account, reset times, last poll, last failure |
| `quota history <provider> <name> [--since DATE] [--limit N] [--json]` | poll entries with tallies |
| `quota poll <provider> <name>` | polls now (server must be running) |
| `quota interval <provider> <name> <duration\|default>` | sets the account's polling interval |
| `quota prune --before DATE [provider [name]]` | deletes older history entries |
| `quota forget <provider> <name>` | deletes an account's history |

Exit codes: 0 ok; 1 usage or validation error; 2 sign-in refused, expired or abandoned; 3 server
not running (only for commands that need it).

## `accounts signin` output

Device code (grok-cli):

```text
Open this page on any device and approve:
  https://accounts.x.ai/device?user_code=WXYZ-1234
Code: WXYZ-1234 (expires in 15 min)
Waiting for approval… done.
grok-cli/work: signed in as a…@example.com (SuperGrok); applied
```

PKCE (xai, anthropic):

```text
Open this link in a browser (on any device):
  https://auth.x.ai/oauth2/authorize?…
If the browser ends on a page that doesn't load, copy its full address and paste it here.
Paste the address or code: _
xai/main: signed in as a…@example.com; applied
```

anthropic prints the terms warning (research R4) before the link and needs `y`.

Abandoned or refused: `xai/main: sign-in ended: access_denied; nothing saved` (exit 2).

## `accounts list` (text)

```text
provider   name   kind    order  secret     state
anthropic  max    signin  0      …h3Kq      active
anthropic  api    key     1      …9fA2      active
xai        main   signin  0      …Zt1c      needs sign-in since 2026-10-03 14:02 (invalid_grant)
grok-cli   work   signin  0      …p0Lm      refreshing (token expired 40 s ago, retrying)
  → run: nullrouter accounts signin xai main
```

## `quota` (text)

```text
anthropic/max            polled 14:20 (3 min ago, every 10 min)
  5-hour                 62% left   resets 16:00
  weekly                 81% left   resets Thu 09:00
grok-cli/work            polled 14:18 (5 min ago); last poll failed 14:08 (timeout)
  monthly included       1,240 / 5,000 credits used   resets Nov 1
opencode-go/main         polled 14:21
  rolling                95% left   resets 18:30
xai/main                 quota not reported
```

Every value line belongs to the poll time shown on its account line (FR-020).

## Operator socket (added ops)

| Op | Request | Answer |
|---|---|---|
| `accounts.state` | (extended) | adds `kind`, `state`, `state_since`, `state_reason`, `expires_at` |
| `quota.list` | `{provider?, name?}` | latest windows, last poll, last failure, interval per account |
| `quota.poll` | `{provider, name}` | runs one poll, returns its entry |
| `quota.checkpoint` | — | flushes tallies (used before `prune`/`forget`) |

Tokens never cross the socket. `accounts signin` writes `tokens.toml` under the lock and sends
`reload`.
