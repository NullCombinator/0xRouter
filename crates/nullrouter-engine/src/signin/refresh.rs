//! The refresh-token grant, rotation, and the write-before-swap order (research R9).
//!
//! [`refresh`] is the HTTP call alone. [`Engine::refresh_account`] is the whole refresh of
//! one account: deduplicated per account ([`Dedup`]), it writes the new tokens to
//! `tokens.toml` under `tokens.lock` *before* swapping the account's token cell, then
//! rebuilds the redactor (which keeps the previous generation masked). A failure is
//! classified ([`classify`]): permanent persists `needs_sign_in`; transient is retried with
//! backoff ([`Timing::backoff`]) and leaves the account `Active` while its token is still
//! valid, `Refreshing` once it has expired (research R10, FR-013).
//!
//! A request already sent keeps the token it was sent with: a running stream is never
//! touched by a refresh.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use nullrouter_registry::SecretString;
use nullrouter_registry::schema::{ProviderEntity, SignInDecl};

use super::classify::{RefreshClass, classify};
use super::dedup::Dedup;
use super::{DEFAULT_EXPIRES_IN, SignInError, SignInHttp, encode_body, token_request};
use crate::files::FileError;
use crate::state::Engine;
use crate::tokens::{self, AccountState, PersistedState, TokenEntry, TokenView, dup_entry};

/// Freshness timing (research R9, R10). [`Timing::default`] is the product's; tests
/// shorten it with [`Refresher::set_timing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timing {
    /// The smallest refresh lead: 9router's background refresher's 30 min.
    pub min_lead: Duration,
    /// A token this close to expiry is refreshed before a request uses it.
    pub use_margin: Duration,
    /// Each refresh call's limit; a request waits at most this long for a refresh.
    pub timeout: Duration,
    /// Waits after consecutive transient failures; the last one repeats.
    pub backoff: Vec<Duration>,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            min_lead: Duration::from_secs(30 * 60),
            use_margin: Duration::from_secs(30),
            timeout: Duration::from_secs(15),
            backoff: [10, 30, 60, 120].map(Duration::from_secs).to_vec(),
        }
    }
}

impl Timing {
    /// The wait after the `attempts`-th consecutive transient failure (1-based).
    pub fn backoff_after(&self, attempts: u32) -> Duration {
        let i = (attempts.max(1) - 1) as usize;
        self.backoff.get(i).or(self.backoff.last()).copied().unwrap_or(Duration::from_secs(120))
    }
}

/// The lead before expiry at which a token is refreshed: the larger of the plugin's
/// `refresh_lead` and `min_lead`, capped at half the token's `lifetime` (research R9).
pub fn lead(plugin_lead: Duration, min_lead: Duration, lifetime: Duration) -> Duration {
    plugin_lead.max(min_lead).min(lifetime / 2)
}

/// How long a token lives: from its receipt (last refresh, else sign-in) to its expiry.
pub fn lifetime(e: &TokenEntry) -> Duration {
    let from = e.last_refresh_at.unwrap_or(e.signed_in_at);
    e.expires_at.duration_since(from).unwrap_or_default()
}

/// When the maintenance task refreshes this token: `expires_at − lead`. `None` when it
/// can't be refreshed (no refresh token) or the account is out of service for good.
pub fn refresh_at(view: &TokenView, decl: &SignInDecl, timing: &Timing) -> Option<SystemTime> {
    if view.entry.refresh_token.is_none()
        || matches!(view.state, AccountState::NeedsSignIn { .. } | AccountState::Refused { .. })
    {
        return None;
    }
    let lead = lead(decl.refresh_lead, timing.min_lead, lifetime(&view.entry));
    Some(view.entry.expires_at.checked_sub(lead).unwrap_or(SystemTime::UNIX_EPOCH))
}

/// Whether a request about to use this token refreshes it first: within the use margin
/// of expiry (capped, like the lead, at half the token's lifetime).
pub fn near_expiry(view: &TokenView, timing: &Timing, now: SystemTime) -> bool {
    let margin = timing.use_margin.min(lifetime(&view.entry) / 2);
    now + margin >= view.entry.expires_at
}

