//! The engine snapshot: registry, accounts, agent keys, settings and redactor, swapped as
//! one. A request holds the snapshot it started with until it ends; a failed reload keeps
//! the previous one.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use arc_swap::ArcSwap;
use nullrouter_registry::{
    LoadReport, OperatorHome, Registry, RegistryHandle, RuntimeSettings, SecretString, StartupError,
};
use nullrouter_wire::codec::Style;

use crate::accounts::{self, Accounts};
use crate::cooldown::Cooldowns;
use crate::files::{DashboardToken, FileError};
use crate::identity::AgentSessions;
use crate::keys::{self, BreakBehaviour, Keys};
use crate::models_live::LiveModels;
use crate::records::RecordStore;
use crate::redact::{Redactor, SharedRedactor};
use crate::connection::pause::ProxyBoard;
use crate::connection::proxy::{self, Proxies};
use crate::tokens::TokenCells;
use crate::upstream;

#[derive(Debug)]
pub struct EngineState {
    pub registry: Arc<Registry>,
    pub accounts: Accounts,
    pub keys: Keys,
    /// The dashboard token's digest (`dashboard.toml`, spec 009), read again at every reload.
    pub dashboard: DashboardToken,
    /// The engine's one redactor, shared by every snapshot (security review L3): a request
    /// holding an older snapshot still masks a token refreshed after it started.
    pub redactor: Arc<SharedRedactor>,
    /// The engine's live sign-in tokens, shared by every snapshot.
    pub tokens: Arc<TokenCells>,
    /// Live model lists (`[models_live]`), shared by every snapshot.
    pub live_models: Arc<LiveModels>,
    /// Every loaded style, compiled once per snapshot, by id.
    pub styles: BTreeMap<String, Arc<Style>>,
    /// The upstream client for this snapshot's `allow_private_endpoints`.
    pub http: reqwest::Client,
    /// The clients per proxy, HTTP mode and reuse, built when first asked for (spec 013).
    pub clients: crate::connection::clients::Clients,
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
    #[error("the record journal can't start: {0}")]
    Journal(String),
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
    /// The redactor every snapshot holds; swapped together with [`redactor`](Self::redactor).
    shared_redactor: Arc<SharedRedactor>,
    /// Secrets of the key accounts the last reload dropped: still masked for one generation,
    /// since a request holding the older snapshot may still send them.
    retired: Mutex<Vec<SecretString>>,
    pub records: RecordStore,
    /// The record journal: one writer thread for `records/` and `routing/` (spec 006, R11).
    pub journal: Arc<crate::journal::Journal>,
    /// The routing decision's live state: salt, warm store, ledgers (spec 006).
    pub router: crate::routing::Router,
    /// Account rests per model and backoff levels (research R6). In memory only.
    pub cooldowns: Cooldowns,
    /// Requests in flight, for `live.snapshot` (spec 013). In memory only.
    pub live: crate::live::Live,
    /// The proxies that are paused, and why (spec 013, research R8). Kept across reloads.
    pub proxy_board: ProxyBoard,
    /// Video jobs by their `vj_` id.
    pub jobs: crate::jobs::JobMap,
    /// Sign-in account tokens (spec 005, research R6). Kept across reloads; re-read only
    /// when `tokens.toml` changed.
    pub tokens: Arc<TokenCells>,
    /// `{session.id}` for agents that send no session (research R7).
    pub sessions: AgentSessions,
    /// Token refreshes in flight, their failure backoff and timing (research R9).
    pub refresher: crate::signin::refresh::Refresher,
    /// Quota polls per account: latest, last failure, schedule (research R11). In memory.
    pub quota: crate::quota::poll::QuotaBoard,
    /// Poll history and the running per-account traffic tally (research R15). Every
    /// completed poll is written to it through a [`quota`](Self::quota) hook.
    pub history: Arc<crate::quota::history::History>,
    /// The quota fit's significant numbers (spec 012).
    pub fits: ArcSwap<crate::quota::fit::Fits>,
    /// The meters in effect, rebuilt at start, on reload and when a fit changes (spec 012, R15).
    pub meters: crate::quota::fit::Meters,
    /// Live model lists, shared with every snapshot.
    pub live_models: Arc<LiveModels>,
    /// Model verdicts per account (spec 011). Kept across reloads.
    pub verdicts: crate::verdict::Board,
    /// Model tests in flight, retests included (spec 011, R10).
    pub test_gate: crate::tests::Gate,
    /// The listeners `serve` bound, for the operator socket's `server.status`.
    pub status: crate::status::ServerStatus,
    /// Wakes the maintenance task after a reload or a token change.
    pub(crate) changed: tokio::sync::Notify,
    install_id: OnceLock<String>,
    generation: AtomicU64,
    reload: Mutex<()>,
}

