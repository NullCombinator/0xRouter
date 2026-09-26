# Implementation Plan: Config Layer Port

**Branch**: `001-config-layer-port` | **Date**: 2026-09-26 | **Spec**: [spec.md](spec.md)

**Input**: Feature specification from `/specs/001-config-layer-port/spec.md`

**Note**: This plan is filled in by the `/speckit-plan` command; its definition describes the execution workflow.

## Summary

Port the 9router config layer (provider registry, model validation, error classification, runtime config,
plugin loading) to Rust with behavioral parity. The routing engine will read provider transport configs,
validate model IDs, classify upstream errors, and manage plugin provider declarations — all matching
9router's behavior exactly. This is the first slice of the fixed port order and unblocks all downstream
routing, translation, and executor layers.

## Technical Context

**Language/Version**: Rust 1.75+ (MSRV to be determined post-design)

**Primary Dependencies**: 
- tokio (async runtime, mandatory)
- serde + serde_json (config serialization)
- toml (plugin declaration format)
- regex or similar (error message classification)
- anyhow or thiserror (error handling)

**Storage**: In-memory static registry (built-in providers) + plugin directory (filesystem .toml files);
no database.

**Testing**: cargo test (unit + integration); parity tests compare Rust output against reference JS output
for all 40+ built-in providers and error classification rules.

**Target Platform**: Linux x86_64 (desktop development); macOS/aarch64 (laptop development); target
deployment: Linux server.

**Project Type**: Library (crate providing config module) — used by other 0router crates (executors,
handlers).

**Performance Goals**: Registry load + query within 100ms for 40+ providers on startup (SC-003);
per-query latency <1ms; plugin validation <10ms per file (SC-002).

**Constraints**: 
- Startup single-threaded (config is read-only after); queries may be concurrent.
- No secrets in config layer (credential injection happens at request time).
- No panic in fail-open paths (translates JS `try { … } catch { return null }` to `Result::ok()` / `Option<T>`).

**Scale/Scope**: 40+ built-in providers, 1000+ models total, ~10 error classification rules, ~8 runtime
tunables, plugin directory support for community-defined providers.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

**Principle I — Plugin Safety**: PASS.
- Plugin declarations are data-only (TOML); no code execution.
- Validation enforces no executable fields (script paths, embedded code, network requests).
- Secrets NOT stored in config layer (requirement FR-001 through FR-010 respect this).

**Principle II — Routing Fidelity**: PASS.
- Config layer is read-only after startup (no hot reload).
- Provides accurate provider capability metadata (transport, models, OAuth).
- Error classification feeds router's retry/cooldown logic (accuracy is non-negotiable).

**Principle III — Scope Discipline**: PASS.
- Config layer only loads/validates declarations; does not compress, rewrite, or forward requests.
- Token optimization is upstream (Agent → Optimizer → 0router).

**Principle IV — Streaming-Native SSE**: N/A (config is not on SSE path).

**Principle V — Parity-First Porting**: PASS.
- Spec requires exact behavioral match to reference (SC-001: identical results).
- 7-section parity audit (FR-004) required before merge.
- Fail-open paths (FR-012 lenient plugin loading) match 9router.

**Principle VI — Trustworthy Model Tests**: PASS.
- Model validity (FR-002, FR-003) is deterministic registry lookup; no provider calls needed.

**Constitution Compliance**: All 6 principles clear. No violations. Proceed.

## Project Structure

### Documentation (this feature)

```text
specs/001-config-layer-port/
├── spec.md                  # Feature specification
├── plan.md                  # This file
├── research.md              # Phase 0 output
├── data-model.md            # Phase 1 output
├── quickstart.md            # Phase 1 output
├── contracts/               # Phase 1 output (none for internal library)
└── tasks.md                 # Phase 2 output (NOT created by /speckit-plan)
```

### Source Code (repository root)

**Since the project is pre-code (no Cargo.toml yet), the plan defines the workspace structure:**

```text
0router/                      # Workspace root
├── Cargo.toml               # Workspace manifest (to be created)
├── crates/
│   ├── 0router-config/      # This feature's crate (config layer)
│   │   ├── Cargo.toml
│   │   ├── src/
│   │   │   ├── lib.rs       # Public API + module exports
│   │   │   ├── registry.rs  # Built-in provider registry (gen-d from ref/9router)
│   │   │   ├── models.rs    # Model validation + lookup functions
│   │   │   ├── error.rs     # Error classification rules
│   │   │   ├── runtime.rs   # Runtime config + env overrides
│   │   │   └── plugin.rs    # Plugin loader + validator
│   │   └── tests/
│   │       ├── parity.rs    # Parity tests vs. ref/9router
│   │       ├── plugin.rs    # Plugin loading + validation tests
│   │       └── fixtures/    # Test provider declarations
│   └── [future: executors, translators, handlers]
├── tests/                   # Integration tests (if needed)
└── identity/                # Landlock gate (pre-existing)
```

