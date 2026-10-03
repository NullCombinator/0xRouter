//! One quota poll of one account: request, fallback, extraction, failure class (research R11).
//!
//! A poll sends the `[quota]` request with the account's credential, released only to the
//! hosts it is bound to (research R5): a key account's secret, or a sign-in account's access
//! token placed where `[signin] auth` says, with the `[identity]` headers. The client is the
//! engine's: SSRF-checked, never following a redirect. When the primary source yields no
//! window, the fallback source is read. A 401 on a sign-in account refreshes the token
//! (deduplicated with every other refresh) and retries once, as a request would.
//!
//! The [`QuotaBoard`] keeps, per account and in memory only, the latest good poll, the last
//! failure, and when the next poll is due: the account's interval with ±10% jitter, or one
//! retry a minute after a failure (none after a 429). Every completed poll is handed to the
//! hooks registered with [`QuotaBoard::on_poll`] (poll history, US5).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use indexmap::IndexMap;
use nullrouter_registry::ProviderEntity;
use nullrouter_registry::schema::{AuthScheme, QuotaAccounts, QuotaBody, QuotaDecl};
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use serde::{Serialize, Serializer};

use super::QuotaWindow;
use crate::accounts::{self, Account, Released};
use crate::identity::{self, FillContext};
use crate::keys::AgentId;
use crate::signin::refresh::Refreshed;
use crate::state::{Engine, EngineState};

/// The agent name background calls (polls, live model lists) fill `{session.id}` for.
const MAINTENANCE_AGENT: &str = "nullrouter-maintenance";

/// Polling timing (research R11). Tests shorten it.
#[derive(Debug, Clone, PartialEq)]
pub struct PollTiming {
    /// Multiplies every account's interval and every `[models_live] refresh` (1.0 in the
    /// product).
    pub scale: f64,
    /// The one retry after a failed poll.
    pub retry_after: Duration,
    /// The interval varies by up to this fraction either way.
    pub jitter: f64,
    /// The longest wait for one answer.
    pub timeout: Duration,
}

impl Default for PollTiming {
    fn default() -> Self {
        Self { scale: 1.0, retry_after: Duration::from_secs(60), jitter: 0.1, timeout: Duration::from_secs(30) }
    }
}

/// Why a poll gave no windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PollErrorClass {
    /// No answer within the timeout.
    Timeout,
    /// The connection failed.
    Network,
    /// The provider answered with a redirect, which is never followed.
    Redirect,
    /// `429`: no retry; the next interval polls again.
    RateLimited,
    /// `401` (after a refresh, for a sign-in account) or `403`.
    Rejected,
    /// Any other non-2xx status.
    Status,
    /// A 2xx answer in which no source reported a window.
    NoWindows,
    /// The credential can't be sent (needs sign-in, refused, unset variable, unbound host).
    Withheld,
    /// The request couldn't be built.
    Invalid,
}

/// A failed poll's class and redacted reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PollError {
    pub class: PollErrorClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    pub reason: String,
}

impl PollError {
    fn new(class: PollErrorClass, reason: impl Into<String>) -> Self {
        Self { class, status: None, reason: reason.into() }
    }

    fn status(status: u16, reason: impl Into<String>) -> Self {
        let class = match status {
            429 => PollErrorClass::RateLimited,
            401 | 403 => PollErrorClass::Rejected,
            300..=399 => PollErrorClass::Redirect,
            _ => PollErrorClass::Status,
        };
        Self { class, status: Some(status), reason: reason.into() }
    }

    /// The short form the CLI shows: `timeout`, `HTTP 500`, `rate limited`, ….
    pub fn summary(&self) -> String {
        match (self.class, self.status) {
            (PollErrorClass::Timeout, _) => "timeout".into(),
            (PollErrorClass::Network, _) => "network error".into(),
            (PollErrorClass::RateLimited, _) => "rate limited".into(),
            (PollErrorClass::NoWindows, _) => "no quota in the answer".into(),
            (PollErrorClass::Withheld, _) => self.reason.clone(),
            (PollErrorClass::Invalid, _) => "invalid request".into(),
            (_, Some(s)) => format!("HTTP {s}"),
            (_, None) => self.reason.clone(),
        }
    }
}

