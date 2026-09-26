# Data Model: Provider Entity & Unified Model Registry

**Feature**: [spec.md](spec.md) | **Research**: [research.md](research.md)

Types are written in Rust-flavoured pseudocode: `Option<T>` means the field may be
absent, and absence is kept distinct from an explicit value. On-disk formats are in
[contracts/plugin-schema.md](contracts/plugin-schema.md) and
[contracts/operator-config.md](contracts/operator-config.md).

---

## Overview

```text
Registry (one immutable snapshot, swapped atomically on reload)
├── providers:        ProviderIdx → ProviderEntity      (bundled ∪ user, after conflicts)
├── alias_index:      token → ProviderIdx               (ids + aliases; unique)
├── unified_models:   name → UnifiedModel               (from operator config)
├── settings:         ProviderId → ProviderSettings     (from operator config)
├── credentials:      ProviderId → ResolvedCredential   (bundled table ∩ host binding)
└── report:           LoadReport                        (conflicts, withheld credentials, skipped plugins)

ProviderEntity ──< Model                   (one catalogue per provider)
ProviderEntity ──< CapabilitySection       (one per offered capability kind)
UnifiedModel   ──< Member ──> ProviderEntity + Model id
```

---

## ProviderEntity

One provider, declared by one plugin file (FR-001, FR-002).

| Field | Type | Rule |
|---|---|---|
| `id` | `ProviderId` (string) | **Required.** Lowercase kebab-case `[a-z0-9][a-z0-9-]*`, must not contain `/`. Unique across active entities |
| `category` | `Category` | **Required.** One of `apikey`, `oauth`, `freeTier`, `free`, `webCookie` (FR-006) |
| `alias` | `Option<String>` | Canonical short alias (9router `alias`). Same character rule as `id`. Defaults to `id` for the id-to-alias view |
| `aliases` | `Vec<String>` | Extra lookup tokens. Each token must be unique across all ids and aliases |
| `ui_alias` | `Option<String>` | Display-only badge. **Not** a lookup token (9router parity) |
| `auth` | `Option<AuthDecl>` | See below |
| `transport` | `Option<Transport>` | Absent = catalogue-only provider (38 bundled) |
| `transports` | `Vec<Transport>` | Extra endpoints by format (9 bundled). Only allowed if `transport` is set |
| `oauth` | `Option<OAuthDecl>` | Public OAuth endpoint data only |
| `models` | `Option<Vec<Model>>` | `None` = catalogue unknown; `Some([])` = offers no models (bundled `zed`) |
| `passthrough_models` | `bool` (default false) | Any model id is valid and resolves to itself |
| `version_separator_tolerance` | `bool` (default false) | Enables dash↔dot model id matching (FR-020). Set only by bundled `kiro` |
| `capabilities` | `Map<CapabilityKind, CapabilitySection>` | Present only for offered capabilities (FR-003) |
| `display` | `Option<Display>` | UI metadata carried for the future dashboard. Not interpreted by this slice |
| `source` | `PluginSource` | Set by the loader, not the file: `Bundled` or `User(path)` |

**Validation**:
- A capability section with no endpoint of its own requires `transport` to be set
  (edge case: unreachable section).
- `version_separator_tolerance` and `passthrough_models` both being `true` is allowed:
  passthrough wins, and tolerance only affects catalogue hits.

### AuthDecl

| Field | Type | Rule |
|---|---|---|
| `kind` | `Option<AuthKind>` | `apikey` \| `oauth` (9router `authType`) |
| `modes` | `Vec<AuthKind>` | 9router `authModes` |
| `no_auth` | `bool` | Needs no credentials |
| `has_oauth` | `bool` | Exposes an OAuth flow |
| `header` / `scheme` | `Option<String>` / `Option<AuthScheme>` | Where the core puts the credential: `bearer` \| `raw` |
| `hooks` | `Vec<AuthHook>` | **Closed enum** of core built-ins: `cline_headers`, `kimi_headers`. Unknown name → rejected |
| `credential_fallback` | `Option<ProviderId>` | Reuse another provider's user credential (bundled `ollama-search`). Must reference an existing provider |

### Transport

Typed fields for everything the core interprets. `executor_params` holds long-tail,
executor-specific values.

