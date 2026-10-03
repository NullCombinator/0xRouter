//! Signed-in account tokens (spec 005, research R5): `tokens.toml`, its lock, the live token
//! cells the attempt loop reads, and the account service states.
//!
//! Every writer (the server refreshing, the CLI signing in or removing) goes through
//! [`update`]: an exclusive lock on `tokens.lock`, a fresh read, one entry changed, an
//! atomic private write. The server keeps one [`TokenCells`] for its life; a refresh swaps
//! one account's cell, so it never needs a full reload.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use arc_swap::ArcSwap;
use nullrouter_registry::SecretString;
use serde::{Deserialize, Serialize};

use crate::accounts::{Account, valid_name};
use crate::clock::{parse_rfc3339, rfc3339};
use crate::files::{self, FileError};

pub const FILE: &str = "tokens.toml";
pub const LOCK: &str = "tokens.lock";
const SCHEMA: u32 = 1;

/// The states kept in `tokens.toml` (research R10); the others live only in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersistedState {
    NeedsSignIn,
    Refused,
}

/// Identity claims read from the id token or profile. Not secret; shown in listings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
}

impl Claims {
    fn is_empty(&self) -> bool {
        self.email.is_none() && self.user_id.is_none() && self.tier.is_none()
    }
}

/// One sign-in account's credentials (data-model § Sign-in credentials).
#[derive(Debug)]
pub struct TokenEntry {
    pub provider: String,
    pub name: String,
    pub access_token: SecretString,
    /// Some providers issue none.
    pub refresh_token: Option<SecretString>,
    /// From `expires_in` and the local clock at receipt (research R9).
    pub expires_at: SystemTime,
    pub scope: String,
    pub claims: Claims,
    /// Hosts the tokens may be sent to: the provider's token hosts at sign-in.
    pub hosts: BTreeSet<String>,
    pub signed_in_at: SystemTime,
    pub last_refresh_at: Option<SystemTime>,
    /// Set only for `needs_sign_in` and `refused`.
    pub state: Option<PersistedState>,
    pub state_since: Option<SystemTime>,
    /// The provider's error code or message, redacted.
    pub state_reason: Option<String>,
}

/// A copy of a secret, for the one place that must keep two: the previous generation.
fn dup(s: &SecretString) -> SecretString {
    s.with_exposed(|v| SecretString::new(v))
}

impl TokenEntry {
    /// The access token, last four characters, for listings (as for keys).
    pub fn shown_token(&self) -> String {
        self.access_token.with_exposed(|s| format!("…{}", crate::accounts::last4(s)))
    }

    /// The tokens, for keeping as the previous generation.
    pub fn pair(&self) -> TokenPair {
        TokenPair { access: dup(&self.access_token), refresh: self.refresh_token.as_ref().map(dup) }
    }
}

/// An access token and its refresh token.
#[derive(Debug)]
pub struct TokenPair {
    pub access: SecretString,
    pub refresh: Option<SecretString>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: u32,
    #[serde(default, rename = "token")]
    tokens: Vec<RawEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    provider: String,
    name: String,
    access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    expires_at: String,
    #[serde(default)]
    scope: String,
    #[serde(default)]
    hosts: Vec<String>,
    signed_in_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_refresh_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state: Option<PersistedState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state_since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Claims::is_empty")]
    claims: Claims,
}

/// The parsed `tokens.toml`.
#[derive(Debug, Default)]
pub struct TokenStore {
    pub entries: Vec<TokenEntry>,
}

pub fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

impl TokenStore {
    /// Reads `home/tokens.toml`; a missing file is no tokens. Refuses a shared file and a
    /// symbolic link.
    pub fn load(home: &Path) -> Result<Self, FileError> {
        let path = path(home);
        match files::read_private_file(&path)? {
            None => Ok(Self::default()),
            Some(text) => Self::parse(&text, &path),
        }
    }

