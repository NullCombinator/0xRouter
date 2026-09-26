# Feature Specification: Provider Entity & Unified Model Registry

**Feature Branch**: `002-provider-model-registry`

**Created**: 2026-09-26

**Status**: Draft

**Input**: User description: "Provider entity & unified model registry — the first compiling, testable slice of 0router. A registry that loads provider entities from declarative TOML plugins and gathers them into unified models that clients address by name. Provider entity: one plugin declares one provider, with transport config, auth scheme, and one section per capability actually offered; each model carries its own type and 'no declared type' is valid. Bundled plugins generated from 9router's registry pass the same validation gate as third-party plugins; a conflicting user plugin triggers replace-or-decline; bundled plugins are never locked. Plugin declarations never hold secrets; the five 9router providers with hardcoded OAuth clientSecret (antigravity, gemini-cli, gemini, iflow, trae) have them extracted to a core-side credential table. Unified models are explicit user declarations gathering provider entities offering the same model, recording each member's upstream model ID. Lookup by id or alias, unified model → members, requested model ID → upstream ID (tolerant normalization only where permitted), structured not-found. Validation rejects malformed plugins with actionable errors; only id and category are required; transport-less entries are valid; categories are apikey/oauth/freeTier/free/webCookie. Parity oracle: ref/9router/tests/__baseline__/{providers,alias,oauth-urls}-baseline.json. Out of scope: routing decision, execution, translators, SSE, OAuth flows, error classification/retry, combos, dashboard, token optimization."

## Clarifications

### Session 2026-09-26

- Q: Are provider models directly addressable, or only declared unified models? → A: Clients may address a specific provider's model directly as `<provider>/<model>`.
- Q: If a client sends `<provider>/<model>` and the provider's catalog doesn't list the model, is the request still forwarded? → A: Forwarded only if the operator's per-provider "allow uncatalogued models" setting is on; the setting is on by default. When it is off, the request is rejected with a structured not-found.
- Q: If a request uses a bare name (no slash) that is not a declared unified model, what happens? → A: Structured not-found; no prefix-based provider guessing. Unified model names cannot contain `/`, so a name with a slash is always a direct request and a name without one is always a unified model.
- Q: If a user plugin replaces a bundled provider that has a core-held OAuth client secret, does the replacement still get it? → A: Only if the replacement's OAuth URLs (authorize, token, refresh) point at the same hosts as the bundled plugin's; otherwise the secret is withheld and the conflict report says so.
- Q: Do changes to plugins or unified model declarations take effect without a restart? → A: Yes, on an explicit reload command: the full new set is validated first, then swapped in atomically; if anything is invalid the old set stays active and the errors are reported; in-flight requests finish on the version they started with.

## User Scenarios & Testing *(mandatory)*

The "users" of this slice are (a) the **operator** running 0router, who installs plugins and
declares unified models, (b) the **plugin author**, who writes a provider declaration, and
(c) every **later 0router slice** (routing decision, cache bookkeeping, execution, model
tests, combos), which asks the registry "which providers can serve unified model X, and how
do I address each one?".

### User Story 1 — Bundled provider set is available and faithful (Priority: P1)

The operator starts 0router with no configuration of their own. The bundled provider set —
0router's own provider plugins, derived from 9router's provider registry — loads through the
same validation gate as any third-party plugin. Every provider 9router knows about is present
as one provider entity, and each one's declared transport, aliases, and OAuth endpoints are
the ones 9router uses.

**Why this priority**: Without a faithful provider catalog nothing downstream can run, and
"moving over feels like an upgrade" (init.md) depends on the operator finding every provider
they used in 9router.

**Independent Test**: Load only the bundled plugins, then compare the registry's
transport view, alias view, and OAuth endpoint view against 9router's baseline snapshots.
No network, no executors, no user config required.

**Acceptance Scenarios**:

1. **Given** only bundled plugins, **When** the registry loads, **Then** it contains one
   provider entity per 9router registry entry (121), of which exactly the 83 that 9router
   gives a transport have a transport, and each transport's endpoint, format, headers, and
   quirks match `providers-baseline.json` for that provider. (The baseline embeds OAuth
   client secrets inside four transports; parity is checked on the core's composed view —
   plugin declaration plus credential table — never on the plugin file alone.)
