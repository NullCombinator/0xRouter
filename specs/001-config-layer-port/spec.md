# Feature Specification: Config Layer Port

**Feature Branch**: `001-config-layer-port`

**Created**: 2026-09-26

**Status**: Draft

**Input**: User description: "Port the 9router config layer to 0router: behaviorally
equivalent loading, parsing, and validation of provider declarations and settings,
matching ref/9router's config/ module behavior exactly. This is the first slice of the
fixed port order."

## User Scenarios & Testing *(mandatory)*

### User Story 1 — Provider Registry Access (Priority: P1)

The routing engine looks up a provider's transport config (endpoint, auth scheme, format),
model list, and OAuth settings by provider ID or short alias. Both built-in providers and
plugin-declared providers are reachable through the same interface. A provider missing from
the registry returns a clear "not found" result, never silently defaults.

**Why this priority**: Every downstream layer (executors, translators, OAuth refresh)
depends on reading provider config. Without this, nothing else can run.

**Independent Test**: Can be fully tested by querying a known provider's transport endpoint
and verifying it matches the reference value from ref/9router's registry. Delivers a
working read-only registry usable by any one executor.

**Acceptance Scenarios**:

1. **Given** a known provider ID (e.g. "claude"), **When** the registry is queried for its
   transport config, **Then** the returned endpoint, auth scheme, and format match the
   values declared for that provider in the reference codebase exactly.
2. **Given** a provider's short alias (e.g. "kr" instead of the full provider id),
   **When** the registry is queried, **Then** the same transport config is returned as
   when queried by the full ID.
3. **Given** a provider ID that does not exist in any declaration, **When** the registry is
   queried, **Then** a structured "not found" result is returned with no panic or silent
   default.
4. **Given** both built-in and plugin-declared providers are loaded, **When** the registry
   is queried, **Then** plugin providers are reachable by the same lookup interface as
   built-ins.

---

### User Story 2 — Model Validation and Lookup (Priority: P1)

The routing engine validates that a requested model ID is declared for a given provider,
resolves it to the upstream model ID the provider expects, and retrieves metadata (type,
format, quota family, strip list). Clients may send normalized variants of a model ID
(e.g. dashes instead of dots in version numbers); the lookup tolerates this for providers
that permit it, and uses exact match for all others.

**Why this priority**: Routing decisions depend on knowing whether a model is valid and
what its upstream identifier is. An incorrect upstream ID causes a hard provider error.

**Independent Test**: Can be tested by validating a known model ID against its provider
and asserting the upstream ID, format, and type all match reference values. Fully
self-contained: no executor or translator needed.

**Acceptance Scenarios**:

1. **Given** a valid provider alias and model ID, **When** model validity is checked,
   **Then** the result is "valid" and the upstream model ID matches the declared value.
2. **Given** a provider that tolerates dash/dot normalization (e.g. "kiro") and a model ID
   with dashes where the declaration uses dots, **When** model validity is checked,
   **Then** the model is found and the canonical upstream ID is returned.
3. **Given** a provider that does NOT tolerate normalization, **When** a model ID with the
   wrong separator is checked, **Then** the result is "not valid."
4. **Given** a model with a declared quota family, **When** the quota family is retrieved,
   **Then** it matches the declared value.
5. **Given** a model with a strip list (content types to drop before forwarding),
   **When** the strip list is retrieved, **Then** it contains exactly the declared entries.
6. **Given** a model ID with a thinking-level suffix (e.g. `model-id(high)`), **When**
   the upstream model ID is resolved, **Then** the base ID is resolved against the
   registry and the suffix is re-appended to the returned upstream ID.

---

### User Story 3 — Error Classification (Priority: P2)

Given an upstream provider response (HTTP status code and/or error message text), the
config layer returns the correct error disposition: a fixed cooldown duration, exponential
backoff, or no cooldown. Text-based rules are checked before status-based rules. The
classification result drives the routing engine's retry and provider-cooldown decisions.

**Why this priority**: Incorrect error classification causes either silent over-cooling
(valid provider capacity excluded) or under-cooling (hammering a broken endpoint).
Both have user-visible impact.

**Independent Test**: Can be tested by feeding known (status, message) pairs and asserting
the returned disposition against the reference error rule table, with no other subsystem
needed.

**Acceptance Scenarios**:

1. **Given** a response with a message containing "rate limit" (case-insensitive),
   **When** the error is classified, **Then** the disposition is "exponential backoff"
   regardless of the HTTP status code.
2. **Given** a response with HTTP 429 and no matching text rule, **When** the error is
   classified, **Then** the disposition is "exponential backoff."
3. **Given** a response with HTTP 401 and no matching text rule, **When** the error is
   classified, **Then** the disposition is "long cooldown" (2 minutes).
4. **Given** a response with HTTP 404 and message text "no credentials", **When** the
   error is classified, **Then** the text rule wins and the disposition is "long cooldown"
   (text rules have priority over status rules).
5. **Given** a response not matching any rule, **When** the error is classified,
   **Then** the disposition is the transient cooldown default (30 seconds).