    pub fn parse(text: &str, path: &Path) -> Result<Self, FileError> {
        let raw: RawFile = toml::from_str(text).map_err(|e| FileError::toml(path, text, &e))?;
        if raw.schema != SCHEMA {
            return Err(FileError::invalid(
                path,
                format!("schema {} is not supported (expected {SCHEMA})", raw.schema),
            ));
        }
        let mut entries: Vec<TokenEntry> = Vec::new();
        for r in raw.tokens {
            let who = format!("token {}/{}", r.provider, r.name);
            if !valid_name(&r.name) {
                return Err(FileError::invalid(path, format!("{who}: bad account name")));
            }
            if entries.iter().any(|e| e.provider == r.provider && e.name == r.name) {
                return Err(FileError::invalid(path, format!("{who}: listed twice")));
            }
            let time = |field: &str, v: &str| {
                parse_rfc3339(v)
                    .ok_or_else(|| FileError::invalid(path, format!("{who}: {field} {v:?} is not RFC 3339")))
            };
            let opt_time = |field: &str, v: Option<&str>| v.map(|v| time(field, v)).transpose();
            entries.push(TokenEntry {
                expires_at: time("expires_at", &r.expires_at)?,
                signed_in_at: time("signed_in_at", &r.signed_in_at)?,
                last_refresh_at: opt_time("last_refresh_at", r.last_refresh_at.as_deref())?,
                state_since: opt_time("state_since", r.state_since.as_deref())?,
                provider: r.provider,
                name: r.name,
                access_token: SecretString::new(r.access_token),
                refresh_token: r.refresh_token.map(SecretString::new),
                scope: r.scope,
                claims: r.claims,
                hosts: r.hosts.into_iter().collect(),
                state: r.state,
                state_reason: r.state_reason,
            });
        }
        Ok(Self { entries })
    }

    pub fn to_toml(&self) -> String {
        let tokens = self
            .entries
            .iter()
            .map(|e| RawEntry {
                provider: e.provider.clone(),
                name: e.name.clone(),
                access_token: e.access_token.with_exposed(str::to_owned),
                refresh_token: e.refresh_token.as_ref().map(|s| s.with_exposed(str::to_owned)),
                expires_at: rfc3339(e.expires_at),
                scope: e.scope.clone(),
                hosts: e.hosts.iter().cloned().collect(),
                signed_in_at: rfc3339(e.signed_in_at),
                last_refresh_at: e.last_refresh_at.map(rfc3339),
                state: e.state,
                state_since: e.state_since.map(rfc3339),
                state_reason: e.state_reason.clone(),
                claims: e.claims.clone(),
            })
            .collect();
        toml::to_string(&RawFile { schema: SCHEMA, tokens }).expect("tokens serialise")
    }

    pub fn get(&self, provider: &str, name: &str) -> Option<&TokenEntry> {
        self.entries.iter().find(|e| e.provider == provider && e.name == name)
    }
}

/// Holds the exclusive lock on `home/tokens.lock` until dropped. Refuses a symbolic link
/// (security review L5): checked before the open, and the opened file must be the one at
/// the path.
fn lock(home: &Path) -> Result<File, FileError> {
    let path = home.join(LOCK);
    let io = |source| FileError::Io { path: path.clone(), source };
    std::fs::create_dir_all(home).map_err(io)?;
    files::refuse_symlink(&path)?;
    let f =
        OpenOptions::new().read(true).write(true).create(true).truncate(false).mode(0o600).open(&path).map_err(io)?;
    let opened = f.metadata().map_err(io)?;
    let at_path = std::fs::symlink_metadata(&path).map_err(io)?;
    if at_path.file_type().is_symlink() || (opened.dev(), opened.ino()) != (at_path.dev(), at_path.ino()) {
        return Err(FileError::invalid(&path, "changed while it was opened (a symbolic link?)"));
    }
    f.lock().map_err(io)?;
    Ok(f)
}