2. **Given** only bundled plugins, **When** every alias token recorded in
   `alias-baseline.json` is resolved, **Then** each resolves to the same provider id as in
   the baseline, and each provider id's canonical short alias matches the baseline's
   id-to-alias map. Exception: the 4 tokens 9router passes through unresolved because no
   active provider owns them (`qw`, `dv`, `devin`, `devin-cli`) return not-found (FR-018).
3. **Given** only bundled plugins, **When** OAuth endpoints are listed, **Then** they match
   `oauth-urls-baseline.json`.
4. **Given** only bundled plugins, **When** the bundled plugin files are inspected,
   **Then** none contains an OAuth client secret or any other credential; the four
   active providers that 9router ships with a hardcoded client secret (antigravity,
   gemini-cli, gemini, iflow) obtain it from the core credential table instead. (trae
   also hardcodes one, but it is disabled in 9router and not bundled.)
5. **Given** the provider "kiro", **When** it is looked up by the short alias "kr",
   **Then** the kiro provider entity is returned.

---

### User Story 2 — Model lookup and upstream ID resolution (Priority: P1)

A later slice holds a provider (by id or alias) and a client-supplied model ID. It asks the
registry whether the model is declared, what upstream ID to send, what type the model is,
and what target format it declares.

**Why this priority**: An incorrect upstream ID is a hard provider error on every request;
this is the most frequently exercised lookup in the whole router.

**Independent Test**: Table-driven checks of (provider, requested model) → (found?, upstream
ID, type, target format) against values computed by 9router for the same inputs.

**Acceptance Scenarios**:

1. **Given** a provider and a model ID declared for it, **When** the upstream ID is
   requested, **Then** the model's declared upstream ID is returned, or its own ID if it
   declares none.
2. **Given** a model ID ending in a thinking suffix such as `(high)`, **When** the
   upstream ID is requested, **Then** lookup uses the ID without the suffix and the
   suffix is re-appended to the result.
3. **Given** a declared upstream ID that itself carries a preset suffix and a request with
   no suffix, **When** the upstream ID is requested, **Then** the preset suffix is kept;
   **and given** a request that does carry a suffix, **Then** the request's suffix
   replaces the preset.
4. **Given** a model ID whose trailing parentheses are nested (e.g. `m(a(b))`),
   **When** the suffix is stripped, **Then** nothing is stripped — only a final
   parenthesized group containing no parentheses counts as a suffix.
5. **Given** a model ID not declared for the provider, **When** the upstream ID is
   requested, **Then** the result is the requested base ID with its suffix re-appended
   (not a not-found), matching 9router; **and when** validity is asked, **Then** the
   model is reported as not declared.
6. **Given** a provider that permits version-separator tolerance (Kiro) and a request
   using dashes where the catalog uses dots (`claude-sonnet-4-5` vs `claude-sonnet-4.5`),
   **When** the model is looked up, **Then** it is found; **and given** any provider that
   does not permit it, **Then** only an exact match is found.
7. **Given** a declared model with neither a type nor a kind, **When** its type is
   requested, **Then** the answer is "no declared type" — not "llm".
8. **Given** a declared model with a target format, **When** its target format is
   requested, **Then** that value is returned; **given** one without, **Then** "none
   declared" is returned.

---

### User Story 3 — Operator declares a unified model (Priority: P1)

The operator declares a unified model, e.g. `sonnet-4.5`, gathering several provider
entities that serve the same underlying model — each member naming the provider and the
provider-specific model ID. Later slices resolve `sonnet-4.5` to its member list.

**Why this priority**: Unified models are the routing target (Constitution III) and the
reason 0router's routing decision is possible at all. This story is what distinguishes the
slice from a copy of 9router's registry.

**Independent Test**: Declare a unified model over two bundled providers, resolve it, and
check the members and their per-member upstream IDs; declare invalid ones and check the
errors.

**Acceptance Scenarios**:

