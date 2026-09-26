# Data Model: Config Layer

**Date**: 2026-09-26 | **Status**: Complete

## Core Entities

### ProviderEntry

A provider declaration (built-in or plugin). Represents one upstream AI model service.

**Fields**:
| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | String | Yes | Unique identifier (kebab-case). Examples: "anthropic", "local-ollama", "my-provider" |
| `alias` | String | No | Short lookup key for PROVIDER_MODELS; defaults to `id`. Example: "kr" for "kiro" |
| `category` | String | Yes | Provider category: "apikey", "oauth", "freeTier", "local". Drives UI grouping. |
| `authType` | String | No | "apikey" or "oauth"; hint for auth flow selection |
| `hasOAuth` | Boolean | No | True if provider exposes OAuth flow (default: false) |
| `noAuth` | Boolean | No | True if provider needs no credentials (default: false) |
| `transport` | TransportConfig | Yes | HTTP runtime config (endpoint, auth, headers, format, etc.) |
| `models` | Vec<ModelEntry> | No | Model list; omit for no models, [] for explicit empty |
| `oauth` | OAuthConfig | No | OAuth flow config (required if hasOAuth=true) |
| `media` | MediaConfig | No | Non-LLM services (TTS, STT, embedding, image, etc.) |

**Uniqueness**:
- `id` must be globally unique across built-ins and plugins
- Installation checks prevent plugin `id` from matching an existing plugin or built-in
- Built-in `id` takes precedence; plugin with matching `id` is rejected

**Relationships**:
- ProviderEntry → 1..many ModelEntry (one provider has 0+ models)
- ProviderEntry → OAuthConfig (0 or 1)
- ProviderEntry → MediaConfig (0 or 1)

---

### ModelEntry

One model declared within a provider's model list.

**Fields**:
| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `id` | String | Yes | Declared model identifier. Examples: "claude-sonnet-4-20250514", "gpt-4o" |
| `name` | String | No | Display name; auto-derived from `id` via regex if omitted |
| `upstreamModelId` | String | No | Upstream model ID if different from `id`. Defaults to `id`. |
| `kind` | String | No | Model type: "llm" (default), "embedding", "image", "tts", "stt", etc. |
| `format` | String | No | Wire format override for multi-endpoint providers (e.g., "openai", "claude") |
| `quotaFamily` | String | No | Quota tracking family; default: "normal". Used by router for amortization. |
| `strip` | Vec<String> | No | Content types to drop before forwarding. Examples: ["image", "audio"] |
| `supportedFormats` | Vec<String> | No | Formats this model supports (for multi-endpoint providers). Null = no guard. |

**Validation Rules**:
- `id` must be non-empty
- If `upstreamModelId` is omitted, defaults to `id`
- `kind` from preset enum; unknown kinds treated as custom (allowed)
- `quotaFamily` from preset list; unknown families treated as "normal"
- `strip` entries validated against known content types (optional; unknown types allowed)

**State Transitions**: None (models are static after registry load)

**Relationships**:
- ModelEntry ← ProviderEntry (each model belongs to exactly one provider)

---

### TransportConfig

HTTP runtime configuration for provider communication.

**Fields**:
| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `baseUrl` | String | Yes | API endpoint base URL. Example: "https://api.anthropic.com/v1/messages" |
| `format` | String | No | Wire format: "openai", "claude", "gemini", etc. Default: "openai" |
| `headers` | Map<String, String> | No | Custom HTTP headers. Default: {} |
| `auth` | AuthScheme | No | Auth config (header name, scheme, source). Default: {header: "Authorization", scheme: "bearer", source: ["accessToken", "apiKey"]} |
| `forceStream` | Boolean | No | Force response streaming (default: false) |
| `urlSuffix` | String | No | Suffix appended to baseUrl (default: "") |
| `quirks` | Map<String, Any> | No | Provider-specific behavior overrides (default: {}) |
| `retry` | Map<u16, RetryPolicy> | No | Retry policy by HTTP status (default: DEFAULT_RETRY_CONFIG) |
| `timeoutMs` | u64 | No | Connection timeout in ms (default: FETCH_CONNECT_TIMEOUT_MS = 60_000) |
| `executor` | String | No | Executor type: "default" (OpenAI-compatible), provider-specific name. Default: "default" |
| `clientId` | String | No | OAuth client ID (injected from oauth config; store here for executor access) |
| `clientSecret` | String | No | OAuth client secret (injected from oauth config) |
| `tokenUrl` | String | No | OAuth token endpoint (injected from oauth config) |

**Validation Rules**:
- `baseUrl` must be a valid URL (https or http)
- `headers` keys must be valid HTTP header names
- `timeoutMs` must be > 0 if provided

**Relationships**:
- TransportConfig ← ProviderEntry (each provider has exactly one transport)
- Fields `clientId`, `clientSecret`, `tokenUrl` are injected from OAuthConfig by build-time codegen

