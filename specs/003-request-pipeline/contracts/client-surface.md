# Contract: Client HTTP surface

What a client (harness, SDK or optimizer hop) sees. Default listen address
`127.0.0.1:20129`. Routes come from the four style files; this table is the shipped set.

## Routes

| Style file | Method and path | Op | Type |
|---|---|---|---|
| openai-chat | `POST /v1/chat/completions` | generate | text |
| openai-chat | `POST /v1/embeddings` | generate | embeddings |
| openai-chat | `POST /v1/images/generations` | generate | image |
| openai-chat | `POST /v1/audio/speech` | generate | tts |
| openai-chat | `POST /v1/audio/transcriptions` (multipart) | generate | stt |
| openai-chat | `POST /v1/videos` | job_submit | video |
| openai-chat | `GET /v1/videos/{id}` | job_get | video |
| openai-chat | `GET /v1/videos/{id}/content` | job_content | video |
| openai-chat | `GET /v1/models` (default) | list_models | — |
| openai-chat | `GET /v1/models/{model*}` (default) | get_model | — |
| anthropic-messages | `POST /v1/messages` | generate | text |
| anthropic-messages | `POST /v1/messages/count_tokens` | count_tokens | text |
| anthropic-messages | `GET /v1/models`, `GET /v1/models/{model*}` when `anthropic-version` is present | list_models, get_model | — |
| openai-responses | `POST /v1/responses` | generate | text |
| openai-responses | `POST /v1/responses/input_tokens` | count_tokens | text |
| gemini | `POST /v1beta/models/{model*}:generateContent` | generate | text, image, tts (by response modality) |
| gemini | `POST /v1beta/models/{model*}:streamGenerateContent` | generate (stream) | text, image, tts |
| gemini | `POST /v1beta/models/{model*}:countTokens` | count_tokens | text |
| gemini | `POST /v1beta/models/{model*}:embedContent`, `:batchEmbedContents` | generate | embeddings |
| gemini | `POST /v1beta/models/{model*}:predictLongRunning` | job_submit | video |
| gemini | `GET /v1beta/operations/{id}` | job_get | video |
| gemini | `GET /v1beta/models`, `GET /v1beta/models/{model*}` | list_models, get_model | — |

`{model*}` accepts `/`, so `openrouter/openai/gpt-5` works as a Gemini path segment.
Unknown paths return 404 in the OpenAI error shape.

## Access key

One agent key works on every route (FR-002). Carriers, in order:

| Style | Carriers |
|---|---|
| anthropic-messages | `x-api-key`, `authorization: Bearer` |
| openai-chat, openai-responses | `authorization: Bearer` |
| gemini | `x-goog-api-key`, `?key=`, `authorization: Bearer` |

Missing, unknown or revoked key: 401 in the style's error shape, checked before the body is
read. Nothing is sent upstream. A record is written with outcome `refused` and no agent.

## Model addressing

`model` (body field or path segment) is a unified model name or `<provider>/<model>`, with
slice 002's resolution rules. A model whose type differs from the route's type is refused
with 400 in the style's error shape, naming both types (FR-012).

## Unknown fields and headers (optimizer pass-through)

A client or optimizer hop may add fields and headers 0router doesn't know. They are never a
reason to refuse a request (FR-038–FR-041, research R27).

