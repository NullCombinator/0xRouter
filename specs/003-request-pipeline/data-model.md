# Data Model: Request Pipeline (slice 003)

Phase 1 output. Entities from the spec's Key Entities, with fields, validation and state.
Slice 002 entities (`Registry`, `ProviderEntity`, `UnifiedModel`, `Resolution`) are reused
and extended, not redefined. Research references: [research.md](research.md).

## Load-time entities (`nullrouter-registry`)

### ApiStyle

One client API style file (`styles/bundled/*.toml`). Immutable after load.

| Field | Type | Rule |
|---|---|---|
| `id` | `StyleId` (`openai-chat`, `anthropic-messages`, `openai-responses`, `gemini`) | unique across loaded styles |
| `access_key` | ordered list of `Carrier { header \| query, scheme: raw \| bearer }` | at least one; names join the security floor |
| `session` | ordered list of session carriers (header, body path, or named extractor) | extractor from a closed set |
| `routes` | list of `Route { method, path, op, type, model, stream, discriminator? }` | no (method, path) collision unless discriminators are disjoint with exactly one default |
| `text`, `embeddings`, `image`, `tts`, `stt`, `video` | per-type codec sections | present only for types the style has routes for |
| `errors` | `ErrorShape { body template, type_map, stream_event }` | body template must place `{error.message}` |
| `keepalive` | stream keepalive form | from a closed set |

`op` ∈ generate, count_tokens, list_models, get_model, job_submit, job_get, job_content.
Codec details: [contracts/api-style-schema.md](contracts/api-style-schema.md).

### ProviderEntity (extended, schema 2)

Adds to slice 002's entity:

| Field | Type | Rule |
|---|---|---|
| `endpoints` | map `ModelType → [Endpoint]` | a model's type must have an endpoint |
| `forwarding` | `Forwarding { to_upstream, to_client }` | floor-checked (R18) |
| `session` | optional header + derive primitive | primitive from a closed set |
| `requires` | list of names | inert; used only by the fit check |

**Endpoint**: `url` (placeholders only in the path), `method`, exactly one of `wire:
StyleId` or inline `body` + `response` mapping, static `headers` (no secrets), `timeout_ms`,
`stall_timeout_ms`, `force_stream`, `retry` overrides, `errors` (in-band rules),
`token_count?`, `continuation?`, `models` (optional restriction), `voices`.

**Continuation**: `method` (`assistant_prefill` \| `prefix_flag`),
`trim_trailing_whitespace`, `unless` ⊆ {`thinking_enabled`, `tool_call_in_progress`},
`models` / `except_models`.

**FitVerdict**: `Fits` \| `Unsupported { parts: [UnsupportedPart { span, path, value,
reason }] }`. A plugin with any unsupported part contributes nothing to the snapshot.

### CommunityPlugin

Embedded file from `plugins/community/`: `id`, source text, precomputed `FitVerdict`
(recomputed at install and at every load). Installed = copied into
`$NULLROUTER_HOME/plugins/`.

## Operator state (`nullrouter-engine`, files in `$NULLROUTER_HOME`)

### ProviderAccount (`accounts.toml`)

| Field | Type | Rule |
|---|---|---|
| `provider` | provider id | must be loaded; else the account is reported unused, not an error |
| `name` | string | unique per provider; `[a-z0-9_-]{1,32}` |
| `secret` | `SecretString` from literal or `{ env = "VAR" }` | never serialised back in clear; `Debug` prints `***` |
| `order` | integer | try order within the provider (R7) |
| `disabled` | bool | skipped when true |

The secret is bound to the provider's endpoint hosts; a replacing plugin that changes a
host withholds the secret until the operator re-confirms (slice 002 FR-012a rule).

### AgentKey (`keys.toml`)

| Field | Type | Rule |
|---|---|---|
| `id` | `ak_` + 8 chars | stable, shown in records |
| `name` | string | operator label, unique |
| `digest` | SHA-256 hex of the key | the key itself is never stored |
| `last4` | string | for listings |
| `created` | RFC 3339 | — |
| `revoked` | RFC 3339 or absent | a revoked key is rejected at once after reload |
| `break_behaviour` | `restart` \| `error_event` \| absent | absent = operator default |

### PipelineConfig (`config.toml` additions)

`[server] listen` (default `127.0.0.1:20129`), `[pipeline] break_behaviour` (default
`restart`), `allow_private_endpoints` (default `false`).

## Runtime entities (`nullrouter-engine`)

### Agent

`AgentId = (agent key id, Option<session id ≤ 256 chars>)`. Derived per request; not
stored except inside records and the warm map.

### WarmMap

