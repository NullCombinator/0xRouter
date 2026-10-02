# Contract: Operator CLI and state files

Extends [slice 002's operator config](../../002-provider-model-registry/contracts/operator-config.md)
and CLI. `$NULLROUTER_HOME` defaults to `~/.0router`.

## Files

| Path | Mode | Contents |
|---|---|---|
| `config.toml` | any | slice 002 content plus `[server]`, `[pipeline]`, `allow_private_endpoints` |
| `accounts.toml` | 0600 required | provider accounts |
| `keys.toml` | 0600 required | agent key digests |
| `plugins/*.toml` | any | user and installed community plugins |
| `run/operator.sock` | 0600 | operator socket of a running server |

`serve` refuses to start if `accounts.toml` or `keys.toml` is group- or world-readable,
and says which file and how to fix it.

```toml
# config.toml additions
allow_private_endpoints = false

[server]
listen = "127.0.0.1:20129"

[pipeline]
break_behaviour = "restart"        # restart | error_event
```

```toml
# accounts.toml
schema = 1
[[account]]
provider = "anthropic"
name = "main"
secret = "sk-ant-…"                # or secret = { env = "ANTHROPIC_KEY_MAIN" }
order = 0
```

```toml
# keys.toml
schema = 1
[[key]]
id = "ak_7h2k9x1q"
name = "claude-code-laptop"
digest = "sha256:…"
last4 = "Qx3f"
created = "2026-09-27T15:00:00Z"
break_behaviour = "error_event"    # optional override
```

## Commands

Existing: `check`, `validate`, `resolve`, `model`, `providers`.

| Command | Effect |
|---|---|
| `nullrouter serve [--listen ADDR]` | run the server; foreground; logs to stderr (redacted) |
| `nullrouter accounts add <provider> <name> [--env VAR] [--order N]` | secret read from stdin unless `--env`; never from argv |
| `nullrouter accounts list [<provider>]` | provider, name, order, `…last4` or `env:VAR`, state (active, disabled, cooling until …) |
| `nullrouter accounts remove <provider> <name>` | — |
| `nullrouter accounts disable\|enable <provider> <name>` | — |
| `nullrouter keys issue <name> [--break restart\|error_event]` | prints the key once; stores the digest |
| `nullrouter keys list` | id, name, `…last4`, created, revoked, override |
| `nullrouter keys revoke <name\|id>` | — |
| `nullrouter keys set-break <name\|id> restart\|error_event\|default` | per-key override (FR-031) |
| `nullrouter behaviour set-break restart\|error_event` | operator default (FR-031) |
| `nullrouter records list [--provider P] [--model UNIFIED] [--limit N] [--json]` | newest first |
| `nullrouter records show <rq_id> [--json]` | full record with attempts |
| `nullrouter plugins list [--community]` | bundled, installed, community with fit status |
| `nullrouter plugins install <id>` | gate + fit, then copy to `plugins/`; refusal message and exit 3 if unsupported |
| `nullrouter plugins uninstall <id>` | — |

Mutating commands write the file atomically, then send `reload` to the running server. The
output says `applied` (server acknowledged) or `saved; applies at next start` (no server).
`records` needs a running server and says so if there is none.

Exit codes: 0 ok; 1 invalid input or file; 2 usage; 3 plugin not supported by this core;
4 no running server (for `records`).

## `records show` output (text)

```
rq_01JAB3…  2026-09-27 15:02:11  succeeded
agent       claude-code-laptop / session 9f1c…
door        anthropic-messages  generate  text
target      claude-sonnet (unified)
served by   openrouter/main  anthropic/claude-sonnet-4
ttft        412.3 ms     total 5 804.9 ms
usage       input 1 204  output 388  cache-read 18 432  cache-write not reported
break       none
attempts
  1  anthropic/main    claude-sonnet-4-20250514  503 transient: overloaded      (3 retries)
  2  anthropic/backup  claude-sonnet-4-20250514  skipped: cooling down 12 s
  3  openrouter/main   anthropic/claude-sonnet-4 ok
```

No key, token or secret is ever printed; account and key names only.

## Operator socket protocol

NDJSON over `run/operator.sock`, one request per line, one response per line:

| Request | Response |
|---|---|
| `{"op":"reload"}` | `{"ok":true,"generation":N}` or `{"ok":false,"error":…}` |
| `{"op":"records.list","provider"?,"unified_model"?,"limit"?}` | `{"ok":true,"records":[…]}` |
| `{"op":"records.get","id":"rq_…"}` | `{"ok":true,"record":{…}}` or not found |
| `{"op":"accounts.state"}` | cooldown state per account and model |

A reload that fails (for example a malformed `accounts.toml`) leaves the previous snapshot in
place and returns the error.
