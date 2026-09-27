# Contract: Outbound request construction and 9router parity

**Feature**: [spec.md](../spec.md) FR-013–FR-017, SC-001 | **Research**:
[R2](../research.md#r2-which-providers-are-executable-fr-013),
[R3](../research.md#r3-transport-and-target-format-choice-fr-014),
[R6](../research.md#r6-outbound-url-and-headers-fr-015-fr-015a),
[R7](../research.md#r7-timeouts-fr-017),
[R8](../research.md#r8-outbound-body),
[R14](../research.md#r14-parity-oracle-extension)

This is the contract between `outbound::build` and the 9router oracle. Every rule below
is checked by a `parity` test against a generated fixture.

## Inputs

`(provider, effective transport, upstream model, connection, stream, client_tool,
client headers)`

## URL

```text
url = transport.base_url
url = url + transport.url_suffix           (if set)
url = url.replace("{accountId}", conn.account_id)
```

**Fixture**: `executor-requests.json`, field `url`, for every executable provider ×
transport × stream.

## Headers (not a native pair)

The header map is built in this order. Later entries with the *exact same* name replace
earlier ones. Names that differ only in case are **both** kept and merged on the wire.

1. `Content-Type: application/json`
2. Each `transport.headers[k] = v`, in declaration order.
3. The auth header:

   | Descriptor | Header |
   |---|---|
   | `transport.auth` declared | `<auth.header>: <auth.scheme ? scheme + " " : "">KEY` |
   | none, format `claude` | `x-api-key: KEY` |
   | none, other format | `Authorization: Bearer KEY` |

4. If the descriptor has `anthropic_version` (or it is the claude fallback), and there
   is no header named exactly `anthropic-version`, add `anthropic-version: 2023-06-01`.
5. If streaming, `Accept: text/event-stream`.

**Comparison view**: the lowercase name to comma-joined value, exactly as
`new Headers(obj)` iterates.

**Credential**: the fixture carries `<KEY>` and `<ACCT>` placeholders. The test runs
with the same placeholders and compares the view exactly. SC-001's "credentials compared
by position" means the placeholder must appear in the same header, and only there.

**Fixture**: `executor-requests.json`, fields `headers` (object form) and `wire_headers`
(merged form).

## Headers (native pair)

```text
wire = generic_view
for (name, value) in client_headers:
    if name in {authorization, x-api-key, host, content-length, accept-encoding,
                connection, keep-alive, proxy-authenticate, proxy-authorization,
                te, trailer, transfer-encoding, upgrade} ∪ tokens(Connection): skip
    wire[lowercase(name)] = value            # replaces declared value
wire[auth_header] = auth_value               # re-applied last
```

**Fixture**: `client-detect.json` decides `native_pair`. The overlay itself is 0router
behaviour (R15 D5), tested by a unit test against the formula above.

## Transport choice

**Fixture**: `transport-choice.json`. For each `(provider, client_format,
model.supported_formats?, model.target_format?)`, it gives the expected `target_format`
and the transport index used, where `-1` means the primary transport.

## Body

- **Chat**: the client's top-level object, with `model` set to the upstream id and,
  when `force_stream && !client_stream`, `stream: true`. Nested bytes are unchanged.
  This is 0router behaviour (R15 D2, D8), tested by unit tests.
- **Embeddings**: `embeddings.json` gives the expected `{url, headers, body}` for each
  embeddings-executable provider over an input corpus that includes a string, an array,
  `encoding_format`, and `dimensions` values `0`, `-1`, `"8"`, `1024`, and `NaN`.

## Timeouts

**Fixture**: `timeouts.json`, `envMs(name, default)` over values `""`, `"0"`, `"-5"`,
`"120000"`, `"120000abc"`, `" 42"`, `"abc"`, `"1e3"`, and `"9007199254740993"`.

The per-provider values come from the registry: transport `timeout_ms` and
`stall_timeout_ms`.

## Responses

| Fixture | Rule |
|---|---|
| `upstream-errors.json` | status × body gives the client body (FR-020, not a native pair) |
| `non-sse.json` | status × content-type × body gives the client body (FR-020b) |
| `stream-errors.json` | client format × message gives the closing frame bytes (FR-018a) |
| `sse-to-json.json` | SSE text gives the assembled JSON (FR-019). Corpus events carry `id` and `created`, so the output is deterministic |
| `usage.json` | event sequence gives the merged usage, mapped to `{input, output, cache_read, cache_write}` with `null` for absent (R11) |
| `sessions.json` | headers × body gives the session or `null` (SC-010) |
| `count-tokens.json` | body gives `input_tokens` (SC-011) |
| `executable-providers.json` | the sorted chat and embeddings sets (R2) |

## Regeneration

```bash
node tools/gen-bundled/generate.mjs     # writes plugins, credentials, and all fixtures
```

Every fixture header records `ref/9router`'s SHA. Fixtures are never hand-edited.