1. **Given** a unified model declaration naming two providers and each one's model ID,
   **When** it is resolved by name, **Then** both members are returned, each with the
   provider entity and the upstream ID that provider expects.
2. **Given** a declaration whose member references an unknown provider, or a model ID the
   provider does not declare (and the provider is not passthrough), **When** declarations
   load, **Then** the declaration is rejected with an error naming the member and the
   reason.
3. **Given** a declaration whose members declare different, conflicting model types,
   **When** declarations load, **Then** it is rejected; a member with no declared type
   does not conflict.
4. **Given** a bare name (no `/`) that is not a declared unified model, e.g.
   `claude-sonnet-4.5`, **When** it is resolved, **Then** a structured not-found result
   is returned — the provider is never guessed from the name.
5. **Given** a client request of the form `<provider>/<model>`, **When** it is resolved,
   **Then** it targets that specific provider's model directly — regardless of whether
   the model is a member of any declared unified model — and resolves to exactly that
   one provider and its upstream model ID.
6. **Given** a direct request `<provider>/<model>` for a model the provider's catalog
   does not list, and that provider's "allow uncatalogued models" setting at its default
   (on), **When** it is resolved, **Then** it resolves to that provider with the
   requested model ID (and suffix) unchanged, marked "not in catalog".
7. **Given** the same request with the operator having turned "allow uncatalogued
   models" off for that provider, **When** it is resolved, **Then** a structured
   not-found result is returned naming the provider and model.

---

### User Story 4 — Plugin author installs a third-party provider (Priority: P2)

A plugin author writes a provider declaration file — one provider, with only the capability
sections it actually offers — and the operator installs it. Valid plugins become provider
entities indistinguishable, for lookup purposes, from bundled ones. Invalid plugins are
rejected with errors precise enough to fix without reading 0router's source.

**Why this priority**: Community plugins are a stated goal, but the bundled set already
exercises the same data model and validation gate, so this story adds conflict handling and
error quality on top of P1.

**Independent Test**: Feed a corpus of valid and deliberately malformed plugin files to the
validation gate and check accept/reject results and the error content.

**Acceptance Scenarios**:

1. **Given** a plugin declaring only `id` and `category`, **When** it is validated,
   **Then** it is accepted as a catalog-only provider entity with no transport.
2. **Given** a plugin missing `id` or `category`, or whose category is not one of
   apikey / oauth / freeTier / free / webCookie, **When** it is validated, **Then** it is
   rejected with an error naming the file, the field, and the allowed values.
3. **Given** a plugin containing any field that could carry a secret (client secret, API
   key, token, password) or any field the schema does not define, **When** it is
   validated, **Then** it is rejected with an error naming the offending field.
4. **Given** a plugin declaring text and embedding capabilities, **When** it is loaded,
   **Then** the entity exposes exactly a text section and an embedding section, and
   queries for other capabilities report "not offered".
5. **Given** a user plugin whose id matches a bundled plugin's id, **When** it is
   installed, **Then** the operator is asked to replace or decline; on replace the user
   plugin becomes the active entity for that id, on decline the bundled one stays active,
   and until a decision is recorded the bundled one stays active and the conflict is
   reported.
6. **Given** a user plugin replacing bundled `gemini-cli` whose token URL keeps Google's
   host, **When** the operator chooses replace, **Then** the core-held client secret
   still applies; **given** one whose token URL points at any other host, **Then** the
   secret is withheld and the operator is told OAuth for `gemini-cli` will not work and
   which URL caused it.
7. **Given** two user plugins with the same id, or an alias claimed by two different
   providers, **When** they are loaded, **Then** the conflict is reported with both
   sources named and neither silently wins.

---

### Edge Cases

- A plugin declares a capability section with no endpoint of its own on a provider that
  has no transport either: rejected as unreachable, with an error saying to add an
  endpoint or remove the section.
- A plugin declares `models = []` explicitly versus omitting models: these stay distinct
  ("offers no models" vs "catalog unknown"), as in 9router.
- A model entry declared as a bare ID string with no name: accepted; a display name is
  derived from the ID.