---

### OAuthConfig

OAuth 2.0 flow configuration.

**Fields**:
| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `clientId` | String | Yes | OAuth application client ID |
| `authorizeUrl` | String | Yes | Authorization endpoint |
| `tokenUrl` | String | Yes | Token endpoint |
| `deviceCodeUrl` | String | No | Device code endpoint (for device flow) |
| `refreshUrl` | String | No | Refresh token endpoint (if different from tokenUrl) |
| `scope` or `scopes` | String or Vec<String> | No | Requested scopes (default: empty) |
| `redirectUri` | String | No | Redirect URI after auth (default: "http://localhost:PORT/callback") |
| `callbackPath` | String | No | Local callback path (default: "/auth/callback") |
| `fixedPort` | u16 | No | Fixed local port for redirect; if omitted, random port used |
| `codeChallengeMethod` | String | No | "S256" (PKCE) or "plain" (default: "S256") |
| `extraParams` | Map<String, String> | No | Extra query params to /authorize (default: {}) |
| `refreshLeadMs` | u64 | No | Proactive refresh trigger (ms before expiry); zero = reactive refresh |
| `userInfoUrl` | String | No | User info endpoint (for fetching user details post-auth) |

**Validation Rules**:
- `clientId`, `authorizeUrl`, `tokenUrl` must be non-empty
- URLs must be valid
- `fixedPort` if provided must be > 1024 (avoid requiring sudo)

**Relationships**:
- OAuthConfig ← ProviderEntry (optional, 0 or 1 per provider)
- At runtime, fields `clientId`, `clientSecret`, `tokenUrl` are injected into TransportConfig

---

### ErrorRule

One classification rule for upstream error responses.

**Fields**:
| Field | Type | Required | Notes |
|-------|------|----------|-------|
| `textPattern` | String | No | Substring to match in error message (case-insensitive). Example: "rate limit" |
| `statusCode` | u16 | No | HTTP status code to match. Example: 429 |
| `disposition` | ErrorDisposition | Yes | Error handling: Backoff, CooldownMs(u64), or DefaultTransient |
| `priority` | u32 | No | Rule order (lower number = higher priority; implicit: order in vec) |

**Validation Rules**:
- At least one of `textPattern` or `statusCode` must be present
- `statusCode` must be valid HTTP (100–599 range)
- `disposition` must be a valid enum variant
- Text rules are checked before status rules (implicit ordering in ERROR_RULES vec)

**Relationships**:
- ErrorRule ← Config global (rules are not per-provider)

---

### ErrorDisposition

Result of error classification.

**Enum Variants**:
```rust
pub enum ErrorDisposition {
    Backoff,              // Exponential backoff (rate limit)
    CooldownMs(u64),      // Fixed cooldown (capped at 30 minutes)
    DefaultTransient,     // 30-second default cooldown
}
```

**Usage**: Router uses disposition to decide retry strategy:
- `Backoff` → exponential backoff (e.g., 2s, 4s, 8s, …)
- `CooldownMs(1800000)` → freeze provider for 30 minutes
- `DefaultTransient` → 30-second cooldown, then retry

---

### RuntimeConfig

Runtime tunables resolved at startup (env overrides + compiled-in defaults).