6. **Given** a provider-reported cooldown duration (e.g. a "retry-after" value),
   **When** the cooldown is resolved, **Then** it is capped at the hard maximum (30
   minutes) regardless of the provider-reported value.

---

### User Story 4 — Runtime Config with Environment Overrides (Priority: P2)

The routing engine reads timeout values, retry policies, and cache TTLs from a central
config object. Values have compiled-in defaults but accept environment-variable overrides
at process startup. An invalid override (non-numeric, negative, or zero) is silently
ignored in favour of the default.

**Why this priority**: Operators must be able to tune timeouts for their deployment without
recompiling. Incorrect parsing of overrides could silently break all upstream calls.

**Independent Test**: Can be tested by setting env vars, initialising the config, and
asserting the resolved values reflect the overrides for valid inputs and fall back to
defaults for invalid ones. No provider registry or network needed.

**Acceptance Scenarios**:

1. **Given** no relevant environment variables are set, **When** the config is read,
   **Then** all timeout and retry values match the reference defaults from the 9router
   codebase exactly.
2. **Given** a valid positive-integer env override for a timeout (e.g.
   `STREAM_STALL_TIMEOUT_MS=120000`), **When** the config is read, **Then** the resolved
   value is 120000.
3. **Given** an env override set to a non-numeric string or zero, **When** the config
   is read, **Then** the compiled-in default is used and no error is raised.
4. **Given** the `DEFAULT_RETRY_CONFIG` table, **When** a retry policy is looked up by
   status code (e.g. 502), **Then** the returned attempt count and delay match the
   reference values.

---

### User Story 5 — Plugin Provider Installation and Loading (Priority: P3)

A user installs a plugin provider declaration by submitting it to the config layer. The
layer validates it, checks for conflicts with already-installed plugins, and — when a
conflict exists — asks the user whether to replace the existing provider or abort the
install. At startup, the config layer loads all successfully installed plugins; valid
plugin providers are indistinguishable from built-ins at query time.

**Why this priority**: Plugin loading is required for the extensibility model but is not
needed for basic routing of built-in providers. It can ship after the core registry is
working.

**Independent Test**: Can be tested with fixture declarations (one fresh, one conflicting
an existing plugin, one conflicting a built-in) to assert correct validation, prompt
behaviour, and final registry state without a running router.

**Acceptance Scenarios**:

1. **Given** a valid plugin declaration with a new provider ID, **When** the user installs
   it, **Then** the declared provider appears in the registry with all transport and model
   data intact.
2. **Given** a plugin declaration containing a field that specifies executable content
   (e.g. a script path or embedded code), **When** the user installs it, **Then** the
   declaration is rejected before any prompt is shown and the provider is NOT installed.
3. **Given** a plugin declaration with a missing required field (e.g. no endpoint),
   **When** the user installs it, **Then** the declaration is rejected with a message
   identifying the missing field.
4. **Given** a plugin declaration whose provider ID matches an already-installed plugin,
   **When** the user installs it, **Then** the user is prompted to confirm replacement;
   if the user confirms, the new declaration replaces the old one; if the user declines,
   the existing plugin is unchanged and the new one is NOT installed.
5. **Given** a plugin declaration whose provider ID matches a built-in provider,
   **When** the user installs it, **Then** the declaration is rejected immediately with a
   message stating that built-in providers cannot be replaced; no prompt is shown.
6. **Given** no plugin directory is configured, **When** the config layer initialises,
   **Then** only built-in providers are available and no error is raised.

---

### Edge Cases

- What happens when a provider is declared in both the built-in registry and a plugin file?
  (Assumption: built-in always wins; plugin is rejected with a conflict error.)
- How does the system handle a plugin file that is syntactically valid but semantically
  ambiguous (e.g. a model list that is an empty array)? (Empty model list is valid;
  provider is registered with no models and model-validation queries return "not valid.")
