# Data Model: Client Side, Harness Adapters (slice 004)

Phase 1 output. It covers the entities in the spec's Key Entities, with their fields,
validation and states. Slice 003 entities (`AgentKey`, `Attempt`, `RequestRecord`,
`EngineState`) are extended here, not redefined. Research references:
[research.md](research.md).

## Names and identifiers

| Type | Form | Rule |
|---|---|---|
| `HarnessName` | `^[a-z][a-z0-9-]{1,31}$` | Built-in names: `hermes`. Reserved names, not installable: `opencode`, `grok-build`, `zcode` |
| `VersionId` | `v` + the package `version` (semver) + `-` + first 8 hex of `source_fp` | Unique per harness. Two submissions with the same semver and different source get different ids |
| `SourceFp` | `sha256:` + hex of the canonical source tree | Canonical form: sorted relative paths, each `path\0len\0bytes` |
| `WasmHash` | `sha256:` + hex of `module.wasm` | — |
| `AlertId` | `al_` + 10 chars | — |

## Operator state (files in `$ZEROROUTER_HOME`)

### AgentKey (`keys.toml`, extended)

| Field | Type | Rule |
|---|---|---|
| `harness` | `HarnessName` or absent | Absent means a plain client. A built-in name is always valid. Any other name is accepted if it is well-formed and not reserved, even before any version exists (spec Edge Cases). The name is never inferred from requests (FR-002) |

`deny_unknown_fields` stays, and older files load unchanged.

### AdaptersConfig (`config.toml` `[adapters]`)

| Field | Default | Rule |
|---|---|---|
| `catalogue_url` | this repository's raw `catalogue/index.toml` URL | Must be HTTPS and pass the SSRF rules (003 R17) |
| `builder` | `zerorouter-builder` found on `PATH`, else absent | Path to the builder binary. Absent means installs stop at `queued` |
| `request_deadline_ms` | 20 | 1–1000 |
| `event_deadline_ms` | 2 | 1–100 |
| `memory_mib` | 64 | 1–512 |
| `max_instances` | 64 | Pooling allocator size |

### ReviewSettings (`adapters/index.toml` `[review]`)

| Field | Type | Rule |
|---|---|---|
| `model` | unified model id, or `provider/model` | Must resolve in the registry. The operator must hold an account for it |
| `budget_tokens` | integer > 0 | Covers the estimated input plus the reserved output, per review |
| `reserve_output` | integer, default 4096 | Sent as `max_tokens` |

When `[review]` is absent, no model is set, so every review ends in `quarantined` with the reason
`no_review_model` (FR-022).

### AdapterIndex (`adapters/index.toml`)

```toml
schema = 1
[review]
model = "claude-sonnet"
budget_tokens = 60000

[[harness]]
name = "claude-code"
active = "v0.3.0-1a2b3c4d"         # the approved version that serves; absent = none
source = "catalogue"               # catalogue | local; where versions came from last

[[harness.version]]
id = "v0.3.0-1a2b3c4d"
semver = "0.3.0"
state = "approved"
source_fp = "sha256:…"
wasm_hash = "sha256:…"              # present from `built` onwards
kit_abi = 1
origin = { catalogue = "https://…/claude-code-0.3.0.tar.gz" }   # or { local = "/path" }
submitted = "2026-09-28T10:00:00Z"
state_reason = ""                   # e.g. "no_review_model", "budget: need 71 204, have 60 000"
```

The index is loaded into `EngineState` on every reload (hot apply, R11). The files of each
version live under `adapters/<harness>/<version-id>/`:

| File | Written at | Contents |
|---|---|---|
| `source/…` | gate pass | The unpacked source tree, exactly as gated |
| `module.wasm` | build | The builder's output |
| `build.json` | build | `{source_fp, wasm_hash, kit_abi, toolchain, built}` |
| `review.json` | review | A `ReviewReport` |
| `decision.json` | operator decision | `{decision: approve\|reject, at, note?}` |

### AdapterVersion state machine