/// One completed poll of one account.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QuotaPoll {
    pub provider: String,
    pub account: String,
    #[serde(serialize_with = "ser_time")]
    pub at: SystemTime,
    /// The windows reported; empty on failure.
    pub windows: Vec<QuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<PollError>,
    /// Whether this poll was the one retry after a failure.
    pub retry: bool,
}

impl QuotaPoll {
    pub fn ok(&self) -> bool {
        self.error.is_none()
    }
}

fn ser_time<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&crate::clock::rfc3339(*t))
}

/// Called with every completed poll, good or failed.
pub type PollHook = Arc<dyn Fn(&QuotaPoll) + Send + Sync>;

#[derive(Debug, Clone)]
struct Slot {
    last_at: SystemTime,
    factor: f64,
    retry_pending: bool,
    latest: Option<QuotaPoll>,
    last_failure: Option<QuotaPoll>,
}

/// What the board knows about one account.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AccountQuota {
    /// The latest good poll.
    pub latest: Option<QuotaPoll>,
    /// The latest failed poll.
    pub last_failure: Option<QuotaPoll>,
}

/// Per-account quota state, in memory (research R11).
#[derive(Default)]
pub struct QuotaBoard {
    slots: Mutex<HashMap<(String, String), Slot>>,
    timing: Mutex<PollTiming>,
    hooks: Mutex<Vec<PollHook>>,
}

impl std::fmt::Debug for QuotaBoard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuotaBoard").field("accounts", &lock(&self.slots).len()).finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A factor in `[1 − spread, 1 + spread]`.
pub fn jitter_factor(spread: f64) -> f64 {
    let mut b = [0u8; 4];
    if getrandom::fill(&mut b).is_err() {
        return 1.0;
    }
    let u = f64::from(u32::from_le_bytes(b)) / f64::from(u32::MAX);
    1.0 + spread * (2.0 * u - 1.0)
}

impl QuotaBoard {
    pub fn timing(&self) -> PollTiming {
        lock(&self.timing).clone()
    }

    /// Replaces the timing (tests shorten it).
    pub fn set_timing(&self, t: PollTiming) {
        *lock(&self.timing) = t;
    }

    /// Registers `hook`, called with every completed poll from then on.
    pub fn on_poll(&self, hook: PollHook) {
        lock(&self.hooks).push(hook);
    }

    pub fn get(&self, provider: &str, name: &str) -> AccountQuota {
        lock(&self.slots)
            .get(&(provider.to_owned(), name.to_owned()))
            .map(|s| AccountQuota { latest: s.latest.clone(), last_failure: s.last_failure.clone() })
            .unwrap_or_default()
    }

    /// When the account is next polled: now when never polled; a retry delay after a failure
    /// not yet retried; else the last poll plus `interval` times its jitter factor.
    pub fn due(&self, provider: &str, name: &str, interval: Duration) -> SystemTime {
        let retry_after = self.timing().retry_after;
        match lock(&self.slots).get(&(provider.to_owned(), name.to_owned())) {
            None => UNIX_EPOCH,
            Some(s) if s.retry_pending => s.last_at + retry_after,
            Some(s) => s.last_at + interval.mul_f64(s.factor),
        }
    }

    fn retry_pending(&self, provider: &str, name: &str) -> bool {
        lock(&self.slots).get(&(provider.to_owned(), name.to_owned())).is_some_and(|s| s.retry_pending)
    }