| Route | Unknown body fields | Unknown client headers | Non-stream response |
|---|---|---|---|
| same-style (the endpoint speaks the client's style) | sent upstream as received, at any depth | sent upstream, except the floor, hop-by-hop and `x-0router-*` | returned as received |
| cross-style | dropped where the target style has no place; each drop is in the record | only the plugin's declared list | rebuilt in the client's style |

## Response headers

| Header | When |
|---|---|
| `x-0router-request-id: rq_…` | every response, including errors and refusals |
| forwarded provider headers | only those the serving plugin declares (`to_client`), after the floor |
| `retry-after` | on 503 after all attempts failed: seconds until the earliest cooldown ends |

## Streams

- SSE styles use `text/event-stream`; events follow the style's own grammar.
- The first content event is the TTFT point. Header events may be held until then (a few
  milliseconds).
- Keepalive while 0router retries or falls back: `event: ping` (Messages), SSE comment
  (others).
- **Continuation**: the client sees one uninterrupted answer. No marker.
- **Restart** (shipped default): the open text or thinking block is closed, a new text
  block holds `— connection lost, answer restarted —`, then the new answer follows with
  fresh block indexes. In Chat Completions and Gemini streams, the note is a text delta
  surrounded by blank lines.
- **Error event**: the style's stream error event, then a clean end:
  - Messages: `event: error` / `{"type":"error","error":{"type":…,"message":…},"nullrouter":{…}}`
  - Chat: `data: {"error":{…,"nullrouter":{…}}}` then `data: [DONE]`
  - Responses: `response.failed` with `response.error` and `nullrouter`
  - Gemini: a final chunk with `error` (`code`, `message`, `status`, `nullrouter`)
- A break while a tool call's arguments are partly sent always ends with the error event.

## Informational error body

The style's error shape with two additions: the attempt summary in the standard message
field and the `nullrouter` extra object.

Messages example (all attempts failed):

```json
{
  "type": "error",
  "error": {
    "type": "api_error",
    "message": "0router: no provider could serve claude-sonnet (record rq_01JAB…). Tried: anthropic/main claude-sonnet-4-20250514: 503 overloaded after 3 retries; anthropic/backup claude-sonnet-4-20250514: 429 rate limited, cooling down 4 s; openrouter/main anthropic/claude-sonnet-4: 502 network error after 3 retries."
  },
  "nullrouter": {
    "record_id": "rq_01JAB…",
    "attempts": [
      { "provider": "anthropic", "account": "main", "model": "claude-sonnet-4-20250514", "status": 503, "class": "transient", "reason": "overloaded", "retries": 3 },
      { "provider": "anthropic", "account": "backup", "model": "claude-sonnet-4-20250514", "status": 429, "class": "rate_limited", "reason": "rate limited, cooling down 4 s", "retries": 0 },
      { "provider": "openrouter", "account": "main", "model": "anthropic/claude-sonnet-4", "status": 502, "class": "network", "reason": "network error", "retries": 3 }
    ]
  }
}
```

Placement per style:

| Style | Message field | Extra field |
|---|---|---|
| Messages | `error.message` | top-level `nullrouter` |
| Chat, Responses (non-stream) | `error.message` | `error.nullrouter` |
| Gemini | `error.message` | `error.nullrouter` (inside `error`, next to `code`, `status`) |

Status: a non-fallback upstream error keeps its status and its upstream message comes first
in the message field; all-attempts-failed is 503. Refusals by 0router: 401 (key), 400 (type
mismatch, request can't be translated), 404 (unknown model).

Every account is named by its operator name; no key, token or secret appears.

## Token counting

| Style | Request | Response |
|---|---|---|
| Messages | `POST /v1/messages/count_tokens` | `{"input_tokens": n}` |
| Responses | `POST /v1/responses/input_tokens` | `{"object":"response.input_tokens","input_tokens": n}` |
| Gemini | `:countTokens` | `{"totalTokens": n}` |

When the count is a local estimate, the response carries header `x-0router-estimate: true`.

## Model lists

Every unified model and every direct model of every type on providers with at least one
account. Shapes: OpenAI `{"object":"list","data":[{"id","object":"model","owned_by"}]}`;
Anthropic `{"data":[{"id","type":"model","display_name"}],"has_more":false}`; Gemini
`{"models":[{"name":"models/…","supportedGenerationMethods":[…]}]}`. The model type appears
in an extra `nullrouter.type` field on each entry.

## Video jobs

The job id returned is a 0router id (`vj_…`). Polls are served from the account that took
the job; there is no fallback after submission. A poll failure is retried on that account
per R7 and otherwise returned with the informational fields.
