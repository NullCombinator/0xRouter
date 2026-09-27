# Contract: Client-facing HTTP API

**Feature**: [spec.md](../spec.md) | **Data model**: [data-model.md](../data-model.md)

**Listener**: `--listen` / `ZEROROUTER_LISTEN`, default `127.0.0.1:20129`. HTTP/1.1 and
h2c.

**No operator routes**: none exist on this listener (FR-028).

## Endpoints

| Method + path | Client format | Body | Notes |
|---|---|---|---|
| `POST /v1/chat/completions` | `openai` | OpenAI Chat Completions | |
| `POST /v1/messages` | `claude` | Anthropic Messages | |
| `POST /v1/messages/count_tokens` | `claude` | Anthropic count_tokens | FR-001a |
| `POST /v1/embeddings` | `openai` | OpenAI embeddings | US4 |
| `GET /v1/models` | `openai` | — | FR-029 |

Unknown paths return 404 in the OpenAI error shape. A wrong method on a known path
returns 405.

## Authentication (FR-004)

- The access key is read from `Authorization: Bearer <key>` first, else from
  `x-api-key: <key>`, on **every** endpoint including `/v1/models` and `count_tokens`.
- A missing key gives `401 Missing API key`.
- An unknown or inactive key gives `401 Invalid API key`.
- Auth is checked before the body is parsed, and nothing is resolved or sent on failure.

## Selection and rejection order

Rejections are evaluated in this order. The first one that applies is returned, and
nothing is sent upstream.

