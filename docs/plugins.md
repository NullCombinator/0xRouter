# Writing a provider plugin

A provider plugin is one TOML file that declares one provider: where its API lives, how
requests authenticate, and which models it offers. It is **data, not code**. The core
reads the declarations and acts on them. A provider plugin cannot run code, make network
requests, read files, or hold secrets.

This page covers provider plugins only. 0router's other plugin kind, the harness adapter,
may be sandboxed code and has its own rules (constitution Principle I).

Install a plugin with `nullrouter plugins install <id>` (community set) or by copying it
into `$NULLROUTER_HOME/plugins/` (default `~/.0router/plugins/`). Only `*.toml` files at
the top level are read. Check it first:

```bash
nullrouter validate my-provider.toml     # OK my-provider.toml, or one line per error
nullrouter check                         # load everything and print the load report
```

A running server sees a copied file at its next start, or at the next reload that a
mutating command (`plugins install`, `accounts add`, `keys issue`, …) sends it.

Full reference: [`specs/003-request-pipeline/contracts/provider-schema-v2.md`](../specs/003-request-pipeline/contracts/provider-schema-v2.md),
which extends slice 002's [`plugin-schema.md`](../specs/002-provider-model-registry/contracts/plugin-schema.md).

## The smallest plugin

Only `id` and `category` are required. This declares a provider without a catalog or an
endpoint. It can be listed, but requests cannot be sent to it.

```toml
schema = 2
id = "my-provider"      # [a-z0-9][a-z0-9-]*
category = "apikey"     # apikey | oauth | freeTier | free | webCookie
```

## A typical plugin

Write new plugins as schema 2. Each model type the provider serves gets an endpoint, and
a text endpoint names the API style it speaks as its `wire` (see
[api-styles.md](api-styles.md)): `openai-chat`, `anthropic-messages`, `openai-responses` or
`gemini`.

```toml
schema = 2
id = "acme"
category = "apikey"
alias = "ac"                 # clients can write ac/<model>
aliases = ["acme-ai"]        # more lookup tokens

[auth]
kind = "apikey"
header = "Authorization"
scheme = "bearer"            # the operator's key is sent as "Bearer <key>"

[endpoints.text]
url = "https://api.acme.example/v1/chat/completions"
wire = "openai-chat"
headers = { "User-Agent" = "0router" }   # static and non-secret
vision = true                # accepts image parts
timeout_ms = 60000           # time to response headers
retry = { 429 = { retries = 1, delay_ms = 2000 } }

[endpoints.embeddings]
url = "https://api.acme.example/v1/embeddings"
wire = "openai-chat"

[[models]]
id = "acme-large-2.1"
upstream_id = "acme/large-2.1"   # what is sent upstream, if it differs
context_length = 200000

[[models]]
id = "acme-embed-1"
kind = "embedding"
dimensions = 1024
```

The secret is never in the plugin: the operator adds it with `nullrouter accounts add acme
main`, and the core places it in the `[auth]` header only when it sends a request.

## Endpoints

`[endpoints.<type>]` holds the endpoint for one model type: `text`, `embeddings`, `image`,
`tts`, `stt`, `video`. Use `[[endpoints.text]]` for one entry per wire when a provider
speaks several; a model's `wires = [...]` then orders the choice, after the client's own
style.

- **`url`**: the only placeholders are `{model}` and `{voice}`, in the path only. Loopback,
  private, link-local and metadata hosts, `localhost`, `*.local` and `*.internal` are
  rejected unless the operator sets `allow_private_endpoints = true`. The address a host
  resolves to is checked again at request time, and redirects are never followed.
- **`wire`** names a style. An endpoint without one declares an inline `body` and
  `response` template instead (non-text types). Body placeholders come from a fixed set
  (`{input.*}`, `{model.upstream_id}`, `{params.*}`, `{output.*}`); `{account.*}`,
  `{secret.*}` and unknown names are rejected.
- **`auth`** moves the secret's header for this endpoint only (`scheme = "bearer"` or
  `"raw"`).
- **Timeouts and retries**: `timeout_ms` (to response headers), `stall_timeout_ms` (between
  stream bytes), and `retry` per status, overriding the core's defaults.
- **`force_stream`**: the endpoint only streams; 0router collects the stream for a client
  that didn't ask for one.
- **`[endpoints.<type>.errors]`**: where the provider puts an error's message and status,
  in a body or a stream event, so an in-band error is classified like an HTTP one.
- **`[endpoints.text.token_count]`**: the provider's own count endpoint, when its wire
  style has one. Without it, 0router estimates.
- **`[endpoints.text.continuation]`**: whether a trailing assistant turn is continued
  (`assistant_prefill` or `prefix_flag`), for which `models`, and `unless` what holds.

## Session

```toml
[session]
header = "x-opencode-session"
derive = "ses_sha256_hex32"        # or ses_time_base62
```

0router sends the provider a session id derived from the agent's id with a salted hash,
so the provider can keep its cache per agent without seeing the agent's key. A client
value in the same header is kept (`ses_time_base62` keeps it only if it has that shape).

## Forwarding and the header floor

When the client's style equals the endpoint's wire, 0router forwards the client's body and
headers as sent, editing only the model id and the stream flags. Fields and headers added
by tools in front of 0router (an optimizer, a proxy) reach the provider. Across styles,
only what a plugin declares crosses:

