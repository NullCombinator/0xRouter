# Contract: CLI changes for 014

Every existing mutating command keeps its syntax, output, exit codes and prompts. Its logic moves
into `nullrouter_server::ops`, and the existing CLI tests are the regression gate for that move.

## New commands (research R6)

```text
nullrouter unified add <name> <provider>/<model>...
nullrouter unified set <name> <provider>/<model>...
nullrouter unified remove <name>
nullrouter combos add <name> <member>...
nullrouter combos set <name> <member>...
nullrouter combos remove <name>
nullrouter providers enable|disable|hide|show <id>
nullrouter providers add-compatible <id> --style openai|anthropic --base-url URL --model ID... [--name TEXT]
nullrouter plugins private-endpoints on|off [--yes]
nullrouter dashboard log [--limit N] [--json]
```

Output, one line on success, in the style of the existing commands:

- `unified add`: `u: declared (2 members)`
- `unified remove`, when a combo uses the model: `u: combo fast uses it; remove it from the combo first` (exit 1)
- `combos add`, with an unknown member: `fast: member x is neither a unified model nor a combo` (exit 1)
- `providers hide`: `openrouter: hidden`
- `providers disable`: `openrouter: disabled (routing skips it)`
- `providers add-compatible`, on success: `local-llm: added (3 models)`. On a refused address:
  `local-llm: base_url http://127.0.0.1:11434 is a private address; turn private endpoints on first (nullrouter plugins private-endpoints on)` (exit 3).
- `plugins private-endpoints on`: asks `Plugins may then reach this machine and your local network. Continue? [y/N]` unless `--yes`.
- `dashboard log`: one line per entry, `<local time>  <op>  <subject>  <result>[: text]`, newest first, default limit 50.

Every one of these applies the change to a running server as the others do (`reload`), and prints
`saved; applies at next start` when no server runs.