```text
                 gate refuses
  submit ──► queued ────────────► refused (terminal; reasons listed)
               │ builder present
               ▼
            building ──── build fails ──► refused
               │ built, hashes stored
               ▼
            in_review ──── no model / budget too small / review failed ──► quarantined
               │                                                            │
               │ report parsed                   `adapters review --retry` ◄┘
               ▼
            reported ── operator reject ──► rejected (terminal)
               │ operator approve
               ▼
            approved ◄──── operator clear ──── suspect
               │  ─────── guardrail violation ──►
               │ a newer version is approved
               ▼
            superseded (terminal)
```

Rules:
- **Serving.** Only the version named by `active` serves, and it must be `approved`.
  - `suspect` doesn't serve and leaves `active` in place; keys work as plain clients
    (FR-017).
  - Clearing it returns the version to `approved`, and it serves again.
- **Approval.** Approving a `reported` version sets `active` to it. The previously active
  version becomes `superseded`, and requests already in flight finish on it (FR-023, US4-3).
- **Removal.** `remove <harness>` deletes the harness's files and index entry. Bound keys
  become plain clients, recorded with the reason `removed`.
  `remove <harness> <version>` refuses the active version unless given `--force`.
- **Waiting without the builder.** `queued` persists across restarts. A missing builder
  leaves the reason `builder_not_installed`, and `adapters build --retry` resumes.
- **Kit upgrades (FR-032).** An `approved` version whose `kit_abi` the core no longer
  supports gets `rebuilding = true` at startup; its state is unchanged.
  - Success: a new `module.wasm` and `build.json` are stored with the same `source_fp`, and
    the flag clears.
  - Failure: the flag becomes `rebuild_failed`, an alert is raised, and keys work as plain
    clients.
  - No state change and no new review, because the source is unchanged.

## Package and catalogue entities

The package and its manifest are in [contracts/adapter-package.md](contracts/adapter-package.md).

### AdapterManifest (`adapter.toml`)

| Field | Type | Rule |
|---|---|---|
| `harness` | `HarnessName` | Not built in and not reserved |
| `style` | `StyleId` | The client style the harness speaks. The guardrail decodes with it |
| `kit` | semver requirement | Must match the kit version the builder has vendored |
| `request.selectors` | list of `Selector` | 1–32 |
| `response.selectors` | list of `Selector` | 0–32. If empty, the adapter never runs on responses |
| `response.events` | bool, default false | Whether the adapter runs per stream event |
| `summary` | string ≤ 200 chars | Shown to the operator and the reviewer |

### Selector

A path pattern such as `messages[*].content[*]`, `tools` or `$`.
- Segments are object keys (`[a-z0-9_]+`, or quoted), `[N]` or `[*]`.
- At most 8 segments.
- `$` means the whole body. It is allowed, but the reviewer sees it.

### CatalogueIndex, CatalogueEntry

The format is in [contracts/catalogue.md](contracts/catalogue.md).

| Field | Rule |
|---|---|
| `entry.harness` | `HarnessName`, unique in the index |
| `entry.summary`, `entry.homepage` | Informational |
| `entry.version[].semver` | Unique per entry |
| `entry.version[].source` | An HTTPS URL to a `.tar.gz` |
| `entry.version[].sha256` | Hex SHA-256 of the archive. A mismatch is refused before unpacking |
| `entry.version[].source_fp` | Must equal the `SourceFp` computed after unpacking. A mismatch is refused before the gate |

## Runtime entities

### AttemptContext (sent to an adapter)

| Field | Type |
|---|---|
| `direction` | `request` \| `response` \| `event` |
| `provider` | provider id |
| `target_style` | wire style of the endpoint (`StyleId`, or `custom` for inline mappings) |
| `same_style` | bool |
| `model` | upstream model id |
| `model_type` | text \| embeddings \| image \| tts \| stt \| video |
| `capabilities` | `{vision, file_input, reasoning}` from the model entry, each unknown/true/false |
| `stream` | bool |
| `attempt` | attempt number, from 1 |

The context carries no secret, key, header, agent id, session id or record id (FR-004).

### Edit (returned by an adapter)