- A passthrough provider (forwards the client's model ID untouched): any model ID is
  considered valid for it and resolves to itself.
- An alias equal to another provider's id: rejected as an alias conflict.
- A unified model declared with a name containing `/`: rejected at load, since that
  shape is reserved for direct requests (FR-014).
- A direct request whose model part itself contains `/` (e.g.
  `openrouter/meta-llama/llama-3`): only the first `/` separates provider from model.
- A plugin file that is not parseable at all: rejected with the file path and the parse
  position; other plugins still load.
- A malformed bundled plugin: this is a release defect; startup fails loudly rather than
  running with a silently shrunken provider set.
- A reload with no file changes: produces an identical registry, no duplicate entities.
- A reload where one user plugin or one unified model declaration is invalid: the whole
  reload is rejected, the previous registry stays active, and every error is reported
  (unlike startup, where the invalid user plugin is skipped — FR-010).
- A reload that removes a provider still referenced by a unified model declaration: the
  reload is rejected with an error naming the unified model and the missing provider.
- A lookup that runs while a reload is being applied: sees either the complete old
  registry or the complete new one, never a mix.

## Requirements *(mandatory)*

### Functional Requirements

**Provider entities and plugins**

- **FR-001**: The system MUST represent each provider as exactly one provider entity,
  declared by exactly one plugin file.
- **FR-002**: A provider entity MUST hold: id, category, optional aliases (a canonical
  short alias plus extra lookup tokens), optional transport configuration, optional auth
  scheme description, optional OAuth endpoint description (public values only), optional
  display metadata, and zero or more capability sections.
- **FR-003**: Capability sections MUST exist only for capabilities the provider offers.
  The capability kinds MUST cover at least those present in 9router's registry: text
  (llm), image, video, text-to-speech, speech-to-text, embedding, web search, and
  "systemone"; the set MUST be extensible without changing existing plugins.
- **FR-004**: Each model MUST carry its own optional type; "no declared type" MUST be
  representable and MUST be reported as such, never defaulted at the registry layer.
- **FR-005**: Each model MUST be able to declare an upstream model ID, a target format, a
  list of supported formats, a quota family, and a strip list; absence of each MUST be
  distinguishable from an explicit value.
- **FR-006**: Only `id` and `category` MUST be required. Category MUST be one of apikey,
  oauth, freeTier, free, webCookie.

**Validation gate (Constitution I)**

- **FR-007**: Every plugin — bundled or third-party — MUST pass through the same
  validation gate before becoming a provider entity.
- **FR-008**: The gate MUST reject any plugin containing a field not defined by the plugin
  schema, and any field whose purpose is to carry a credential (client secret, API key,
  access/refresh token, password, cookie).
- **FR-009**: The gate MUST reject plugins whose declarations imply code execution,
  filesystem access, or plugin-initiated network access (e.g. scripts, file paths to
  read, hooks). Endpoint URLs are data the core may later act on; they are allowed.
- **FR-010**: Every rejection MUST name the plugin file, the field path, the violated
  rule, and — where the rule is an enumeration — the allowed values. At startup, one
  invalid user plugin MUST NOT prevent other valid plugins from loading, and any invalid
  bundled plugin MUST fail startup. On reload, FR-024 applies instead.

**Bundled provider set**

- **FR-011**: 0router MUST ship a bundled plugin per active 9router registry entry (121;
  entries commented out of 9router's registry index, such as trae and devin-cli, are
  not bundled), generated
  from 9router's evaluated provider registry (not by text-parsing its source files, 14 of
  which compute values from imports).
- **FR-012**: OAuth client secrets for antigravity, gemini-cli, gemini, and iflow
  MUST live in a core-side credential table keyed by provider id, and MUST NOT appear in
  any plugin file. The credential table MUST NOT be readable through any plugin-facing
  interface.
- **FR-012a**: Each credential-table entry MUST be bound to the hosts 9router sends that
  secret to: the OAuth hosts (authorize, token, refresh) declared by the bundled plugin,
  or, where the bundled plugin declares none (gemini), the provider's OAuth hosts that
  the core knows about. A replacing user plugin
  MUST receive the credential only if every OAuth URL it declares uses one of those
  hosts; otherwise the credential MUST be withheld for that provider, and the conflict
  report MUST state that OAuth will not work and which URL caused it.