```toml
[forwarding.to_upstream]
headers = [
  { name = "anthropic-beta", merge = "append_csv", from_styles = ["anthropic-messages"] },
  { name = "anthropic-version", merge = "replace", from_styles = ["anthropic-messages"] },
]

[forwarding.to_client]
headers = ["request-id", "retry-after", "anthropic-ratelimit-*"]
```

`to_upstream` governs cross-style attempts; on a same-style attempt a declared `merge`
rule still applies to its header. `to_client` applies to both. What a cross-style attempt
leaves out is listed in its request record (`nullrouter records show <id>`).

The **floor** is never forwarded, in either direction, whatever a plugin declares:
credential headers (`authorization`, `x-api-key`, `cookie`, …), any secret-like name, every
style's access-key carrier and every provider's auth header, hop-by-hop headers, and
`x-0router-*`. A header value containing a configured secret is dropped. In a user plugin,
a floor name in a forwarding list is stripped with a diagnostic; a bare `*` or a prefix
shorter than 3 characters is an error.

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
- `wires = [...]` lists the wires a model may be sent on, in order of preference.

## Schema 1

Schema 1 (slice 002's format, and most of the community set) declares `[transport]` and
`[capabilities.<kind>]` sections instead of endpoints. A schema 1 plugin still loads when
it fits (see [the fit check](#community-plugins-and-the-fit-check)) and is converted to
endpoints on load. Schema 2 rejects the schema 1 execution keys (`transport`,
`transports`, `capabilities.*.endpoint`, `quirks`, `executor_params`, `auth.hooks`), so
each fact has one home.

## Named built-ins (schema 1)

A schema 1 plugin can **select** core behaviour by name but never **supply** it. Unknown names are
rejected, and the error lists the allowed values.

| Key | Allowed |
|---|---|
| `transport.quirks` | `preserve_cache_control`, `drop_client_metadata`, `cline_envelope`, `drop_output_config`, `require_claude_tool_type`, `cloak_tools_on_oauth` |
| `auth.hooks`, `transport.auth.hooks` | `cline_headers`, `kimi_headers`, `kilocode_org` |
| `transport.format` | `openai`, `openai-responses`, `claude`, `gemini`, `gemini-cli`, `vertex`, `antigravity`, `kiro`, `cursor`, `commandcode`, `ollama`, `grok-web`, `perplexity-web` |
| `[transport.executor_params]` | `cli_version`, `client_version`, `api_client`, `client_identifier`, `token_auth`, `no_auth`, `auth_type`, `copilot` |
| `[oauth.params]` | the core's known OAuth parameters (`nullrouter validate` prints the list) |

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
  - `schema` 0 or negative (a newer schema is *unsupported*, see below).
- **Structure**:
  - a capability section with no reachable endpoint;
  - a duplicate model id;
  - `[[transports]]` without `[transport]`;
  - a `default_region` that is not a key of `regions`;
  - `auth.credential_fallback` naming an unknown provider.

## Community plugins and the fit check

0router bundles five providers: anthropic, openrouter, opencode-zen, opencode-go and
elevenlabs. The other providers 9router knows ship inside the binary as the **community
set**, generated from 9router, and are installed on request:

```bash
nullrouter plugins list --community   # every community plugin: fits, unsupported, installed
nullrouter plugins install groq       # gate + fit check, then copy to plugins/groq.toml
nullrouter plugins uninstall groq
```

The self-hosted ones (`ollama-local`, `comfyui`, `selfhosted-tts`, …) point at
`localhost`; they install only after the operator sets `allow_private_endpoints = true`.

After the gate, every user plugin goes through the **fit check**, on every load. A plugin
is *invalid* when it is malformed or unsafe (the gate), and *unsupported* when it is well
formed but needs something this core lacks. An unsupported plugin is refused whole: none of
its models, aliases or unified-model members load, and `plugins install` exits 3. The
message lists every unsupported part:

```text
plugins/qoder.toml: not supported by this core (0router 0.1.0, plugin schema 1-2)
  - plugins/qoder.toml:4:1 category = "oauth": account sign-in is not supported
  - plugins/qoder.toml:5:13 requires[0] = "9router-executor:qoder": needs a provider-specific executor
No part of this plugin was loaded.
```

Unsupported today:

- OAuth or cookie sign-in, even next to an API key;
- wire formats other than `openai`, `claude`, `openai-responses` and `gemini`;
- web search, web fetch and systemone sections;
- quirks, auth hooks, `executor_params`, `credential_fallback`, regions, reasoning
  injection, a provider-specific reasoning format, and URLs filled from account data;
- media formats other than the OpenAI-compatible one;
- any `requires` entry (the generator writes one for each provider 9router serves with its
  own executor or media handler);
- a `schema` newer than 2.

A schema 1 plugin that fits is converted on load: `[transport]` becomes
`[endpoints.text]` (`format` names its wire), each capability endpoint becomes
`[endpoints.<type>]`, and `image_to_text` becomes `vision = true`.

## Replacing a bundled provider

If your file's `id` matches a bundled provider, the bundled one stays active until the
operator decides in `config.toml`. See [operator-config.md](operator-config.md#replacing-a-bundled-provider).
Bundled OAuth client secrets are released only while every OAuth URL of the active plugin
points at the hosts they were issued for. Otherwise `check` reports the credential as
withheld.