/// The refresh-token grant for `entry` at `provider`'s declared token URL, with its
/// declared body encoding: exactly `grant_type`, `refresh_token`, `client_id`. Returns the
/// successor entry: the new access token, the new refresh token or the old one when the
/// response omits it (rotation, `providers.js:131`), and `expires_at` from the local
/// clock. No state change is made here.
pub async fn refresh(
    http: &SignInHttp,
    provider: &ProviderEntity,
    entry: &TokenEntry,
) -> Result<TokenEntry, SignInError> {
    let decl = provider.signin.as_ref().ok_or_else(|| SignInError::NotSignIn(provider.id.clone()))?;
    let old = entry.refresh_token.as_ref().ok_or_else(|| SignInError::Rejected {
        what: "token endpoint",
        status: 400,
        code: "no refresh token".into(),
    })?;
    let body = old.with_exposed(|r| {
        encode_body(
            decl.body,
            &[("grant_type", "refresh_token"), ("refresh_token", r), ("client_id", decl.client_id.as_str())],
        )
    });
    let grant = token_request(http, &decl.token_url, body).await?;
    let now = SystemTime::now();
    let mut next = dup_entry(entry);
    next.access_token = grant.access_token;
    if let Some(r) = grant.refresh_token {
        next.refresh_token = Some(r);
    }
    next.expires_at = now + grant.expires_in.unwrap_or(DEFAULT_EXPIRES_IN);
    if let Some(scope) = grant.scope {
        next.scope = scope;
    }
    next.last_refresh_at = Some(now);
    next.state = None;
    next.state_since = None;
    next.state_reason = None;
    Ok(next)
}

/// Writes `entry` over its account's entry in `home/tokens.toml` under `tokens.lock`.
/// `false` (nothing written) when the account has no entry any more: it was removed
/// while the refresh ran. Blocking.
pub fn persist(home: &Path, entry: &TokenEntry) -> Result<bool, FileError> {
    tokens::update(home, &entry.provider, &entry.name, |slot| match slot {
        Some(_) => {
            *slot = Some(dup_entry(entry));
            true
        }
        None => false,
    })
}

/// How one refresh ended, as every waiter sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refreshed {
    /// The cell holds a fresh token (or already did).
    Fresh,
    /// Failed transiently; retried with backoff. The reason is redacted.
    Transient(String),
    /// Failed for good: the account needs signing in again. The reason is redacted.
    Permanent(String),
}

type Key = (String, String);

/// Consecutive transient failures of one account.
#[derive(Debug, Clone, Copy)]
struct Failures {
    attempts: u32,
    last: SystemTime,
    since: SystemTime,
}

/// The engine's refresh state: in-flight refreshes, failure backoff and timing.
#[derive(Debug, Default)]
pub struct Refresher {
    inflight: Dedup<Key, Refreshed>,
    failures: Mutex<HashMap<Key, Failures>>,
    timing: Mutex<Timing>,
}

