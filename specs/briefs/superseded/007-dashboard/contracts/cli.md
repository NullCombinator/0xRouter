# Contract: CLI additions and changes

All new commands take the global `--home` and `--json`. Exit codes follow the existing CLI: 0 ok,
1 error, 2 not found.

## `nullrouter unified [NAME]` (new, FR-016a)

Lists every loaded unified model, or one.

```text
$ nullrouter unified
sonnet  llm
  0. kiro claude-sonnet-4-5 → upstream claude-sonnet-4.5
  1. openrouter anthropic/claude-sonnet-4.5 → upstream anthropic/claude-sonnet-4.5
  note: unified model sonnet: members differ in context_length: kiro 200000, openrouter 128000
dropped unified model opus: member provider xx was skipped
```

- `--json`: the `unified` view (data-model.md), with `dropped`.
- With NAME: only that model. Exit 2 if it isn't loaded, with the same message `resolve` gives.
- With no unified models: `no unified models; declare one with [[unified_model]] in config.toml`.

## `nullrouter behaviour show` (new, FR-016a)

```text
$ nullrouter behaviour show
break_behaviour  restart  (default)
```

- `--json`: the `behaviour` view.
- Reads `config.toml`. Needs no server.

## `nullrouter dashboard token` (new)

Issues a dashboard token, replacing any previous one.

```text
$ nullrouter dashboard token
nrd_3q2J…(43 chars)
This is shown once. Open http://127.0.0.1:20130 and enter it.
Browsers signed in with the previous token must enter this one.
```

- Writes `dashboard.toml` (data-model.md). It needs no server, but tells a running one to reload
  as other mutating commands do (`applied` / `saved; applies at next start`). The new digest is
  in effect from the next page load.
- `--json`: `{"token": "nrd_…", "issued": "…", "url": "http://127.0.0.1:20130", "status": "applied"}`.
  The token appears only in this output.

## `nullrouter dashboard status` (new)

```text
$ nullrouter dashboard status
dashboard: listening on 127.0.0.1:20130
token: issued 2026-10-05T13:50:00Z
```

- Asks the running server (`dashboard.status` op). Without a server: `dashboard: no server
  running`, plus the file's settings.
- When the bind failed: `dashboard: not listening: 127.0.0.1:20130: address in use`. Exit 0: this
  is a report, not an error.
- `enabled = false`: `dashboard: off (config.toml [dashboard] enabled = false)`.

## `nullrouter check` (changed)

New lines:
- `warning: dashboard not listening: <addr>: <reason>` (server running, bind failed).
- `note: dashboard listens on <addr>; config.toml now says <addr2>; applies at next start`.
- File-mode line for `dashboard.toml`, as for `keys.toml`.
- `--json` gains `"dashboard": {"enabled", "listen", "bound", "error"?, "token_issued"?}`.

## `nullrouter records list` (changed)

- `--before <ID>`: only records older than that id (the paging cursor the dashboard uses).
- Output is unchanged. With `--limit`, the read stops early (R9).

## Operator socket (changed)

| Request | Answer |
|---|---|
| `{"op":"dashboard.status"}` | `{"ok":true,"enabled":bool,"listen":"…","bound":bool,"error"?:"…","token_issued"?:"…"}` |
| `{"op":"records.list", …, "before"?: "rq_…"}` | as before, older than `before` only |

## Moved code (no behaviour change)

The `--json` builders of `accounts list`, `quota`, `routing`, `records list`/`show`, `providers`,
`model`, `plugins list`, `keys list` and `check` move to `nullrouter_server::views` (R1). Their
text printers stay in the CLI. Every existing CLI test must pass unchanged. Any diff in their
expected output is a regression.
