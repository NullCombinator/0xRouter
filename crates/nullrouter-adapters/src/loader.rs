//! Loading the serving version of a third-party adapter from the store (user story 2).
//!
//! At reload the engine asks for a [`WasmHandle`] per harness. The serving version's source and
//! module are hashed again and compared with the index; only a match is loaded. Compiled
//! modules are kept by `wasm_hash`, so a reload that changes nothing compiles nothing.

use std::collections::HashMap;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
use nullrouter_sandbox::{LoadedModule, ModuleFlags, Redactor, SandboxEngine, load};
use serde::Deserialize;

use crate::HarnessName;
use crate::alerts::{AlertKind, AlertLog, NewAlert};
use crate::record::NotRunReason;
use crate::runner::{WasmHandle, WasmModule};
use crate::selector::Selector;
use crate::store::{Store, StoreError, VersionEntry};

/// How long a module has on a request (research R5).
pub const REQUEST_DEADLINE: Duration = Duration::from_millis(20);

/// `adapter.toml`: what the adapter says it reads. The gate (T056) holds it to the rules in the
/// data model; the loader only needs the selectors and which exports to demand.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub harness: String,
    pub style: String,
    pub kit: String,
    #[serde(default)]
    pub summary: String,
    pub request: RequestSide,
    #[serde(default)]
    pub response: ResponseSide,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestSide {
    pub selectors: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseSide {
    #[serde(default)]
    pub selectors: Vec<String>,
    #[serde(default)]
    pub events: bool,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Manifest, String> {
        toml::from_str(text).map_err(|e| e.message().to_owned())
    }

    fn request_selectors(&self) -> Result<Vec<Selector>, String> {
        self.request.selectors.iter().map(|s| Selector::parse(s)).collect()
    }

    fn flags(&self) -> ModuleFlags {
        ModuleFlags { response: !self.response.selectors.is_empty(), events: self.response.events }
    }
}

/// The codes an alert names for a version that could not be loaded.
type Refusal = &'static [&'static str];

const MANIFEST_INVALID: Refusal = &["manifest_invalid"];

/// Loads third-party adapters from a store.
pub struct Loader {
    store: Store,
    alerts: Arc<AlertLog>,
    sandbox: Arc<SandboxEngine>,
    redact: Redactor,
    /// Compiled modules by `wasm_hash`.
    cache: Mutex<HashMap<String, Arc<LoadedModule>>>,
}

impl Loader {
    pub fn new(store: Store, alerts: Arc<AlertLog>, sandbox: Arc<SandboxEngine>, redact: Redactor) -> Self {
        Self { store, alerts, sandbox, redact, cache: Mutex::new(HashMap::new()) }
    }

    /// The handle for `name`: its serving version loaded, or a handle that records why nothing
    /// runs. Never fails: a harness that cannot be loaded is a plain client (FR-018).
    pub fn handle(&self, name: &HarnessName) -> WasmHandle {
        let absent = || WasmHandle::absent(name.as_str());
        let index = match self.store.load_index() {
            Ok(i) => i,
            Err(e) => {
                tracing::error!("adapter index not read, {name} runs no adapter: {e}");
                return absent();
            }
        };
        let Some(entry) = index.serving(name) else {
            // A suspect version stays active but does not serve; say that, not "none approved".
            let suspect = index.harness(name).and_then(|h| {
                let active = h.active.as_ref()?;
                h.versions.iter().find(|v| &v.id == active && v.state == crate::store::VersionState::Suspect)
            });
            return match suspect {
                Some(v) => WasmHandle::unavailable(name.as_str(), v.id.as_str(), NotRunReason::Suspect),
                None => absent(),
            };
        };
        let version = entry.id.as_str();
        if let Err(e) = self.store.verify(name, entry) {
            return match e {
                StoreError::SourceMismatch { .. } => {
                    self.raise(
                        AlertKind::SourceMismatch,
                        name,
                        entry,
                        "files changed after approval",
                        &["source_mismatch"],
                    );
                    WasmHandle::unavailable(name.as_str(), version, NotRunReason::SourceMismatch)
                }
                other => {
                    tracing::error!("adapter {name} {version} not verified: {other}");
                    WasmHandle::unavailable(name.as_str(), version, NotRunReason::NoApprovedVersion)
                }
            };
        }
        match self.load_module(name, entry) {
            Ok(module) => WasmHandle::loaded(name.as_str(), version, module),
            Err(codes) => {
                self.raise(AlertKind::ModuleRefused, name, entry, "module not loaded", codes);
                WasmHandle::unavailable(name.as_str(), version, NotRunReason::NoApprovedVersion)
            }
        }
    }

    fn load_module(&self, name: &HarnessName, entry: &VersionEntry) -> Result<WasmModule, Refusal> {
        let dir = self.store.version_dir(name, &entry.id);
        let read = |file: &str| fs::read(dir.join(file)).map_err(|_| &["unreadable"][..]);
        let manifest = std::str::from_utf8(&read("source/adapter.toml")?)
            .map_err(|_| MANIFEST_INVALID)
            .and_then(|t| Manifest::parse(t).map_err(|_| MANIFEST_INVALID))?;
        let request_selectors = manifest.request_selectors().map_err(|_| MANIFEST_INVALID)?;
        let expected = entry.wasm_hash.as_deref().ok_or(&["module_missing"][..])?;
        let module = match self.cached(expected) {
            Some(m) => m,
            None => {
                let wasm = read("module.wasm")?;
                let loaded = load(&self.sandbox, &wasm, expected, manifest.flags()).map_err(|e| {
                    tracing::warn!("adapter {name} {} refused by the load gate: {e}", entry.id);
                    &["load_refused"][..]
                })?;
                let loaded = Arc::new(loaded);
                self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(expected.to_owned(), loaded.clone());
                loaded
            }
        };
        Ok(WasmModule {
            sandbox: self.sandbox.clone(),
            module,
            request_selectors,
            request_deadline: REQUEST_DEADLINE,
            redact: self.redact.clone(),
        })
    }

    fn cached(&self, hash: &str) -> Option<Arc<LoadedModule>> {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(hash).cloned()
    }

    /// Drops cached modules no version in the index still names.
    pub fn forget_unused(&self) {
        let Ok(index) = self.store.load_index() else { return };
        let live: Vec<&str> =
            index.harnesses.iter().flat_map(|h| &h.versions).filter_map(|v| v.wasm_hash.as_deref()).collect();
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).retain(|hash, _| live.contains(&hash.as_str()));
    }

    fn raise(
        &self,
        kind: AlertKind,
        name: &HarnessName,
        entry: &VersionEntry,
        message: &'static str,
        codes: &'static [&'static str],
    ) {
        let new = NewAlert { kind, harness: name.clone(), version: entry.id.clone(), record: None, message, codes };
        if let Err(e) = self.alerts.raise(new, Timestamp::now()) {
            tracing::error!("alert for {name} not saved: {e}");
        }
    }
}
