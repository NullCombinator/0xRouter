# Writing a provider plugin

A provider plugin is one TOML file that declares one provider: where its API lives, how
requests authenticate, and which models it offers. It is **data, not code**. The core
reads the declarations and acts on them. A provider plugin cannot run code, make network
requests, read files, or hold secrets.

This page covers provider plugins only. 0router's other plugin kind, the harness adapter,
may be sandboxed code and has its own rules (constitution Principle I).

Install a plugin by copying it into `$ZEROROUTER_HOME/plugins/` (default
`~/.0router/plugins/`). Only `*.toml` files at the top level are read. Check it first:

```bash
zerorouter-cli validate my-provider.toml     # OK my-provider.toml, or one line per error
zerorouter-cli check                         # load everything and print the load report
```

Full reference: [`specs/002-provider-model-registry/contracts/plugin-schema.md`](../specs/002-provider-model-registry/contracts/plugin-schema.md).

## The smallest plugin

Only `id` and `category` are required. This declares a provider without a catalog or a
transport. It can be listed, but requests cannot be sent to it yet.

```toml
schema = 1
id = "my-provider"      # [a-z0-9][a-z0-9-]*
category = "apikey"     # apikey | oauth | freeTier | free | webCookie
```

## A typical plugin

```toml
schema = 1
id = "acme"
category = "apikey"
alias = "ac"                 # clients can write ac/<model>
aliases = ["acme-ai"]        # more lookup tokens

[auth]
kind = "apikey"
header = "Authorization"
scheme = "bearer"            # the operator's key is sent as "Bearer <key>"

[transport]
base_url = "https://api.acme.example/v1/chat/completions"
format = "openai"            # wire format; default openai
validate_url = "https://api.acme.example/v1/models"
headers = { "User-Agent" = "0router" }
quirks = ["preserve_cache_control"]

[capabilities.llm]            # served by [transport]

[capabilities.embedding]
endpoint = { base_url = "https://api.acme.example/v1/embeddings" }

[[models]]
id = "acme-large-2.1"
upstream_id = "acme/large-2.1"   # what is sent upstream, if it differs
context_length = 200000

[[models]]
id = "acme-embed-1"
kind = "embedding"
dimensions = 1024
```

## Models

- Leaving `models` out means the catalog is **unknown**. `models = []` means the provider
  offers **none**.
- `models = ["m-a", "m-b"]` is shorthand for `[[models]]` tables with just an `id`. The
  display name is derived from the id. A file uses one form or the other.
- A model without `kind` is untyped. It is listed under `llm` but keeps `kind = None`.
- One id may appear once per `kind`.
- `passthrough_models = true` accepts any model id the client sends.
- `version_separator_tolerance = true` lets `claude-sonnet-4-5` find
  `claude-sonnet-4.5`.
- A trailing `(high)`-style suffix on a request is stripped for lookup and re-appended
  upstream. A suffix in a declared id (`m(low)`) is a preset, used when the request
  has none.

## Capabilities

One plugin can serve several modalities. Add a `[capabilities.<kind>]` section for each
modality: `llm`, `image`, `image_to_text`, `video`, `tts`, `stt`, `embedding`,
`web_search`, `web_fetch`, `systemone`. A section without an `endpoint` is served by
`[transport]`, so a plugin with neither is rejected.

## Named built-ins

A plugin can **select** core behaviour by name but never **supply** it. Unknown names are
rejected, and the error lists the allowed values.

| Key | Allowed |
|---|---|
| `transport.quirks` | `preserve_cache_control`, `drop_client_metadata`, `cline_envelope`, `drop_output_config`, `require_claude_tool_type`, `cloak_tools_on_oauth` |
| `auth.hooks`, `transport.auth.hooks` | `cline_headers`, `kimi_headers`, `kilocode_org` |
| `transport.format` | `openai`, `openai-responses`, `claude`, `gemini`, `gemini-cli`, `vertex`, `antigravity`, `kiro`, `cursor`, `commandcode`, `ollama`, `grok-web`, `perplexity-web` |
| `[transport.executor_params]` | `cli_version`, `client_version`, `api_client`, `client_identifier`, `token_auth`, `no_auth`, `auth_type`, `copilot` |
| `[oauth.params]` | the core's known OAuth parameters (`zerorouter-cli validate` prints the list) |

## What is rejected

Every error names the file, `line:col`, the field path, and the rule:

```text
acme.toml:9:13 transport.headers.Authorization: credential-bearing header not allowed in plugins
```

- **Unknown keys** anywhere, including `oauth.client_secret`.
- **Secrets**:
  - credential-bearing header names (`Authorization`, `x-api-key`, …);
  - secret-like model `params`;
  - URLs with `user:pass@` or `?api_key=`-style query keys.
- **Bad values**:
  - an unknown category, format, quirk, hook, capability or executor param;
  - an `id` outside `[a-z0-9][a-z0-9-]*`;
  - `schema` other than 1.
- **Structure**:
  - a capability section with no reachable endpoint;
  - a duplicate model id;
  - `[[transports]]` without `[transport]`;
  - a `default_region` that is not a key of `regions`;
  - `auth.credential_fallback` naming an unknown provider.

## Replacing a bundled provider

If your file's `id` matches a bundled provider, the bundled one stays active until the
operator decides in `config.toml`. See [operator-config.md](operator-config.md#replacing-a-bundled-provider).
Bundled OAuth client secrets are released only while every OAuth URL of the active plugin
points at the hosts they were issued for. Otherwise `check` reports the credential as
withheld.