- **FR-013**: Bundled plugins MUST NOT be locked: a user plugin with the same id MUST
  trigger a replace-or-decline decision. Until the operator records "replace", the
  bundled plugin remains active and the pending conflict is reported.

**Unified models**

- **FR-014**: The operator MUST be able to declare a unified model: a unique name, an
  optional model type, and one or more members, each a (provider, provider model ID)
  pair. Unified model names MUST NOT contain `/`.
- **FR-014a**: A requested target MUST be classified by shape alone: containing `/` → a
  direct request (FR-017), split at the first `/`; otherwise → a unified model name
  (FR-016). A bare name that is not a declared unified model MUST return a structured
  not-found; the system MUST NOT infer a provider from the model name.
- **FR-015**: Declarations MUST be validated at load: every member's provider exists;
  every member's model ID is declared by that provider unless the provider is
  passthrough; members' declared types do not conflict with each other or with the
  unified model's type; a unified model has at least one member; no provider appears
  twice in one unified model.
- **FR-016**: Resolving a unified model by name MUST return its members in declaration
  order, each with its provider entity and resolved upstream model ID.
- **FR-017**: The system MUST resolve a requested target of the form `<provider>/<model>`
  as a direct request for that provider's model: the provider part is resolved per
  FR-018, the model part per FR-019–FR-021, and the result is exactly one provider and
  its upstream model ID. Direct requests do not require the model to belong to any
  unified model. If the provider's catalog does not list the model, the request resolves
  (requested ID and suffix unchanged, marked "not in catalog") only when the operator's
  per-provider "allow uncatalogued models" setting is on; otherwise it is a structured
  not-found. The setting defaults to on for every provider. Passthrough providers always
  resolve, regardless of the setting.

**Lookup**

- **FR-018**: The system MUST resolve a provider by id or by any declared alias token,
  returning a structured not-found result for unknown tokens — never a default provider.
- **FR-019**: Model lookup MUST strip a thinking suffix — a final parenthesized group
  containing no parentheses, optionally followed by whitespace — before matching, and
  MUST leave nested parentheses unstripped.
- **FR-020**: Model lookup MUST use exact ID match, except for providers that declare
  version-separator tolerance, for which a digit-dash-digit sequence in the request MUST
  also match digit-dot-digit in the catalog. In the bundled set only Kiro declares this.
- **FR-021**: Upstream ID resolution MUST follow 9router's behavior: declared upstream
  ID, else the model's own ID, with the request's suffix (else the declared preset
  suffix) appended; for an undeclared model, the requested base ID plus its suffix.
- **FR-022**: Model validity, display name, type, target format, supported formats,
  quota family, and strip list MUST each be queryable per (provider, model ID), with
  "not declared" results distinguishable from declared values.

**Lifecycle**

- **FR-024**: Plugins, unified model declarations, provider operator settings, and
  replace-or-decline decisions MUST take effect without a restart when the operator
  issues an explicit reload. A reload MUST validate the complete new set before it
  becomes active and MUST swap it in atomically. If any part is invalid, the previous
  registry MUST stay active and all errors MUST be reported.
- **FR-025**: Every lookup MUST see one consistent registry version. A request that has
  already resolved its target MUST keep that resolution for its whole lifetime, even if a
  reload happens meanwhile.
- **FR-026**: The system MUST NOT reload by itself (no file watching) in this slice.

**Parity (Constitution VI)**

- **FR-023**: The bundled registry's transport view, alias view (all 117 probe tokens and
  the id-to-alias map), and OAuth endpoint view MUST reproduce 9router's
  `providers-baseline.json`, `alias-baseline.json`, and `oauth-urls-baseline.json`. The
  transport view compared is the core's composed view (plugin + credential table), since
  the baseline itself contains client secrets that FR-012 keeps out of plugins. The 4
  alias tokens 9router echoes back unresolved (`qw`, `dv`, `devin`, `devin-cli`) are an
  intentional deviation: they return not-found per FR-018.