`(AgentId, Target) → (provider id, account name)`, where `Target` = unified model name or
direct `provider/model`. Updated on successful completion only. In memory.

### Cooldown

`(provider id, account name, model id) → { until: Instant, backoff_level: u8 (≤ 15) }`.
Set by classification (R6); a success clears the model's cooldown and, when no other
cooldown is active, resets the level (9router `auth.js:326-333`).

### RequestPlan

Built per request from the `Resolution`: an ordered list of `Candidate { provider,
endpoint, account, upstream model id }`. Order: warm account first (if not cooling), then
the rest of that provider's accounts, then other members (unified targets only).

### Attempt

| Field | Type |
|---|---|
| `n` | ordinal within the request |
| `provider`, `account`, `model` | ids and names (never secrets) |
| `kind` | `initial` \| `same_account_retry` \| `next_account` \| `next_member` \| `continuation` \| `restart` \| `skipped` |
| `started`, `ended` | monotonic offsets from request arrival |
| `outcome` | `ok` \| `failed { status?, class, reason }` \| `skipped { reason }` \| `cancelled` |
| `usage` | `Usage?` for segments that produced output |
| `dropped` | `[{ path, reason }]`: fields a cross-style attempt couldn't place (R27). Paths only, never values. Empty on same-style attempts |

`class` ∈ `transient`, `rate_limited`, `auth`, `not_found`, `request_error`, `network`,
`timeout`, `stall`, `in_band`, `cannot_carry`, `no_account`.

### Usage

`input`, `output`, `cache_read`, `cache_write`, `reasoning`: each `Option<u64>` (`None` =
not reported). Plus `input_semantics` (`includes_cache` \| `excludes_cache`) and
`estimated: bool` (count requests only).

### RequestRecord

| Field | Type | Rule |
|---|---|---|
| `id` | `rq_` + 26-char time-sortable id | also in `x-0router-request-id` |
| `arrived` | wall-clock timestamp | — |
| `agent` | `AgentId` | — |
| `style` | `StyleId` | client door used |
| `op`, `model_type` | from the route | — |
| `target` | requested name | — |
| `unified_model` | `Option<name>` | — |
| `served_by` | `Option<(provider, account, model)>` | the attempt that finished |
| `attempts` | `[Attempt]` | in order |
| `outcome` | state below | — |
| `break_handling` | `none` \| `continued` \| `restarted` \| `error_event { reason }` | — |
| `ttft_ms` | `Option<f64>` | streamed or chunked responses |
| `total_ms` | `Option<f64>` | set at completion |
| `usage` | `Usage` | summed over segments |
| `job` | `Option<{ nullrouter_job_id, upstream id }>` | video |

**Record state machine**:

```
in_progress ──► succeeded
     │     └──► failed          (all attempts failed, or a non-fallback error)
     │     └──► refused         (bad key, type mismatch, validation; nothing sent upstream)
     └────────► cancelled       (client disconnect)
```

A video record stays `in_progress` from submission until the client fetches the final
result or the job fails; then `succeeded` or `failed`.

### RecordStore

Ring of 10 000 records, oldest evicted; indexes by id, provider and unified model. Queries:
by id, by provider, by unified model, newest first, with a limit.

### JobMap

`nullrouter job id → (provider, account, upstream job id, record id)`. In memory.

## Stream state (`nullrouter-wire`, per client response)

### ClientStreamState

| Field | Use |
|---|---|
| `preamble_sent` | header events sent to the client yet |
| `output_seen` | at least one content event sent (R9 threshold) |
| `open_block` | `None` \| `Text { index }` \| `Thinking { index, signed }` \| `ToolCall { index, args_started }` |
| `next_block_index`, `next_output_index`, `sequence_number` | client-visible counters, carried across segments |
| `partial_text` | text of the current answer, kept for continuation (bounded by the answer size; this is a copy, the stream is not held) |
| `usage` | accumulated |

**Break transitions** (after `output_seen`):

```
open_block = ToolCall{args_started}  ──► error_event            (R9 exception)
continuation target available        ──► continue (merge into open text block)
else behaviour = restart             ──► close block, note block, re-send, shift indexes
else behaviour = error_event         ──► close block, style error event, end
```

## Relationships

```
ApiStyle 1 ── * Route
ProviderEntity 1 ── * Endpoint ── 0..1 wire → ApiStyle
UnifiedModel * ── * ProviderEntity            (slice 002)
ProviderEntity 1 ── * ProviderAccount
AgentKey 1 ── * Agent (by session) 1 ── * RequestRecord 1 ── * Attempt
WarmMap: Agent × Target → ProviderAccount
```