| Field | Type | Rule |
|---|---|---|
| `base_url` | `Option<Url>` | https or http. No userinfo, no secret query keys (R5) |
| `base_urls` | `Vec<Url>` | Fallback URL list (e.g. kiro) |
| `format` | `Option<WireFormat>` | Closed enum of the 13 values in the bundled set: `openai`, `openai-responses`, `claude`, `gemini`, `gemini-cli`, `vertex`, `antigravity`, `kiro`, `cursor`, `commandcode`, `ollama`, `grok-web`, `perplexity-web`. Composed view fills `openai` when absent (9router `buildTransport`) |
| `headers` | `OrderedMap<String, String>` | Header-name denylist (R5). Order kept for parity |
| `url_suffix`, `validate_url`, `models_url`, `responses_url`, `messages_url`, `chat_path`, `user_url`, `billing_url`, `refresh_url`, `token_url`, `auth_url` | `Option<Url or String>` | URL fields: same URL checks as `base_url` |
| `force_stream` | `Option<bool>` | |
| `timeout_ms`, `stall_timeout_ms` | `Option<u64>` | |
| `retry` | `Option<Map<String, u32>>` | Keys are HTTP status codes or `default`. Carried for the execution slice |
| `quirks` | `Set<Quirk>` + `claude_supported_tool_types: Vec<String>` + `force_auto_tool_choice_models: Vec<String>` | **Closed enum** of the 8 observed quirk names |
| `thinking_format` | `Option<String>` | |
| `reasoning_inject` | `Option<Map<String, Scalar>>` | |
| `usage` | `Option<Map<String, Scalar or Url>>` | Usage-reporting endpoints |
| `regions`, `default_region` | `Option<Map<String, Url>>`, `Option<String>` | `default_region` must be a key of `regions` |
| `auth` | `Option<TransportAuth>` | Per-transport override (multi-endpoint providers) |
| `executor_params` | `Map<String, Scalar>` | `cliVersion`, `clientVersion`, `apiClient`, `copilot.*`, `clientIdentifier`, `tokenAuth`, `noAuth`, `authType`. Key denylist (R5) |

`client_id`, `client_secret`, and `token_url` are **never** read from a transport block.
The composed view takes `client_id`/`token_url` from `oauth` and `client_secret` from the
credential table (R5).

### OAuthDecl

| Field | Type | Rule |
|---|---|---|
| `client_id` | `Option<String>` | Public |
| `authorize_url`, `token_url`, `refresh_url`, `device_code_url`, `user_info_url` | `Option<Url>` | These, plus `endpoints`, define the provider's **OAuth host set** (FR-012a) |
| `scopes` | `Vec<String>` | 9router `scope` (string) and `scopes` (array) normalise here |
| `code_challenge_method` | `Option<String>` | |
| `refresh_lead_ms` | `Option<u64>` | |
| `endpoints` | `OrderedMap<String, Url>` | Long-tail provider URLs (`apiBaseUrl`, `stateUrl`, `ssoOidcEndpoint`, …) |
| `params` | `OrderedMap<String, Scalar>` | Long-tail non-URL values. Key denylist (R5) |

### Model

One entry in a provider's catalogue (FR-004, FR-005).

| Field | Type | Rule |
|---|---|---|
| `id` | `String` | **Required.** Unique within the provider. Must not end in a thinking suffix unless it is a declared preset (see `upstream_id`) |
| `name` | `Option<String>` | Absent → derived display name (9router `deriveModelName`) |
| `kind` | `Option<ModelKind>` | `None` = no declared type (FR-004). 9router `type` normalises here |
| `upstream_id` | `Option<String>` | May carry a preset thinking suffix |
| `target_format` | `Option<WireFormat>` | |
| `supported_formats` | `Option<Vec<WireFormat>>` | |
| `quota_family` | `Option<String>` | |
| `strip` | `Option<Vec<ContentKind>>` | `image`, `audio`, … |
| `context_length`, `max_output_tokens`, `dimensions` | `Option<u64>` | |
| `rate_multiplier` | `Option<f64>` | |
| `capabilities` | `Option<Map<String, bool>>` | Model feature flags (not `CapabilityKind`) |
| `params` | `Option<Map<String, Scalar>>` | Key denylist (R5) |
| `description` | `Option<String>` | |

### CapabilitySection

| Field | Type | Rule |
|---|---|---|
| `kind` | `CapabilityKind` | `llm`, `image`, `image_to_text`, `video`, `tts`, `stt`, `embedding`, `web_search`, `web_fetch`, `systemone` (every 9router `serviceKinds` value). `#[non_exhaustive]` so kinds can be added without breaking existing plugins |
| `endpoint` | `Option<SectionEndpoint>` | `base_url`, `auth_type`, `auth_header`, `format`, `headers`, `method`, `timeout_ms`, `default_model`, `poll_url`, `body_fields`, `model_map` |
| `models` | `Option<Vec<SectionModel>>` | Capability-owned lists (e.g. TTS voices, embedding models with `dimensions`). The 8 synthetic TTS tables land here |
| `limits` | `Option<Map<String, Scalar>>` | Search/fetch knobs: `cost_per_query`, `free_monthly_quota`, `max_max_results`, … |
| `hidden` | `bool` | 9router `hiddenKinds` |