    /// Keeps `poll` and tells the hooks.
    fn record(&self, poll: QuotaPoll) {
        let factor = jitter_factor(self.timing().jitter);
        {
            let mut slots = lock(&self.slots);
            let slot = slots.entry((poll.provider.clone(), poll.account.clone())).or_insert(Slot {
                last_at: poll.at,
                factor,
                retry_pending: false,
                latest: None,
                last_failure: None,
            });
            slot.last_at = poll.at;
            slot.factor = factor;
            match &poll.error {
                None => {
                    slot.retry_pending = false;
                    slot.latest = Some(poll.clone());
                }
                Some(e) => {
                    slot.retry_pending = !poll.retry && e.class != PollErrorClass::RateLimited;
                    slot.last_failure = Some(poll.clone());
                }
            }
        }
        let hooks = lock(&self.hooks).clone();
        for h in hooks {
            h(&poll);
        }
    }
}

/// The `[quota]` section that covers `account`: its provider declares one for the account's
/// kind. `None`: "quota not reported".
pub fn reported<'p>(provider: &'p ProviderEntity, account: &Account) -> Option<&'p QuotaDecl> {
    let q = provider.quota.as_ref()?;
    let covered = match q.accounts {
        QuotaAccounts::Any => true,
        QuotaAccounts::Signin => account.is_signin(),
        QuotaAccounts::Key => !account.is_signin(),
    };
    covered.then_some(q)
}

/// One credentialed call of a background job (a quota source, a live model list).
pub(crate) struct AccountCall<'a> {
    pub url: &'a str,
    pub method: &'a str,
    pub headers: &'a IndexMap<String, String>,
    pub body: Bytes,
}

impl Engine {
    /// Polls `provider/name` now and keeps the result. `None` when there is no such account
    /// or its quota isn't reported.
    pub async fn poll_quota(self: &Arc<Self>, provider: &str, name: &str) -> Option<QuotaPoll> {
        let st = self.snapshot();
        let account = st.accounts.get(provider, name)?;
        let entity = st.registry.provider(provider).ok()?;
        let decl = reported(entity, account)?;
        let retry = self.quota.retry_pending(provider, name);
        let result = self.read_quota(&st, entity, account, decl).await;
        let at = SystemTime::now();
        let (windows, error) = match result {
            Ok(w) => (w, None),
            Err(e) => (Vec::new(), Some(e)),
        };
        if let Some(e) = &error {
            tracing::warn!(provider, account = name, "quota poll failed: {} ({})", e.summary(), e.reason);
        }
        let poll = QuotaPoll { provider: provider.to_owned(), account: name.to_owned(), at, windows, error, retry };
        self.quota.record(poll.clone());
        Some(poll)
    }

    /// The primary source, then the fallback when the primary yields no window.
    async fn read_quota(
        self: &Arc<Self>,
        st: &EngineState,
        entity: &ProviderEntity,
        account: &Account,
        decl: &QuotaDecl,
    ) -> Result<Vec<QuotaWindow>, PollError> {
        for source in decl.sources() {
            let req = &source.request;
            let body = match req.body {
                QuotaBody::None => Bytes::new(),
                QuotaBody::GrpcWebEmpty => Bytes::from_static(&[0u8; 5]),
            };
            let call = AccountCall { url: &req.url, method: &req.method, headers: &req.headers, body };
            let answer = self.account_call(st, entity, account, call).await?;
            let windows = super::read(source, &answer);
            if !windows.is_empty() {
                return Ok(windows);
            }
        }
        Err(PollError::new(PollErrorClass::NoWindows, "the answer reported no quota window"))
    }

