# Research Phase Findings

**Date**: 2026-09-26 | **Status**: Complete

## Overview

All clarifications from the spec were resolved during `/speckit-clarify`. Phase 0 research consolidates
findings about technology choices, registry generation, plugin format, and error classification.

---

## Technology Stack Selection

### Language: Rust 1.75+

**Decision**: Rust 1.75+ (minimum MSRV to be refined during implementation)

**Rationale**:
- Project mandate (0router is pure Rust; CLAUDE.md constraint)
- Ecosystem maturity: serde, tokio, toml all stable and well-maintained
- Trait system supports fail-open semantics (Result::ok() for middleware)
- No nightly features required for this slice

**Alternatives Considered**:
- C++: Would break project mandate; no Rust ecosystem
- Python: Wrong domain (core routing layer); wrong concurrency model

---

### Async Runtime: Tokio

**Decision**: Tokio (all async paths use tokio::spawn, tokio::select!, CancellationToken)

**Rationale**:
- Constitution Principle IV (Streaming-Native SSE) requires CancellationToken + tokio utilities
- Config layer is read-only after startup (no async I/O needed in queries themselves)
- Plugin loading at startup is CPU-bound (no async needed), but design preserves tokio-friendliness
  for future hot-reload if needed

**Note**: Config queries themselves are synchronous; tokio is used for initialization-time operations
and for consistency with downstream layers (executors, handlers).

---

### Registry Generation: Build-Time Codegen

**Decision**: Auto-generate Rust code from ref/9router provider registry at compile time (build.rs)

**Rationale**:
- Built-in registry never changes at runtime (read-only after startup per assumption)
- Compile-time generation guarantees consistency: code and registry version-locked
- Zero runtime registry load overhead (already in memory as constants)
- Parity: All 40+ providers ported at once during build, reducing post-port defects

