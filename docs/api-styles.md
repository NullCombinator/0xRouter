# Client API styles

An API style is the shape of one client-facing API: which routes it serves, where a client
puts its access key, how a request and its answer are laid out, and how errors look. 0router
ships four, as data files in `styles/bundled/`:

| Style | Clients | Routes |
|---|---|---|
| `openai-chat` | OpenAI SDKs, most agents | `POST /v1/chat/completions`, `GET /v1/models[/{model}]`, `POST /v1/embeddings`, `POST /v1/images/generations`, `POST /v1/audio/speech`, `POST /v1/audio/transcriptions`, `POST /v1/videos`, `GET /v1/videos/{id}[/content]` |
| `anthropic-messages` | Claude Code, Anthropic SDKs | `POST /v1/messages`, `POST /v1/messages/count_tokens`, `GET /v1/models[/{model}]` when `anthropic-version` is sent |
| `openai-responses` | Codex, the Responses API | `POST /v1/responses`, `POST /v1/responses/input_tokens` |
| `gemini` | Gemini SDKs | `POST /v1beta/models/{model}:generateContent` (text, image, speech), `:streamGenerateContent`, `:embedContent`, `:batchEmbedContents`, `:predictLongRunning`, `:countTokens`; `GET /v1beta/models[/{model}]`, `/v1beta/operations/{id}`, `/v1beta/files/{id}:download` |