    /// Sends `call` with `account`'s credential and returns a 2xx body. A 401 on a sign-in
    /// account refreshes its token and retries once.
    pub(crate) async fn account_call(
        self: &Arc<Self>,
        st: &EngineState,
        entity: &ProviderEntity,
        account: &Account,
        call: AccountCall<'_>,
    ) -> Result<Bytes, PollError> {
        let redact = |s: String| st.redactor.redact(&s).into_owned();
        let url =
            Url::parse(call.url).map_err(|e| PollError::new(PollErrorClass::Invalid, format!("{}: {e}", call.url)))?;
        let allow_private = st.settings().allow_private_endpoints;
        crate::upstream::check_ip_host(&url, allow_private)
            .map_err(|e| PollError::new(PollErrorClass::Invalid, e.to_string()))?;
        let timeout = self.quota.timing().timeout;
        let mut refreshed = false;
        loop {
            let released = accounts::release(account, entity, &st.tokens)
                .map_err(|w| PollError::new(PollErrorClass::Withheld, redact(w.to_string())))?;
            let headers = self.call_headers(st, entity, account, &released, &url, call.headers)?;
            let method = reqwest::Method::from_bytes(call.method.to_ascii_uppercase().as_bytes())
                .map_err(|_| PollError::new(PollErrorClass::Invalid, format!("method {}", call.method)))?;
            let req = st.http.request(method, url.clone()).headers(headers).body(call.body.clone());
            let sent = tokio::time::timeout(timeout, async {
                let resp = req.send().await?;
                let status = resp.status().as_u16();
                let body = resp.bytes().await?;
                Ok::<_, reqwest::Error>((status, body))
            })
            .await;
            let (status, body) = match sent {
                Err(_) => {
                    return Err(PollError::new(
                        PollErrorClass::Timeout,
                        format!("no answer within {} s", timeout.as_secs_f64()),
                    ));
                }
                Ok(Err(e)) if e.is_timeout() => {
                    return Err(PollError::new(PollErrorClass::Timeout, redact(e.to_string())));
                }
                Ok(Err(e)) => return Err(PollError::new(PollErrorClass::Network, redact(e.to_string()))),
                Ok(Ok(answer)) => answer,
            };
            if (200..300).contains(&status) {
                return Ok(body);
            }
            if status == 401
                && !refreshed
                && let Released::Token(view) = &released
            {
                refreshed = true;
                match self.refresh_rejected(&account.provider, &account.name, view).await {
                    Refreshed::Fresh => continue,
                    Refreshed::Transient(r) | Refreshed::Permanent(r) => {
                        return Err(PollError::status(401, format!("token refresh failed: {r}")));
                    }
                }
            }
            let text = String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned();
            return Err(PollError::status(status, redact(text)));
        }
    }

