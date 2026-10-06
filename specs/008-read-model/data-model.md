# Data Model: Read Model (slice 008)

Nothing new is stored. The slice adds a way to read what is already on disk and in the running
server, and one filter field.

## View

One per read (research R1). Built by a sync function from two inputs and nothing else (R2).

| Part | Meaning |
|---|---|
| `json` | Exactly the value the CLI prints with `--json` today. Unchanged by this slice (FR-004). |
| `extra` | Facts the CLI's text shows that `json` lacks (R3). Empty for most views. Never printed by `--json`. |

### Inputs

| Input | Source, both routes | Notes |
|---|---|---|
| Home files | read by the view from the operator home | Registry-backed views load the registry from files, as the CLI does today (R2). |
| Live answers | one per operator op the view declares | CLI: `operator::call` over the socket. In-server: `operator::handle(engine, req).await`. |
| `running` | whether the socket accepts a connection, checked even when the view needs no op | In-server: always true. Names unfinished records "in flight" or "cut short". |

### Views and their live ops

| View | CLI command | Live ops | `extra` |
|---|---|---|---|
| `accounts` | `accounts list [--long]` | `accounts.state` | tokens-file warnings |
| `quota` | `quota [provider [name]]` | `quota.list` | offline flag |
| `quota_history` | `quota history` | `quota.checkpoint` (flushes queued poll entries to disk first, as today) | — |
| `routing` | `routing [target]` | `routing.view` | — |
| `records` | `records list` | — | — |
| `record` | `records show` | `records.get` | key id → name (`key_names`) |
| `providers` | `providers [--capability]` | — | — |
| `model` | `model <provider> <model>` | — | — |
| `plugins` | `plugins list [--community]` | — | — |
| `keys` | `keys list` | — | — |
| `check` | `check` | `routing.health` | `withheld`, `notes`, `mode_lines` |
| `resolve` | `resolve <target>` | — | `notes` |
| `unified` (new) | `unified [NAME]` | — | — |
| `behaviour` (new) | `behaviour show` | — | — |

The `extra` column lists the cases known when planning. Each read is audited while it moves; any
case found is added here and to `contracts/read-model.md`.

## Record filter (`records list`, `records.list` op)

Existing fields: `provider`, `account`, `agent`, `model`, `reason`, `since`, `unified_model`,
`limit`. New:

| Field | Type | Rule |
|---|---|---|
| `before` | record id (`rq_` + ULID) | Only records whose id sorts below it. The id must name a record, else `no record <ID>` (exit 1). Combines with every other field. |

## Record page

The result of a read with a filter: records newest first (id descending), at most `limit`, all
older than `before` when it is set. Paging from the newest page by passing each page's last id
as the next `before` yields every matching record exactly once, in the full listing's order
(SC-005).

## `unified` view (new)

Each entry is exactly the object `resolve <name> --json` prints for a unified model today, minus
nothing and plus nothing; members are in order:

```json
{ "unified": [{ "kind": "unified", "name": "sonnet", "model_kind": "llm",
    "members": [{"provider": "kiro", "requested": "claude-sonnet-4-5", "upstream_id": "claude-sonnet-4.5", "catalogued": true}],
    "limits_notes": [{"unified": "sonnet", "limit": "context_length", "values": [{"provider": "kiro", "value": 200000}]}] }],
  "dropped": [{"name": "opus", "reason": "member provider xx was skipped"}] }
```

With NAME, the output is the one entry for that model, so `unified <name> --json` and
`resolve <name> --json` print the same object (FR-009). `dropped` takes its names and reasons from
the load report.

## `behaviour` view (new)

```json
{ "break_behaviour": {"value": "restart", "default": true} }
```

One entry per `[pipeline]` setting.
