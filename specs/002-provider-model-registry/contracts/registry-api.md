# Contract: `nullrouter-registry` Library API and `nullrouter-cli`

**Consumers**: later 0router slices (routing decision, execution, model tests, combos,
dashboard) and the operator via the CLI.

The shapes below are the contract. Exact generic and lifetime details are left to
implementation.

## Library

```rust
/// Process-wide handle. Cheap to clone. Reads are lock-free.
pub struct RegistryHandle { /* ArcSwap<Registry> + reload mutex + home path */ }

impl RegistryHandle {
    /// Loads bundled plugins, user plugins, config.toml.
    /// Fatal: an invalid bundled plugin or an invalid config.toml.
    /// Invalid user plugins are skipped and appear in the report.
    pub fn open(home: OperatorHome) -> Result<Self, StartupError>;

    /// Consistent snapshot. Hold it for the lifetime of a request (FR-025).
    pub fn snapshot(&self) -> Arc<Registry>;

    /// Validate the full new set, then swap atomically (FR-024).
    /// On any error the active snapshot is unchanged.
    /// Blocking file I/O: call from `spawn_blocking` in async contexts.
    pub fn reload(&self) -> Result<LoadReport, ReloadError>;
}

impl Registry {
    // ── Targets (FR-014a, FR-016, FR-017) ──────────────────────────
    pub fn resolve(&self, target: &str) -> Result<Resolution<'_>, NotFound>;

    // ── Providers (FR-018) ─────────────────────────────────────────
    pub fn provider(&self, token: &str) -> Result<&ProviderEntity, NotFound>;
    pub fn providers(&self) -> impl Iterator<Item = &ProviderEntity>;

    // ── Models (FR-019 – FR-022) ───────────────────────────────────
    pub fn model(&self, provider: &str, model_id: &str) -> Result<ModelInfo<'_>, NotFound>;
    pub fn upstream_id(&self, provider: &str, model_id: &str) -> Result<String, NotFound>;

    // ── Unified models ─────────────────────────────────────────────
    pub fn unified_model(&self, name: &str) -> Result<&UnifiedModel, NotFound>;
    pub fn unified_models(&self) -> impl Iterator<Item = &UnifiedModel>;

    // ── Capabilities (FR-003) ──────────────────────────────────────
    pub fn capability(&self, provider: &str, kind: CapabilityKind)
        -> Result<Option<&CapabilitySection>, NotFound>;   // Ok(None) = "not offered"

    pub fn report(&self) -> &LoadReport;
}

pub struct ModelInfo<'a> {
    pub declared: bool,                         // isValidModel (passthrough → true)
    pub name: Cow<'a, str>,                     // findModelName
    pub kind: Option<ModelKind>,                // getModelType — None, never "llm" (FR-004)
    pub target_format: Option<WireFormat>,
    pub supported_formats: Option<&'a [WireFormat]>,
    pub quota_family: Option<&'a str>,          // parity view maps None → "normal"
    pub strip: Option<&'a [ContentKind]>,       // parity view maps None → []
    pub upstream_id: String,                    // FR-021
}
```

`upstream_id` never fails for a known provider. For an undeclared model it returns the
base ID with the suffix re-appended, matching 9router (US2 scenario 5). `resolve()` is the
function that applies `allow_uncatalogued_models` (FR-017).

### Composed transport (public; secret stays opaque)

```rust
/// Composed transport as the executor will need it: the plugin transport, plus
/// the OAuth `client_id`/`token_url` where the transport lacks them, plus the
/// credential if the bound hosts match.
pub fn composed_transport(&self, provider: &str) -> Option<ComposedTransport<'_>>;

pub struct ComposedTransport<'a> {
    // …public transport fields…
    pub client_secret: Option<&'a SecretString>,
}

impl SecretString {
    pub fn matches(&self, candidate: &str) -> bool;  // compare without revealing
    pub(crate) fn expose(&self) -> &str;             // crate-internal only
}
// Debug/Display → "***"; not Serialize.
```

The parity tests serialise this view without `clientSecret` and compare it against
`providers.json`. They then check the secret with
`client_secret.unwrap().matches(fixture["clientSecret"])`. The value is compared but
never read out through the public API.

## Behavioural guarantees

| Guarantee | Test |
|---|---|
| `resolve("a/b/c")` splits at the first `/` → provider `a`, model `b/c` | unit |
| `resolve("name")` never infers a provider | unit |
| `resolve("")`, `resolve("/m")`, `resolve("p/")` → `NotFound::EmptyTarget` | unit |
| Undeclared alias token → `NotFound::Provider` (4 baseline tokens included) | parity |
| No public function returns a raw `client_secret`: `SecretString::expose` is `pub(crate)` | `compile_fail` doc-test calling `expose()` from outside the crate + API review |
| Lookups during reload see old or new, never mixed | concurrency test (SC-007) |

## CLI: `nullrouter-cli`

Operator and quickstart tool. It does not serve requests.

| Command | Output | Exit |
|---|---|---|
| `nullrouter-cli check [--home DIR]` | Load report: counts, conflicts, withheld credentials, skipped plugins, all errors | 0 ok · 1 errors |
| `nullrouter-cli validate FILE…` | Gate result per plugin file | 0 all valid · 1 otherwise |
| `nullrouter-cli resolve TARGET [--home DIR] [--json]` | The `Resolution` (provider(s), upstream IDs, catalogued flag) or `NotFound` | 0 found · 2 not found |
| `nullrouter-cli model PROVIDER MODEL [--json]` | `ModelInfo` | 0 · 2 |
| `nullrouter-cli providers [--capability KIND]` | Provider list | 0 |

`--json` output is stable and is used by the quickstart checks.