type OperatorFiles = (Accounts, Keys, DashboardToken, Proxies);

fn operator_files(home: &OperatorHome) -> Result<OperatorFiles, FileError> {
    let accounts = Accounts::load(&home.path().join(accounts::FILE))?;
    let keys = Keys::load(&home.path().join(keys::FILE))?;
    let dashboard = DashboardToken::load(home.path())?;
    let proxies = Proxies::load(&home.path().join(proxy::FILE))?;
    Ok((accounts, keys, dashboard, proxies))
}

/// Every proxy's password and username, for the redactor (SC-008).
fn proxy_secrets(proxies: &Proxies) -> Vec<SecretString> {
    proxies.iter().flat_map(proxy::Proxy::secrets).collect()
}

fn assemble(
    registry: Arc<Registry>,
    (mut accounts, keys, dashboard, proxies): OperatorFiles,
    redactor: Arc<SharedRedactor>,
    tokens: Arc<TokenCells>,
    live_models: Arc<LiveModels>,
    generation: u64,
) -> (EngineState, StateReport) {
    if accounts.bind_unbound(&registry)
        && let Err(e) = accounts.save()
    {
        tracing::warn!("account hosts bound but not saved: {e}");
    }
    let unused_accounts = accounts.unused(&registry).map(|a| format!("{}/{}", a.provider, a.name)).collect();
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
    let clients = crate::connection::clients::Clients::new(
        registry.runtime().allow_private_endpoints,
        proxies,
    );
    (
        EngineState {
            registry,
            accounts,
            keys,
            dashboard,
            redactor,
            tokens,
            live_models,
            styles,
            http,
            clients,
            generation,
        },
        report,
    )
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
        let (accounts, keys, dashboard, proxies) = operator_files(&home)?;
        let tokens = Arc::new(TokenCells::load(home.path())?);
        let history = Arc::new(crate::quota::history::History::open(home.path()));
        let quota = crate::quota::poll::QuotaBoard::default();
        quota.on_poll(history.hook());
        let registry = open(home)?;
        accounts.check_routing(&registry.snapshot())?;
        let router = crate::routing::Router::open(registry.home().path())?;
        let journal = Arc::new(
            crate::journal::Journal::start(registry.home().path(), Default::default())
                .map_err(|e| StateError::Journal(e.to_string()))?,
        );
        let (verdicts, replay) = crate::verdict::Board::open(registry.home().path(), journal.clone());
        if replay.malformed > 0 {
            tracing::warn!("routing/verdicts.jsonl: {} malformed lines skipped", replay.malformed);
        }
        let live_models = Arc::new(LiveModels::default());
        let shared_redactor =
            Arc::new(SharedRedactor::new(Redactor::for_state_with(&accounts, &tokens, &proxy_secrets(&proxies))));
        let (state, report) = assemble(
            registry.snapshot(),
            (accounts, keys, dashboard, proxies),
            shared_redactor.clone(),
            tokens.clone(),
            live_models.clone(),
            1,
        );
        let proxy_board = ProxyBoard::open(registry.home().path());
        proxy_board.reconcile(&crate::connection::fingerprints(&state));
        let redactor = Arc::new(ArcSwap::new(shared_redactor.current()));
        let engine = Self {
            registry,
            state: ArcSwap::from_pointee(state),
            redactor,
            shared_redactor,
            retired: Mutex::new(Vec::new()),
            records: RecordStore::journaled(journal.clone()),
            journal,
            router,
            cooldowns: Cooldowns::default(),
            live: Default::default(),
            proxy_board,
            jobs: crate::jobs::JobMap::default(),
            tokens,
            sessions: AgentSessions::default(),
            refresher: Default::default(),
            quota,
            history,
            fits: ArcSwap::from_pointee(Default::default()),
            meters: Default::default(),
            live_models,
            verdicts,
            test_gate: Default::default(),
            changed: tokio::sync::Notify::new(),
            status: Default::default(),
            install_id: OnceLock::new(),
            generation: AtomicU64::new(1),
            reload: Mutex::new(()),
        };
        let st = engine.snapshot();
        engine.rebuild_meters();
        let restored = crate::route::restore(&engine, &st, crate::clock::now());
        tracing::info!("routing state restored: {} fingerprints, {} ledgers", restored.fingerprints, restored.ledgers);
        engine.recheck_verdicts(&st);
        Ok((engine, report))
    }

    /// Makes the newest record segments whole after a crash: cuts a torn final line and closes
    /// every request that was still open as `interrupted`. Returns how many. Blocking; the
    /// server runs it before it listens.
    pub fn recover_journal(&self) -> std::io::Result<usize> {
        self.journal.flush_blocking();
        let now = crate::clock::now();
        let closed = crate::journal::records::recover(self.home().path(), now)?;
        self.history.recover_counters(now);
        Ok(closed)
    }

    /// Recomputes the meters in effect from the current snapshot and fits (spec 012).
    pub fn rebuild_meters(&self) {
        self.meters.rebuild(&self.snapshot(), &self.fits.load());
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
        let (accounts, keys, dashboard, proxies) = operator_files(self.registry.home())?;
        let tokens = self.tokens.changed(self.registry.home().path())?;
        self.registry.reload().map_err(|e| StateError::Registry(e.to_string()))?;
        accounts.check_routing(&self.registry.snapshot())?;
        // Key accounts this reload drops stay masked for one generation.
        let old = self.snapshot();
        let kept = |s: &SecretString| {
            accounts.iter().any(|a| a.secret.as_ref().is_some_and(|n| s.with_exposed(|v| n.matches(v))))
        };
        let retired: Vec<SecretString> = old
            .accounts
            .iter()
            .filter_map(|a| a.secret.as_ref())
            .filter(|s| !kept(s))
            .map(|s| s.with_exposed(|v| SecretString::new(v)))
            .collect();
        *self.retired.lock().unwrap_or_else(|e| e.into_inner()) = retired;
        // The redactor learns new accounts and tokens before any snapshot or cell holds them
        // (security review L3).
        match &tokens {
            Some((store, _)) => {
                let incoming =
                    store.entries.iter().flat_map(|e| std::iter::once(&e.access_token).chain(&e.refresh_token));
                self.swap_redactor(self.build_redactor(&accounts, &proxies, incoming));
            }
            None => self.swap_redactor(self.build_redactor(&accounts, &proxies, std::iter::empty())),
        }
        if let Some((store, at)) = tokens {
            self.tokens.apply(store, at);
        }
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (state, report) = assemble(
            self.registry.snapshot(),
            (accounts, keys, dashboard, proxies),
            self.shared_redactor.clone(),
            self.tokens.clone(),
            self.live_models.clone(),
            generation,
        );
        self.swap_redactor(self.build_redactor(&state.accounts, state.clients.proxies(), std::iter::empty()));
        self.proxy_board.reconcile(&crate::connection::fingerprints(&state));
        // An account that was added, enabled again or given a new priority starts its deficit at 0
        // (spec 006).
        let fresh: Vec<String> = state
            .accounts
            .iter()
            .filter(|a| {
                !a.disabled
                    && old.accounts.get(&a.provider, &a.name).is_none_or(|o| o.disabled || o.priority != a.priority)
            })
            .map(|a| format!("{}/{}", a.provider, a.name))
            .collect();
        if !fresh.is_empty() {
            for ledger in self.router.lock().ledgers.values_mut() {
                for key in &fresh {
                    ledger.forget(key);
                }
            }
        }
        // A removed account's fingerprints and deficits go with it.
        let removed: Vec<(String, String)> = old
            .accounts
            .iter()
            .filter(|a| state.accounts.get(&a.provider, &a.name).is_none())
            .map(|a| (a.provider.clone(), a.name.clone()))
            .collect();
        for (provider, name) in &removed {
            crate::route::drop_account(self, &state, provider, name, SystemTime::now());
        }
        let state = Arc::new(state);
        self.state.store(state.clone());
        self.rebuild_meters();
        self.recheck_verdicts(&state);
        self.changed.notify_one();
        Ok(report)
    }

    /// Rebuilds the redactor from the current accounts and token cells and swaps it into
    /// both the current snapshot and the log writer's cell, without a reload. Call after a
    /// token cell changes (research R9).
    pub fn rebuild_redactor(&self) {
        let _guard = self.reload.lock().unwrap_or_else(|e| e.into_inner());
        let st = self.snapshot();
        self.swap_redactor(self.build_redactor(&st.accounts, st.clients.proxies(), std::iter::empty()));
        self.recheck_verdicts(&st);
        self.changed.notify_one();
    }

    /// Returns to untested every verdict whose account, sign-in or plugin changed, or whose
    /// account or provider is gone (research R7).
    fn recheck_verdicts(&self, st: &EngineState) {
        let cleared = crate::verdict::recheck(self, st, crate::clock::now());
        if cleared > 0 {
            tracing::info!("{cleared} model verdicts back to untested: their account or plugin changed");
        }
    }

    /// Adds `secrets` to the redactor before they go into a token cell (security review
    /// L3): there is no moment when a cell holds a token the redactor doesn't know. Call
    /// [`rebuild_redactor`](Self::rebuild_redactor) after the swap.
    pub fn admit_secrets<'a>(&self, secrets: impl IntoIterator<Item = &'a SecretString>) {
        let _guard = self.reload.lock().unwrap_or_else(|e| e.into_inner());
        let st = self.snapshot();
        self.swap_redactor(self.build_redactor(&st.accounts, st.clients.proxies(), secrets));
    }

    /// Every key-account secret in `accounts`, every token-cell generation, the retired
    /// secrets, and `extra`.
    fn build_redactor<'a>(
        &self,
        accounts: &Accounts,
        proxies: &Proxies,
        extra: impl IntoIterator<Item = &'a SecretString>,
    ) -> Redactor {
        let proxy_secrets = proxy_secrets(proxies);
        let views = self.tokens.views();
        let retired = self.retired.lock().unwrap_or_else(|e| e.into_inner());
        let mut all: Vec<&SecretString> = accounts
            .iter()
            .filter_map(|a| a.secret.as_ref())
            .chain(views.iter().flat_map(|v| v.secrets()))
            .chain(retired.iter())
            .chain(proxy_secrets.iter())
            .collect();
        for s in extra {
            all.push(s);
        }
        Redactor::new(all)
    }

    /// Swaps `r` into the shared redactor and the log writer's cell.
    fn swap_redactor(&self, r: Redactor) {
        let r = Arc::new(r);
        self.shared_redactor.store(r.clone());
        self.redactor.store(r);
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

    /// L3: one redactor for every snapshot. A request that holds a snapshot from before a
    /// reload, then refreshes its token, still masks the new one.
    #[test]
    fn a_held_snapshot_masks_tokens_refreshed_after_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let e = crate::tokens::tests::entry("xai", "main", "xai-gen1-SENTINEL", &["api.x.ai"]);
        crate::tokens::update(home, "xai", "main", |s| *s = Some(e)).unwrap();
        write_private(
            &home.join(accounts::FILE),
            "schema = 1\n[[account]]\nprovider = \"anthropic\"\nname = \"gone\"\nsecret = \"sk-ant-RETIRED-0001\"\n",
        );
        let (engine, _) = Engine::open(OperatorHome::new(home)).unwrap();
        let held = engine.snapshot();
        write_private(&home.join(accounts::FILE), "schema = 1\n");
        engine.reload_blocking().unwrap();
        assert_eq!(held.redact_probe("sk-ant-RETIRED-0001"), "***", "a dropped key stays masked a generation");

        let next = crate::tokens::tests::entry("xai", "main", "xai-gen2-SENTINEL", &["api.x.ai"]);
        engine.admit_secrets([&next.access_token]);
        assert_eq!(held.redact_probe("xai-gen2-SENTINEL"), "***", "known before the cell holds it");
        engine.tokens.replace(next);
        engine.rebuild_redactor();
        assert_eq!(held.redact_probe("xai-gen2-SENTINEL xai-gen1-SENTINEL"), "*** ***");
        assert!(Arc::ptr_eq(&held.redactor, &engine.snapshot().redactor), "one redactor");
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
