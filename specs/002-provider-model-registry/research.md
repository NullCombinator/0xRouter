# Research: Provider Entity & Unified Model Registry

**Feature**: [spec.md](spec.md) | **Plan**: [plan.md](plan.md) | **Date**: 2026-09-26

All findings below were verified on 2026-09-26 against `ref/9router` at commit `39e36d3`
(v0.5.86). Numbers come from running 9router's own modules in Node. They were not
regex-scraped.

---

## R1. Toolchain availability

**Finding**: `rustc`, `cargo`, and `rustup` resolve to `~/.cargo/bin/`, but the
`claude-0router` Landlock gate denies access to `~/.cargo` and `~/.rustup`. Nothing can be
compiled from this identity today.

**Decision**: Treat as an implementation prerequisite, not a design question. Before
`/speckit-implement`, the gate needs read+execute on `~/.rustup` and `~/.cargo/bin`. It
also needs read+write on a cargo home, either `~/.cargo/registry` and `~/.cargo/git`, or a
project-local `CARGO_HOME` under `~/Desktop/0router/.cargo-home`. The
local option keeps the gate narrower. The gate and `identity/` are security configuration,
so the change is left to the user.

**Alternatives considered**: System-wide toolchain under `/usr` (not installed); vendoring
a toolchain into the repo (large, and still needs a registry cache for crates).

---

## R2. Language, edition, crate layout

**Decision**: Rust stable, edition 2024 (MSRV 1.85). Cargo workspace at repo root. This
slice adds one library crate, `zerorouter-registry`, plus a thin binary crate,
`zerorouter-cli`, used for the validation and resolve checks in quickstart.

**Rationale**: Constitution requires Rust. The crate name can't start with a digit (a 001
error). The registry is pure data plus lookups, so it takes **no async runtime**. Reads
are lock-free (R6) and the reload path does blocking file I/O. The future server slice
calls reload through `tokio::task::spawn_blocking`, which satisfies the "no blocking on
the executor" constraint.

**Alternatives considered**: A single crate with a `bin` target. Rejected because later
slices (routing, execution) depend on the library without pulling the CLI's dependencies.

---

## R3. Plugin file format and strict parsing

