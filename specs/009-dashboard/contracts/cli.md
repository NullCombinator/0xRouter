# Contract: CLI additions and changes

All commands take the global `--home` and `--json`. Exit codes follow the existing CLI: 0 ok,
1 error, 2 not found.

## `nullrouter dashboard token` (new)

Issues a dashboard token, replacing any previous one.

```text
$ nullrouter dashboard token
nrd_3q2J…(43 chars)
This is shown once. Open http://127.0.0.1:20130 and enter it.
Browsers signed in with the previous token must enter this one.
applied
```

- Writes `dashboard.toml` (data-model.md). Needs no server; tells a running one to reload, as the
  other mutating commands do (`applied` / `saved; applies at next start`).
- `--json`: `{"token": "nrd_…", "issued": "…", "url": "http://127.0.0.1:20130", "status": "applied"}`.
  The token appears only in this output.
- `[dashboard] enabled = false`: the token is still issued, and the output adds
  `The dashboard is off (config.toml [dashboard] enabled = false).`
- A running server whose dashboard didn't bind: the second line is
  `This is shown once. The dashboard is not listening (<error>); don't open <url>.` The address
  may belong to another program, which would collect the token (security-review.md L2).
- A running server that refuses the reload: the token is still printed (it is saved and works
  from the next good reload on), the status line adds
  `; the previous token still works there until a reload succeeds`, and the exit code is 1
  (security-review.md L1).

## `nullrouter dashboard status` (new)

```text
$ nullrouter dashboard status
dashboard: on, listening on 127.0.0.1:20130
token: issued 2026-10-05T11:50:00Z
```

| State | First line |
|---|---|
| serving | `dashboard: on, listening on <addr>` |
| bind failed | `dashboard: on, not listening: <addr>: <reason>` |
| off | `dashboard: off (config.toml [dashboard] enabled = false)` |
| no server | `dashboard: no server running; config.toml: on, <addr>` (or `off`) |

- Second line: `token: issued <time>` or `token: none; run nullrouter dashboard token`.
- `--json`: the `dashboard` view (data-model.md). Exit 0 in every state: this is a report.
- Asks the running server with `server.status`.

## `nullrouter check` (changed)

- After `home:`, a new line: `endpoint: http://127.0.0.1:20129/v1`, or with no server
  `endpoint: http://127.0.0.1:20129/v1 (configured; no server running)` (research R6).
- New line kinds: `note: logo ignored: <plugin id>: <reason>` (printed right after the `skipped:`
  lines);
  `warning: dashboard not listening: <addr>: <reason>` (server running, bind failed); the file mode
  of `dashboard.toml`.
- Every other line is unchanged, byte for byte, in the same order.
- `--json` gains `endpoint`, `endpoint_source`, `logos_ignored` and `notices` (data-model.md). Existing fields are
  unchanged.

## `nullrouter keys list` (changed)

A new last column, last used: the RFC 3339 UTC time, as the `created` column shows its time, or
`never`. Lines are otherwise unchanged (the column is appended after the break behaviour).
`--json` gains `last_used` per key. With a server running, the value comes from the server's
journal index (`keys.last_used`); without one, from the journal on disk. Both give the arrival
time of the newest record for that key.

## `nullrouter plugins install` / `uninstall` (changed)

`install <id>` copies the community plugin's logo, when it has one, to
`<home>/plugins/logos/<file>` with the TOML. `uninstall <id>` removes it. Output unchanged.

## Operator socket (new ops)

| Request | Answer |
|---|---|
| `{"op":"server.status"}` | `{"ok":true,"client_listen":"127.0.0.1:20129","dashboard":{"enabled":bool,"listen":"…","serving":bool,"error":"…"\|null}}` |
| `{"op":"keys.last_used"}` | `{"ok":true,"last_used":{"<key id>":"<RFC 3339>"\|null}}` |

## Unchanged

Every other command's text and `--json` output. Their existing tests must pass unchanged.