    /// Static headers, then `[identity]` (sign-in accounts), then the credential, which goes
    /// only to a host the account is bound to.
    fn call_headers(
        &self,
        st: &EngineState,
        entity: &ProviderEntity,
        account: &Account,
        released: &Released<'_>,
        url: &Url,
        extra: &IndexMap<String, String>,
    ) -> Result<HeaderMap, PollError> {
        let invalid = |k: &str| PollError::new(PollErrorClass::Invalid, format!("header {k}: invalid"));
        let host = url.host_str().unwrap_or_default();
        let bound = match released {
            Released::Key(_) => account.hosts.contains(host),
            Released::Token(v) => v.entry.hosts.contains(host),
        };
        if !bound {
            return Err(PollError::new(
                PollErrorClass::Withheld,
                format!("host {host} is not one {}/{}'s credential is bound to", account.provider, account.name),
            ));
        }
        let floor = st.registry.floor();
        let mut headers = HeaderMap::new();
        for (k, v) in extra {
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|_| invalid(k))?;
            if floor.lists(name.as_str()) {
                continue;
            }
            headers.insert(name, HeaderValue::from_str(v).map_err(|_| invalid(k))?);
        }
        if let (Released::Token(view), Some(decl)) = (released, &entity.identity) {
            let session_id = self.sessions.id_for(&AgentId::new(MAINTENANCE_AGENT.to_owned(), None));
            let install_id =
                self.install_id().map_err(|e| PollError::new(PollErrorClass::Invalid, e.to_string()))?.to_owned();
            let ctx = FillContext {
                session_id: &session_id,
                request_id: &identity::uuid_v4(),
                turns: 0,
                upstream_model: "",
                claims: Some(&view.entry.claims),
                install_id: &install_id,
            };
            for (k, v) in identity::headers(decl, &ctx).into_iter().filter(|(_, v)| !v.is_empty()) {
                let name = HeaderName::from_bytes(k.as_bytes()).map_err(|_| invalid(k))?;
                if floor.lists(name.as_str()) {
                    continue;
                }
                headers.insert(name, HeaderValue::from_str(&v).map_err(|_| invalid(k))?);
            }
        }
        let (header, scheme) = match (released, &entity.signin) {
            (Released::Token(_), Some(s)) => (Some(s.auth.header.as_str()), Some(s.auth.scheme)),
            _ => {
                let a = entity.auth.as_ref();
                (a.and_then(|a| a.header.as_deref()), a.and_then(|a| a.scheme))
            }
        };
        let name =
            header.map_or(Ok(AUTHORIZATION), |h| HeaderName::from_bytes(h.as_bytes()).map_err(|_| invalid(h)))?;
        let scheme = scheme.unwrap_or(if name == AUTHORIZATION { AuthScheme::Bearer } else { AuthScheme::Raw });
        let value = released.secret().with_exposed(|s| match scheme {
            AuthScheme::Bearer => HeaderValue::from_str(&format!("Bearer {s}")).ok(),
            AuthScheme::Raw => HeaderValue::from_str(s).ok(),
            _ => None,
        });
        let mut value = value.ok_or_else(|| {
            PollError::new(PollErrorClass::Invalid, format!("auth scheme {scheme} can't carry this credential"))
        })?;
        value.set_sensitive(true);
        headers.insert(name, value);
        Ok(headers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_its_spread() {
        for _ in 0..2000 {
            let f = jitter_factor(0.1);
            assert!((0.9..=1.1).contains(&f), "{f}");
        }
    }

    #[test]
    fn the_board_schedules_one_retry_and_none_after_a_429() {
        let board = QuotaBoard::default();
        board.set_timing(PollTiming { retry_after: Duration::from_secs(60), ..PollTiming::default() });
        let interval = Duration::from_secs(600);
        assert_eq!(board.due("p", "a", interval), UNIX_EPOCH, "never polled: due now");
        let t0 = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let poll = |at, error: Option<PollError>, retry| QuotaPoll {
            provider: "p".into(),
            account: "a".into(),
            at,
            windows: Vec::new(),
            error,
            retry,
        };
        board.record(poll(t0, None, false));
        let next = board.due("p", "a", interval);
        assert!(next >= t0 + Duration::from_secs(540) && next <= t0 + Duration::from_secs(660), "{next:?}");

        let failed = PollError::new(PollErrorClass::Timeout, "slow");
        board.record(poll(t0, Some(failed.clone()), false));
        assert_eq!(board.due("p", "a", interval), t0 + Duration::from_secs(60), "one retry after a minute");
        assert!(board.retry_pending("p", "a"));
        board.record(poll(t0, Some(failed), true));
        assert!(!board.retry_pending("p", "a"), "the retry failed too: wait for the interval");
        assert!(board.due("p", "a", interval) >= t0 + Duration::from_secs(540));

        board.record(poll(t0, Some(PollError::status(429, "slow down")), false));
        assert!(!board.retry_pending("p", "a"), "a 429 skips the retry");
        let kept = board.get("p", "a");
        assert!(kept.latest.as_ref().is_some_and(QuotaPoll::ok), "the last good poll stays");
        assert_eq!(kept.last_failure.unwrap().error.unwrap().class, PollErrorClass::RateLimited);
    }

    #[test]
    fn hooks_see_every_poll() {
        let board = QuotaBoard::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        board.on_poll(Arc::new(move |p: &QuotaPoll| lock(&s).push(p.ok())));
        let at = SystemTime::now();
        let p =
            QuotaPoll { provider: "p".into(), account: "a".into(), at, windows: Vec::new(), error: None, retry: false };
        board.record(p.clone());
        board.record(QuotaPoll { error: Some(PollError::new(PollErrorClass::Network, "down")), ..p });
        assert_eq!(*lock(&seen), [true, false]);
    }

    #[test]
    fn summaries_are_short() {
        assert_eq!(PollError::status(500, "x").summary(), "HTTP 500");
        assert_eq!(PollError::status(429, "x").summary(), "rate limited");
        assert_eq!(PollError::new(PollErrorClass::Timeout, "x").summary(), "timeout");
    }
}
