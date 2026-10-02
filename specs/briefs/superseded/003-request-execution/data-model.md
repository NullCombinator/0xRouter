# Data Model: Request Execution Walking Skeleton

**Feature**: [spec.md](spec.md) | **Research**: [research.md](research.md)

All types live in `crates/nullrouter-server` unless noted otherwise. Registry types
(`Registry`, `ProviderEntity`, `Transport`, `UnifiedModel`, `Resolution`, `NotFound`,
`ModelInfo`, `WireFormat`) are 002's and are used unchanged.

---

## Secret

A core-only wrapper for key material (access keys and provider API keys).

| Field | Type | Notes |
|---|---|---|
| (inner) | `Box<str>` | never serialized |

**Rules**

- `Debug` prints `Secret(***)`. There is no `Display` and no `Serialize`.
- `expose(&self) -> &str` is `pub(crate)`. It is called only in `outbound`, when the
  auth header is written (FR-009).
- There is no zeroize-on-drop in this slice. Secrets live only in `Keys` snapshots and
  in the outbound auth `HeaderValue`, which is marked `set_sensitive(true)` so that it is
  never printed in `Debug`.

---

## AccessKey

One operator-declared 0router key per agent (FR-004a).

| Field | Type | Validation |
|---|---|---|
| `agent` | `AgentName` | `[A-Za-z0-9._-]{1,64}`, unique across access keys |
| `key` | `Secret` | literal or `{ env = "VAR" }`; the variable must be set and non-empty; length ≥ 16; unique across access keys |
| `active` | `bool` | default `true` |

**Indexing**

