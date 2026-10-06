# Contract: CLI changes (slice 008)

Every new command takes the global `--home` and `--json`. Exit codes follow the existing CLI:
0 ok, 1 error, 2 not found.

## Existing read commands: no change

`accounts list [--long]`, `quota [provider [name]]`, `quota history`, `routing [target]`,
`records list`, `records show`, `providers`, `model`, `plugins list [--community]`, `keys list`,
`check`, `resolve`: stdout, stderr and exit code stay byte for byte the same, with the server
running or stopped. The golden suite (research R4) is the contract.

## `nullrouter unified [NAME]` (new)

```text
$ nullrouter unified
sonnet  llm
  0. kiro claude-sonnet-4-5 → upstream claude-sonnet-4.5
  1. openrouter anthropic/claude-sonnet-4.5 → upstream anthropic/claude-sonnet-4.5
  note: unified model sonnet: members differ in context_length: kiro 200000, openrouter 128000
dropped unified model opus: member provider xx was skipped
```

- Member and note lines have the form `resolve <name>` prints them today.
- `--json`: the `unified` view (data-model.md).
- With NAME: only that model, and no `dropped` lines. A dropped unified model isn't loaded, so its
  name behaves like any unknown name. Exit 2 if it isn't loaded, with the message
  `resolve` gives for that name (stderr, `not found: …`), and for `--json` the same `{"kind":"not_found","error"}`
  object `resolve` prints.
- No unified models: `no unified models; declare one with [[unified_model]] in config.toml`,
  exit 0. `--json`: `{"unified":[],"dropped":[]}`.
- A home whose plugins or config don't load: the registry's startup errors, exit 1, as other
  registry-backed reads do today.

## `nullrouter behaviour show` (new)

```text
$ nullrouter behaviour show
break_behaviour  restart  (default)
```

- After `behaviour set-break error_event`: `break_behaviour  error_event`.
- `--json`: the `behaviour` view (data-model.md).
- Reads `config.toml` only; needs no server.
- A `config.toml` that doesn't load: the load error with its path, as `check` gives it, exit 1.

## `nullrouter records list --before <ID>` (new option)

- Lists only records older than `<ID>`, newest first, after every other filter and before
  `--limit`.
- `<ID>` names no record: `no record <ID>` on stderr, exit 1 (the message `records show` gives).
- Text and `--json` lines have the same form as `records list` today.

## Operator socket

| Request | Change |
|---|---|
| `{"op":"records.list", …, "before"?: "rq_…"}` | New optional `before`, same meaning as the CLI option. Unknown id: `{"ok":false,"error":"no record rq_…"}`. |

No other op changes.
