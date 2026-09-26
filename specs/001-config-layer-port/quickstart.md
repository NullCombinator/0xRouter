# Quickstart Validation Guide

**Date**: 2026-09-26 | **Status**: Complete

## Overview

This guide proves that the config layer works end-to-end by running a series of independent validation
scenarios. Each scenario tests one core capability and delivers measurable evidence (test passes, output matches reference).

---

## Prerequisites

- Rust 1.75+ with cargo
- 0router repository with ref/9router at `ref/9router/`
- Workspace configured with `crates/0router-config/` crate
- Dependencies: serde, serde_json, toml, tokio, anyhow (in Cargo.toml)

## Setup

```bash
cd /home/ali/Desktop/0router
cargo build --package 0router-config
```

If build fails, check:
- `build.rs` has access to ref/9router/open-sse/providers/registry/
- serde codegen is correctly wired

---

## Validation Scenarios

### Scenario 1: Built-in Provider Registry Loads

**Goal**: Verify all 40+ built-in providers are compiled into the binary and queryable.

**Run**:
```bash
cargo test --package 0router-config test_provider_registry_load -- --nocapture
```

**Expected Output**:
```
test test_provider_registry_load ... ok

Loaded 42 providers
✓ anthropic (category: apikey, alias: anthropic)
✓ openai (category: apikey, alias: cx)
✓ gemini (category: oauth, alias: gemini)
... (40+ entries)
```

**Pass Criteria**:
- Test runs in <100ms (SC-003: 100ms startup goal)
- All 40+ providers present
- Each provider has required fields: `id`, `category`, `transport.baseUrl`