/// Changes one account's entry in `home/tokens.toml` under the writers' lock: re-reads the
/// file, hands `f` the entry (`None` when absent; leave `None` to delete it), and writes the
/// result privately. Blocking.
pub fn update<R>(
    home: &Path,
    provider: &str,
    name: &str,
    f: impl FnOnce(&mut Option<TokenEntry>) -> R,
) -> Result<R, FileError> {
    let _lock = lock(home)?;
    let mut store = TokenStore::load(home)?;
    let at = store.entries.iter().position(|e| e.provider == provider && e.name == name);
    let mut slot = at.map(|i| store.entries.remove(i));
    let out = f(&mut slot);
    if let Some(mut e) = slot {
        e.provider = provider.to_owned();
        e.name = name.to_owned();
        let i = at.unwrap_or(store.entries.len());
        store.entries.insert(i, e);
    }
    files::write_private(&path(home), &store.to_toml())?;
    Ok(out)
}

/// Takes `provider/name` out of service in `home/tokens.toml` (research R10): `state` with
/// the time and the (already redacted) reason. Only while the stored access token is
/// `token`, when given: a token replaced meanwhile (a new sign-in) is not marked. Returns
/// the entry as written, `None` when nothing was. Blocking.
pub fn persist_state(
    home: &Path,
    provider: &str,
    name: &str,
    state: PersistedState,
    reason: &str,
    at: SystemTime,
    token: Option<&SecretString>,
) -> Result<Option<TokenEntry>, FileError> {
    if TokenStore::load(home)?.get(provider, name).is_none() {
        return Ok(None);
    }
    update(home, provider, name, |slot| {
        let e = slot.as_mut()?;
        if token.is_some_and(|t| !t.with_exposed(|t| e.access_token.matches(t))) {
            return None;
        }
        e.state = Some(state);
        e.state_since = Some(at);
        e.state_reason = Some(reason.to_owned());
        Some(dup_entry(e))
    })
}

/// `accounts enable`: clears a `refused` state so the account is tried again. `true` when
/// there was one. Leaves `needs_sign_in` alone: only a sign-in clears it. Blocking.
pub fn clear_refused(home: &Path, provider: &str, name: &str) -> Result<bool, FileError> {
    let refused = |s: &TokenStore| s.get(provider, name).is_some_and(|e| e.state == Some(PersistedState::Refused));
    if !refused(&TokenStore::load(home)?) {
        return Ok(false);
    }
    update(home, provider, name, |slot| match slot {
        Some(e) if e.state == Some(PersistedState::Refused) => {
            e.state = None;
            e.state_since = None;
            e.state_reason = None;
            true
        }
        _ => false,
    })
}

/// `accounts remove`: deletes the account's tokens under the lock. `true` when it had
/// some. Blocking.
pub fn remove(home: &Path, provider: &str, name: &str) -> Result<bool, FileError> {
    if TokenStore::load(home)?.get(provider, name).is_none() {
        return Ok(false);
    }
    update(home, provider, name, |slot| slot.take().is_some())
}

/// A sign-in account's service state (data-model § Account state machine).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountState {
    Active,
    /// The access token is past its expiry and the last refresh failed transiently
    /// (research R10, amended). In memory only.
    Refreshing {
        since: SystemTime,
        attempts: u32,
    },
    /// A permanent refresh failure, or no tokens. Persisted.
    NeedsSignIn {
        since: SystemTime,
        reason: String,
    },
    /// The provider refused a fresh token's request. Persisted.
    Refused {
        since: SystemTime,
        reason: String,
    },
    /// Operator choice (`accounts.toml`); never stored in a cell.
    Disabled,
}

impl AccountState {
    fn from_entry(e: &TokenEntry) -> Option<Self> {
        let since = e.state_since.unwrap_or(e.signed_in_at);
        let reason = e.state_reason.clone().unwrap_or_default();
        Some(match e.state? {
            PersistedState::NeedsSignIn => Self::NeedsSignIn { since, reason },
            PersistedState::Refused => Self::Refused { since, reason },
        })
    }

    /// Whether requests may use the account.
    pub fn serves(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// The state's name on the operator socket and in JSON listings.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Refreshing { .. } => "refreshing",
            Self::NeedsSignIn { .. } => "needs_sign_in",
            Self::Refused { .. } => "refused",
            Self::Disabled => "disabled",
        }
    }

    /// When the account entered the state, for the states that keep it.
    pub fn since(&self) -> Option<SystemTime> {
        match self {
            Self::Refreshing { since, .. } | Self::NeedsSignIn { since, .. } | Self::Refused { since, .. } => {
                Some(*since)
            }
            Self::Active | Self::Disabled => None,
        }
    }

    /// The provider's reason (redacted), for `needs_sign_in` and `refused`.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::NeedsSignIn { reason, .. } | Self::Refused { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