**Catalogue view**: `section(kind).catalogue()` = provider models whose `kind == Some(kind)`,
plus `section.models`. For `llm`, it also includes models with `kind == None`: an
untyped model served by the main transport is listed under text but keeps `kind = None`,
so the "no declared type" answer (FR-004) is unchanged.

---

## Credential table

| Field | Type | Rule |
|---|---|---|
| `provider_id` | `ProviderId` | Key |
| `client_secret` | `SecretString` | `Debug`/`Display` print `***`. Not `Serialize` |
| `bound_hosts` | `Set<Host>` | OAuth host set of the bundled plugin it came from; if that plugin declares no OAuth URLs (gemini), the matching 9router `OAUTH_ENDPOINTS` entry (R5) |

**ResolvedCredential** (per snapshot): `Available(&secret)` if the active plugin for that
id has an OAuth host set ⊆ `bound_hosts`. Otherwise `Withheld { offending_url }`, which is
recorded in `LoadReport` (FR-012a).

No function in the plugin-facing API returns a credential. Only the composed transport
view, used by parity tests and later the executor, can see it.

---

## Operator state

### UnifiedModel (FR-014, FR-015, FR-016)

| Field | Type | Rule |
|---|---|---|
| `name` | `String` | Unique. Must not contain `/` (FR-014). Must not be empty |
| `kind` | `Option<ModelKind>` | If set, every member with a declared kind must match |
| `members` | `Vec<Member>` (≥ 1) | Ordered. No provider twice |

### Member

| Field | Type | Rule |
|---|---|---|
| `provider` | token | Resolved via the alias index at load. Unknown → error naming the member |
| `model` | `String` | Must be in the provider's catalogue (suffix-stripped, tolerance applied) unless the provider is passthrough |
| *(derived)* `upstream_id` | `String` | Resolved at load with the FR-021 algorithm |

### ProviderSettings

| Field | Type | Default |
|---|---|---|
| `allow_uncatalogued_models` | `bool` | `true` (Clarify Q1) |

Settings are keyed by provider id and live outside plugins, so replacing a plugin
keeps them (Key Entities).

### PluginDecision

| Field | Type | Rule |
|---|---|---|
| `provider_id` | `ProviderId` | Must be a bundled id |
| `decision` | `Replace` \| `Decline` | |

---

## Resolution results

```text
resolve(target: &str) -> Result<Resolution, NotFound>

Resolution::Direct  { provider: &ProviderEntity, requested: String, upstream_id: String,
                      catalogued: bool }                           // FR-017
Resolution::Unified { model: &UnifiedModel, members: &[ResolvedMember] }  // FR-016

NotFound::Provider { token }                     // FR-018
NotFound::UnifiedModel { name }                  // FR-014a (bare name)
NotFound::Model { provider, model }              // uncatalogued + setting off
NotFound::EmptyTarget
```

Model queries (FR-022) take `(provider token, model id)` and return a `ModelInfo` view.
Every field is an `Option`, so "not declared" can be told apart from a declared value.

---

## Lifecycle and state transitions

### Plugin conflict (FR-013)

```text
                 user plugin with bundled id appears
   (no conflict) ─────────────────────────────────────▶ Pending
                                                        │  active = bundled
                                                        │  report: conflict pending
                  config: decision = replace            │   config: decision = decline
          ┌─────────────────────────────────────────────┴──────────────────────┐
          ▼                                                                    ▼
      Replaced                                                             Declined
  active = user plugin                                             active = bundled
  credential: bound-host check                                     user plugin ignored (reported)
          │                                                                    │
          └──── user plugin removed / decision removed ──▶ (no conflict) ◀─────┘
```

Two **user** plugins with the same id, or an alias token claimed by two active entities:
**Error**. At startup, both user plugins are skipped and reported. On reload, the whole
reload is rejected.

### Registry snapshot (FR-024 – FR-026)

```text
 Startup:  bundled (must all validate, else fatal) + user plugins (invalid ones skipped)
           + config.toml (invalid → fatal: no previous snapshot to fall back to)
           → Snapshot v1 → Active

 Reload:   build candidate from disk ─▶ validate everything
                    │ any error                          │ ok
                    ▼                                    ▼
          Active stays vN; errors returned      store(vN+1): atomic swap
                                                in-flight holders of vN finish on vN
```

Reload validation checks everything startup does, and in addition:
- no user plugin may be invalid;
- every unified model member must still resolve (edge case: removed provider).
