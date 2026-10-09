//! Loading the serving version of a third-party adapter from the store (user story 2).
//!
//! At reload the engine asks for a [`WasmHandle`] per harness. The serving version's source and
//! module are hashed again and compared with the index; only a match is loaded. Compiled
//! modules are kept by `wasm_hash`, so a reload that changes nothing compiles nothing.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use jiff::Timestamp;
pub use nullrouter_sandbox::Redactor;
use nullrouter_sandbox::{LoadedModule, ModuleFlags, SandboxEngine, load};
use serde::Deserialize;

use crate::HarnessName;
use crate::alerts::{AlertKind, AlertLog, NewAlert};
use crate::record::{AdapterOutcome, AdapterRun, NotRunReason};
use crate::runner::{WasmHandle, WasmModule};
use crate::selector::Selector;
use crate::store::{Store, StoreError, VersionEntry, VersionId, VersionState};

/// How long a module has on a request or a whole answer, and on one stream event (research R5).
pub const REQUEST_DEADLINE: Duration = Duration::from_millis(20);
pub const EVENT_DEADLINE: Duration = Duration::from_millis(2);

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

    fn response_selectors(&self) -> Result<Vec<Selector>, String> {
        self.response.selectors.iter().map(|s| Selector::parse(s)).collect()
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
    /// What new handles get as request/answer and event deadlines. The constants unless a test
    /// has set them: no manifest or setting reaches this (contracts/adapter-kit.md, Limits).
    deadlines: Mutex<(Duration, Duration)>,
}

/// Why the loader could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("the adapter sandbox did not start: {0}")]
    Sandbox(String),
}

/// How many module instances may run at once.
const MAX_INSTANCES: u32 = 16;

impl Loader {
    /// Opens `$home/adapters` (creating it, or refusing one other users can enter) and starts the
    /// sandbox.
    pub fn open(home: &Path, redact: Redactor) -> Result<Loader, OpenError> {
        let store = Store::open(home)?;
        let alerts = Arc::new(AlertLog::open(&store));
        let sandbox = SandboxEngine::new(MAX_INSTANCES).map_err(|e| OpenError::Sandbox(e.to_string()))?;
        Ok(Self::new(store, alerts, Arc::new(sandbox), redact))
    }

    /// Calls that hold a sandbox instance right now.
    pub fn live_instances(&self) -> usize {
        self.sandbox.live_instances()
    }

    /// Sets the request/answer and event deadlines for handles made from now on. Tests only.
    #[cfg(feature = "testkit")]
    pub fn override_deadlines(&self, request: Duration, event: Duration) {
        *self.deadlines.lock().unwrap_or_else(|e| e.into_inner()) = (request, event);
    }

    /// The harnesses the index names.
    pub fn harnesses(&self) -> Vec<HarnessName> {
        match self.store.load_index() {
            Ok(i) => i.harnesses.into_iter().map(|h| h.name).collect(),
            Err(e) => {
                tracing::error!("adapter index not read: {e}");
                Vec::new()
            }
        }
    }

    pub fn new(store: Store, alerts: Arc<AlertLog>, sandbox: Arc<SandboxEngine>, redact: Redactor) -> Self {
        Self {
            store,
            alerts,
            sandbox,
            redact,
            cache: Mutex::new(HashMap::new()),
            deadlines: Mutex::new((REQUEST_DEADLINE, EVENT_DEADLINE)),
        }
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
            if let Some(v) = suspect {
                return WasmHandle::unavailable(name.as_str(), v.id.as_str(), NotRunReason::Suspect);
            }
            // An entry with no versions is what `remove` leaves behind.
            return if index.harness(name).is_some_and(|h| h.versions.is_empty()) {
                WasmHandle::unavailable(name.as_str(), "none", NotRunReason::Removed)
            } else {
                absent()
            };
        };
        let version = entry.id.as_str();
        // A kit-upgrade rebuild is under way, or failed: the old module is not loaded (the
        // sandbox would refuse its ABI), and the harness runs as a plain client.
        if entry.rebuilding {
            return WasmHandle::unavailable(name.as_str(), version, NotRunReason::Rebuilding);
        }
        if entry.rebuild_failed {
            return WasmHandle::unavailable(name.as_str(), version, NotRunReason::RebuildFailed);
        }
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
        let response_selectors = manifest.response_selectors().map_err(|_| MANIFEST_INVALID)?;
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
        let (request_deadline, event_deadline) = *self.deadlines.lock().unwrap_or_else(|e| e.into_inner());
        Ok(WasmModule {
            sandbox: self.sandbox.clone(),
            module,
            request_selectors,
            request_deadline,
            response_selectors,
            events: manifest.response.events,
            response_deadline: request_deadline,
            event_deadline,
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

    /// Acts on a finished run of a third-party adapter: a `failed` one raises an `adapter_failed`
    /// alert (repeats fold), a `blocked` one marks the version `suspect`, so it stops serving at
    /// the next load, and raises a `guardrail` alert. Other outcomes, and runs of built-in
    /// adapters, do nothing. `true`: the index changed and the caller should load again.
    pub fn note_run(&self, run: &AdapterRun, record: &str) -> bool {
        let (kind, message, codes) = match &run.outcome {
            AdapterOutcome::Failed { reason } => (AlertKind::AdapterFailed, "the adapter's run failed", reason.codes()),
            AdapterOutcome::Blocked => match &run.guardrail {
                Some(g) => (AlertKind::Guardrail, "the guardrail discarded the adapter's edits", g.rule.codes()),
                None => return false,
            },
            _ => return false,
        };
        // Built-in adapters and test fixtures have no version in the store.
        if matches!(run.version.as_str(), "builtin" | "fixture") {
            return false;
        }
        let Ok(name) = HarnessName::new(&run.harness) else { return false };
        let id = VersionId::from_run(&run.version);
        let new = NewAlert {
            kind,
            harness: name.clone(),
            version: id.clone(),
            record: Some(record.to_owned()),
            message,
            codes,
        };
        if let Err(e) = self.alerts.raise(new, Timestamp::now()) {
            tracing::error!("alert for {name} not saved: {e}");
        }
        kind == AlertKind::Guardrail && self.mark_suspect(&name, &id)
    }

    /// Moves a serving version to `suspect`. It stays the active one but no longer serves.
    fn mark_suspect(&self, name: &HarnessName, id: &VersionId) -> bool {
        let moved = self.store.load_index().and_then(|mut index| {
            index.transition(name, id, VersionState::Suspect, "the guardrail discarded its edits")?;
            self.store.save_index(&index)
        });
        match moved {
            Ok(()) => true,
            // Already suspect, superseded or gone: another request got there first.
            Err(e) => {
                tracing::warn!("adapter {name} {id} not marked suspect: {e}");
                false
            }
        }
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
