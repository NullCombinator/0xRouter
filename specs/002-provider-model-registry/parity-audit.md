# Parity audit: lookup.rs and views.rs (T066)

Protocol: `/rust-parity-audit`. Date: 2026-09-27. Oracle: `ref/9router@39e36d3`.

## Scope

| Rust | JS | Purpose |
|---|---|---|
| `crates/zerorouter-registry/src/lookup.rs` | `open-sse/config/providerModels.js`, `providers/models/schema.js`, `providers/models/namePatterns.js` | Model lookup, thinking suffix, version-separator tolerance, upstream id, derived names |
| `crates/zerorouter-registry/src/views.rs` | `open-sse/providers/index.js` (`buildTransport`, `PROVIDER_ID_TO_ALIAS`), `tests/__baseline__/verify-oauth-urls.mjs` | Composed transport, alias maps, OAuth URL groups |

Side effects: none on either side. Both are pure reads of static data.

## Interface parity

| JS | Rust | Notes |
|---|---|---|
| `findModel(models, id, alias)` | `Catalog::find` | Takes the first index that matches either the full id or the stripped base. Normalised fallback only for `tolerant` providers ✓ |
| `isValidModel` | `ModelInfo::declared`, `Registry::resolve` | Passthrough is a plugin flag, not a caller-supplied set (R10) |
| `findModelName` | `ModelInfo::name` | ✓ after fix #1 |
| `getModelType` | `ModelInfo::kind` | `None` when there is no kind, as JS gives `null` ✓ |
| `getModelTargetFormat` / `getModelSupportedFormats` / `getModelQuotaFamily` / `getModelStrip` | `ModelInfo` fields | Raw `Option`, with no defaults filled in (R10). Muse Spark branch excluded (spec Assumptions) |
| `getModelUpstreamId` | `Catalog::find_with_upstream`, `upstream_id` | Suffix split, second strip inside `findModel`, empty `upstreamModelId` fallback, and preset suffix ✓. Codex review branch excluded |
| `normalizeModelId` | `normalise_version_sep` | Non-overlapping, ASCII digits ✓ |
| `deriveModelName`, `titleCase` | `derive_model_name`, `title_case` | Same patterns in the same order ✓ |
| `getProviderModels`, `getDefaultModel`, `getModelsByProviderId` | `Registry::catalog` | The first catalog entry is the default |
| `buildTransport` | `Registry::composed_transport` | `format` defaults to `openai`. `clientId`, `tokenUrl` and `clientSecret` are injected only when absent. The secret is bound to its hosts (FR-012a) and never serialised |
| `PROVIDER_ID_TO_ALIAS` | `Registry::id_to_alias` | `mimo-free` deviation (R9) |
| OAuth URL groups | `Registry::oauth_urls_view` | All 5 groups match `oauth-urls.json` ✓ |

## Error contracts

Every JS function here is total: it returns `undefined`, `null` or the input. The Rust side
never panics on input. Lookups return `Option`, `upstream_id` always returns a `String`, and
`composed_transport` returns `None` for catalog-only providers and unknown tokens. Unknown
provider tokens give `NotFound::Provider`, where JS falls back to returning the input
(R10, FR-018).

## State, streaming, config, translator

- **State**: none. `Catalog` is built once per snapshot and is immutable.
- **Streaming**: n/a.
- **Config**: `DOT_VERSION_PROVIDERS` is now the `version_separator_tolerance` flag, set only on `kiro`, which covers both `kr` and `kiro` ✓. `MODEL_DEFAULTS` is deliberately not applied (R10).
- **Translator**: n/a.

## Findings

| # | File | Pattern | Finding | Severity | Status |
|---|---|---|---|---|---|
| 1 | `registry.rs` (`ModelInfo::name`) | Interface parity | An empty declared `name` gave `""`. JS `found?.name \|\| modelId` gives the requested id | Low | Fixed |
| 2 | `lookup.rs` (`title_case`) | Interface parity | Words are split on `char::is_whitespace` rather than the JS `\s` set, which differ on U+0085 and U+FEFF. `(?i)` in regex-lite is ASCII-only, while JS `/i` folds Unicode | Low | Accepted: model ids are ASCII |
| 3 | `lookup.rs` (`title_case`) | Interface parity | `charAt(0)` works on UTF-16 code units, so JS splits an astral first character, while Rust uppercases the whole scalar value | Low | Accepted: model ids are ASCII |

There are no Critical, High or Medium findings. The fixture-backed parity tests pass: 83
transports, 113 alias tokens, 5 OAuth groups and 2493 lookup rows.