### Key Entities

- **Provider entity**: One provider, as declared by one plugin. Identity (id, aliases),
  category, optional transport, optional auth scheme, optional OAuth endpoints, display
  metadata, and its capability sections.
- **Capability section**: One kind of service a provider offers (text, image, TTS, STT,
  embedding, video, web search, …), with that capability's endpoint details and its model
  catalog.
- **Model**: An entry in a capability section's catalog: ID, optional display name,
  optional type, optional upstream ID, optional target/supported formats, optional quota
  family and strip list.
- **Plugin**: The data file declaring one provider entity. Has a source (bundled or user)
  that determines conflict precedence and nothing else.
- **Credential table**: Core-owned secrets keyed by provider id, each bound to the OAuth
  hosts it was issued for. Never part of a plugin.
- **Unified model**: Operator-declared routing target: name, optional type, ordered list
  of members.
- **Member**: A (provider entity, provider model ID) pair inside a unified model, with its
  resolved upstream ID.
- **Provider operator settings**: Operator-owned, per-provider settings that sit outside
  the plugin — in this slice, "allow uncatalogued models" (default on). Replacing a
  plugin does not reset them.
- **Plugin conflict**: A pending or recorded replace/decline decision between a bundled
  and a user plugin sharing an id.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of the 83 provider transports, 113 resolvable alias probe tokens (plus
  the 4 unowned tokens returning not-found), 83 id-to-alias
  entries, and all five OAuth baseline sections (endpoints, token URLs, auth URLs, refresh
  URLs, client IDs) in 9router's baseline snapshots are reproduced by the bundled registry
  with zero differences.
- **SC-002**: Zero bundled or accepted plugin files contain a credential value; a scan of
  the shipped plugin set for the four known client secrets finds none.
- **SC-003**: For a corpus covering every validation rule, 100% of invalid plugins are
  rejected and 100% of rejection messages name the file, field, and rule.
- **SC-004**: Every model-lookup acceptance scenario in User Story 2 gives the same result
  as 9router for the same inputs, across every model declared in the bundled set plus the
  suffix, nested-paren, dash/dot, and undeclared-model cases.
- **SC-005**: An operator can declare a two-member unified model and resolve it
  successfully on the first attempt using only the plugin and declaration documentation.
- **SC-006**: Loading the full bundled set plus a unified model declaration completes
  fast enough to be imperceptible at startup (under one second on the operator's machine).
- **SC-007**: During repeated reloads under concurrent lookups, zero lookups fail or
  return a mixed-version result, and a reload containing an invalid file leaves the
  previously active registry fully in service.

## Assumptions

- The plugin file format is TOML (fixed by the constitution); the exact schema is a
  planning decision.
- "Replace or decline" in this slice is a recorded operator decision (e.g. in the
  operator's configuration), not an interactive dashboard prompt; the dashboard is out of
  scope.
- Unified models are declared by the operator only. Plugins do not propose unified
  models in this slice.
- OAuth client IDs, authorize/token URLs, and scopes are public values and may appear in
  plugins; only client secrets and user credentials are forbidden.
- 9router behaviors implemented as provider-specific code rather than data — the Codex
  review-suffix rewrite and the opencode Muse Spark format override — are not part of this
  slice; they are revisited when execution is specified, and must then be expressed
  either as declarative plugin fields or as core built-ins.
- 9router's bare-name prefix inference (`claude-` → anthropic, fallback openai) and its
  user model-alias map are deliberately not inherited; a declared unified model covers
  both uses.
- 9router's free-form `quirks` object becomes a closed set of declared flags the core
  understands; each quirk present in the bundled set is carried over.
- Per-provider amortization parameters (rate limits, quotas, offers) are out of scope
  here and will be added to the provider entity by the routing-decision slice.
- Parity baselines are those committed in `ref/9router/tests/__baseline__/` at the time of
  planning; they are regenerated with `snapshot-providers.mjs` if the reference is updated.
- Out of scope: the routing decision (cache-aware, amortization), request execution,
  translators, SSE, OAuth flows, error classification and retry, combos, dashboard, token
  optimization.
