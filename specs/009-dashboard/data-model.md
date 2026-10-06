# Data Model: Dashboard (slice 009)

The dashboard owns little data. It renders the read model's views (research R1). New stored
things: the dashboard token's digest, the `[dashboard]` settings, and plugin logos.

## Stored

### `dashboard.toml` (new, operator home)

| Field | Type | Rule |
|---|---|---|
| `schema` | integer | `1` |
| `token_digest` | hex, 64 chars | SHA-256 of the full token text (`nrd_…`). Absent until the first `dashboard token`. |
| `issued` | RFC 3339 UTC | When the current token was issued. |

- Mode 0600, atomic write. `check` reports any other mode (subject `settings`).
- One token at a time; issuing replaces both fields. The token itself is never stored.

### `config.toml`: new `[dashboard]` table

| Key | Type | Default | Rule |
|---|---|---|---|
| `enabled` | bool | `true` | `false`: `serve` opens no dashboard port (FR-003). |
| `listen` | `host:port` | `127.0.0.1:20130` | Host must be loopback (`127.0.0.0/8`, `::1`, `localhost`); else the load error `config.toml:L:C dashboard.listen: must be a loopback address; network binding is not supported`. |

Applies at the next `serve` start; the listener isn't rebound on reload.

### Plugin `logo` field (schema 2, optional, top level)

| Field | Type | Rule |
|---|---|---|
| `logo` | string | A bare file name ending in `.png`: no `/`, `\` or `..`. Otherwise the field fails validation like any other bad field. |

Resolved in the `logos/` directory beside the plugin's set (contracts/plugin-logo.md). Checked at
load: at most 65,536 bytes; PNG signature; `IHDR` first, width and height 1 to 256. A failed check
keeps the plugin and adds a load-report note.

### Logo files

| Where | What |
|---|---|
| `plugins/bundled/logos/<id>.png`, `plugins/community/logos/<id>.png` | Embedded at build. Written by the generator from `ref/9router/public/providers/` or `tools/gen-bundled/seeds/logos/`. |
| `<home>/plugins/logos/<file>` | User plugins' logos; `plugins install` copies a community logo here, `uninstall` removes it. |
| `plugins/LOGOS.md` | Each shipped logo's source path in `ref/9router`, or its override. |

## In memory (server process)

### Registry snapshot: per provider

| Field | Meaning |
|---|---|
| `logo` | `Some(bytes)` when the logo passed the check, else `None` (the page shows the text icon). |

### Segment index (journal): per segment, added

| Field | Meaning |
|---|---|
| `last_arrival_by_agent` | Agent key id → newest `arrived` among the segment's `open` lines. Updated by the same incremental refresh that indexes new lines. |

### Dashboard state

| Field | Meaning |
|---|---|
| `bound` | `Ok(addr)` or `Err(reason)` from binding at startup; answered by `server.status`. |
| `failures` | Wrong tokens since the last success; delay = min(1 s × 2ⁿ, 30 s). |
| `builds` | A semaphore of 2 page-build permits. |

### Page snapshot (per build)

| Field | Meaning |
|---|---|
| `as_of` | When the engine snapshot was taken; shown in the machine's zone. |
| `views` | The `views::*` values the page renders, each exactly the CLI's `--json`. |

Built, rendered and dropped per request; never cached (FR-022).

## View changes (shapes the CLI's `--json` prints)

### `check`: added

```json
{
  "endpoint": "http://127.0.0.1:20129/v1",
  "endpoint_source": "server",
  "notices": [
    {"level": "warning", "subject": "quota",
     "text": "warning: sign-in account anthropic/work has no tokens and can't serve; run `nullrouter accounts login anthropic work`"},
    {"level": "note", "subject": "providers",
     "text": "note: logo ignored: crush: 2700 × 1392 px, over 256 px"}
  ]
}
```

- `endpoint_source`: `"server"` (the running server's address) or `"config"` (no server).
- `level`: `error`, `warning` or `note`. `subject`: `endpoint`, `providers`, `combo`, `usage`,
  `quota` or `settings` (research R5 table). `text`: the exact line `check` prints; a multi-line
  notice (a skipped plugin and its errors) has its lines joined by `\n`.
- Existing fields are unchanged.

### `keys`: added per key

```json
{"id": "k_…", "name": "claude-code", "key": "…9f2a", "created": "…", "revoked": null, "break": null,
 "last_used": "2026-10-05T12:01:58.120Z"}
```

`last_used`: RFC 3339, or `null` for never (text: `never`).

### `dashboard` (new; `dashboard status`)

```json
{"enabled": true, "listen": "127.0.0.1:20130", "server": "running",
 "serving": true, "error": null, "token_issued": "2026-10-05T11:50:00Z"}
```

- `server`: `running` or `none`. With no server, `serving` is `null` and `listen` comes from the
  file.
- `serving: false` comes with `error` (`address in use`, …), or with `enabled: false`.
- `token_issued`: `null` until a token is issued. The token is never in this view.

## Notice

| Field | Meaning |
|---|---|
| `level` | error, warning or note |
| `subject` | the page id it appears on |
| `text` | `check`'s line |

Account notices in the housekeeping panel come from `accounts` (needs signing in again, cooling
down) in the words `accounts list` uses; they have subject `quota`.

## State transitions

```text
dashboard token:  none ──issue──▶ T1 ──issue──▶ T2 …   (each issue signs out every browser holding the previous one)

browser:          signed out ──POST /signin (right token)──▶ signed in
                  signed in  ──token replaced / cookie cleared──▶ signed out
                  signed out ──POST /signin (wrong token)──▶ signed out, failures += 1, delay

dashboard port:   enabled ──serve starts──▶ serving | not serving (error)
                  disabled ──serve starts──▶ no port
                  (a config change applies at the next serve start)

logo:             declared ──load──▶ shown (passed) | ignored + note (failed) ; not declared ──▶ text icon
```