**Structure Decision**: Single crate `0router-config` inside a workspace. The config layer is a
reusable library; future crates (executors, handlers, etc.) will depend on it. Plugin fixtures live
in `crates/0router-config/tests/fixtures/` for easy unit testing of loader and validator.

## Complexity Tracking

No complexity violations. This is a straightforward porting task within established patterns:
- Registry is static (no incremental sync or versioning).
- Model lookups are exact/normalized match (no fuzzy matching).
- Error rules are deterministic classification (no ML, no heuristics).
- Plugin loading is filesystem scan + schema validation (no code execution).

---

## Phase 0: Research & Findings

**Status**: Complete (no NEEDS CLARIFICATION items from spec)

### Findings Summary

**Decision: Rust 1.75+**
- Latest stable is 1.82+; MSRV will be determined after initial impl (likely 1.70+).
- Reason: serde_json and tokio both support 1.70+; no nightly features needed.

**Decision: Built-in registry as embedded data (not file-based)**
- Built-in providers are compiled into the binary from generated Rust code.
- Rationale: Load time O(1), no filesystem overhead, consistent version lock (registry matches binary).
- Generation: Auto-generate Rust structs from ref/9router's registry/ directory pre-compile time.
- Tools: A build script (build.rs) or separate codegen tool will scan ref/9router/open-sse/providers/registry/
  and emit Rust code defining PROVIDERS, PROVIDER_MODELS, PROVIDER_OAUTH constants.

**Decision: Plugin directory discovery via env + conventional location**
- Default: `~/.0router/plugins/` (user home).
- Override: Env var `OROUTER_PLUGIN_DIR` or runtime config parameter.
- Rationale: Aligns with plugin system design (data, not code); operators control where plugins live.
- Discovery: Top-level .toml files only (no recursion); each file is one provider declaration.

**Decision: Plugin format — TOML with schema validation**
- Rationale: TOML is human-editable and industry-standard for configs (Cargo.toml, pyproject.toml, etc.).
- Schema: Derive from PROVIDER_DEFAULTS + ModelDefaults in ref/9router; use serde for deserialization.
- Validation: Reject executable fields (no `script`, `command`, `exec`, etc.); enforce required fields
  (at minimum: `id`, `category`, `transport.baseUrl`, `models`).

**Decision: Lenient plugin loading with defaults (FR-012)**
- Missing fields in older plugin files receive documented defaults (e.g., `format: "openai"`).
- Unknown fields are silently ignored (forward compatibility).
- Rationale: Minimizes upgrade friction; operator does not need to manually fix plugin files after 0router
  version bump.

**Decision: Error classification as static rule table**
- ERROR_RULES is a compiled-in Vec<ErrorRule> (text rules, status rules, priority).
- No dynamic loading of error rules (no plugin-configurable rules).
- Rationale: Error classification is runtime-critical; any error in this path affects provider cooldown
  decisions. Keep it auditable and non-extensible.

---

## Phase 1: Design & Contracts

### 1. Data Model (data-model.md)

**ProviderEntry** (built-in or plugin-loaded):
- `id`: String (unique provider identifier, kebab-case, e.g., "anthropic", "local-ollama")
- `alias`: Option<String> (short lookup key for PROVIDER_MODELS; defaults to id)
- `category`: String (enum: "apikey", "oauth", "freeTier", "local"; UI grouping)
- `transport`: TransportConfig (endpoint, auth, format, headers, timeouts)
- `models`: Vec<ModelEntry> (may be empty)
- `oauth`: Option<OAuthConfig> (if provider uses OAuth)
- `media`: Option<MediaConfig> (non-LLM services: TTS, STT, embedding, image, etc.)