/// One `warn` line when an account's state changes kind (research R10). Counter updates
/// inside `refreshing` are not changes.
fn log_change(provider: &str, name: &str, old: &AccountState, new: &AccountState) {
    if std::mem::discriminant(old) == std::mem::discriminant(new) {
        return;
    }
    let (p, n) = (provider, name);
    match new {
        AccountState::Active => tracing::warn!(provider = p, account = n, "sign-in account is back in service"),
        AccountState::Refreshing { .. } => tracing::warn!(
            provider = p,
            account = n,
            "sign-in account token expired and its refresh is failing; retrying with backoff"
        ),
        AccountState::NeedsSignIn { reason, .. } => tracing::warn!(
            provider = p,
            account = n,
            "sign-in account needs sign-in ({reason}): run nullrouter accounts signin {p} {n}"
        ),
        AccountState::Refused { reason, .. } => tracing::warn!(
            provider = p,
            account = n,
            "sign-in account refused by the provider ({reason}): run nullrouter accounts signin {p} {n}, \
             or nullrouter accounts enable {p} {n} to retry"
        ),
        AccountState::Disabled => {}
    }
}

/// What a token cell holds: the entry, its state, and the generation it replaced (still
/// masked in logs, research R9).
#[derive(Debug)]
pub struct TokenView {
    pub entry: TokenEntry,
    pub state: AccountState,
    pub previous: Option<TokenPair>,
}

impl TokenView {
    /// `entry` replacing `old` (if any). The previous generation is `old`'s tokens when
    /// they differ, else `old`'s own previous one. A persisted state wins; otherwise `old`'s
    /// in-memory state is kept for the same access token, and a new token is `Active`.
    pub fn succeeding(entry: TokenEntry, old: Option<&TokenView>) -> Self {
        let same = old.is_some_and(|o| o.entry.access_token.with_exposed(|a| entry.access_token.matches(a)));
        let previous = match old {
            Some(o) if same => {
                o.previous.as_ref().map(|p| TokenPair { access: dup(&p.access), refresh: p.refresh.as_ref().map(dup) })
            }
            Some(o) => Some(o.entry.pair()),
            None => None,
        };
        let state = match (AccountState::from_entry(&entry), old) {
            (Some(s), _) => s,
            (None, Some(o)) if same && matches!(o.state, AccountState::Refreshing { .. }) => o.state.clone(),
            (None, _) => AccountState::Active,
        };
        Self { entry, state, previous }
    }

    /// Whether the access token is past its expiry at `now`.
    pub fn expired(&self, now: SystemTime) -> bool {
        now >= self.entry.expires_at
    }

    /// Every token this view knows, current and previous, for the redactor.
    pub fn secrets(&self) -> impl Iterator<Item = &SecretString> {
        let prev = self.previous.as_ref();
        std::iter::once(&self.entry.access_token)
            .chain(self.entry.refresh_token.as_ref())
            .chain(prev.map(|p| &p.access))
            .chain(prev.and_then(|p| p.refresh.as_ref()))
    }
}

type Key = (String, String);
/// One account's live token.
pub type TokenCell = Arc<ArcSwap<TokenView>>;

/// What `tokens.toml` looked like when it was last read: inode and modification time.
/// `write_private` renames a new file in, so every write changes the inode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp(Option<(u64, SystemTime)>);

fn stamp(home: &Path) -> Result<Stamp, FileError> {
    let path = path(home);
    match std::fs::symlink_metadata(&path) {
        Ok(m) => Ok(Stamp(Some((m.ino(), m.modified().unwrap_or(SystemTime::UNIX_EPOCH))))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Stamp(None)),
        Err(source) => Err(FileError::Io { path, source }),
    }
}