**Implementation Approach**:
1. Cargo build script (build.rs) executes during `cargo build`
2. Script scans ref/9router/open-sse/providers/registry/*.js
3. Parses each JS export via regex/serde (extract id, transport, models, oauth, media)
4. Generates Rust code in target/generated/providers.rs defining:
   - `PROVIDERS: &[ProviderEntry]`
   - `PROVIDER_MODELS: &[(String, Vec<ModelEntry>)]`
   - `PROVIDER_OAUTH: &[(String, OAuthConfig)]`
   - Constants for OLLAMA_LOCAL_DEFAULT_HOST, XIAOMI regions, etc.
5. src/lib.rs includes the generated module

**Alternatives Considered**:
- Runtime .json file loading: adds startup I/O, version skew risk, file I/O errors
- Hand-written Rust: extreme maintenance burden (40+ providers); error-prone duplication

---

### Plugin Format: TOML with Schema Validation

**Decision**: Plugins are .toml files matching ProviderEntry schema, validated at installation

**Rationale**:
- TOML is human-editable and industry-standard (Cargo.toml, pyproject.toml)
- Aligns with plugin safety invariant: data declaration, no code execution
- serde_derive can auto-validate structure at deserialization time
- Top-level discovery (no recursion) keeps loading predictable and secure

**Plugin File Example**:
```toml
id = "my-provider"
alias = "mp"
category = "apikey"

[transport]
baseUrl = "https://api.example.com/v1/chat/completions"
format = "openai"
[transport.headers]
"X-Custom-Header" = "value"

[[models]]
id = "model-x"
name = "Model X"
kind = "llm"

[[models]]
id = "model-y"
name = "Model Y"
upstream_model_id = "upstream-y-id"
```

**Validation Rules** (enforced at install):
- Required fields: `id`, `category`, `transport.baseUrl`
- Forbidden fields: any key matching `^(script|command|exec|spawn|call|code|run).*`, any path references
- Unknown fields: silently ignored (forward compat)
- Missing fields: receive defaults (lenient compat with older plugin files)

**Alternatives Considered**:
- JSON: Less human-friendly; larger file size
- YAML: Slower to parse; more security surface (arbitrary code tags)
- Binary (protobuf): Breaks transparency; harder for operators to edit

---

### Error Classification: Static Rule Table

**Decision**: ERROR_RULES as compiled-in Vec<ErrorRule>; no dynamic/plugin-configurable rules

**Rationale**:
- Error classification affects provider cooldown → high impact on availability
- Keep it auditable, non-extensible, consistent across versions
- Rules are simple (text pattern + status code → disposition); no complex logic
- Aligns with constitution: core logic stays in core, not plugins

**Rules Priority**:
1. Text-based rules (checked first, in declaration order) — e.g., "rate limit" substring match
2. Status-based rules (fallback) — e.g., HTTP 429 when no text match
3. Default fallback — 30-second transient cooldown if no rule matches

**Alternatives Considered**:
- Operator-configurable rules: High complexity; audit trail needed; breaks availability guarantees
- Plugin rules: Violates plugin safety invariant (would require plugin code execution to evaluate custom rules)

---

### Plugin Installation UX: User-Prompted Replacement

**Decision**: Installation asks user to confirm replacement when plugin ID conflicts with existing plugin

**Rationale** (from `/speckit-clarify` Q1):
- User controls whether to replace existing plugin or abort install
- Prevents silent overwrites that could cause unexpected behavior shifts
- Built-in providers are always protected (rejection before prompt; no replace option)
- Aligns with operator trust model: operator makes explicit decisions

**Flow**:
```
install_plugin(decl) →
  1. Validate schema (reject if bad)
  2. Check for conflict (existing plugin or built-in with same id)
  3. If conflict with plugin:
       Prompt: "Plugin 'old-plugin' already exists. Replace? (yes/no)"
       User responds → replace_existing (install) or abort_install
  4. If conflict with built-in:
       Return error: "Cannot replace built-in provider X"
       No prompt
  5. On success: write .toml to plugin directory, register in memory
```

---

### Plugin Discovery: Top-Level Directory Only

**Decision** (from `/speckit-clarify` Q2):
Plugin directory discovery is **top-level only** — no recursive subdirectory traversal.

**Rationale**:
- Matches industry norms (npm node_modules flat; Cargo plugins; VS Code extensions)
- Predictable: operator knows exactly which files are loaded
- Security: reduces surprise loading of deeply nested files
- Simplicity: single-level scan, no recursive traversal overhead

**Implementation**:
```rust
pub fn load_plugins(plugin_dir: &Path) -> Vec<PluginLoadResult> {
  // Read directory entries (top level only)
  for entry in std::fs::read_dir(plugin_dir)? {
    if entry.path().extension() == Some("toml") {
      // Load & validate
    }
  }
}
```

---

### Plugin Schema Evolution: Lenient Parsing with Defaults

**Decision** (from `/speckit-clarify` Q3):
When new required fields are added to the plugin schema in future 0router versions, existing plugin
files missing those fields are **loaded leniently** with documented defaults; they are NOT rejected.

**Rationale**:
- Operator upgrade experience: plugins keep working without manual fixes
- Defect rate: fewer support tickets; fewer operator errors
- Precedent: npm, Cargo, browser extensions all use lenient config parsing
- Testability: default values must be documented and tested

**Example**:
- v1.0: schema requires `[id, category, transport.baseUrl, models]`
- v1.1: adds required `transport.format` (but default is "openai")
- Behavior: plugin file from v1.0 is loaded in v1.1 with `format="openai"` applied automatically

**Alternatives Considered**:
- Strict versioning (v1.0 plugins rejected by v1.1): forces operator to manually update every
  plugin file after upgrade; high friction
- Semver in plugin file: possible, but adds complexity to schema and no clear benefit

---

## Best Practices Applied

### Fail-Open Middleware

From Constitution Principle V (Parity-First Porting):
- All config layer queries return `Option<T>` or `Result<T, E>`
- No panics on missing model, missing provider, or malformed error rule
- Callers (routing engine) decide how to handle "not found" (fallback, retry, error response)

### Testability Strategy

1. **Unit Tests**: Query functions tested against known (provider_id, model_id) pairs
2. **Integration Tests**: Full provider registry loaded; all queries validated against JS reference
3. **Property Tests**: Error classification rules for all 9router error logs (fuzzing approach)
4. **Parity Audit**: 7-section audit per FR-004 requirement (automated comparison Rust vs. JS)

---

## Deferred Decisions

**Plugin Load Logging Level** (Low Priority):
- Spec deferred: what log level for plugin validation errors?
- Decision: Use `warn!` for validation failures; `info!` for load summary
- Rationale: Operators need to see validation errors without verbose debug output
- Will be finalized during implementation phase

---

## Summary

| Area | Decision | Rationale |
|------|----------|-----------|
| Language | Rust 1.75+ | Project mandate; ecosystem support |
| Async | Tokio | Constitution requirement; downstream dependency |
| Registry | Build-time codegen | Load performance; version consistency |
| Plugin Format | TOML + serde validation | Standard format; safe parsing |
| Error Rules | Compiled-in static table | Audit, consistency, availability criticality |
| Conflict Resolution | User-prompted replace | Operator control; no silent overwrites |
| Discovery Depth | Top-level only | Predictable; security; industry norm |
| Schema Evolution | Lenient + defaults | Operator upgrade UX; precedent in ecosystem |

All research findings support Phase 1 design. No blockers for implementation.