**Fields**:
| Field | Type | Default (ms) | Env Var |
|-------|------|----------|---------|
| `streamStallTimeoutMs` | u64 | 360000 | STREAM_STALL_TIMEOUT_MS |
| `streamFirstChunkTimeoutMs` | u64 | 200000 | STREAM_FIRST_CHUNK_TIMEOUT_MS |
| `fetchConnectTimeoutMs` | u64 | 60000 | FETCH_CONNECT_TIMEOUT_MS |
| `geminiNativeTtsTimeoutMs` | u64 | 45000 | GEMINI_NATIVE_TTS_FETCH_TIMEOUT_MS |
| `defaultMaxTokens` | u64 | 64000 | — |
| `defaultMinTokens` | u64 | 32000 | — |
| `cacheUserInfoTtlSec` | u64 | 300 | — |
| `cacheModelAliasTtlSec` | u64 | 3600 | — |
| `defaultRetryConfig` | Map<u16, RetryPolicy> | See [DEFAULT_RETRY_CONFIG](#default-retry-config) | — |
| `skipPatterns` | Vec<String> | See [SKIP_PATTERNS](#skip-patterns) | — |
| `searchxngUrl` | String | "http://localhost:8888/search" | SEARXNG_URL |

**Resolution Logic** (per timeout field):
1. Check environment variable (if set and valid positive integer, use it)
2. Otherwise, use compiled-in default
3. Invalid env values (non-numeric, ≤0, unparseable) are silently ignored

**Relationships**: Global singleton (one instance per process)

---

### DEFAULT_RETRY_CONFIG

Retry policy table by HTTP status code.

| Status | Attempts | Delay (ms) | Notes |
|--------|----------|-----------|-------|
| 429 | 0 | 0 | Rate limit: use backoff, not retry |
| 502 | 3 | 3000 | Bad gateway: transient upstream error |
| 503 | 3 | 2000 | Service unavailable |
| 504 | 2 | 3000 | Gateway timeout |
| (default) | 0 | 0 | Unknown status: use error rule disposition |

---

### SKIP_PATTERNS

Request text patterns that bypass provider routing (e.g., title generation prompts).

**Values** (compiled-in):
```
[
  "Please write a 5-10 word title for the following conversation:"
]
```

---

### MediaConfig

Non-LLM services (TTS, STT, embedding, image, search, etc.).

**Fields** (abbreviated; full schema in ref/9router):
| Field | Type | Purpose |
|-------|------|---------|
| `serviceKinds` | Vec<String> | Services provided: "tts", "stt", "embedding", "image", "search", etc. |
| `ttsConfig` | TtsServiceConfig | TTS endpoint, format, models, voice support |
| `sttConfig` | SttServiceConfig | STT endpoint, format, models |
| `embeddingConfig` | EmbeddingServiceConfig | Embedding endpoint, format, model |
| `imageConfig` | ImageServiceConfig | Image gen endpoint, format, models, modelMap (optional) |
| `hiddenKinds` | Vec<String> | Services present but hidden from UI |

**Validation Rules**:
- `serviceKinds` entries must be from a known enum (optional: custom kinds allowed)
- Each service config (tts, stt, etc.) must be well-formed if present

**Relationships**:
- MediaConfig ← ProviderEntry (optional, 0 or 1)

---

## Key Invariants

1. **Provider ID Uniqueness**: No two ProviderEntries (built-in + plugins) share the same `id`.
   - Enforced at plugin install (reject duplicates; user-prompted replace for existing plugins)
   - Built-in always wins against plugin

2. **Model ID Within Provider**: Model IDs within a single provider are scoped (no global uniqueness requirement);
   same model ID can appear in different providers.

3. **No Panic on Query**: All lookup functions return `Option<T>` or `Result<T, E>` (fail-open).
   - Missing provider → None
   - Missing model → None
   - Invalid model ID for provider → false (not valid)

4. **Read-Only After Startup**: Registry is immutable after config initialization.
   - No hot-reload (plugins are loaded once at startup)
   - Plugin install is a separate operation (user-initiated, before startup)
   - All queries are safe concurrent reads (no locks needed after init)

5. **Secrets Isolation**: API keys, OAuth secrets NOT stored in config layer.
   - TransportConfig has placeholder fields (`clientId`, `clientSecret`, `tokenUrl`) injected from OAuthConfig
   - Actual secrets injected by core at request time (not config layer responsibility)

6. **Error Classification Determinism**: Given (status, message), same disposition always returned.
   - No state changes; no randomness; 100% deterministic
   - Text rule priority is declaration order in ERROR_RULES vec

7. **Plugin Schema Forward-Compat**: Missing fields get defaults; unknown fields ignored.
   - Allows older plugin files to load in newer 0router versions
   - Prevents version-mismatch rejections

---

## Relationships Diagram

```
┌─────────────────┐
│ ProviderEntry   │ (built-in or plugin)
│                 │
├─ id: String    │ (unique)
├─ alias: Option │
├─ category      │
├─ transport ────┼──────→ TransportConfig
│   │            │       ├─ baseUrl
│   │            │       ├─ format
│   │            │       ├─ headers
│   │            │       ├─ auth
│   │            │       └─ [other fields]
│   │            │
│   ├─ oauth ────┼──────→ OAuthConfig
│   │            │       ├─ clientId
│   │            │       ├─ authorizeUrl
│   │            │       ├─ tokenUrl
│   │            │       └─ [other fields]
│   │            │
│   └─ media ────┼──────→ MediaConfig
│   │            │       ├─ serviceKinds
│   │            │       ├─ ttsConfig
│   │            │       ├─ sttConfig
│   │            │       └─ [other fields]
│   │            │
│   └─ models ───┼──────→ Vec<ModelEntry>
│                │       ├─ id: String
└────────────────┘       ├─ name: String
                         ├─ upstreamModelId: Option
                         ├─ kind: String
                         ├─ format: Option
                         ├─ quotaFamily: Option
                         ├─ strip: Vec<String>
                         └─ supportedFormats: Option
```

---

## Storage & Serialization

### Built-in Registry
- **Format**: Rust code (generated from build.rs)
- **Storage**: In-memory constants (no I/O)
- **Serialization**: N/A (no serialize/deserialize at runtime)

### Plugin Declarations
- **Format**: TOML files
- **Storage**: `~/.0router/plugins/` (or OROUTER_PLUGIN_DIR env override)
- **Serialization**: serde_derive with TOML codec
- **Naming**: `{provider_id}.toml` (convention; not enforced)