/// The live tokens of every sign-in account, kept by the engine across snapshots: a
/// reload re-reads `tokens.toml` only when it changed, and a refresh swaps one cell.
#[derive(Debug)]
pub struct TokenCells {
    cells: ArcSwap<BTreeMap<Key, TokenCell>>,
    loaded: Mutex<Stamp>,
}

impl Default for TokenCells {
    fn default() -> Self {
        Self { cells: ArcSwap::from_pointee(BTreeMap::new()), loaded: Mutex::new(Stamp(None)) }
    }
}

impl TokenCells {
    /// Reads `home/tokens.toml`. Refuses a shared file.
    pub fn load(home: &Path) -> Result<Self, FileError> {
        let cells = Self::default();
        cells.reload_if_changed(home)?;
        Ok(cells)
    }

    /// The file's content when it changed since it was last read; `None` keeps the cells.
    /// Reads only, so a caller can check every file before changing anything.
    pub fn changed(&self, home: &Path) -> Result<Option<(TokenStore, Stamp)>, FileError> {
        let now = stamp(home)?;
        if *self.loaded.lock().unwrap_or_else(|e| e.into_inner()) == now {
            return Ok(None);
        }
        Ok(Some((TokenStore::load(home)?, now)))
    }

    /// Makes the cells match `store` (read at `at`): changed entries swap their cell,
    /// rotating the previous generation; new entries get a cell; removed ones lose it.
    pub fn apply(&self, store: TokenStore, at: Stamp) {
        let old = self.cells.load();
        let mut next = BTreeMap::new();
        for entry in store.entries {
            let key = (entry.provider.clone(), entry.name.clone());
            let cell = match old.get(&key) {
                Some(cell) => {
                    let old = cell.load();
                    let view = TokenView::succeeding(entry, Some(&old));
                    log_change(&key.0, &key.1, &old.state, &view.state);
                    cell.store(Arc::new(view));
                    cell.clone()
                }
                None => Arc::new(ArcSwap::from_pointee(TokenView::succeeding(entry, None))),
            };
            next.insert(key, cell);
        }
        self.cells.store(Arc::new(next));
        *self.loaded.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }

    /// [`changed`](Self::changed) then [`apply`](Self::apply). Returns whether it re-read.
    pub fn reload_if_changed(&self, home: &Path) -> Result<bool, FileError> {
        let Some((store, at)) = self.changed(home)? else { return Ok(false) };
        self.apply(store, at);
        Ok(true)
    }

    /// The current view of one account's tokens: one atomic load.
    pub fn get(&self, provider: &str, name: &str) -> Option<Arc<TokenView>> {
        self.cell(provider, name).map(|c| c.load_full())
    }

    /// The cell itself, for the refresher.
    pub fn cell(&self, provider: &str, name: &str) -> Option<TokenCell> {
        self.cells.load().get(&(provider.to_owned(), name.to_owned())).cloned()
    }

    /// Swaps in an entry just written to `tokens.toml` (sign-in or refresh), keeping the
    /// replaced tokens as the previous generation. Rebuild the redactor afterwards.
    pub fn replace(&self, entry: TokenEntry) {
        let key = (entry.provider.clone(), entry.name.clone());
        if let Some(cell) = self.cell(&key.0, &key.1) {
            let old = cell.load();
            let view = TokenView::succeeding(entry, Some(&old));
            log_change(&key.0, &key.1, &old.state, &view.state);
            cell.store(Arc::new(view));
            return;
        }
        self.cells.rcu(|m| {
            let mut m = BTreeMap::clone(m);
            m.entry(key.clone())
                .or_insert_with(|| Arc::new(ArcSwap::from_pointee(TokenView::succeeding(dup_entry(&entry), None))));
            m
        });
    }

    /// Sets an account's in-memory state; `false` when it has no tokens.
    pub fn set_state(&self, provider: &str, name: &str, state: AccountState) -> bool {
        let Some(cell) = self.cell(provider, name) else { return false };
        let old = cell.rcu(|v| {
            Arc::new(TokenView {
                entry: dup_entry(&v.entry),
                state: state.clone(),
                previous: v
                    .previous
                    .as_ref()
                    .map(|p| TokenPair { access: dup(&p.access), refresh: p.refresh.as_ref().map(dup) }),
            })
        });
        log_change(provider, name, &old.state, &state);
        true
    }

