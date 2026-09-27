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

---

# Parity audit, round 2: everything else that copies 9router behaviour

Date: 2026-09-27. Gates first: `cargo fmt --all --check`, `cargo clippy --workspace
--all-targets -- -D warnings` and `cargo test --workspace` all pass.

## Scope

| Rust | JS | Purpose |
|---|---|---|
| `resolve.rs` (`Registry::resolve`) | `services/model.js` (`parseModel`, `getModelInfoCore`) | Target classification |
| `registry.rs` (alias index, `catalog`, `model`) | `services/model.js` (`ALIAS_TO_PROVIDER_ID`, `MEDIA_ONLY_ALIASES`), `providers/index.js` (`PROVIDER_MODELS`) | Token → provider, provider → catalog |
| `schema/model.rs` (`Entry`) | `providers/models/schema.js` (`normalizeModel`) | Bare-string model entries, name fallback |
| `schema/enums.rs`, `oauth_params.rs`, `section_formats.rs` | registry `category`, `serviceKinds`, `strip`, `executors/default.js` `HEADER_HOOKS`, `providers/schema.js` `PROVIDER_DEFAULTS` | Closed value sets |
| `credentials/` | `buildTransport` `clientSecret` injection | Bundled OAuth client secrets |

Out of scope: `tools/gen-bundled/generate.mjs` is JavaScript that reads 9router directly.
Its output is checked end to end by the fixture-backed parity tests.

## Checks

- **Target parsing**: the target is split at the first `/`, as `indexOf("/")` does ✓. The
  empty target, `/x`, `x/`, bare names and unknown tokens return not-found. These are
  intentional deviations (R10, FR-014a, FR-018).
- **Alias index**: id, `alias` and `aliases[]` map to the id ✓. The media-only aliases (`el`,
  `jina`, `polly`) are plugin `alias` fields and are covered by the 117-token oracle ✓.
  JS object assignment lets the last registry entry win, while 0router rejects clashes at
  load. The one bundled clash, `mmf`, is the R9 deviation.
- **Closed sets**:
  - Categories (`apikey`, `oauth`, `free`, `freeTier`, `webCookie`), `HEADER_HOOKS`
    (`kimiHeaders`, `clineHeaders`, `kilocodeOrg`) and `serviceKinds` (10 values) match
    9router exactly ✓.
  - The default transport format is `openai`, as in `PROVIDER_DEFAULTS.format` ✓.
  - `strip` has only `image` and `audio` as content kinds. The other `strip:` lists in
    9router are tool-name filters, not model entries.
- **normalizeModel**: a bare string becomes `{ id }`, and the name is derived only when it
  is absent ✓. An empty name stays empty, as `name !== undefined` does. `findModelName`
  then falls back to the requested id (round 1, fix #1) ✓.
- **Credentials**: every composed transport that 9router gives a `clientSecret` gets the
  same secret (checked with `matches` in the 83-transport test) ✓. Host binding
  (FR-012a) is 0router-only.

## Findings

| # | File | Pattern | Finding | Severity | Status |
|---|---|---|---|---|---|
| 4 | `registry.rs` (catalogs) | Interface parity | 9router keys `PROVIDER_MODELS` by `alias \|\| id`, so a lookup by provider id or by an `aliases[]` entry finds no models. 0router reaches the catalog from any token. The request path is unaffected: `chatCore.js:81` always converts the provider id to its alias before looking anything up, and `grok-cli.js` tries both keys | Low | Accepted |
| 5 | `registry.rs` (alias index) | State parity | JS alias map: the last writer wins on a clash. 0router rejects clashes at load (FR-013) | Low | Accepted: no bundled clashes remain (R9) |

There are no Critical, High or Medium findings, and no code changes were needed in round 2.