**Decision**: TOML via `toml` + `serde`. Every plugin struct uses
`#[serde(deny_unknown_fields)]`. Errors carry the field path (`serde_path_to_error`) and
the byte span (`toml`'s span info), rendered as `file:line:col field.path: rule`.

**Rationale**: FR-008 requires rejecting unknown fields. 001's research chose "lenient
parsing with defaults", which is incompatible with FR-008 and dropped here. Strictness
also means a plugin can only say what the schema lets it say. That is the basis of the
secret guarantee (R5).

**Alternatives considered**: JSON Schema validation (extra dependency, worse errors), and
lenient parsing plus an unknown-key warning. That second option fails FR-008.

---

## R4. What the schema must carry (registry census)

Census of all 121 active registry entries. One file, `devin-cli.js`, is commented out of
`registry/index.js` and is not a bundled plugin.

| Area | Observed |
|---|---|
| Top-level keys | 37 distinct. UI keys: `display`, `priority`, `uiAlias`, `hidden`, `hasFree`, `authHint`. Capability keys: `serviceKinds`, `ttsConfig`, `sttConfig`, `embeddingConfig`, `imageConfig`, `videoConfig`, `searchConfig`, `searchViaChat`, `fetchConfig`, `systemoneConfig`, `hiddenKinds`, `mediaPriority`, `modelsFetcher` |
| Transport | 83 truthy (9 more have `transport: undefined`); 37 distinct keys; 9 providers also have a `transports[]` multi-endpoint array |
| Quirks | 8 distinct flags: `preserveCacheControl`, `dropClientMetadata`, `clineEnvelope`, `dropOutputConfig`, `requireClaudeToolType`, `cloakToolsOnOAuth`, `claudeSupportedToolTypes`, `forceAutoToolChoiceModels` |
| Auth hooks | Named strings only: `clineHeaders`, `kimiHeaders` |
| OAuth | 21 providers; ~70 distinct keys, of which ~10 are common (`tokenUrl`, `authorizeUrl`, `refreshUrl`, `userInfoUrl`, `clientId`, `scopes`, `deviceCodeUrl`, …) and the rest provider-specific URLs and flags |
| Models | 935 entries. Keys: `id`, `name`, `kind` (191), `params`, `supportedFormats`, `contextLength`, `upstreamModelId` (49), `targetFormat` (38), `capabilities`, `rateMultiplier`, `description`, `quotaFamily` (8), `strip` (2), `thinking`, `imageGen`, `dimensions`, `maxOutputTokens` |
| Model kinds | none 744, image 92, embedding 34, tts 29, stt 22, video 10, systemone 4 |
| Categories | apikey 78, oauth 19, freeTier 18, free 4, webCookie 2 |
| Passthrough | 14 providers; `zed` is the only explicit `models: []` |
| Header names | 39 distinct, none credential-bearing (no `authorization`, `x-api-key`, `cookie`) |

**Decision**:
- Typed structs cover every common key.
- Things that name core behaviour (quirks, auth hooks, capability kinds, formats) are
  closed enums of **named built-ins**. The validator rejects unknown names. A plugin can
  only select behaviour the core already has, never supply its own. This is how FR-009
  is met structurally.
- Long-tail, provider-specific values go in three typed maps:
  - `oauth.endpoints` (name → URL) and `oauth.params` (name → string, number, or bool)
    carry OAuth values.
  - `transport.executor_params` carries executor-specific values.
  - These maps still pass the secret checks in R5.
- The generator (R8) must map every observed key. If it finds a key it doesn't
  recognise, the build fails. It never drops the key silently.

**Alternatives considered**: One fully typed struct per OAuth flow. That is premature,
since OAuth flows are out of scope; the OAuth slice will promote map entries to typed
fields.

---

## R5. Keeping secrets out of plugins

**Findings**:
- Hardcoded `clientSecret` appears in `registry/{antigravity,gemini-cli,gemini,iflow,trae}.js`
  and in `providers/shared.js` constants. **trae is commented out of `registry/index.js`**,
  as is devin-cli, so neither is bundled. That leaves 4 active secrets:
  - antigravity, gemini-cli, and gemini keep theirs in `transport`;
  - iflow keeps its in `oauth`, and `buildTransport` injects it into the transport.

  That is why `providers-baseline.json` embeds exactly 4.
- `gemini` declares **no** OAuth URLs. Its secret goes to Google's OAuth hosts, which
  9router hardcodes outside the registry (`config/appConstants.js` `OAUTH_ENDPOINTS.google`).
  antigravity and gemini-cli declare `oauth2.googleapis.com` and `accounts.google.com`.
  iflow declares `iflow.cn`.
- 9router's `OAUTH_INJECT_FIELDS` = `clientId`, `clientSecret`, `tokenUrl` are copied
  from `oauth` into `transport` when absent.

**Decision**: Defence in depth, in this order:
1. **Structural.** No schema field exists whose purpose is a secret, and
   `deny_unknown_fields` means one can't be added.
2. **Closed maps.** Where the bundled set shows a fixed set of keys, the map is closed:
   - `transport.executor_params` is a typed struct (data model). Its field `token_auth`
     holds an auth-mode name (grok-cli: `"xai-grok-cli"`), not a credential.
   - `oauth.params` keys must be in `KNOWN_OAUTH_PARAMS`, which the generator writes
     from the bundled set. Some keys there contain "token" (e.g. gitlab `tokenUrlPath`)
     but hold paths or names, not credentials. A closed list is stricter than a
     denylist, and a key the core doesn't know would do nothing anyway.
3. **Open maps.** The keys of `headers` and model `params` are checked against a
   denylist, case-insensitively:
   - `secret`, `password`, `passwd`, `api_key`/`apikey`, `access_token`, `refresh_token`,
     `cookie`, `authorization`, `x-api-key`, `proxy-authorization`, `x-goog-api-key`;
   - any key matching `token` unless it ends in `url`, `_url`, or `endpoint`.

   None of the 39 bundled header names match (verified).
4. **URLs.** Any URL containing userinfo (`user:pass@`) or a query parameter from the
   same denylist is rejected.
5. **Credential table.** Secrets live in a generated Rust source file with a static table
   (R8). Each entry is `{ provider_id, value, bound_hosts }`.
   - `bound_hosts` is the bundled plugin's **OAuth host set**: the hosts of
     `oauth.{authorize,token,refresh}_url` plus `token_url`/`refresh_url`/`auth_url` on
     any transport.
   - `user_info_url`, `device_code_url`, and `oauth.endpoints` are excluded: they never
     receive the secret. Counting them would bar gemini-cli and antigravity from their
     own secret, because `userInfoUrl` is on `www.googleapis.com`.
   - If the bundled plugin declares none (gemini), the generator uses the hosts of
     `OAUTH_ENDPOINTS.google` (`config/appConstants.js`): `oauth2.googleapis.com` and
     `accounts.google.com` (FR-012a).
   - The generator fails if an entry would end up with no hosts.
   - Invariant for the execution slice: the secret is only ever sent to a host in
     `bound_hosts`. Transport `token_url` is part of the set because a replacing
     plugin could otherwise keep `oauth.token_url` and redirect the transport's.
6. **Composition.** The composed transport view (the parity view) re-creates 9router's
   injection:
   - `clientId` and `tokenUrl` are copied from `oauth` only when the transport does not
     declare them. antigravity, gemini-cli, gemini, kimi, and xai declare `clientId` on
     the transport; kiro's transport `tokenUrl` differs from its `oauth.tokenUrl`.
   - `clientSecret` comes from the credential table only when the active plugin's OAuth
     host set is a subset of `bound_hosts`.
7. **No raw secret in the public API.** `SecretString` exposes only `matches(&str)` and a
   `***` rendering. Raw access is `pub(crate)`. Parity tests compare the composed
   `clientSecret` with `matches` against the value in the oracle fixture. They never
   read it out.

These are public "installed-app" OAuth secrets that 9router ships in open source.
Compiling them into the binary does not reduce their secrecy. It does keep them out of
plugin files and out of any plugin-visible interface.

**Alternatives considered**:
- Reading secrets from the operator's config. Rejected: that breaks "works out of the
  box" for bundled OAuth providers.
- Binding by provider id alone. Rejected: this is clarification Q3, option C, which the
  user declined.

---

## R6. Consistent snapshots and atomic reload

**Decision**: `arc_swap::ArcSwap<Registry>`. Lookups call `load_full()` and get an
`Arc<Registry>` they can hold for a whole request (FR-025). Reload builds and validates a
new `Registry` completely off to the side, then `store()`s it in one atomic pointer swap
(FR-024). A `Mutex<()>` makes concurrent reload calls run one at a time. Nothing watches
files (FR-026).

**Rationale**: Reads never wait for a lock, and old snapshots are freed when the last
request holding them finishes. This is the standard pattern for read-heavy config in Rust
services.

**Alternatives considered**: `RwLock<Registry>`. Rejected: readers stall during a swap,
and a request can't safely hold a guard across `.await`.

---

## R7. Operator-owned state location and format

**Decision**: One operator config file plus one user-plugin directory, under a home
directory resolved as `$ZEROROUTER_HOME` if set, otherwise `~/.0router/`:

```
~/.0router/
├── config.toml          # unified models, provider settings, plugin decisions
└── plugins/*.toml       # user plugins (top level only; no recursion)
```

See [contracts/operator-config.md](contracts/operator-config.md).

**Rationale**: This mirrors 9router's `~/.9router` so it feels familiar. One file keeps
reload atomic across all operator state, with a single place to validate. Subdirectories
are not scanned, so a plugin can't be hidden in a nested folder.

**Alternatives considered**: One file per unified model (more files for reload to
coordinate, with no benefit at this scale); SQLite like 9router (state here is small,
edited by hand, and belongs in version control).

---

## R8. Generating bundled plugins from 9router

**Decision**: A Node script, `tools/gen-bundled/generate.mjs`, imports
`ref/9router/open-sse/providers/registry/index.js` (the evaluated registry) and writes:
- `plugins/bundled/<id>.toml`: one file per active registry entry, with secrets removed;
- `crates/zerorouter-registry/src/credentials/bundled.rs`: the static credential table
  with `bound_hosts`;
- `tests/fixtures/9router/*.json`: the parity oracle (R9).

The generated files are committed. Regenerating is a manual step whenever `ref/9router`
is updated, and the commit records the ref SHA it was generated from. At build time, the
bundled TOML is embedded into the binary: `build.rs` writes an `include_str!` table, and
nothing is read from disk at runtime. At load, bundled plugins go through the same
validation function as user plugins (FR-007).

**Rationale**: The registry files compute values from imports (14 files import
`shared.js` constants). The registry must be evaluated, not text-parsed. That is an
error 001 made. Node is already needed for 9router's baseline scripts, and it is a
dev-time tool only, not a runtime dependency. Because the generated TOML is committed,
reviewers can read every bundled provider.

**Alternatives considered**:
- `build.rs` running Node on every build. Rejected: every Rust build would then need
  Node and the ref checkout.
- Hand-written TOML. Rejected: 121 files and 935 models can't be maintained by hand.

---

## R9. Parity oracle

**Findings**:
- `verify-alias.mjs` and `verify-oauth-urls.mjs` pass against the current registry.
- `verify-providers.mjs` **fails**: the committed `providers-baseline.json` is stale by
  one value. `claude.headers.User-Agent` changed from `claude-cli/2.1.258` to
  `claude-cli/2.1.280` when `CLAUDE_CLI_VERSION` was bumped without a re-snapshot.
- `alias-baseline.json` has 117 tokens. 4 of them (`qw`, `dv`, `devin`, `devin-cli`)
  resolve to themselves only because `resolveProviderAlias` echoes unknown input. No
  active provider owns them (devin-cli is disabled; qw has no provider).
- `modelKeys` (100) = 92 provider catalogs keyed by canonical alias, plus 8 TTS
  model/voice tables keyed by synthetic names (`openai-tts-models`, `gemini-tts-voices`,
  …) from `config/ttsModels.js`.
- The lookup functions in `config/providerModels.js` import and run fine in plain Node.
  For example, `getModelUpstreamId("kr","claude-sonnet-4-5(high)")` returns
  `claude-sonnet-4.5(high)` and `getModelType("cc", …)` returns `null`.

**Decision**: The oracle is **regenerated from the pinned ref** by `generate.mjs` into
`tests/fixtures/9router/`. The stale committed baselines are not used as-is. The oracle
files are:
- `providers.json`: the `PROVIDERS` object as it is now;
- `alias.json`: the same probe-token list as `verify-alias.mjs`;
- `oauth-urls.json`: the same shape as `verify-oauth-urls.mjs`;
- `lookup.json`: the new model-lookup oracle. For every (alias, model) in
  `PROVIDER_MODELS`, plus synthetic edge inputs, it records `isValidModel`,
  `getModelUpstreamId`, `getModelType`, `getModelTargetFormat`,
  `getModelSupportedFormats`, `getModelQuotaFamily`, `getModelStrip`, and
  `findModelName`. The edge inputs are: a thinking suffix, a nested paren, a dash/dot
  variant, an undeclared ID, trailing whitespace, and a preset suffix.

The 8 synthetic TTS keys are **not** turned into `lookup.json` rows, because they are
not providers. The generator writes them separately to `tts-tables.json`: synthetic key
→ `{ provider, models }`, with the owning provider taken from `ttsModels.js`.

The Rust parity tests compare the composed registry views against these fixtures. There
are three documented deviations, and they are asserted explicitly:
- the 4 unowned alias tokens return not-found;
- the 8 TTS tables are checked against `capabilities.tts.models` of their owning
  provider, not as top-level keys;
- `mimo-free` has no `mmf` alias, because `mmf` is also the id of another provider.
  `id_to_alias` maps `mimo-free` to itself (`ui_alias = "mmf"` keeps the display name),
  and the three `lookup.json` rows for undeclared models under `mmf` expect not-valid,
  because `mmf` is not passthrough.

The Codex review-suffix and Muse Spark branches in `getModelUpstreamId` and
`getModelTargetFormat` are excluded from the oracle inputs (spec Assumptions).

**Alternatives considered**: Fixing `ref/9router`'s baseline in place. Rejected:
`ref/` is a read-only oracle and should not be edited.

---

## R10. Where 0router deliberately differs from 9router lookups

| 9router | 0router | Source |
|---|---|---|
| `resolveProviderAlias(x)` returns `x` for unknown tokens | not-found | FR-018 |
| Bare model name → user alias map → prefix inference → `openai` | Unified model name, or not-found | FR-014a, Clarify Q2 |
| `DOT_VERSION_PROVIDERS` hardcoded to `{kr, kiro}` | Plugin flag `version_separator_tolerance = true`, set only in bundled `kiro` | FR-020 |
| `modelKind()` defaults to `"llm"` | `Option<ModelKind>`; `None` = no declared type | FR-004 |
| `PROVIDER_MODELS` keyed by alias, with `[]` vs missing collapsed at some call sites | `models: Option<Vec<Model>>` per capability section; explicit `[]` stays distinct | Edge case |
| Quirks as a free-form object | Closed enum of named quirks | R4 |
| `isValidModel` is true for any model when the caller passes the provider in `passthroughProviders` | `passthrough_models` is a plugin flag. A passthrough provider accepts uncatalogued models whatever `allow_uncatalogued_models` says | FR-017 |
| `getModelQuotaFamily` / `getModelStrip` / `getModelTargetFormat` fill in defaults (`"normal"`, `[]`, `null`) | `ModelInfo` fields are `Option`, with no defaults filled in | FR-022 |

---

## R11. Performance

**Decision**: Build lookup indices once per snapshot:
- `HashMap<&str, ProviderIdx>` for ids and alias tokens;
- a per-provider `HashMap<&str, ModelIdx>` for model IDs, plus a second map keyed by the
  normalised ID, only for providers with version-separator tolerance;
- `HashMap<&str, UnifiedIdx>` for unified models.

Resolving a target is then a few hash lookups plus one suffix scan, with no allocation
except the returned upstream-ID string. A Criterion benchmark (`benches/resolve.rs`)
covers direct, unified, suffix, and not-found targets, as the constitution's performance
gate requires.

**Targets**:
- full load of 121 bundled plugins plus a small config in < 50 ms (SC-006 allows 1 s);
- `resolve()` p50 < 1 µs on the dev machine.

These are the baselines future regressions are judged against.

---

## R12. Crates

| Crate | Purpose |
|---|---|
| `serde` (derive) | Plugin and config structs |
| `toml` | Parsing with spans |
| `serde_path_to_error` | Field paths in errors (FR-010) |
| `thiserror` | Typed error enums |
| `arc-swap` | Snapshot swap (R6) |
| `url` | Host extraction, userinfo/query checks (R5) |
| `regex-lite` | The single thinking-suffix pattern `\([^()]+\)\s*$`; no need for full `regex` |
| `indexmap` (serde) | Ordered `headers` / `endpoints` / `params` maps, so parity comparisons keep 9router's key order |
| `serde_json` (dev) | Loading fixtures in parity tests |
| `criterion` (dev) | Benchmarks |
| `clap` (cli only) | `zerorouter-cli` arguments |

No `tokio`, `inventory`, or `once_cell` in the library. Static tables use `std::sync::LazyLock`.

---

## R13. Implementation deviations from these documents (slice 002)

These were found while implementing. The code and the contracts in `docs/` are
authoritative wherever they differ from the earlier sections.

**Schema**, to cover the full bundled census:
- `AuthKind` adds `cookie` and `none`, and `AuthScheme` adds a `cursor` template variant.
- Model `capabilities` and `params` are string lists, not maps. The capability set adds
  `thinking` and `image_gen`.
- Section endpoints carry extra fields.
- New fields: `models_fetcher`, `api_key_hint`, and the display extras.
- Transport `retry` values are a number or `{ attempts, delay_ms }`.
- `usage` values are a string or a list.
- `headers` is optional.
- `anthropic_version` is a bool.
- The `kilocode_org` auth hook is added.
- URLs are stored as `String`, and checked by the gate rather than typed.
- An empty transport `base_url` means the operator supplies it (azure).
- The sentinel `base_url` in sections is dropped.

**Uniqueness**: a model id is unique per `(id, kind)`, not per provider. `gemini-2.5-pro` is
both an `llm` and an `stt` model. A lookup by id returns the first entry, as `findModel`
does.

**`catalogued`** in `Resolution::Direct` and `UnifiedMember` means "found in the catalog".
When it is `false`, the model was forwarded because the provider is passthrough or allows
uncatalogued models.

**CLI**: `validate_user_plugin` runs the gate plus the cross-plugin checks: `credential_fallback`
must name a bundled provider or the plugin itself.

**Lookup**: the thinking-suffix regex is replaced by a backward scan. It is equivalent,
as a unit test checks against the regex, and it uses JavaScript's whitespace set.
`find` and `upstream_id` share one pass, which brings direct resolve from about 5 µs to
about 465 ns.

**Build**: the dev profile optimises dependencies at `opt-level = 2` and the registry at
`opt-level = 1`, so debug loads and the 200-reload test stay fast. `rustfmt.toml` sets
`max_width = 120`.

**Placeholder**: bundled plugins carry `license = "MIT"` as a placeholder until 9router's
licence is confirmed.