**Data Model Ref**: ProviderEntry struct; [link to data-model.md#providerentry](data-model.md#providerentry)

---

### Scenario 2: Provider Query by ID and Alias

**Goal**: Verify lookups work by both provider ID and short alias.

**Run**:
```bash
cargo test --package 0router-config test_provider_query_claude -- --nocapture
cargo test --package 0router-config test_provider_query_by_alias_kr -- --nocapture
```

**Expected Output**:
```
test test_provider_query_claude ... ok
Query by id "anthropic":
  ✓ Found: Anthropic (apikey)
  ✓ baseUrl matches ref/9router: https://api.anthropic.com/v1/messages

test test_provider_query_by_alias_kr ... ok
Query by alias "kr":
  ✓ Found: kiro (OAuth)
  ✓ Transport config matches exact reference values
```

**Pass Criteria**:
- Both queries return the same ProviderEntry
- Transport config (baseUrl, format, headers) matches ref/9router exactly
- Alias correctly resolves to full provider ID

**Data Model Ref**: ProviderEntry.alias; [link to data-model.md#providerentry](data-model.md#providerentry)

---

### Scenario 3: Model Validation and Upstream ID Resolution

**Goal**: Verify model validity checks and upstream ID resolution.

**Run**:
```bash
cargo test --package 0router-config test_model_valid_claude_sonnet -- --nocapture
cargo test --package 0router-config test_model_upstream_id_resolution -- --nocapture
cargo test --package 0router-config test_model_invalid_unknown_model -- --nocapture
```

**Expected Output**:
```
test test_model_valid_claude_sonnet ... ok
Provider "anthropic", Model "claude-sonnet-4-20250514":
  ✓ Valid: true
  ✓ Upstream ID: "claude-sonnet-4-20250514"
  ✓ Name: "Claude Sonnet 4"
  ✓ Kind: "llm"

test test_model_upstream_id_resolution ... ok
Provider "anthropic", Model "claude-opus-4-20250514":
  ✓ Upstream ID resolves to declared upstreamModelId if present
  ✓ Falls back to id if upstreamModelId absent

test test_model_invalid_unknown_model ... ok
Provider "anthropic", Model "non-existent-model":
  ✓ Valid: false
  ✓ Query returns None (fail-open)
```

**Pass Criteria**:
- Valid models return true; invalid return false
- Upstream ID matches declared value (or falls back to id)
- No panic on invalid model ID

**Data Model Ref**: ModelEntry; [link to data-model.md#modelentry](data-model.md#modelentry)

---

### Scenario 4: Model ID Normalization (Dash/Dot)

**Goal**: Verify dash/dot version separator normalization for providers that support it.

**Run**:
```bash
cargo test --package 0router-config test_model_normalization_kiro -- --nocapture
cargo test --package 0router-config test_model_normalization_strict -- --nocapture
```

**Expected Output**:
```
test test_model_normalization_kiro ... ok
Provider "kiro" (allows normalization), Model "claude-sonnet-4-5":
  ✓ Normalized to: "claude-sonnet-4.5"
  ✓ Valid: true
  ✓ Matches declared model in registry

test test_model_normalization_strict ... ok
Provider "anthropic" (strict match), Model "claude-sonnet-4-5":
  ✓ No normalization applied
  ✓ Valid: false (exact match required)
```

**Pass Criteria**:
- Providers "kr" and "kiro" normalize digit-hyphen-digit to digit-dot-digit
- All other providers use exact match (no normalization)
- Invalid normalized ID still returns false (no magic)

**Data Model Ref**: ModelEntry validation in [data-model.md#modelentry](data-model.md#modelentry)

**Spec Ref**: FR-003

---

### Scenario 5: Model Metadata (Kind, Format, Quota Family, Strip)

**Goal**: Verify model metadata queries return correct values.

**Run**:
```bash
cargo test --package 0router-config test_model_metadata_kind -- --nocapture
cargo test --package 0router-config test_model_metadata_strip -- --nocapture
cargo test --package 0router-config test_model_metadata_quota_family -- --nocapture
```

**Expected Output**:
```
test test_model_metadata_kind ... ok
Provider "anthropic", Model "claude-sonnet-4-20250514":
  ✓ Kind: "llm"
  ✓ Format: (inherits from provider, or model-specific override)

test test_model_metadata_strip ... ok
Model with strip list ["image", "audio"]:
  ✓ Retrieved strip list: ["image", "audio"]
  ✓ Model without strip list: [] (default)

test test_model_metadata_quota_family ... ok
Model with quotaFamily "high":
  ✓ Quota family: "high"
  ✓ Model without quotaFamily: "normal" (default)
```

**Pass Criteria**:
- Kind, format, quotaFamily, strip all retrievable for valid models
- Defaults applied correctly (kind="llm", quotaFamily="normal", strip=[], format=null)
- Return None if model not found

**Data Model Ref**: ModelEntry; [link to data-model.md#modelentry](data-model.md#modelentry)

---

### Scenario 6: Thinking-Level Suffix Parsing and Resolution

**Goal**: Verify model IDs with thinking-level suffixes (e.g., `model(high)`) parse and resolve correctly.

**Run**:
```bash
cargo test --package 0router-config test_model_thinking_suffix -- --nocapture
```

**Expected Output**:
```
test test_model_thinking_suffix ... ok
Model ID "claude-opus-4-20250514(high)":
  ✓ Base ID extracted: "claude-opus-4-20250514"
  ✓ Base ID validated: true (exists in registry)
  ✓ Upstream ID resolved: "claude-opus-4-20250514"
  ✓ Suffix re-appended: "claude-opus-4-20250514(high)"
  ✓ Full resolved model: "claude-opus-4-20250514(high)"
```

**Pass Criteria**:
- Thinking suffix is correctly parsed and removed for base ID lookup
- Base ID is validated against registry
- Suffix is re-appended to final upstream ID
- Invalid base ID (no suffix removal) returns None

**Spec Ref**: FR-002 acceptance scenario 6; [spec.md](spec.md#user-story-2--model-validation-and-lookup-priority-p1)

---

### Scenario 7: Error Classification

**Goal**: Verify error disposition is determined correctly by text rules and status rules.

**Run**:
```bash
cargo test --package 0router-config test_error_classification_rate_limit -- --nocapture
cargo test --package 0router-config test_error_classification_auth -- --nocapture
cargo test --package 0router-config test_error_classification_priority -- --nocapture
```

**Expected Output**:
```
test test_error_classification_rate_limit ... ok
Status 429, message "":
  ✓ Disposition: Backoff
Status 200, message "rate limit exceeded":
  ✓ Disposition: Backoff (text rule overrides status)

test test_error_classification_auth ... ok
Status 401, message "":
  ✓ Disposition: CooldownMs(120000) (2 min long cooldown)

test test_error_classification_priority ... ok
Status 404, message "no credentials":
  ✓ Disposition: CooldownMs(120000)
  ✓ Text rule "no credentials" wins over status rule 404
```

**Pass Criteria**:
- Text rules are checked before status rules (text wins on match)
- All rules from ERROR_RULES vec are correctly applied
- Unknown status/message → DefaultTransient (30 sec)
- Deterministic: same input → same output always

**Data Model Ref**: ErrorRule, ErrorDisposition; [link to data-model.md#errorrule](data-model.md#errorrule)

**Spec Ref**: FR-004; [spec.md](spec.md#user-story-3--error-classification-priority-p2)

---

### Scenario 8: Cooldown Hard Cap (30 Minutes)

**Goal**: Verify provider-reported cooldown durations are capped at 30 minutes.

**Run**:
```bash
cargo test --package 0router-config test_cooldown_cap_resolve -- --nocapture
```

**Expected Output**:
```
test test_cooldown_cap_resolve ... ok
Provider reports retry-after: 6 hours (21600000 ms)
  ✓ Resolved cooldown: 1800000 ms (30 minutes)
  ✓ Cap enforced: true

Provider reports retry-after: 5 minutes (300000 ms)
  ✓ Resolved cooldown: 300000 ms (unchanged, under cap)
```

**Pass Criteria**:
- Any reported cooldown > 30 minutes is capped to 30 minutes
- Reported cooldown ≤ 30 minutes is passed through unchanged
- Cap is enforced at resolution time (FR-005)

---

### Scenario 9: Runtime Config with Environment Variable Override

**Goal**: Verify timeout values accept env overrides and fall back to defaults on invalid input.

**Run**:
```bash
STREAM_STALL_TIMEOUT_MS=120000 cargo test --package 0router-config test_runtime_config_env_override -- --nocapture
STREAM_STALL_TIMEOUT_MS=invalid cargo test --package 0router-config test_runtime_config_env_invalid -- --nocapture
cargo test --package 0router-config test_runtime_config_env_absent -- --nocapture
```

**Expected Output**:
```
test test_runtime_config_env_override ... ok
STREAM_STALL_TIMEOUT_MS=120000:
  ✓ Resolved timeout: 120000 ms
  ✓ Override applied: true

test test_runtime_config_env_invalid ... ok
STREAM_STALL_TIMEOUT_MS=invalid:
  ✓ Resolved timeout: 360000 ms (default)
  ✓ Invalid override silently ignored: true
  ✓ No error logged or raised: true

test test_runtime_config_env_absent ... ok
(no STREAM_STALL_TIMEOUT_MS set):
  ✓ Resolved timeout: 360000 ms (default)
```

**Pass Criteria**:
- Valid positive-integer env overrides are applied (SC-005)
- Invalid env values (non-numeric, ≤0) fall back to default silently (SC-005)
- No error message or process exit on invalid override
- All defaults match ref/9router exactly

**Data Model Ref**: RuntimeConfig; [link to data-model.md#runtimeconfig](data-model.md#runtimeconfig)

**Spec Ref**: FR-007; SC-005

---

### Scenario 10: Plugin Validation (Forbidden Fields)

**Goal**: Verify plugin declarations with executable content are rejected.

**Run**:
```bash
cargo test --package 0router-config test_plugin_validation_forbidden_fields -- --nocapture
```

**Expected Output**:
```
test test_plugin_validation_forbidden_fields ... ok

Plugin with [script] field:
  ✓ Rejected: true
  ✓ Error message contains "forbidden": true

Plugin with [command] field:
  ✓ Rejected: true

Plugin with [exec] field:
  ✓ Rejected: true

Plugin with all valid fields:
  ✓ Passes validation: true
```

**Pass Criteria**:
- Declarations with `script`, `command`, `exec`, `spawn`, etc. are rejected
- Error messages clearly identify the forbidden field (SC-002: human-readable)
- Validation <10ms (SC-002)
- Valid plugins pass validation

**Data Model Ref**: ProviderEntry plugin schema; [link to data-model.md#validation-rules-4](data-model.md#validation-rules-4)

**Spec Ref**: FR-008; FR-009

---

### Scenario 11: Plugin Conflict Detection (User-Prompted Replacement)

**Goal**: Verify plugin conflict is detected and user is prompted to replace or decline.

**Run** (with mocked user input):
```bash
cargo test --package 0router-config test_plugin_conflict_existing_plugin -- --nocapture
cargo test --package 0router-config test_plugin_conflict_built_in -- --nocapture
```

**Expected Output**:
```
test test_plugin_conflict_existing_plugin ... ok
Installing plugin "my-provider" (already exists):
  ✓ Conflict detected: true
  ✓ User prompt shown: "Replace existing plugin? (yes/no)"
  
  (When user answers "yes"):
    ✓ Existing plugin replaced: true
    ✓ New plugin installed: true
  
  (When user answers "no"):
    ✓ Installation aborted: true
    ✓ Existing plugin unchanged: true

test test_plugin_conflict_built_in ... ok
Installing plugin "anthropic" (built-in exists):
  ✓ Rejected immediately: true
  ✓ Error message: "Cannot replace built-in provider"
  ✓ No prompt shown: true
```

**Pass Criteria**:
- Plugin vs. existing plugin: prompt user (FR-008, from clarify Q1)
- Plugin vs. built-in: reject with error, no prompt (FR-009)
- Installation completes only on explicit user acceptance (FR-008)

**Spec Ref**: FR-008; FR-009; Clarifications → Session 2026-09-26 Q1

---

### Scenario 12: Plugin Discovery (Top-Level Only)

**Goal**: Verify plugin files are discovered at top level only, not recursively.

**Run** (with fixture plugin directory):
```bash
cargo test --package 0router-config test_plugin_discovery_top_level -- --nocapture
```

**Expected Output**:
```
test test_plugin_discovery_top_level ... ok

Plugin directory structure:
  ~/.0router/plugins/
  ├── provider-a.toml          (loaded)
  ├── provider-b.toml          (loaded)
  └── subdir/
      └── provider-c.toml      (NOT loaded)

Discovery result:
  ✓ Loaded: provider-a, provider-b
  ✓ Not loaded: provider-c (in subdir)
  ✓ Total plugins loaded: 2
```

**Pass Criteria**:
- Files directly in plugin directory are loaded
- Files in subdirectories are ignored
- No recursive traversal occurs

**Spec Ref**: FR-011; Clarifications → Session 2026-09-26 Q2

---

### Scenario 13: Plugin Lenient Parsing (Schema Evolution)

**Goal**: Verify old plugin files (missing new required fields) load successfully with defaults.

**Run** (with old-version plugin fixture):
```bash
cargo test --package 0router-config test_plugin_lenient_loading -- --nocapture
```

**Expected Output**:
```
test test_plugin_lenient_loading ... ok

Plugin file (v1.0 format, missing v1.1 fields):
  [id = "my-provider"]
  [transport]
  baseUrl = "https://api.example.com/chat"
  # (missing format, timeoutMs, etc.)

Load result:
  ✓ Plugin loaded: true
  ✓ Missing fields received defaults:
    - format: "openai"
    - timeoutMs: 60000
    - retry: DEFAULT_RETRY_CONFIG
  ✓ Schema mismatch error: false
```

**Pass Criteria**:
- Old plugin files (before new fields added) load without error
- Missing fields receive documented defaults
- Unknown fields are silently ignored
- No version mismatch rejection

**Spec Ref**: FR-012; Clarifications → Session 2026-09-26 Q3

---

## Parity Audit (7-Section Audit per FR-004)

All scenarios above form the **parity audit** covering:

1. **Registry Loading**: Scenario 1 (all 40+ providers load correctly)
2. **Provider Queries**: Scenario 2 (lookup by id and alias)
3. **Model Validation**: Scenarios 3–6 (validity, upstream ID, normalization, metadata, suffix)
4. **Error Classification**: Scenario 7 (rules applied correctly)
5. **Cooldown Enforcement**: Scenario 8 (30-min cap)
6. **Runtime Config**: Scenario 9 (env overrides, defaults)
7. **Plugin Loading**: Scenarios 10–13 (validation, conflict, discovery, evolution)

**Run Full Parity Audit**:
```bash
cargo test --package 0router-config parity:: -- --nocapture
```

**Acceptance Criteria** (SC-004):
- All 7 sections pass
- Rust output matches JS reference for all test inputs
- No behavioral divergence
- Audit runs in <1 second

---

## Success Criteria Summary

After all 13 scenarios pass:

✅ **SC-001**: Every query function returns identical results to ref/9router (parity audit)
✅ **SC-002**: Plugin validation <10ms with human-readable error messages
✅ **SC-003**: Registry loads + queries within 100ms (scenario 1)
✅ **SC-004**: All 7 parity audit sections pass
✅ **SC-005**: Invalid env overrides silently ignored (scenario 9)

---

## Troubleshooting

| Issue | Solution |
|-------|----------|
| Build fails: "cannot find module" | Run `cargo build --package 0router-config` first; check build.rs |
| Test timeout (>1 sec) | Registry load should be <100ms; check if loading from file (not built-in) |
| Parity test fails: model mismatch | Compare Rust output with `ref/9router/open-sse/config/providerModels.js` |
| Plugin test fails: permission denied | Check `~/.0router/plugins/` exists and is writable |
| Normalization test fails | Verify "kr" and "kiro" are in DOT_VERSION_PROVIDERS set |

