//! The engine snapshot: registry, accounts, agent keys, settings and redactor, swapped as
//! one. A request holds the snapshot it started with until it ends; a failed reload keeps
//! the previous one.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use arc_swap::ArcSwap;
use nullrouter_registry::{LoadReport, OperatorHome, Registry, RegistryHandle, RuntimeSettings, StartupError};
use nullrouter_wire::codec::Style;

use crate::accounts::{self, Accounts};
use crate::cooldown::Cooldowns;
use crate::files::FileError;
use crate::identity::AgentSessions;
use crate::keys::{self, BreakBehaviour, Keys};
use crate::plan::WarmMap;
use crate::records::RecordStore;
use crate::redact::{Redactor, SharedRedactor};
use crate::tokens::TokenCells;
use crate::upstream;

#[derive(Debug)]
pub struct EngineState {
    pub registry: Arc<Registry>,
    pub accounts: Accounts,
    pub keys: Keys,
    /// Swappable on its own: a token refresh extends it (`Engine::rebuild_redactor`).
    pub redactor: Arc<SharedRedactor>,
    /// The engine's live sign-in tokens, shared by every snapshot.
    pub tokens: Arc<TokenCells>,
    /// Every loaded style, compiled once per snapshot, by id.
    pub styles: BTreeMap<String, Arc<Style>>,
    /// The upstream client for this snapshot's `allow_private_endpoints`.
    pub http: reqwest::Client,
    pub generation: u64,
}

impl EngineState {
    pub fn settings(&self) -> &RuntimeSettings {
        self.registry.runtime()
    }

    pub fn style(&self, id: &str) -> Option<&Arc<Style>> {
        self.styles.get(id)
    }

    /// What a stream that breaks after output does for the agent key `key_id`: the key's
    /// own setting, else `[pipeline] break_behaviour` (itself `restart` when unset).
    pub fn break_behaviour(&self, key_id: &str) -> BreakBehaviour {
        self.keys
            .iter()
            .find(|k| k.id == key_id)
            .and_then(|k| k.break_behaviour)
            .unwrap_or(self.settings().pipeline.break_behaviour)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error(transparent)]
    File(#[from] FileError),
    #[error("{0}")]
    Registry(String),
    #[error("reload task failed: {0}")]
    Join(String),
}

impl From<StartupError> for StateError {
    fn from(e: StartupError) -> Self {
        Self::Registry(e.to_string())
    }
}

/// What a load or reload reports besides succeeding.
#[derive(Debug, Clone, Default)]
pub struct StateReport {
    pub generation: u64,
    pub registry: LoadReport,
    /// `provider/name` of accounts whose provider isn't loaded.
    pub unused_accounts: Vec<String>,
}

pub struct Engine {
    registry: RegistryHandle,
    state: ArcSwap<EngineState>,
    /// Shared with the log writer, so log lines follow reloads.
    redactor: Arc<ArcSwap<Redactor>>,
    pub records: RecordStore,
    /// Account rests per model and backoff levels (research R6). In memory only.
    pub cooldowns: Cooldowns,
    /// The account each agent was last served by, per target (research R8).
    pub warm: WarmMap,
    /// Video jobs by their `vj_` id.
    pub jobs: crate::jobs::JobMap,
    /// Sign-in account tokens (spec 005, research R6). Kept across reloads; re-read only
    /// when `tokens.toml` changed.
    pub tokens: Arc<TokenCells>,
    /// `{session.id}` for agents that send no session (research R7).
    pub sessions: AgentSessions,
    /// Token refreshes in flight, their failure backoff and timing (research R9).
    pub refresher: crate::signin::refresh::Refresher,
    /// Wakes the maintenance task after a reload or a token change.
    pub(crate) changed: tokio::sync::Notify,
    install_id: OnceLock<String>,
    generation: AtomicU64,
    reload: Mutex<()>,
}

fn operator_files(home: &OperatorHome) -> Result<(Accounts, Keys), FileError> {
    let accounts = Accounts::load(&home.path().join(accounts::FILE))?;
    let keys = Keys::load(&home.path().join(keys::FILE))?;
    Ok((accounts, keys))
}

fn assemble(
    registry: Arc<Registry>,
    mut accounts: Accounts,
    keys: Keys,
    tokens: Arc<TokenCells>,
    generation: u64,
) -> (EngineState, StateReport) {
    if accounts.bind_unbound(&registry)
        && let Err(e) = accounts.save()
    {
        tracing::warn!("account hosts bound but not saved: {e}");
    }
    let unused_accounts = accounts.unused(&registry).map(|a| format!("{}/{}", a.provider, a.name)).collect();
    let redactor = Arc::new(SharedRedactor::new(Redactor::for_state(&accounts, &tokens)));
    let report = StateReport { generation, registry: registry.report().clone(), unused_accounts };
    // The gate proved every loaded style compiles; one that doesn't is left out, not fatal.
    let styles = registry
        .styles()
        .filter_map(|f| match Style::compile(f) {
            Ok(s) => Some((f.id.clone(), Arc::new(s))),
            Err(e) => {
                tracing::error!("style {} not compiled: {e}", f.id);
                None
            }
        })
        .collect();
    let http = upstream::client(registry.runtime().allow_private_endpoints);
    (EngineState { registry, accounts, keys, redactor, tokens, styles, http, generation }, report)
}

impl Engine {
    /// Loads everything under `home`. Refuses shared `accounts.toml` / `keys.toml` /
    /// `tokens.toml`.
    pub fn open(home: OperatorHome) -> Result<(Self, StateReport), StateError> {
        Self::open_with(home, RegistryHandle::open)
    }