The same file describes a style both ways: as the **client style** a request arrives in, and
as the **wire** a provider endpoint speaks (`wire = "anthropic-messages"` in a plugin). When
the two match, 0router forwards the client's body with only the model id changed (see
[Forwarding](#forwarding)). When they differ, the request goes through 0router's intermediate
representation and is written out in the wire's layout.

Style files are embedded in the binary and pass through the same validation gate as
plugins. Operators can't add or replace a style in this release. A style file is data the
core interprets, never code: every construct below is either a field path, a template, or
a name chosen from a closed set the core implements.

## Top level

```toml
schema = 1
kind = "api-style"
id = "anthropic-messages"
```

Unknown keys are rejected everywhere. Secret-like keys and values are rejected with the
plugin secret checks. A style file can't declare `[forwarding]`; only plugins can.

## Front door

```toml
[access_key]
carriers = [{ header = "x-api-key", scheme = "raw" }, { header = "authorization", scheme = "bearer" }]

[session]
carriers = [{ extractor = "claude_code_user_id" }, { header = "x-claude-code-session-id" }]

[[routes]]
method = "POST"
path = "/v1/messages"
op = "generate"
type = "text"
model = { body = "model" }
stream = { body = "stream" }
discriminator = { header_present = "anthropic-version" }   # optional
```

- **`access_key.carriers`**: where the client puts its agent key, tried in order. Each
  carrier is a `header` (with `scheme = "raw"` or `"bearer"`) or a `query` parameter. The
  key is checked before the body is read.
- **`session.carriers`**: where the client carries its session id: a `header`, a body
  `path`, or a named `extractor` (`claude_code_user_id`).
- **`routes`**: `op` is one of `generate`, `count_tokens`, `list_models`, `get_model`,
  `job_submit`, `job_get`, `job_content`. `type` is one of `text`, `embeddings`, `image`,
  `tts`, `stt`, `video`. `model` is read from a body field (`{ body = "model" }`) or a path
  parameter (`{ path = "model" }`); `stream` from a body field or a path suffix
  (`{ path_suffix = ":streamGenerateContent" }`). A route may name a codec `variant` when its
  body differs from the type's default shape.
- **Path templates**: `{name}` matches one segment, `{name*}` the rest including `/`.
- **Discriminators** pick between routes that share a method and path: `header_present`,
  `path_present` (a body field exists) or `path_equals = [path, value]`. The gate refuses two
  routes on the same method and path unless their discriminators are disjoint and exactly
  one has none; that one is the default. `/v1/models` is shared this way: clients sending
  `anthropic-version` get the Anthropic list, everyone else the OpenAI one.

## Mapping vocabulary

Every mapping is one of these, and nothing else:

| Construct | Form | Notes |
|---|---|---|
| Field path | `"a.b[0].c"`, `"a[*].b"` | read-only selector |
| Enum map | TOML table `from = "to"` | exact string match |
| Template | JSON-shaped TOML value with `"{placeholder}"` strings | a whole-string placeholder keeps its type; `"x{p}y"` interpolates a string; `"{p?}"` leaves the key out when the value is absent; `{{` escapes |
| Match rule | `{ path_equals = [...] }`, `{ path_present = "..." }`, `when_request = "path"` | literal equality or presence only |
| Named choice | a value from a closed set | listed below |

Templates have no expressions, conditionals or loops, and each context has a fixed set of
placeholders. Templates are reversible: decoding matches the literal parts and reads the
placeholders back. For each style usable as a wire, the gate proves that every response and
stream rule reverses unambiguously.

## The text codec

```toml
[text.layout]
system = "top_level_field"
messages = "messages"
content = "content"
content_form = "string_when_text_only"
tool_calls = "content_part"
tool_results = "content_part"
arguments = "json_object"

[text.parts.text]
data = { type = "text", text = "{part.text}", cache_control = "{part.cache_control?}" }

[text.params]
max_tokens = { path = "max_tokens", default = 64000 }
temperature = "temperature"

[text.finish]                 # style reason → IR reason, total over the style's reasons
[text.usage]                  # IR usage field → path, plus input_semantics
[text.response]               # the non-stream body template and its id prefix
[text.stream]                 # framing, block model, and one template per stream event
```

**Layout** (`[text.layout]`) places the conversation:

| Key | Values |
|---|---|
| `system` | `top_level_field`, `first_message`, `instructions_field`, `system_instruction` |
| `tool_calls` | `content_part`, `message_field`, `output_item`, `function_call_part` |
| `tool_results` | `content_part`, `tool_role_message`, `output_item`, `function_response_part` |
| `arguments` | `json_object`, `json_string` |
| `alternation` | `merge_adjacent` (default), `as_is` |
| `tool_result_match` | `id` (default), `name` |
| `content_form` | `always_parts` (default), `string_when_text_only` |
| `tool_result_content` | `string_or_parts` (default), `string`, `parts`, `object` |

`messages` and `content` name the message list and the parts field; `roles` maps IR roles
that differ (Gemini's `assistant = "model"`).

**Parts** (`[text.parts.<kind>]`): one template per part kind (text, image, audio,
tool call, tool result, thinking). `match` gives a decode template when it differs from
`data`, `roles` overrides `data` per role, and media parts name a `codec`.

**Params** (`[text.params]`): an IR parameter maps to a field path, to `{ path, form }` for a
parameter with a named form (thinking, structured output), or to `{ path, default }` for
`max_tokens` on a wire that requires it. The default is sent only when the client gave none.

**Usage** (`[text.usage]`): paths for `input`, `output`, `cache_read`, `cache_write` and
`reasoning`. `input_semantics = "includes_cache"` says the style's input count includes
cached tokens; 0router's records count uncached input only and convert both ways.

**Stream** (`[text.stream]`):

```toml
[text.stream]
framing = "sse_named"
blocks = "explicit"
tool_arguments = "fragments"

[[text.stream.events]]
on = "text_delta"
event = "content_block_delta"
data = { type = "content_block_delta", index = "{block.index}", delta = { type = "text_delta", text = "{delta.text}" } }
```

- `framing`: `sse_named`, `sse_data`, `sse_data_done`, `ndjson`, `json_array`.
- `blocks`: `explicit` (the stream opens and closes content blocks) or `implicit`.
- `tool_arguments`: `fragments` or `whole`.
- `on`: `preamble`, `block_start:text`, `block_start:thinking`, `block_start:tool_call`,
  `text_delta`, `thinking_delta`, `signature`, `tool_arguments`, `block_stop`, `usage`,
  `finish`, `error`, `keepalive`, `done`. `when_request` emits an event only when a request
  field is true (Chat's `stream_options.include_usage`).
- Counters and ids for templates: `{block.index}`, `{tool.ordinal}`, `{output.index}`,
  `{sequence.number}`, `{response.id}`. Accumulations: `{block.full_text}`,
  `{block.full_arguments}`, and `{response.rendered}` (the non-stream template applied to
  the answer so far, for Responses' `*.done` and `response.completed` events).

**Repairs** (`repairs = [...]`) run only when the client style differs from the wire, and
never change message text: `ensure_tool_call_ids`, `fill_missing_tool_results`,
`gemini_schema_sanitize`, `gemini_function_name_sanitize`.

**Token counts** (`[text.count_tokens]`): the response template (`{count.input}`) and the
local `estimator` (`estimate_9router`) used when the provider has no count endpoint.

## Other model types

`[embeddings]`, `[image]`, `[tts]`, `[stt]` and `[video]` each hold a request and a response
template plus the type's named choices:

| Choice | Values |
|---|---|
| Body `encoding` | `json`, `multipart`, `binary` |
| Embedding `vector` | `float`, `base64_f32le` |
| Video job | `poll_on_client_request`, with a job status template (`{job.id}`, `{job.status}`, `{job.done}`, …) |
| Media codec | `data_url`, `anthropic_source`, `gemini_inline_data`, `url` |
| Audio collector | `chat_audio_delta_collect` (speech through a chat stream, for openrouter) |

Named `variants` give alternative shapes that a route selects (Gemini's `:embedContent`
next to `:batchEmbedContents`).

## Errors

```toml
[errors]
body = { type = "error", error = { type = "{error.type}", message = "{error.message}" }, nullrouter = "{error.details}" }
type_map = { 400 = "invalid_request_error", 401 = "authentication_error", 404 = "not_found_error", 429 = "rate_limit_error", 500 = "api_error", 503 = "api_error" }
stream_event = { event = "error", data = "{error.body}" }
keepalive = { event = "ping", data = { type = "ping" } }
```

Every error 0router produces itself is written in the client's style, so a client's own
error handling works. The body template must hold `{error.message}` and `{error.details}`
(0router's attempt trail and request id), and `type_map` must cover 400, 401, 404, 429, 500
and 503. `stream_event` is how an error is reported once a stream has started;
`keepalive` is what 0router sends while a stream is idle.

## Forwarding

When a request's client style equals the endpoint's wire, the body is forwarded as the
client sent it. 0router edits only named paths: the model id, the stream flag when the
endpoint forces streaming, and `stream_options.include_usage` on a streamed Chat wire.
Unknown body fields and unknown client headers go through, except the header floor (the
access key, hop-by-hop headers and `x-0router-*`), which never reaches a provider. The
provider's answer comes back unchanged, and usage is still read from it.

Across styles, unknown fields and undeclared headers can't be placed in the other layout.
They are left out, and the request record lists each dropped path with a reason. A plugin
can declare extra headers to carry across styles in its `[forwarding]` table (see
[plugins.md](plugins.md#forwarding-and-the-header-floor)).
