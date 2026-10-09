# Contract: Operator CLI, socket and records (slice 004 additions)

This extends [slice 003's operator contract](../../003-request-pipeline/contracts/operator-cli.md).
Mutating commands write their file atomically and then send `reload` to a running server. The
output says either `applied` or `saved; applies at next start` (FR-029: no restart needed).

## Files

| Path | Mode | Contents |
|---|---|---|
| `keys.toml` | 0600 | Each `[[key]]` gains an optional `adapter` field (the binding; slice 010's `harness` is a separate display-only tag) |
| `config.toml` | any | Gains `[adapters]` ([data-model.md](../data-model.md#adaptersconfig-configtoml-adapters)) |
| `adapters/index.toml` | 0600 | Harnesses, versions, states, `[review]` |
| `adapters/<harness>/<version-id>/…` | 0600 files, 0700 dirs | Source, module, build, review, decision |
| `adapters/alerts.toml` | 0600 | Alerts |

`serve` refuses to start if `adapters/` is group-readable or world-readable, just as it does
for `keys.toml`.

## Commands

**Keys** (FR-001):

| Command | Effect |
|---|---|
| `nullrouter keys issue <name> [--adapter H] [--harness TEXT] [--break …]` | As in slice 003, with an optional adapter binding (and slice 010's display tag) |
| `nullrouter keys set-adapter <name\|id> <H>` / `--clear` | Binds or unbinds a harness's adapter |
| `nullrouter keys list` | Gains a `harness` column |

**Adapters** (FR-029):

| Command | Effect |
|---|---|
| `nullrouter adapters list` | For each harness: its versions, states, the active version and a count of unacknowledged alerts. hermes shows as `built-in` |
| `nullrouter adapters show <H> [<version>]` | The manifest summary and selectors, state and reason, origin, `source_fp`, `wasm_hash`, the review report, and the decision |
| `nullrouter adapters install <dir\|archive.tar.gz>` | Gate, then queue. Prints the version id and the next state. A refusal prints the gate lines and exits 3 |
| `nullrouter adapters review <H> <version> [--retry]` | Shows the report. `--retry` requeues a quarantined review |
| `nullrouter adapters build <H> <version> --retry` | Requeues a build (for example once the builder has been installed) |
| `nullrouter adapters approve <H> <version> [--note T]` | Only from `reported`. The version becomes active from the next request |
| `nullrouter adapters reject <H> <version> [--note T]` | From `reported` or `quarantined` |
| `nullrouter adapters clear <H> <version>` | `suspect` → `approved`. It asks for confirmation and shows the guardrail events first |
| `nullrouter adapters remove <H> [<version>] [--force]` | Removes a version, or the whole harness. The active version needs `--force` |
| `nullrouter adapters rebuild [<H>]` | Rebuilds approved versions against the current kit (R9). Run it before upgrading to avoid the plain-client window |
| `nullrouter adapters review-settings --model M --budget N [--reserve-output N]` | Sets `[review]` (FR-022). `M` is a unified model id or `provider/model`, so it names the provider too. `--clear` removes it |

**Updating** (FR-029): there is no separate `update` command. Installing a newer version of an
installed harness, with `adapters install` or `catalogue install`, is the update. It goes through
the gate, build and review like a first install. The active version keeps serving until the
operator approves the new one (FR-023).

**Alerts**:

| Command | Effect |
|---|---|
| `nullrouter alerts list [--all]` | Unacknowledged alerts, newest first: id, kind, harness@version, record, detail, time |
| `nullrouter alerts ack <id\|--all>` | Acknowledges one alert or all of them |

**Catalogue** (FR-031, [catalogue.md](catalogue.md)). These are the only commands that contact
the catalogue:

| Command | Effect |
|---|---|
| `nullrouter catalogue list` | Harness, summary, newest version, and whether it is installed |
| `nullrouter catalogue show <H>` | All versions, source URLs and fingerprints |
| `nullrouter catalogue install <H> [<semver>]` | Fetch, verify, gate and queue. The newest version is the default |
| `nullrouter catalogue check` | Lists newer versions of installed harnesses. Installs nothing |

Exit codes: 0 ok; 1 invalid input or file; 2 usage; 3 refused by the gate or by
verification; 4 no running server (for commands that need one); 5 the catalogue is
unreachable.

## Operator socket additions

| Request | Response |
|---|---|
| `{"op":"adapters.state"}` | The live `AdapterIndex` view: active versions, suspect marks, rebuild flags |
| `{"op":"adapters.review","harness","version"}` | `{"ok":true,"queued":true}`. The review runs in the background |
| `{"op":"alerts.list","all"?}` | `{"ok":true,"alerts":[…]}` |

Approve, reject, clear and remove are file writes followed by `reload`, as with every other
mutating command.

## `records show` additions (text)

```
rq_01JAC7…  2026-09-28 11:04:52  succeeded
agent       hermes-desktop / session 44ab…   harness hermes (built-in)
…
attempts
  1  openrouter/main  deepseek/deepseek-r1  ok
     adapter hermes built-in: ran, 3 changes
       converted  messages[4].images         format_conversion
       converted  messages[4].content        format_conversion
       removed    messages[3].reasoning_content  target_rejects_field
response adapter: none
```

A guardrail block looks like this:

```
     adapter claude-code v0.1.0-1a2b3c4d: blocked by guardrail
       rule tool_call_added  paths messages[7].content[2]
       sent unmodified; adapter marked suspect (alert al_3k…)
```

When an adapter is held back:

```
     adapter claude-code v0.1.0-1a2b3c4d: not run (suspect); served as plain client
```

`--json` gives the `AdapterRun` fields from [data-model.md](../data-model.md#adapterrun-on-attempt-and-response_adapter-on-requestrecord).
Records never hold removed or converted values.