    /// [`open`](Self::open) over the registry's parity set: the fit check is off, so a
    /// test's user plugin may declare `[signin]` and `[identity]` (open to bundled plugins
    /// only) while pointing at a mock upstream.
    #[cfg(feature = "testkit")]
    pub fn open_parity(home: OperatorHome) -> Result<(Self, StateReport), StateError> {
        Self::open_with(home, RegistryHandle::open_parity)
    }

    fn open_with(
        home: OperatorHome,
        open: impl FnOnce(OperatorHome) -> Result<RegistryHandle, StartupError>,
    ) -> Result<(Self, StateReport), StateError> {
        let (accounts, keys) = operator_files(&home)?;
        let tokens = Arc::new(TokenCells::load(home.path())?);
        let registry = open(home)?;
        let (state, report) = assemble(registry.snapshot(), accounts, keys, tokens.clone(), 1);
        let redactor = Arc::new(ArcSwap::new(state.redactor.current()));
        let engine = Self {
            registry,
            state: ArcSwap::from_pointee(state),
            redactor,
            records: RecordStore::default(),
            cooldowns: Cooldowns::default(),
            warm: WarmMap::default(),
            jobs: crate::jobs::JobMap::default(),
            tokens,
            sessions: AgentSessions::default(),
            refresher: Default::default(),
            changed: tokio::sync::Notify::new(),
            install_id: OnceLock::new(),
            generation: AtomicU64::new(1),
            reload: Mutex::new(()),
        };
        Ok((engine, report))
    }

    /// The snapshot to hold for one request.
    pub fn snapshot(&self) -> Arc<EngineState> {
        self.state.load_full()
    }

    pub fn home(&self) -> &OperatorHome {
        self.registry.home()
    }

    /// The redactor cell the log writer reads.
    pub fn redactor(&self) -> Arc<ArcSwap<Redactor>> {
        self.redactor.clone()
    }

