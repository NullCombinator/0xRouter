# Contract: the read model (`nullrouter_server::views`)

The interface a front end uses. The CLI is the only caller in this slice; the dashboard slice
adds the second.

## Shape

For each view (data-model.md, "Views and their live ops"):

- `NEEDS: &[&str]`: the operator ops the view needs, possibly none.
- `build(home, args, live) -> Result<View, ViewError>`: sync. Reads the home's files, takes the
  live answers, and returns the view. Changes no state (FR-006).
  - `args`: the command's arguments (filters, target, NAME, flags such as `--long`).
  - `live`: for each op in `NEEDS`, the answer, or "no server"; plus `running`. A refusal (`ok: false`) is kept as the answer, and a view that must fail on it calls `Live::ok`; a socket error is an answer `{"ok":false,"error":"operator socket: …"}`.
- `View { json, extra }` (research R3). `json` is what `--json` prints.
- `ViewError` carries the exact message and exit code the CLI gives today for that failure.

## Routes

| Route | Live answers from | Runs the view |
|---|---|---|
| CLI | `operator::call(home, req)` per op; a refused connection is "no server" | on the calling thread |
| In-server | `operator::handle(engine, req).await` per op; `running` is true | in `spawn_blocking` |

A helper per route runs `NEEDS`, then `build`. The two helpers are the only difference between
the routes; tests compare their results for every view (research R5).

## Rules

- A view reads files and live answers only. It never reads the engine's loaded snapshot, the
  environment beyond the home path, or the clock except through `engine::clock` (so tests pin it).
- Text rendering stays in the CLI and reads `json` and `extra` only.
- A view never puts a secret in `json` or `extra` beyond `…last4` or `env:VAR` (FR-007).

## Text-only facts found (`extra`)

| View | Fact | Why the JSON lacks it |
|---|---|---|
| `record` | agent key name for the record's key id | `--json` has always carried the id only |
| `accounts` | `warnings`: a `tokens.toml` that doesn't load, which the CLI prints on stderr in text and `--json` alike | the warning goes to stderr, not into the `--json` rows |
| `quota` | `offline`: no server answered, so the CLI adds its "no server is running" line on stderr | `--json` prints the same rows whether or not a server answered |

Add a row for each case found while moving the reads.
