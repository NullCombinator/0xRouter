# Data Model: Dashboard (slice 007)

The dashboard owns very little data. It reads what the CLI reads (R1). The only new stored
things are the dashboard token's digest and the `[dashboard]` settings.

## Stored

### `dashboard.toml` (new, in the operator home)

| Field | Type | Rule |
|---|---|---|
| `schema` | integer | `1` |
| `token_digest` | hex string, 64 chars | SHA-256 of the full token text (`nrd_…`). Absent until the first `dashboard token`. |
| `issued` | RFC 3339 UTC | When the current token was issued. Shown by `dashboard status` and the sign-in page. |

- Mode 0600, written atomically (temporary file, then rename). `check` warns about any other
  mode, as it does for `keys.toml`.
- One token at a time. Issuing replaces both fields.
- The plaintext token is never stored. A lost token is replaced, not recovered.

### `config.toml`: new `[dashboard]` table

| Key | Type | Default | Rule |
|---|---|---|---|
| `enabled` | bool | `true` | `false`: `serve` opens no dashboard port (FR-003). |
| `listen` | `host:port` | `127.0.0.1:20130` | The host must be a loopback address (`127.0.0.0/8`, `::1`, `localhost`). Anything else is a load error: `config.toml:L:C dashboard.listen: must be a loopback address; network binding is not supported`. |

Read through the existing `OperatorConfig`. A change applies at the next `serve` start: the
listener isn't rebound on reload. `check` says so when the running server's address differs from
the file.

## In memory (server process)

### Dashboard state

| Field | Meaning |
|---|---|
| `bound` | `Ok(addr)` or `Err(reason)`: the result of binding the dashboard listener at startup. Answered by the `dashboard.status` op. |
| `failures` | Wrong-token count since the last success. Drives the delay (1 s × 2ⁿ, cap 30 s). |
| `builds` | A semaphore of 2 page-build permits. |

The current token digest isn't cached. It is read from the engine snapshot, which loads
`dashboard.toml` like the other home files and reloads it when the CLI asks.

### Page snapshot (per page build)

| Field | Meaning |
|---|---|
| `as_of` | The instant the engine snapshot was taken. Shown in the machine's zone. |
| `generation` | The engine snapshot's generation. Every view in the build uses it. |
| `views` | The `views::*` values the page renders (R1), each the exact JSON the CLI prints with `--json`. |

Built, rendered and dropped per request. Never cached: each load shows the state at load time
(FR-017).

## Views (moved from `nullrouter-cli`, shape unchanged)

The JSON shapes are the CLI's current `--json` outputs. Moving them changes no field, and the
CLI's existing tests pin them. Two new views:

### `unified`

```json
[{ "name": "sonnet", "kind": "llm",
   "members": [{"order": 0, "provider": "kiro", "model": "claude-sonnet-4-5", "upstream": "claude-sonnet-4.5"}],
   "limits_notes": [{"unified": "sonnet", "limit": "context_length", "values": [{"provider": "kiro", "value": 200000}]}] }]
```

Plus `dropped`: `[{name, provider}]` from the load report. Members and notes reuse `resolve`'s
member and `note_json` shapes.

### `behaviour`

```json
{ "break_behaviour": {"value": "restart", "default": true} }
```

There is one entry per `[pipeline]` setting the operator can set today. A setting added later
joins this map.

## State transitions

```text
dashboard token:  none ──issue──▶ T1 ──issue──▶ T2 …   (each issue invalidates every cookie holding the previous token)

browser:          signed out ──POST /signin (right token)──▶ signed in
                  signed in  ──token replaced / cookie cleared──▶ signed out
                  signed out ──POST /signin (wrong token)──▶ signed out, failures += 1, delay before answer
```