    /// An account's state for listings: `Disabled` when the operator disabled it,
    /// `NeedsSignIn` for a sign-in account without tokens, `Active` for a key account.
    pub fn state(&self, account: &Account) -> AccountState {
        if account.disabled {
            return AccountState::Disabled;
        }
        if !account.is_signin() {
            return AccountState::Active;
        }
        match self.get(&account.provider, &account.name) {
            Some(v) => v.state.clone(),
            None => AccountState::NeedsSignIn { since: SystemTime::UNIX_EPOCH, reason: "not signed in".into() },
        }
    }

    /// Every account's current view.
    pub fn views(&self) -> Vec<Arc<TokenView>> {
        self.cells.load().values().map(|c| c.load_full()).collect()
    }

    pub fn len(&self) -> usize {
        self.cells.load().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A copy of an entry (secrets included), for building a successor view in memory.
pub fn dup_entry(e: &TokenEntry) -> TokenEntry {
    TokenEntry {
        provider: e.provider.clone(),
        name: e.name.clone(),
        access_token: dup(&e.access_token),
        refresh_token: e.refresh_token.as_ref().map(dup),
        expires_at: e.expires_at,
        scope: e.scope.clone(),
        claims: e.claims.clone(),
        hosts: e.hosts.clone(),
        signed_in_at: e.signed_in_at,
        last_refresh_at: e.last_refresh_at,
        state: e.state,
        state_since: e.state_since,
        state_reason: e.state_reason.clone(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    /// An active entry for `provider/name` with `access`, bound to `hosts`.
    pub(crate) fn entry(provider: &str, name: &str, access: &str, hosts: &[&str]) -> TokenEntry {
        let now = SystemTime::now();
        TokenEntry {
            provider: provider.into(),
            name: name.into(),
            access_token: SecretString::new(access),
            refresh_token: Some(SecretString::new(format!("{access}-refresh"))),
            expires_at: now + Duration::from_secs(3600),
            scope: "0".into(),
            claims: Claims { email: Some("a@example.com".into()), user_id: Some("u-1".into()), tier: None },
            hosts: hosts.iter().map(|h| (*h).to_owned()).collect(),
            signed_in_at: now,
            last_refresh_at: None,
            state: None,
            state_since: None,
            state_reason: None,
        }
    }

    fn put(home: &Path, e: TokenEntry) {
        let (p, n) = (e.provider.clone(), e.name.clone());
        update(home, &p, &n, |slot| *slot = Some(e)).unwrap();
    }

    #[test]
    fn round_trips_and_stays_private() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = entry("xai", "main", "xai-access-SENTINEL-1", &["api.x.ai"]);
        e.state = Some(PersistedState::NeedsSignIn);
        e.state_since = Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000));
        e.state_reason = Some("invalid_grant".into());
        put(dir.path(), e);
        let file = path(dir.path());
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir.path().join(LOCK)).unwrap().permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("state = \"needs_sign_in\"") && text.contains("[token.claims]"), "{text}");
        let s = TokenStore::load(dir.path()).unwrap();
        let e = s.get("xai", "main").unwrap();
        assert!(e.access_token.matches("xai-access-SENTINEL-1"));
        assert!(e.refresh_token.as_ref().unwrap().matches("xai-access-SENTINEL-1-refresh"));
        assert_eq!(e.claims.email.as_deref(), Some("a@example.com"));
        assert_eq!(e.state_reason.as_deref(), Some("invalid_grant"));
        assert_eq!(e.shown_token(), "…EL-1");
        assert!(!format!("{s:?}").contains("SENTINEL"), "Debug hides tokens");