impl Refresher {
    pub fn timing(&self) -> Timing {
        self.timing.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replaces the timing (tests shorten it).
    pub fn set_timing(&self, t: Timing) {
        *self.timing.lock().unwrap_or_else(|e| e.into_inner()) = t;
    }

    fn failures(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Failures>> {
        self.failures.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// When the next retry after transient failures is allowed; `None` without failures.
    pub fn retry_at(&self, provider: &str, name: &str) -> Option<SystemTime> {
        let f = *self.failures().get(&(provider.to_owned(), name.to_owned()))?;
        Some(f.last + self.timing().backoff_after(f.attempts))
    }

    /// Consecutive transient failures so far.
    pub fn attempts(&self, provider: &str, name: &str) -> u32 {
        self.failures().get(&(provider.to_owned(), name.to_owned())).map_or(0, |f| f.attempts)
    }

    /// Whether a refresh of the account is running.
    pub fn busy(&self, provider: &str, name: &str) -> bool {
        self.inflight.busy(&(provider.to_owned(), name.to_owned()))
    }

    fn backing_off(&self, key: &Key, now: SystemTime) -> bool {
        self.retry_at(&key.0, &key.1).is_some_and(|at| now < at)
    }

    fn failed(&self, key: &Key, now: SystemTime) -> Failures {
        let mut map = self.failures();
        let f = map.entry(key.clone()).or_insert(Failures { attempts: 0, last: now, since: now });
        f.attempts += 1;
        f.last = now;
        *f
    }

    fn cleared(&self, key: &Key) {
        self.failures().remove(key);
    }
}

impl Engine {
    /// Refreshes `provider/name` now, deduplicated: concurrent callers share one refresh
    /// (FR-012). Ignores backoff; [`Engine::fresh_for_use`] and the maintenance task check
    /// it first.
    pub async fn refresh_account(self: &Arc<Self>, provider: &str, name: &str) -> Refreshed {
        let key = (provider.to_owned(), name.to_owned());
        let this = self.clone();
        let k = key.clone();
        let aborted = Refreshed::Transient("the refresh task failed".into());
        self.refresher.inflight.run(key, async move { this.refresh_now(&k).await }, aborted).await
    }

    /// Use-time freshness (research R9): when the account's token is within the use
    /// margin of expiry, refresh it and wait (at most the refresh timeout). While a
    /// transient failure's backoff runs, a token still valid is used as it is, and an
    /// expired one marks the account `Refreshing` (the request falls back).
    pub async fn fresh_for_use(self: &Arc<Self>, provider: &str, name: &str) -> Refreshed {
        let Some(view) = self.tokens.get(provider, name) else { return Refreshed::Fresh };
        let now = SystemTime::now();
        if !matches!(view.state, AccountState::Active) || !near_expiry(&view, &self.refresher.timing(), now) {
            return Refreshed::Fresh;
        }
        self.refresh_unless_backing_off(&view, provider, name, now).await
    }

    /// A provider rejected `sent` (401, or a 403 read as an auth rejection): refresh,
    /// unless another request already replaced that token, in which case the new one is
    /// simply used (research R9).
    pub async fn refresh_rejected(self: &Arc<Self>, provider: &str, name: &str, sent: &TokenView) -> Refreshed {
        let Some(view) = self.tokens.get(provider, name) else {
            return Refreshed::Permanent("not signed in".into());
        };
        if !sent.entry.access_token.with_exposed(|t| view.entry.access_token.matches(t)) {
            return Refreshed::Fresh;
        }
        self.refresh_unless_backing_off(&view, provider, name, SystemTime::now()).await
    }

    async fn refresh_unless_backing_off(
        self: &Arc<Self>,
        view: &TokenView,
        provider: &str,
        name: &str,
        now: SystemTime,
    ) -> Refreshed {
        let key = (provider.to_owned(), name.to_owned());
        if self.refresher.backing_off(&key, now) && !self.refresher.inflight.busy(&key) {
            if view.expired(now) {
                self.mark_refreshing(&key, now);
            }
            return Refreshed::Transient("token refresh failed; retrying with backoff".into());
        }
        self.refresh_account(provider, name).await
    }

    /// Marks the account `Refreshing` (its token has expired and refreshing failed).
    fn mark_refreshing(&self, key: &Key, now: SystemTime) {
        let (since, attempts) = self.refresher.failures().get(key).map_or((now, 1), |f| (f.since, f.attempts));
        // The cell logs the change (one `warn` line).
        self.tokens.set_state(&key.0, &key.1, AccountState::Refreshing { since, attempts });
    }

    /// One refresh, not deduplicated: the HTTP call, then the write before the swap.
    async fn refresh_now(self: &Arc<Self>, key: &Key) -> Refreshed {
        let (provider, name) = (key.0.as_str(), key.1.as_str());
        let Some(view) = self.tokens.get(provider, name) else {
            return Refreshed::Permanent("not signed in".into());
        };
        let st = self.snapshot();
        let Ok(entity) = st.registry.provider(provider) else {
            return Refreshed::Transient(format!("provider {provider} isn't loaded"));
        };
        let allow_private = st.settings().allow_private_endpoints;
        let http =
            SignInHttp::with_client(st.http.clone(), allow_private).with_timeout(self.refresher.timing().timeout);
        let result = refresh(&http, entity, &view.entry).await;
        let now = SystemTime::now();
        match result {
            Ok(next) => {
                let this = self.clone();
                let swapped = tokio::task::spawn_blocking(move || {
                    // Write first: a crash after this line still leaves the newest refresh
                    // token on disk (FR-014).
                    match persist(this.home().path(), &next) {
                        Ok(true) => {}
                        Ok(false) => return false,
                        Err(e) => tracing::error!("refreshed tokens not saved (kept in memory): {e}"),
                    }
                    this.tokens.replace(next);
                    this.rebuild_redactor();
                    true
                })
                .await
                .unwrap_or(false);
                self.refresher.cleared(key);
                if swapped {
                    tracing::debug!(provider, account = name, "sign-in token refreshed");
                    Refreshed::Fresh
                } else {
                    Refreshed::Permanent("not signed in".into())
                }
            }
            Err(e) => {
                let reason = st.redactor.redact(&e.to_string()).into_owned();
                match classify(&e) {
                    RefreshClass::Permanent => {
                        self.refresher.cleared(key);
                        self.needs_sign_in(key, &e).await;
                        Refreshed::Permanent(reason)
                    }
                    RefreshClass::Transient => {
                        let f = self.refresher.failed(key, now);
                        if view.expired(now) {
                            self.mark_refreshing(key, now);
                        }
                        tracing::warn!(
                            provider,
                            account = name,
                            attempts = f.attempts,
                            "sign-in token refresh failed ({reason}); retrying in {} s",
                            self.refresher.timing().backoff_after(f.attempts).as_secs()
                        );
                        Refreshed::Transient(reason)
                    }
                }
            }
        }
    }

    /// Persists `needs_sign_in` with the time and the provider's error code (research R10),
    /// then swaps the cell.
    async fn needs_sign_in(self: &Arc<Self>, key: &Key, e: &SignInError) {
        let code = match e {
            SignInError::Rejected { code, .. } => code.clone(),
            other => other.to_string(),
        };
        self.take_out_of_service(&key.0, &key.1, PersistedState::NeedsSignIn, &code, None).await;
    }

    /// The provider refused a fresh token's request (FR-004b): persists `refused` with the
    /// provider's reason, unless the account's token is no longer `sent`'s.
    pub async fn refuse(self: &Arc<Self>, provider: &str, name: &str, reason: &str, sent: &TokenView) {
        let token = sent.entry.access_token.with_exposed(|t| SecretString::new(t));
        self.take_out_of_service(provider, name, PersistedState::Refused, reason, Some(token)).await;
    }

    /// Writes `state` to `tokens.toml` under the lock, then swaps the cell, which logs the
    /// change. When the write fails the state is still set in memory.
    async fn take_out_of_service(
        self: &Arc<Self>,
        provider: &str,
        name: &str,
        state: PersistedState,
        reason: &str,
        token: Option<SecretString>,
    ) {
        let reason = short(&self.snapshot().redactor.redact(reason));
        let now = SystemTime::now();
        let this = self.clone();
        let (p, n) = (provider.to_owned(), name.to_owned());
        let _ = tokio::task::spawn_blocking(move || {
            match tokens::persist_state(this.home().path(), &p, &n, state, &reason, now, token.as_ref()) {
                Ok(Some(entry)) => this.tokens.replace(entry),
                Ok(None) => {}
                Err(err) => {
                    tracing::error!("{p}/{n}: account state not saved: {err}");
                    let st = match state {
                        PersistedState::NeedsSignIn => AccountState::NeedsSignIn { since: now, reason },
                        PersistedState::Refused => AccountState::Refused { since: now, reason },
                    };
                    this.tokens.set_state(&p, &n, st);
                }
            }
        })
        .await;
    }
}

/// A reason kept with a state: one line, at most 200 characters.
pub(crate) fn short(reason: &str) -> String {
    let line = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(200) {
        Some((i, _)) => format!("{}…", &line[..i]),
        None => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_is_the_larger_capped_at_half_the_lifetime() {
        let (m, h) = (Duration::from_secs(60), Duration::from_secs(3600));
        let min = Duration::from_secs(30 * 60);
        assert_eq!(lead(5 * m, min, 24 * h), 30 * m, "xai: 30 min, not 5");
        assert_eq!(lead(4 * h, min, 24 * h), 4 * h, "anthropic: its own 4 h");
        assert_eq!(lead(5 * m, min, 40 * m), 20 * m, "a 40-minute token refreshes after 20");
        assert_eq!(lead(Duration::from_secs(1), min, Duration::from_secs(2)), Duration::from_secs(1));
    }

    #[test]
    fn backoff_repeats_its_last_step() {
        let t = Timing::default();
        let secs: Vec<u64> = (1..=6).map(|n| t.backoff_after(n).as_secs()).collect();
        assert_eq!(secs, [10, 30, 60, 120, 120, 120]);
    }
}