| # | Check | Status | `error_type` | Message |
|---|---|---|---|---|
| 1 | access key | 401 | `missing_access_key` / `invalid_access_key` | `Missing API key` / `Invalid API key` |
| 2 | body ≤ 32 MiB | 413 | `request_too_large` | `Request body exceeds 32 MiB` |
| 3 | body is a JSON object | 400 | `invalid_json` | `Invalid JSON body` |
| 4 | `model` is a non-empty string | 400 | `missing_model` | `Missing model` |
| 5 | count_tokens, not native pair | 200 | — | local estimate (below) |
| 6 | registry resolve | 404 | `target_not_found` | `Model not found: <target> (<NotFound kind>)` |
| 7 | provider executable ([R2](../research.md#r2-which-providers-are-executable-fr-013)) | 400 | `provider_not_supported` | `Provider not supported yet: <provider> (<reason>)` |
| 8 | embeddings endpoint and target offers embeddings | 400 | `not_embeddings` | `<target> does not offer embeddings` |
| 9 | an active connection exists (FR-011/012) | 404 | `no_usable_connection` | `No active credentials for provider: <provider>`, or `No active credentials for any member of unified model: <name>` |
| 10 | transport and format ([R3](../research.md#r3-transport-and-target-format-choice-fr-014)) | 400 | `format_mismatch` | `<target>: client format <fmt> not served; provider speaks <fmts>` |

Notes on the table:

- For a unified target, rows 7–10 apply to the selected member. The member is the first
  one whose provider has an active connection. Members whose provider is not executable
  count as having no usable connection.
- The row 1, 3, 4, and 9 messages equal 9router's strings
  (`src/sse/handlers/chat.js`).

## Error envelopes

**0router-originated errors (FR-003)** are in the client's wire format. Every one also
carries the response header `x-zerorouter-error: <error_type>`.

- OpenAI clients:
  `{"error":{"message":"…","type":"<9router ERROR_TYPES[status].type>","code":"<error_type>"}}`
- Anthropic clients:
  `{"type":"error","error":{"type":"<anthropic type>","message":"…"}}`

  | Status | Anthropic `type` |
  |---|---|
  | 400, 413 | `invalid_request_error` |
  | 401 | `authentication_error` |
  | 404 | `not_found_error` |
  | 502, 504 | `api_error` |

| Upstream failure | Status | `error_type` | Message |
|---|---|---|---|
| connect timeout | 504 | `upstream_timeout` | `[504]: <provider> did not respond within <n> ms` (R15 D1) |
| unreachable | 502 | `upstream_unreachable` | `[502]: <provider>: <cause kind>` (`dns`, `connect`, `tls`, `io`); never the URL query or key |

**Upstream error responses (FR-020)**

- **Native pair**: the upstream status, headers (FR-020a), and body are passed
  unchanged.
- **Otherwise**:
  - The status is the upstream status.
  - The headers are `Content-Type: application/json` and
    `Access-Control-Allow-Origin: *`.
  - The body is `{"error":{"message":"[<status>]: <msg>","type":…,"code":…}}`, where:
    - `msg` is `error.message`, else `message`, else `error`, else the raw text.
      Non-strings are JSON-stringified. An empty value is replaced by 9router's default
      for that status.
    - `type` and `code` come from 9router's `ERROR_TYPES` by status.
  - The shape is OpenAI's for both client formats, as in 9router (R15 D4).

**Non-SSE page on a streaming request (FR-020b)**

- **Native pair**: passed through unchanged.
- **Otherwise**: the status is the upstream status, and the body is
  `{"error":{"message":"[<status>]: <short>"}}`, with no type and no code.

## Streaming (FR-018, FR-018a, FR-021)

**Response headers**

- Not a native pair: `Content-Type: text/event-stream`, `Cache-Control: no-cache`,
  `Access-Control-Allow-Origin: *`. `Connection: keep-alive` is added on HTTP/1.1 only.
  This is 9router's `SSE_HEADERS_CORS`.
- Native pair: the upstream headers minus `set-cookie`, `content-length`, and hop-by-hop
  headers.

**Body**

- The upstream bytes are relayed one complete SSE event at a time, and unchanged
  ([R9](../research.md#r9-relay-cancellation-and-the-sse-question-fr-018-fr-018a-fr-021)).
- **Break or stall after the first byte**: one closing frame is sent, then the stream
  closes.
  - Anthropic clients: `event: error\ndata: {"type":"error","error":{"message":"…","type":"server_error","code":"gateway_timeout"}}\n\n`
  - OpenAI clients: `data: {"error":{…same…}}\n\ndata: [DONE]\n\n`
  - The frame bytes equal the oracle's `stream-errors.json`.
- **Client disconnect**: the upstream is cancelled, and nothing more is written.

## Non-streaming

- The response is the upstream status and body as received.
- Headers: for a native pair, the upstream headers (FR-020a); otherwise
  `Content-Type: application/json` and `Access-Control-Allow-Origin: *`.
- **Forced streaming with a non-streaming client** (FR-019, OpenAI format only; the
  forced-stream providers at the pin are openai and api-airforce):
  - the outbound body gets `"stream": true`;
  - the SSE is assembled by the `parseSSEToOpenAIResponse` rules, and `reasoning_content`
    is stripped when `content` is non-empty;
  - the client gets 200 JSON.
  - An in-band error chunk gives its `status` if it is in 400–599, else 502, formatted
    as an upstream error.
  - No chunks gives `502 Invalid SSE response for non-streaming request`.
  - If the upstream answered with non-SSE content, the response is relayed as-is
    (9router returns `null`, falling through to the normal path).

## count_tokens (FR-001a)

- **Native pair** (Claude Code → `anthropic`):
  - The request goes to `POST <anthropic messages base_url>/count_tokens`, which is
    `https://api.anthropic.com/v1/messages/count_tokens`.
  - Header rules: FR-015a. Body rules: FR-016 (model replaced). Response rules: FR-020
    and FR-020a.
  - There is no fallback to the estimate.
- **Otherwise**: `200 {"input_tokens": N}`, where N is `estimateAnthropicInputTokens` of
  the body, and the target is not resolved. The headers are
  `Content-Type: application/json` and 9router's CORS headers. The observation outcome
  is `estimated_locally`.

## Embeddings (US4)

- The provider must be in the embeddings-executable set
  ([R2](../research.md#r2-which-providers-are-executable-fr-013)), and the model's `kind`
  must be `embedding`. An uncatalogued model on an allow-uncatalogued provider is
  accepted.
- The URL is the provider's `capabilities.embedding.endpoint.base_url`.
- The headers are `Content-Type: application/json`, then `Authorization: Bearer <key>`,
  then the endpoint's declared headers.
- The body is `{model, input, encoding_format?, dimensions?}`.
- The response is relayed as in the non-streaming case.

## Model listing (FR-029)

`GET /v1/models` returns:

```json
{"object":"list","data":[
  {"id":"sonnet","object":"model","owned_by":"zerorouter","kind":"llm"},
  {"id":"anthropic/claude-sonnet-4-5","object":"model","owned_by":"anthropic","kind":"llm"}
]}
```

- Unified models come first, in declaration order. Only those with at least one member
  whose provider has an active connection are listed.
- Catalogued models of every provider with an active connection follow, in provider-id
  then catalog order.
- `kind` is omitted when it is undeclared.
- No keys or connection names appear.