    /// Re-reads every file and swaps the snapshot. Operator files are read before the
    /// registry is touched, so any failure leaves everything as it was. Blocking.
    pub fn reload_blocking(&self) -> Result<StateReport, StateError> {
        let _guard = self.reload.lock().unwrap_or_else(|e| e.into_inner());
        let (accounts, keys) = operator_files(self.registry.home())?;
        let tokens = self.tokens.changed(self.registry.home().path())?;
        self.registry.reload().map_err(|e| StateError::Registry(e.to_string()))?;
        if let Some((store, at)) = tokens {
            self.tokens.apply(store, at);
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (state, report) = assemble(self.registry.snapshot(), accounts, keys, self.tokens.clone(), generation);
        self.redactor.store(state.redactor.current());
        self.state.store(Arc::new(state));
        self.changed.notify_one();
        Ok(report)
    }

    /// Rebuilds the redactor from the current accounts and token cells and swaps it into
    /// both the current snapshot and the log writer's cell, without a reload. Call after a
    /// token cell changes (research R9).
    pub fn rebuild_redactor(&self) {
        let _guard = self.reload.lock().unwrap_or_else(|e| e.into_inner());
        let st = self.snapshot();
        let r = Arc::new(Redactor::for_state(&st.accounts, &self.tokens));
        st.redactor.store(r.clone());
        self.redactor.store(r);
        self.changed.notify_one();
    }

    /// `$NULLROUTER_HOME/install-id`, created on first use (research R7).
    pub fn install_id(&self) -> Result<&str, FileError> {
        if let Some(id) = self.install_id.get() {
            return Ok(id);
        }
        let id = crate::identity::install_id(self.home().path())?;
        Ok(self.install_id.get_or_init(|| id))
    }

    /// [`reload_blocking`](Self::reload_blocking) on the blocking pool.
    pub async fn reload(self: &Arc<Self>) -> Result<StateReport, StateError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.reload_blocking())
            .await
            .map_err(|e| StateError::Join(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn write_private(path: &std::path::Path, text: &str) {
        crate::files::write_private(path, text).unwrap();
    }

    #[tokio::test]
    async fn reload_swaps_and_a_failed_reload_keeps_the_old_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        write_private(
            &dir.path().join(accounts::FILE),
            "schema = 1\n[[account]]\nprovider = \"anthropic\"\nname = \"main\"\nsecret = \"sk-ant-SENTINEL-000\"\n[[account]]\nprovider = \"nope\"\nname = \"x\"\nsecret = \"sk-nope-00000000\"\n",
        );
        let (engine, report) = Engine::open(home).unwrap();
        let engine = Arc::new(engine);
        assert_eq!(report.unused_accounts, ["nope/x"]);
        let before = engine.snapshot();
        assert_eq!(before.generation, 1);
        assert!(!before.accounts.get("anthropic", "main").unwrap().hosts.is_empty(), "bound at load");
        assert_eq!(engine.redactor().load().redact("sk-ant-SENTINEL-000"), "***");

        let r = engine.reload().await.unwrap();
        assert_eq!(r.generation, 2);
        assert_eq!(before.generation, 1, "a held snapshot doesn't change");

        let path = dir.path().join(accounts::FILE);
        fs::write(&path, "schema = 1\n[[account]]\nbroken").unwrap();
        assert!(engine.reload().await.is_err());
        assert_eq!(engine.snapshot().generation, 2);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let err = engine.reload().await.unwrap_err().to_string();
        assert!(err.contains("chmod 600"), "{err}");
        assert_eq!(engine.snapshot().generation, 2);
    }

    #[test]
    fn tokens_load_at_open_and_the_redactor_follows_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let e = crate::tokens::tests::entry("xai", "main", "xai-gen1-SENTINEL", &["api.x.ai"]);
        crate::tokens::update(home, "xai", "main", |s| *s = Some(e)).unwrap();
        let (engine, _) = Engine::open(OperatorHome::new(home)).unwrap();
        let log = engine.redactor();
        assert_eq!(engine.snapshot().redactor.redact("xai-gen1-SENTINEL"), "***");

        // A refresh swaps the cell; the redactor follows without a reload.
        engine.tokens.replace(crate::tokens::tests::entry("xai", "main", "xai-gen2-SENTINEL", &["api.x.ai"]));
        let held = engine.snapshot();
        assert_eq!(held.redact_probe("xai-gen2-SENTINEL"), "xai-gen2-SENTINEL");
        engine.rebuild_redactor();
        assert_eq!(held.generation, 1, "no reload");
        assert_eq!(held.redact_probe("xai-gen2-SENTINEL xai-gen1-SENTINEL"), "*** ***");
        assert_eq!(log.load().redact("xai-gen2-SENTINEL"), "***");

        // A reload keeps the cells while tokens.toml is unchanged.
        let cell = engine.tokens.cell("xai", "main").unwrap();
        engine.reload_blocking().unwrap();
        assert!(cell.load().entry.access_token.matches("xai-gen2-SENTINEL"), "unchanged file: in-memory kept");
        // ...and re-reads it once it changed.
        let e = crate::tokens::tests::entry("xai", "main", "xai-gen3-SENTINEL", &["api.x.ai"]);
        crate::tokens::update(home, "xai", "main", |s| *s = Some(e)).unwrap();
        engine.reload_blocking().unwrap();
        assert!(cell.load().entry.access_token.matches("xai-gen3-SENTINEL"));
        assert_eq!(engine.snapshot().redactor.redact("xai-gen2-SENTINEL xai-gen3-SENTINEL"), "*** ***");

        let id = engine.install_id().unwrap().to_owned();
        assert_eq!(engine.install_id().unwrap(), id);
    }

    impl EngineState {
        fn redact_probe(&self, s: &str) -> String {
            self.redactor.redact(s).into_owned()
        }
    }

    #[test]
    fn open_refuses_shared_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(keys::FILE);
        fs::write(&path, "schema = 1\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let err = Engine::open(OperatorHome::new(dir.path())).err().unwrap().to_string();
        assert!(err.contains("keys.toml") && err.contains("chmod 600"), "{err}");
    }
}