        update(dir.path(), "xai", "main", |slot| *slot = None).unwrap();
        assert!(TokenStore::load(dir.path()).unwrap().entries.is_empty(), "None deletes");
    }

    #[test]
    fn a_shared_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), entry("xai", "main", "xai-access-0000", &[]));
        std::fs::set_permissions(path(dir.path()), std::fs::Permissions::from_mode(0o640)).unwrap();
        let err = TokenStore::load(dir.path()).unwrap_err().to_string();
        assert!(err.contains("tokens.toml") && err.contains("chmod 600"), "{err}");
        assert!(TokenCells::load(dir.path()).is_err());
        assert!(update(dir.path(), "xai", "main", |_| ()).is_err(), "writers refuse it too");
    }

    /// L5: a symlinked tokens.toml or tokens.lock is refused by readers and writers, and
    /// the link's target is left alone.
    #[test]
    fn symlinked_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let other = dir.path().join("other");
        put(&other, entry("xai", "main", "xai-access-0000", &[]));
        std::fs::create_dir_all(&home).unwrap();
        std::os::unix::fs::symlink(path(&other), path(&home)).unwrap();
        let err = TokenStore::load(&home).unwrap_err().to_string();
        assert!(err.contains("symbolic link"), "{err}");
        assert!(TokenCells::load(&home).is_err(), "serve refuses it");
        assert!(update(&home, "xai", "main", |_| ()).is_err(), "writers refuse it");
        assert!(std::fs::symlink_metadata(path(&home)).unwrap().file_type().is_symlink(), "the link is untouched");

        let lock_home = dir.path().join("lock");
        std::fs::create_dir_all(&lock_home).unwrap();
        std::os::unix::fs::symlink(dir.path().join("lock-target"), lock_home.join(LOCK)).unwrap();
        let err = update(&lock_home, "xai", "main", |_| ()).unwrap_err().to_string();
        assert!(err.contains("tokens.lock") && err.contains("symbolic link"), "{err}");
        assert!(!dir.path().join("lock-target").exists(), "nothing created through the link");
    }

    #[test]
    fn concurrent_writers_never_lose_an_update() {
        let dir = tempfile::tempdir().unwrap();
        put(dir.path(), entry("xai", "main", "xai-access-0000", &[]));
        const EACH: usize = 25;
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let home = dir.path().to_owned();
                std::thread::spawn(move || {
                    for _ in 0..EACH {
                        update(&home, "xai", "main", |slot| {
                            let e = slot.as_mut().unwrap();
                            e.scope = (e.scope.parse::<usize>().unwrap() + 1).to_string();
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(TokenStore::load(dir.path()).unwrap().get("xai", "main").unwrap().scope, (2 * EACH).to_string());
    }

    #[test]
    fn bad_files_are_refused() {
        let p = Path::new("tokens.toml");
        let ok = "schema = 1\n[[token]]\nprovider = \"x\"\nname = \"m\"\naccess_token = \"a\"\nexpires_at = \"2026-10-02T00:00:00Z\"\nsigned_in_at = \"2026-10-02T00:00:00Z\"\n";
        assert_eq!(TokenStore::parse(ok, p).unwrap().entries.len(), 1);
        assert!(TokenStore::parse(&ok.replace("2026-10-02T00:00:00Z\"\nsigned", "soon\"\nsigned"), p).is_err());
        assert!(TokenStore::parse(&format!("{ok}{}", &ok[11..]), p).unwrap_err().to_string().contains("twice"));
        assert!(TokenStore::parse("schema = 2\n", p).is_err());
        assert!(TokenStore::parse(&ok.replace("name = \"m\"", "name = \"M!\""), p).is_err());
    }

    #[test]
    fn cells_follow_the_file_and_keep_the_previous_generation() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let cells = TokenCells::load(home).unwrap();
        assert!(cells.is_empty());
        put(home, entry("xai", "main", "gen-1-access", &[]));
        assert!(cells.reload_if_changed(home).unwrap());
        assert!(!cells.reload_if_changed(home).unwrap(), "unchanged file: cells kept");
        let held = cells.cell("xai", "main").unwrap();
        cells.set_state("xai", "main", AccountState::Refreshing { since: SystemTime::now(), attempts: 1 });

        put(home, entry("xai", "main", "gen-2-access", &[]));
        assert!(cells.reload_if_changed(home).unwrap());
        let v = held.load();
        assert!(Arc::ptr_eq(&held, &cells.cell("xai", "main").unwrap()), "the same cell is swapped");
        assert!(v.entry.access_token.matches("gen-2-access"));
        assert!(v.previous.as_ref().unwrap().access.matches("gen-1-access"));
        assert_eq!(v.state, AccountState::Active, "a new token is active");
        assert_eq!(v.secrets().count(), 4);

        cells.replace(entry("xai", "main", "gen-3-access", &[]));
        let v = cells.get("xai", "main").unwrap();
        assert!(v.previous.as_ref().unwrap().access.matches("gen-2-access"));

        let mut refused = entry("xai", "main", "gen-3-access", &[]);
        refused.state = Some(PersistedState::Refused);
        refused.state_reason = Some("only authorized for Claude Code".into());
        cells.replace(refused);
        let v = cells.get("xai", "main").unwrap();
        assert!(matches!(&v.state, AccountState::Refused { reason, .. } if reason.contains("Claude Code")));
        assert!(v.previous.as_ref().unwrap().access.matches("gen-2-access"), "same token: previous kept");

        cells.replace(entry("grok-cli", "work", "grok-access", &[]));
        assert_eq!(cells.len(), 2);
        update(home, "xai", "main", |s| *s = None).unwrap();
        cells.reload_if_changed(home).unwrap();
        assert!(cells.get("xai", "main").is_none() && cells.get("grok-cli", "work").is_none(), "file is the truth");
    }

    #[test]
    fn listing_states() {
        let cells = TokenCells::default();
        let mut a = Account::signin("xai", "main", 0);
        assert!(matches!(cells.state(&a), AccountState::NeedsSignIn { .. }));
        cells.replace(entry("xai", "main", "xai-access-0000", &[]));
        assert_eq!(cells.state(&a), AccountState::Active);
        a.disabled = true;
        assert_eq!(cells.state(&a), AccountState::Disabled);
        assert!(!cells.set_state("xai", "nobody", AccountState::Active));
    }

    #[test]
    fn out_of_service_states_persist_and_clear() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_000_000);
        assert!(persist_state(home, "xai", "main", PersistedState::Refused, "x", at, None).unwrap().is_none());
        assert!(!path(home).exists(), "no tokens: nothing written");
        put(home, entry("xai", "main", "gen-1-access", &[]));

        let other = SecretString::new("gen-0-access");
        let stale = persist_state(home, "xai", "main", PersistedState::Refused, "x", at, Some(&other)).unwrap();
        assert!(stale.is_none(), "a replaced token is not marked");
        let mine = SecretString::new("gen-1-access");
        let e = persist_state(home, "xai", "main", PersistedState::Refused, "only Claude Code", at, Some(&mine))
            .unwrap()
            .unwrap();
        assert_eq!(e.state, Some(PersistedState::Refused));
        let cells = TokenCells::load(home).unwrap();
        let v = cells.get("xai", "main").unwrap();
        assert_eq!(
            v.state,
            AccountState::Refused { since: at, reason: "only Claude Code".into() },
            "survives a restart"
        );
        assert_eq!(
            (v.state.name(), v.state.since(), v.state.reason()),
            ("refused", Some(at), Some("only Claude Code"))
        );

        assert!(clear_refused(home, "xai", "main").unwrap());
        assert!(!clear_refused(home, "xai", "main").unwrap(), "only once");
        assert!(cells.reload_if_changed(home).unwrap());
        assert_eq!(cells.get("xai", "main").unwrap().state, AccountState::Active);

        persist_state(home, "xai", "main", PersistedState::NeedsSignIn, "invalid_grant", at, None).unwrap();
        assert!(!clear_refused(home, "xai", "main").unwrap(), "enable doesn't clear needs sign-in");
        assert_eq!(
            TokenStore::load(home).unwrap().get("xai", "main").unwrap().state,
            Some(PersistedState::NeedsSignIn)
        );

        assert!(remove(home, "xai", "main").unwrap());
        assert!(!remove(home, "xai", "main").unwrap());
        assert!(TokenStore::load(home).unwrap().entries.is_empty());
        assert!(!clear_refused(home, "nobody", "x").unwrap());
    }
}