| Field | Type | Rule |
|---|---|---|
| `op` | `remove` \| `replace` | There is no insert. `replace` takes `value` (JSON) |
| `path` | concrete path, such as `messages[3].content[1]` | Must exist, and must sit under a declared selector |
| `kind` | `removed` \| `converted` | `removed` requires `remove`; `converted` requires `replace` |
| `reason` | `ReasonCode` | From the kit's closed set |

The checks run before the guardrail (R5):
- ≤ 1,024 edits;
- no two paths where one is a prefix of the other;
- each `replace` value ≤ 4 MiB.

A failure means `invalid_output`, and the adapter is not marked suspect.

### ReasonCode (closed; defined by the kit's ABI version)

`target_rejects_field`, `target_cannot_carry_block`, `foreign_block`, `format_conversion`,
`param_unsupported_by_model`, `empty_after_removal`, `duplicate_tool`, `role_not_accepted`.
Adding a code is a minor kit version. Removing one is a major ABI version.

### AdapterRun (on `Attempt`, and `response_adapter` on `RequestRecord`)

| Field | Type |
|---|---|
| `harness` | `HarnessName` |
| `version` | `VersionId`, or `builtin` |
| `outcome` | `ran` \| `not_run{reason}` \| `failed{reason}` \| `blocked` |
| `changes` | list of `ContentChange` |
| `guardrail` | `GuardrailEvent` or absent |
| `duration_us` | integer |

Values of `not_run.reason`: `no_approved_version`, `suspect`, `source_mismatch`, `rebuilding`,
`rebuild_failed`, `removed`, `no_selector_match`.

Values of `failed.reason`: `trap`, `deadline`, `memory`, or `invalid_output{rule}`.

Values of `invalid_output.rule`: `not_json`, `outside_selector`, `path_missing`, `overlap`,
`too_many_edits`, `value_too_large`, `kind_mismatch`, `unknown_reason` (R5), and `undecodable`
(R6: the edited body or event no longer decodes in the client style).

`blocked` means the guardrail discarded the edits.

### ContentChange

| Field | Type | Rule |
|---|---|---|
| `path` | string | For events, prefixed `event[N].` |
| `kind` | `removed` \| `converted` | — |
| `reason` | `ReasonCode` | — |

It holds no value, no preview and no length (FR-025). The redactor runs over `path` too.

### GuardrailEvent

| Field | Type |
|---|---|
| `direction` | `request` \| `response` \| `event` |
| `rule` | `tool_call_added` \| `tool_call_changed` \| `tool_def_added` \| `tool_def_changed` \| `tool_result_added` \| `tool_result_changed` \| `opaque_added` \| `unplaced_added` |
| `paths` | the edit paths involved (at most 16) |
| `adapter` | harness and version |
| `at` | RFC 3339 |

### Alert (`adapters/alerts.toml`)

| Field | Type |
|---|---|
| `id` | `AlertId` |
| `kind` | `guardrail` \| `adapter_failed` \| `source_mismatch` \| `module_refused` \| `rebuild_failed` \| `quarantined` \| `refused` |
| `harness`, `version` | — |
| `record` | request id, or absent |
| `detail` | a fixed message plus codes, never content |
| `at`, `acked` | RFC 3339 |

Repeated `adapter_failed` alerts for the same version and reason within 60 s fold into one
alert with a count.

### ReviewReport (`review.json`)

| Field | Type |
|---|---|
| `risk` | `low` \| `medium` \| `high` |
| `summary` | string ≤ 2,000 chars |
| `findings` | list of `{location: "file:line", concern}`, at most 50 |
| `model`, `provider`, `tokens_in`, `tokens_out`, `record` | how the review ran |

`location` refers to the scrambled copy. The CLI shows it alongside the original file, since
the scrambler keeps a line map on the operator's side only.

## Relationships

```text
AgentKey ──harness──► HarnessName ──► builtin (hermes)
                                  └─► AdapterIndex.harness ──active──► AdapterVersion
AdapterVersion ──► source/, module.wasm, build.json, review.json, decision.json
Attempt ──► AdapterRun ──► ContentChange*, GuardrailEvent?
GuardrailEvent / failure / mismatch ──► Alert
CatalogueEntry.version ──install──► AdapterVersion (origin = catalogue)
```