**ModelEntry** (one model in a provider's list):
- `id`: String (declared model identifier, e.g., "claude-sonnet-4-20250514")
- `name`: String (display name; auto-derived from id if omitted)
- `upstream_model_id`: Option<String> (what to send to upstream; defaults to id)
- `kind`: String (model type; default: "llm"; others: "embedding", "image", "tts", etc.)
- `format`: Option<String> (wire format override for multi-endpoint providers)
- `quota_family`: Option<String> (default: "normal"; used by router for quota tracking)
- `strip`: Vec<String> (content types to drop before forwarding; e.g., ["image", "audio"])
- `supported_formats`: Option<Vec<String>> (formats this model supports if provider multi-endpoint)

**ErrorRule** (one classification rule):
- `text_pattern`: Option<String> (substring to match in error message, case-insensitive)
- `status_code`: Option<u16> (HTTP status to match)
- `disposition`: ErrorDisposition (Backoff | CooldownMs(u64) | DefaultTransient)
- Rule priority: text rules checked first (in declaration order), then status rules.

**ErrorDisposition** (enum):
- `Backoff` → exponential backoff (rate limit)
- `CooldownMs(u64)` → fixed cooldown duration (cap enforced: max 30 minutes)
- `DefaultTransient` → 30-second default cooldown

**RuntimeConfig** (resolved at startup):
- `stream_stall_timeout_ms`: u64 (inter-chunk timeout; default 360_000)
- `stream_first_chunk_timeout_ms`: u64 (time-to-first-token; default 200_000)
- `fetch_connect_timeout_ms`: u64 (connection timeout; default 60_000)
- `gemini_native_tts_fetch_timeout_ms`: u64 (TTS-specific; default 45_000)
- `default_max_tokens`: u64 (default 64_000)
- `default_min_tokens`: u64 (default 32_000)
- `default_retry_config`: Map<u16, RetryPolicy> (by HTTP status)
- `cache_ttl_user_info`: u64 (seconds; default 300)
- `cache_ttl_model_alias`: u64 (seconds; default 3600)
- `skip_patterns`: Vec<String> (request text to bypass provider)

**Public API Surface:**

```rust
// Registry queries
pub fn get_provider(id: &str) -> Option<&ProviderEntry>
pub fn get_provider_by_alias(alias: &str) -> Option<&ProviderEntry>
pub fn list_all_providers() -> Vec<&ProviderEntry>

// Model queries
pub fn is_valid_model(provider_id: &str, model_id: &str) -> bool
pub fn get_upstream_model_id(provider_id: &str, model_id: &str) -> Option<String>
pub fn get_model_info(provider_id: &str, model_id: &str) -> Option<ModelInfo>
  // ModelInfo = {name, kind, format, quota_family, strip}
pub fn get_model_name(provider_id: &str, model_id: &str) -> Option<String>

// Error classification
pub fn classify_error(status: u16, message: &str) -> ErrorDisposition
pub fn resolve_cooldown_ms(reported_ms: u64) -> u64  // applies 30min cap

// Runtime config
pub fn get_runtime_config() -> &'static RuntimeConfig
pub fn resolve_timeout_ms(env_var: &str, default_ms: u64) -> u64

// Plugin installation (only during setup, not after startup)
pub fn install_plugin(declaration: &str) -> Result<(), PluginError>
  // Validates, checks conflicts, prompts user if needed

// Plugin loading (at startup)
pub fn load_plugins(plugin_dir: &Path) -> Vec<PluginLoadResult>
  // Loads all .toml files; returns (ok, error) entries
```

### 2. Contracts (if applicable)

Internal library — no public network contracts or file format exports in this slice. Plugin format
contract is documented in the data model (ProviderEntry/ModelEntry TOML schema).

### 3. Quickstart Validation (quickstart.md)

**Prerequisites:**
- Rust 1.75+ with cargo
- Reference JS codebase at ref/9router/ (for parity test fixtures)

**Validation Scenarios:**

1. **Provider Registry Load & Query**
   ```bash
   cargo test test_provider_registry_load -- --nocapture
   # Verifies 40+ providers loaded, all have required fields
   ```

2. **Model Validation (built-in)**
   ```bash
   cargo test test_valid_model_claude -- --nocapture
   # Asserts claude + models are queryable and match ref values
   ```

3. **Model Normalization (dash/dot)**
   ```bash
   cargo test test_model_normalization_kiro -- --nocapture
   # Asserts "claude-sonnet-4-5" resolves to "claude-sonnet-4.5"
   ```

4. **Error Classification**
   ```bash
   cargo test test_error_classification -- --nocapture
   # Asserts known (status, message) pairs classify correctly
   ```

5. **Runtime Config with Env Override**
   ```bash
   STREAM_STALL_TIMEOUT_MS=120000 cargo test test_runtime_config_env -- --nocapture
   # Verifies override is applied; invalid override falls back to default
   ```

6. **Plugin Validation & Conflict Handling**
   ```bash
   cargo test test_plugin_install_conflict -- --nocapture
   # Simulates plugin conflict, verifies user prompt (mocked), tests replace logic
   ```

7. **Parity Audit: Full Spec Coverage**
   ```bash
   cargo test parity:: -- --nocapture
   # Runs 7-section parity audit (reference JS vs. Rust)
   ```

**Expected Outcome**: All tests pass; Rust and JS produce identical results for all 40+ providers,
1000+ models, and error classification rules. SC-001, SC-003, SC-004, SC-005 measurable outcomes met.

---

## Gates & Validation

✅ Constitution Check: PASS (all 6 principles)
✅ Spec Clarifications: Complete (3/3 resolved)
✅ Phase 0 Research: Complete (no blockers)
✅ Phase 1 Design: Ready for implementation

**Next Phase**: `/speckit-tasks` → task breakdown for implementation.
