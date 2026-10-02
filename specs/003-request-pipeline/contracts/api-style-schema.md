# Contract: API-style file schema

Files in `styles/bundled/*.toml`, embedded in the binary and loaded through the same gate as
plugins. Operators can't add styles in this slice; the format is still validated strictly,
because a style file is data the core interprets (Constitution I).

## Top level

```toml
schema = 1
kind = "api-style"
id = "anthropic-messages"          # openai-chat | anthropic-messages | openai-responses | gemini
```

Unknown keys are rejected everywhere (`deny_unknown_fields`). Secret-like keys and values
are rejected with slice 002's secret checks. A style file can't declare forwarding.

## Front door

```toml
[access_key]
carriers = [{ header = "x-api-key", scheme = "raw" }, { header = "authorization", scheme = "bearer" }]

[session]
carriers = [{ extractor = "claude_code_user_id" }, { header = "x-claude-code-session-id" }]

[[routes]]
method = "POST"
path = "/v1/messages"
op = "generate"                    # generate | count_tokens | list_models | get_model | job_submit | job_get | job_content
type = "text"                      # text | embeddings | image | tts | stt | video
model = { body = "model" }         # or { path = "model" }
stream = { body = "stream" }       # or { path_suffix = ":streamGenerateContent" }
discriminator = { header_present = "anthropic-version" }   # optional
```

Path templates: `{name}` = one segment, `{name*}` = the rest including `/`. Gate rule: no two
loaded routes share (method, path) unless their discriminators are disjoint and exactly one
has none (the default).

## Mapping vocabulary (closed)

| Construct | Form | Limits |
|---|---|---|
| Field path | `"a.b[0].c"`, `"a[*].b"` | read-only selectors |
| Enum map | TOML table `from = "to"` | exact string match |
| Typed template | JSON-shaped TOML value with `"{placeholder}"` strings | whole-string placeholder keeps the type; `"x{p}y"` is string interpolation; `"{p?}"` omits the key when absent; `{{` escapes; each context has a fixed placeholder set; no expressions, conditionals or loops |
| Match rule | `{ path_equals = [...] }`, `{ path_present = "..." }`, `when_request = "path"` | literal equality or presence only |
| Layout choice | named value from a closed set | see below |
| Primitive | named value from a closed set | see below |

Templates are reversible: decoding matches literal parts and extracts placeholders. The
gate proves, for each style usable as an upstream wire, that every response and stream rule
reverses unambiguously.

## Per-type codec sections

```toml
[text.layout]
system = "top_level_field"         # top_level_field | first_message | instructions_field | system_instruction
tool_calls = "content_part"        # content_part | message_field | output_item | function_call_part
tool_results = "content_part"      # content_part | tool_role_message | output_item | function_response_part
arguments = "json_object"          # json_object | json_string
alternation = "merge_adjacent"     # merge_adjacent | as_is
tool_result_match = "id"           # id | name

[text.parts]                       # one template per IR part kind
[text.params]                      # IR param → field path, { form, path }, or { path, default } (max_tokens only: sent when the client gave none)
[text.finish]                      # style reason → IR reason (text.finish is total over the style's reasons)
[text.usage]                       # IR usage field → path; input_semantics
[text.stream]
framing = "sse_named"              # sse_named | sse_data | sse_data_done | ndjson | json_array
blocks = "explicit"                # explicit | implicit
tool_arguments = "fragments"       # fragments | whole
[[text.stream.events]]             # IR event → style event template
on = "text_delta"
event = "content_block_delta"
data = { type = "content_block_delta", index = "{block.index}", delta = { type = "text_delta", text = "{delta.text}" } }
```

IR stream events (`on` values): `preamble`, `block_start:{text|thinking|tool_call}`,
`text_delta`, `thinking_delta`, `signature`, `tool_arguments`, `block_stop`, `usage`,
`finish`, `error`, `keepalive`, `done`.

Counters and ids available to templates: `{block.index}`, `{tool.ordinal}`,
`{output.index}`, `{sequence.number}`, `{response.id}` (with a declared prefix).
Accumulations: `{block.full_text}`, `{block.full_arguments}`, `{response.rendered}` (the
style's non-stream template applied to the answer so far; used by Responses `*.done` and
`response.completed`).

## Named primitives (closed sets)

| Family | Members |
|---|---|
| Media codec | `data_url`, `anthropic_source`, `gemini_inline_data`, `url` |
| Thinking form | `budget_tokens`, `effort`, `gemini_thinking_config` |
| Embedding vector | `float`, `base64_f32le` |
| Body encoding | `json`, `multipart`, `binary` |
| Async job | `poll_on_client_request` |
| Token estimator | `estimate_9router` |
| Session extractor | `claude_code_user_id` |
| Cross-style repair | `ensure_tool_call_ids`, `fill_missing_tool_results`, `gemini_schema_sanitize`, `gemini_function_name_sanitize` |
| Audio collector | `chat_audio_delta_collect` (openrouter TTS fallback) |

Repairs run only when client style ≠ provider wire, and never change message text.

## Errors

```toml
[errors]
body = { type = "error", error = { type = "{error.type}", message = "{error.message}" }, nullrouter = "{error.details}" }
type_map = { 400 = "invalid_request_error", 401 = "authentication_error", 403 = "permission_error", 404 = "not_found_error", 429 = "rate_limit_error", 500 = "api_error", 503 = "api_error", 529 = "overloaded_error" }
stream_event = { event = "error", data = "{error.body}" }
keepalive = { event = "ping", data = { type = "ping" } }
```

Gate: the body template must contain `{error.message}` and `{error.details}`; `type_map`
must cover 400, 401, 404, 429, 500 and 503.

## Gate corpus additions (`tests/gate/invalid/styles/`)

`unknown-key`, `unknown-placeholder` (`{request.api_key}`), `bad-path-template`,
`route-collision` (two files), `missing-codec`, `ambiguous-stream-rules`,
`finish-map-incomplete`, `error-template-missing-message`, `bad-carrier-scheme`,
`unknown-session-extractor`, `unknown-framing`, `expression-in-template` (`{a+b}`).