`Keys.access_by_digest: HashMap<[u8; 32], usize>` maps SHA-256 digests of all keys to
their position ([R5](research.md#r5-access-keys-and-agent-identity)). An inactive key is
still indexed, so that it gives `Invalid API key` rather than being indistinguishable
from an unknown key. Both cases return the same client message.

---

## Connection

One operator-declared account for one provider (FR-006).

| Field | Type | Validation |
|---|---|---|
| `name` | `String` | non-empty; unique within the provider |
| `provider` | `ProviderId` | the canonical id, resolved from an id or alias at load; the provider must be executable ([R2](research.md#r2-which-providers-are-executable-fr-013)) |
| `api_key` | `Secret` | literal or `{ env = "VAR" }`; the variable must be set and non-empty |
| `active` | `bool` | default `true` |
| `account_id` | `Option<String>` | required when any of the provider's transports' URLs contain `{accountId}`; must be non-empty and contain no `/`, `?`, `#`, or whitespace |
| `decl_index` | `usize` | position in the file; decides "first active" (FR-011) |

**Relationships**: many connections per provider. `Keys.by_provider: HashMap<ProviderId,
Vec<ConnIdx>>` lists them in declaration order.

---

## Keys (snapshot)

The validated content of `keys.toml`. It is immutable once built.

| Field | Type |
|---|---|
| `access_keys` | `Vec<AccessKey>` |
| `access_by_digest` | `HashMap<[u8; 32], usize>` |
| `connections` | `Vec<Arc<Connection>>` |
| `by_provider` | `HashMap<ProviderId, Vec<usize>>` |
| `source` | `Option<PathBuf>` (`None` when the file is absent) |

A missing `keys.toml` is valid, but gives zero access keys, so every client request is
rejected. `serve` warns about this at start.

---

## State

The unit of atomic swap ([R10](research.md#r10-reload-and-snapshots)).

| Field | Type |
|---|---|
| `registry` | `Arc<Registry>` |
| `keys` | `Arc<Keys>` |
| `generation` | `u64` (incremented on every successful reload; recorded in observations) |

`Server.state: ArcSwap<State>`. Each request calls `load_full()` exactly once.

---

## AgentIdentity

| Field | Type | Notes |
|---|---|---|
| `agent` | `Option<AgentName>` | `None` only for requests rejected by auth (FR-022) |
| `session` | `Option<String>` | the normalized client session ([R5](research.md#r5-access-keys-and-agent-identity)); `None` means "none" |

Identity equality is the pair. The same session under different agents is a different
identity (FR-005b).

---

## ClientRequest

What arrived, after auth and body parse.

| Field | Type | Notes |
|---|---|---|
| `endpoint` | `Endpoint` | `ChatCompletions` \| `Messages` \| `CountTokens` \| `Embeddings` \| `Models` |
| `client_format` | `WireFormat` | `openai` for ChatCompletions and Embeddings; `claude` for Messages and CountTokens |
| `target` | `String` | the raw `model` field |
| `stream_requested` | `bool` | `body.stream == true` (9router's `clientRequestedStreaming`) |
| `stream_mode` | `bool` | `force_stream \|\| body.stream != false`, adjusted by 9router's Accept rule: `Accept` present, without `text/event-stream`, and `body.stream != true` gives `false` |
| `client_tool` | `Option<ClientTool>` | from `detectClientTool` ([R6](research.md#r6-outbound-url-and-headers-fr-015-fr-015a)) |
| `headers` | `http::HeaderMap` | kept only for the native-pair overlay; never logged |
| `body` | `IndexMap<String, Box<RawValue>>` | top-level map ([R8](research.md#r8-outbound-body)) |
| `identity` | `AgentIdentity` | |
| `received_at` | `Instant` + `SystemTime` | |

`stream_mode` is decided after selection, because `force_stream` depends on the chosen
transport.

---

## ExecutionTarget

The output of placeholder selection (FR-011). This is the only type the routing slice
replaces the producer of.

| Field | Type | Notes |
|---|---|---|
| `provider` | `&ProviderEntity` (borrowed from `State.registry`) | |
| `unified` | `Option<(name, member_index)>` | set for unified targets |
| `upstream_model` | `String` | from `Resolution::Direct.upstream_id` or the member's resolved id |
| `catalogued` | `bool` | |
| `model_info` | `Option<ModelInfo>` | gives `target_format`, `supported_formats`, and `kind` |
| `transport` | `EffectiveTransport` | [R3](research.md#r3-transport-and-target-format-choice-fr-014) |
| `connection` | `Arc<Connection>` | |
| `native_pair` | `bool` | `isNativePassthrough(client_tool, provider.id)` |

**Selection function**

```text
select(state, client_request) -> Result<ExecutionTarget, Rejection>
```

It is pure: no I/O and no clock. It lives in `select.rs`
([contracts/http-api.md §Selection](contracts/http-api.md#selection-and-rejection-order)).

---

## EffectiveTransport

The fields of the chosen transport, with the primary transport as fallback per field
(9router's `rt ? rt.x : config.x`).

| Field | Source |
|---|---|
| `format` | the chosen transport's `format`, else the provider default, else `openai` |
| `base_url`, `url_suffix` | the chosen transport, else the primary |
| `headers` | the chosen transport's `headers`, else the primary's (replaced whole, never merged) |
| `auth` | the chosen transport's `auth`, else the primary's, else the format fallback |
| `force_stream` | the primary transport (a provider-level flag in 9router) |
| `timeout_ms`, `stall_timeout_ms` | the primary transport (provider-level in 9router) |

---

## OutboundRequest

Built by `outbound::build(&ExecutionTarget, &ClientRequest)`.

| Field | Type |
|---|---|
| `url` | `Url` |
| `headers` | `http::HeaderMap` (auth header marked sensitive) |
| `body` | `Bytes` |
| `streaming` | `bool` (true when `stream_mode` is on or the provider forces streaming) |
| `assemble` | `bool` (true for forced streaming with a non-streaming client: FR-019) |
| `connect_timeout` | `Duration` |
| `stall_timeout` | `Duration` |

---

## Observation

One record per request (FR-022). It is `Serialize` for the operator channel and has no
secret fields by construction.

| Field | Type | Notes |
|---|---|---|
| `id` | `u64` | monotonic |
| `at` | RFC 3339 UTC | request received |
| `generation` | `u64` | the state generation used |
| `agent` | `Option<String>` | |
| `session` | `Option<String>` | |
| `endpoint` | `Endpoint` | |
| `target` | `Option<String>` | as requested; `None` if the body had none |
| `unified_model` | `Option<String>` | |
| `provider` | `Option<String>` | the canonical id |
| `connection` | `Option<String>` | the connection **name** |
| `upstream_model` | `Option<String>` | |
| `stream` | `bool` | the outbound streaming mode |
| `native_pair` | `bool` | |
| `outcome` | `Outcome` | see below |
| `status` | `Option<u16>` | the status sent to the client |
| `upstream_status` | `Option<u16>` | |
| `ttft_ms` | `Option<f64>` | streamed: first complete upstream event; non-streamed: full body received |
| `duration_ms` | `f64` | received until the last byte is handed off or a terminal event occurs |
| `usage` | `Usage` | |
| `upstream_headers` | `Vec<(String, String)>` | excludes `set-cookie` and hop-by-hop headers |
| `upstream_error_body` | `Option<String>` | the first 8 KiB, lossy UTF-8 (FR-020c) |

### Outcome

| Variant | When |
|---|---|
| `success` | 2xx upstream, fully relayed |
| `upstream_error { status }` | non-2xx upstream, or the FR-020b non-SSE page |
| `rejected { error_type }` | before anything was sent: `missing_access_key`, `invalid_access_key`, `invalid_json`, `missing_model`, `target_not_found`, `format_mismatch`, `provider_not_supported`, `no_usable_connection`, `not_embeddings`, `request_too_large` |
| `connect_timeout` | no response headers within the connect timeout |
| `unreachable` | DNS, connection refused, or TLS failure |
| `cancelled_by_client` | client dropped before completion |
| `stream_stalled` | stall timeout fired |
| `incomplete_stream` | upstream read error or break mid-stream |
| `estimated_locally` | count_tokens answered by the estimate |

### Usage

| Field | Type |
|---|---|
| `input` | `Option<u64>` |
| `output` | `Option<u64>` |
| `cache_read` | `Option<u64>` |
| `cache_write` | `Option<u64>` |

`None` means "not reported". It serializes as JSON `null` and is shown as `—` in the CLI.

---

## ObservationFilter and ObservationSummary

**Filter**: `provider?`, `unified?`, `agent?`, `session?`, `endpoint?`, `since?`,
`until?`, `include_count_tokens` (default `false`), and `limit` (default 50, newest
first).

`provider` accepts an id or alias and is resolved against the current registry. An
unknown value is an error. Records whose provider was since removed are still listed.

**Summary** (over the filtered set, before `limit`):

| Field | Type |
|---|---|
| `count` | `u64` |
| `success` | `u64` |
| `ttft_ms` | `{ p50, p95 }`, `Option<f64>` each; nearest-rank over records with a TTFT |
| `duration_ms` | `{ p50, p95 }`, `Option<f64>` each |
| `tokens` | sums of `input`, `output`, `cache_read`, and `cache_write` over records that report them, plus `reported` counts |

---

## ObservationStore

A `Mutex<VecDeque<Observation>>` with `cap` (default 10 000, at least 1). `push` pops
the front when full. `query(filter)` clones under the lock, then filters outside it
([R13](research.md#r13-observations-store-and-summaries-fr-022-fr-024fr-026)).

---

## State transitions

### Request lifecycle

```text
Received
  ├─ auth fails ─────────────────────────────────▶ Rejected (401)            ─▶ Recorded
  ├─ body invalid / no model ────────────────────▶ Rejected (400/413)        ─▶ Recorded
  ├─ [count_tokens, not native] ─────────────────▶ EstimatedLocally (200)    ─▶ Recorded
  └─ select
       ├─ not found / mismatch / unsupported /
       │  no connection / not embeddings ─────────▶ Rejected (4xx)           ─▶ Recorded
       └─ Sending (connect timer)
            ├─ timeout ──────────────────────────▶ ConnectTimeout (504)      ─▶ Recorded
            ├─ network error ────────────────────▶ Unreachable (502)         ─▶ Recorded
            ├─ client dropped ───────────────────▶ CancelledByClient         ─▶ Recorded
            └─ Headers received
                 ├─ non-2xx ─────────────────────▶ UpstreamError (status)    ─▶ Recorded
                 ├─ 2xx non-SSE page on stream ──▶ UpstreamError (FR-020b)   ─▶ Recorded
                 └─ Relaying (stall timer per read)
                      ├─ end of stream ──────────▶ Success                   ─▶ Recorded
                      ├─ stall ──────────────────▶ StreamStalled + error frame ─▶ Recorded
                      ├─ read error ─────────────▶ IncompleteStream + error frame ─▶ Recorded
                      └─ client dropped ─────────▶ CancelledByClient (upstream closed) ─▶ Recorded
```

For assembly (FR-019), the same states apply. The "error frame" becomes a 502 JSON error
(9router: `Invalid SSE response…` or the in-band error's status). A stall or read error
during assembly records `stream_stalled` or `incomplete_stream`, and the client receives
504 JSON.

`Recorded` happens exactly once per request. The builder's `finish()` takes `self` by
value, and the drop guard records only if `finish()` never ran.

### Reload

```text
Serving(gen N) ──reload──▶ Building (spawn_blocking; registry candidate → keys candidate)
   ▲                           ├─ any error ──▶ Serving(gen N), errors returned
   └───────────────────────────┴─ ok ─────────▶ Serving(gen N+1), LoadReport returned
```

In-flight requests hold `Arc<State>` for gen N until they finish.
