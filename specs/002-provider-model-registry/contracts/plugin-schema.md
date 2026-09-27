# Contract: Provider Plugin File (TOML)

**Consumers**: plugin authors, the bundled-plugin generator, the validation gate.
**Entity mapping**: [data-model.md § ProviderEntity](../data-model.md#providerentity)

One file declares one provider. Keys use `snake_case`. Unknown keys are **rejected**
(FR-008). Only `id` and `category` are required (FR-006).

## Minimal (catalog-only)

```toml
schema = 1
id = "my-provider"
category = "apikey"
```

## Typical

```toml
schema = 1
id = "acme"
category = "apikey"
alias = "ac"
aliases = ["acme-ai"]

[auth]
kind = "apikey"
header = "Authorization"
scheme = "bearer"

[transport]
base_url = "https://api.acme.example/v1/chat/completions"
format = "openai"
validate_url = "https://api.acme.example/v1/models"
headers = { "User-Agent" = "0router" }
quirks = ["preserve_cache_control"]

[capabilities.llm]            # served by [transport]

[capabilities.embedding]
endpoint = { base_url = "https://api.acme.example/v1/embeddings" }

[[models]]
id = "acme-large-2.1"
upstream_id = "acme/large-2.1"
context_length = 200000

[[models]]
id = "acme-embed-1"
kind = "embedding"
dimensions = 1024
```

## Top-level keys

| Key | Type | Notes |
|---|---|---|
| `schema` | integer | Schema version. Only `1` is accepted in this slice; absent is treated as `1` |
| `id` | string | **Required.** `[a-z0-9][a-z0-9-]*` |
| `category` | string | **Required.** `apikey` \| `oauth` \| `freeTier` \| `free` \| `webCookie` |
| `alias`, `aliases`, `ui_alias` | string, [string], string | Lookup tokens (`ui_alias` is display-only) |
| `passthrough_models` | bool | |
| `version_separator_tolerance` | bool | |
| `[auth]` | table | `kind`, `modes`, `no_auth`, `has_oauth`, `header`, `scheme`, `hooks`, `credential_fallback` |
| `[transport]` | table | See data model. Only the listed keys. Executor-specific values go in `[transport.executor_params]`, which is a closed set of keys too (`cli_version`, `client_version`, `api_client`, `client_identifier`, `token_auth`, `no_auth`, `auth_type`, `[transport.executor_params.copilot]`) |
| `[[transports]]` | array of tables | Same shape as `[transport]` |
| `[oauth]` | table | Public values only. Long-tail values go in `[oauth.endpoints]` (URLs) and `[oauth.params]` (scalars). `oauth.params` keys must be ones the core knows (`KNOWN_OAUTH_PARAMS`) |
| `[capabilities.<kind>]` | table | `<kind>` ∈ `llm`, `image`, `image_to_text`, `video`, `tts`, `stt`, `embedding`, `web_search`, `web_fetch`, `systemone` |
| `[[models]]` or `models = [...]` | array of tables, or array of ID strings | Omit entirely = "catalog unknown"; `models = []` = "offers none". A bare string `"acme-small"` means `{ id = "acme-small" }` with a derived display name (9router `normalizeModel`). TOML cannot mix `[[models]]` tables with an inline string array in one file; use one form or the other |
| `[display]` | table | `name`, `icon`, `color`, `text_icon`, `website`, `notice`, `deprecated`, `deprecation_notice`, `priority`, `hidden`, `has_free`, `auth_hint`, `features`, `thinking` |

## Named built-ins (closed sets — unknown values are rejected)

| Key | Allowed values |
|---|---|
| `transport.quirks` | `preserve_cache_control`, `drop_client_metadata`, `cline_envelope`, `drop_output_config`, `require_claude_tool_type`, `cloak_tools_on_oauth` (plus the list-valued `claude_supported_tool_types`, `force_auto_tool_choice_models`) |
| `auth.hooks` | `cline_headers`, `kimi_headers`, `kilocode_org` |
| `transport.format`, `models.target_format`, `models.supported_formats` | see data model `WireFormat` |
| `capabilities.<kind>` | see above |

A plugin can **select** core behaviour by name. It can never **supply** behaviour
(Constitution I).

## Rejected content (validation gate)

| Rule | Example that fails | Error (shape) |
|---|---|---|
| Unknown key | `client_secret = "…"` anywhere | `acme.toml:12:1 oauth.client_secret: unknown field` |
| Unknown OAuth param | `[oauth.params] foo = "x"` | `…oauth.params.foo: unknown OAuth parameter; allowed: …` |
| Secret-like map key | `headers = { Authorization = "Bearer x" }` | `acme.toml:9:13 transport.headers.Authorization: credential-bearing header not allowed in plugins` |
| Secret in URL | `base_url = "https://u:p@x"` or `?api_key=` | `…transport.base_url: URL must not carry credentials` |
| Bad enum | `category = "local"` | `…category: expected one of apikey, oauth, freeTier, free, webCookie` |
| Unknown built-in | `quirks = ["run_script"]` | `…transport.quirks[0]: unknown quirk "run_script"; allowed: …` |
| Unreachable section | `[capabilities.tts]` with no endpoint, no `[transport]` | `…capabilities.tts: no endpoint and provider has no transport` |
| Duplicate model id | two `[[models]]` with the same `id` | `…models[3].id: duplicate of models[1]` |
| Bad reference | `auth.credential_fallback = "nope"` | `…auth.credential_fallback: unknown provider "nope"` |
| Parse error | invalid TOML | `acme.toml:4:7: <toml parser message>` |

Every error names the file, the position, the field path, and the rule (FR-010).

## Bundled plugins

`plugins/bundled/<id>.toml` is generated from the evaluated 9router registry
([research R8](../research.md#r8-generating-bundled-plugins-from-9router)). Each file starts
with:

```toml
# Generated from ref/9router@<sha> by tools/gen-bundled/generate.mjs — do not edit.
```

Bundled files use this same schema and go through the same gate.