- What happens when an upstream model ID resolution encounters a model whose declaration
  has no `upstreamModelId` field? (Falls back to the declared model ID as-is, matching
  9router's `found?.id` fallback.)
- What happens when the thinking-level suffix parser encounters a malformed suffix
  (e.g. nested parens)? (The innermost well-formed group is used; no panic.)

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The config layer MUST expose a provider registry queryable by provider ID
  and by short alias, returning transport config, model list, OAuth config, and media
  config for each provider.
- **FR-002**: The config layer MUST expose model-level query functions: validity check,
  upstream ID resolution, name lookup, format/kind/quota-family retrieval, and strip-list
  retrieval — all matching the behaviour of the reference `open-sse/config/providerModels.js`
  functions exactly.
- **FR-003**: The config layer MUST implement dash/dot version normalization for model
  lookups on providers that declare it (currently "kr" and "kiro"), and strict exact-match
  for all others.
- **FR-004**: The config layer MUST implement error classification: given a status code
  and/or message text, it MUST return a disposition (backoff, cooldown-duration, or
  default-transient) by applying text rules before status rules in declaration order.
- **FR-005**: The config layer MUST enforce the provider-reported cooldown hard cap (30
  minutes) on any externally supplied retry-after value.
- **FR-006**: The config layer MUST expose HTTP status code constants, cache TTLs, memory
  management config, retry policies, and skip patterns matching the reference values.
- **FR-007**: Timeout and retry values MUST accept positive-integer environment-variable
  overrides at startup; invalid or absent overrides MUST fall back to compiled-in defaults
  silently.
- **FR-008**: The config layer MUST support a plugin installation operation that: (1)
  validates the incoming declaration against the plugin safety invariant (data only, no
  executable fields, all required fields present); (2) checks for a provider ID conflict
  with any already-installed plugin; (3) if a conflict exists, presents the user with a
  replace-or-decline prompt; (4) installs the plugin only on explicit user acceptance or
  when no conflict exists.
- **FR-012**: When loading an installed plugin file, the config layer MUST apply lenient
  parsing: fields absent from the file receive their documented defaults, and unrecognised
  fields are silently ignored. A plugin file MUST NOT be rejected solely because it was
  written against an older schema version.
- **FR-009**: Built-in providers MUST take precedence over all plugin declarations. An
  installation attempt for a plugin whose provider ID matches a built-in MUST be rejected
  immediately with a descriptive error; no replacement prompt is shown.
- **FR-011**: At process startup, the config layer MUST load all previously installed,
  validated plugin declarations from the top level of the plugin directory only (no
  recursive subdirectory traversal) and merge them into the registry alongside built-ins.
- **FR-010**: The config layer MUST expose the `OAUTH_ALIASES` and `PROVIDER_ID_TO_ALIAS`
  maps derived from the provider registry, matching the reference derivation logic.

### Key Entities

- **ProviderEntry**: A single provider's declaration. Contains: id (unique string), alias
  (optional short name), transport config (endpoint URL, auth scheme, wire format),
  model list, OAuth config (optional), media config (optional).
- **ModelEntry**: One model within a provider's model list. Contains: id, name, upstream
  model ID (optional, defaults to id), kind/type, format, quota family (optional), strip
  list (optional).
- **ErrorRule**: One classification rule. Contains: optional text pattern (substring,
  case-insensitive) and/or HTTP status code, plus a disposition (backoff flag or fixed
  cooldown duration in milliseconds).
- **RuntimeConfig**: The resolved set of runtime tunables. Contains: timeout values,
  retry policies by status code, cache TTLs, memory management limits, and skip patterns.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Every query function in the 0router config layer returns identical results to
  its ref/9router counterpart for all inputs covered by the 9router test suite and the
  parity audit test suite.
- **SC-002**: A plugin declaration failing any validation rule is rejected in under 10ms
  with a human-readable error message naming the offending field.
- **SC-003**: The full built-in provider registry (40+ providers) loads and is queryable
  within 100ms of process startup on the target deployment machine.
- **SC-004**: All 7 sections of the `/rust-parity-audit` pass before merge, with zero
  behavioural divergence from the reference for the functions covered by FR-001 through
  FR-010.
- **SC-005**: Invalid environment-variable overrides are silently ignored in 100% of cases
  (no error logged, no process exit, default value used).

## Assumptions

- The config layer is read-only after startup; hot reload of provider declarations is out
  of scope for this slice.
- Built-in provider declarations are authoritative and compiled into the binary; they are
  not loaded from external files at runtime.
- The plugin directory path defaults to a conventional location (e.g. `~/.0router/plugins/`)
  when not explicitly configured.
- The first version of the plugin declaration format covers only the fields present in the
  9router provider registry: transport, models, OAuth, and the media/capability fields.
  Advanced plugin features (custom translators, custom executors) are out of scope.
- Secrets (API keys, OAuth client secrets) are NOT stored in provider declarations. The
  config layer exposes only structural metadata; credential injection is a separate concern
  handled at request time.
- The `appConstants.js` provider-specific constants (Gemini CLI version, GitHub Copilot
  versions, etc.) are ported as static built-in values, not as plugin-configurable fields.
  They do not need to be runtime-overridable in this slice.
- The `SKIP_PATTERNS` list (request bypass patterns) is ported as a compiled-in constant
  with no external configurability in this slice.

## Clarifications

### Session 2026-09-26

- Q: When two plugin files both declare the same provider ID (neither is a built-in), which one wins — or are both rejected? → A: The user is prompted at installation time to choose whether to replace the existing plugin or decline; declining means the new plugin is not installed. The installation step enforces uniqueness, so startup loading never encounters a duplicate.
- Q: When the config layer scans the plugin directory at startup, does it discover plugin files only at the top level of that directory, or does it search recursively through subdirectories? → A: Top-level only — files directly inside the plugin directory; subdirectories are ignored.
- Q: When the plugin declaration format gains a new required field in a future version, how should the loader treat existing plugin files missing that field? → A: Lenient — missing fields receive documented defaults; unrecognised fields are silently ignored; no plugin is rejected solely due to schema version drift.
