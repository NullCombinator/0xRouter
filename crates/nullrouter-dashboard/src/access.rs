//! Who may see a page (contracts/dashboard-http.md "Access"): the dashboard token's cookie, the
//! delay a wrong token costs, the `next` a sign-in returns to, and the `Origin` of a sign-in post.
//!
//! The token is never stored: the snapshot holds its SHA-256, and a sign-in is checked by hashing
//! what the browser sent and comparing the two digests in constant time. The cookie is not the
//! token: it is a random session id made at sign-in (security-review.md M1), and the server keeps
//! only its SHA-256 in [`Sessions`].

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use axum::http::header::{COOKIE, HeaderMap, HeaderValue};
use nullrouter_engine::files::DashboardToken;
use nullrouter_engine::state::Engine;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::headers::HostRule;

/// The session cookie's name on the dashboard bound to `port` (spec 014 R10).
pub fn cookie_name(port: u16) -> String {
    format!("nrd_session_{port}")
}

/// The cookie's `Max-Age`: the absolute session lifetime, 7 days.
const MAX_AGE: u64 = 7 * 24 * 3600;

/// How long a session may sit unused, and how long it lives at most, whatever its use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub idle: Duration,
    pub absolute: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self { idle: Duration::from_secs(12 * 3600), absolute: Duration::from_secs(MAX_AGE) }
    }
}

/// The most sessions kept; a new sign-in beyond it evicts the oldest.
pub const MAX_SESSIONS: usize = 64;

struct Session {
    /// SHA-256 of the session id; the id itself is never kept.
    digest: [u8; 32],
    /// The dashboard token digest current at sign-in: a reissued token ends the session.
    token: String,
    created: Instant,
    seen: Instant,
}

/// The live sign-ins. In memory only: a restart of `serve` ends every session, and the operator
/// signs in again. That is deliberate; nothing about a session is written to disk.
pub struct Sessions {
    limits: Limits,
    table: Mutex<VecDeque<Session>>,
}

impl Default for Sessions {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

impl Sessions {
    pub fn new(limits: Limits) -> Self {
        Self { limits, table: Mutex::new(VecDeque::new()) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<Session>> {
        self.table.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A new session for the token whose digest is `token_digest`: its 256-bit id (hex), made from
    /// OS randomness. `None` when the OS gives none: the sign-in is then refused, never made with
    /// a weaker id.
    pub fn create(&self, token_digest: &str, now: Instant) -> Option<String> {
        let mut id = [0u8; 32];
        getrandom::fill(&mut id).ok()?;
        let mut t = self.lock();
        t.retain(|s| self.live(s, token_digest, now));
        while t.len() >= MAX_SESSIONS {
            t.pop_front();
        }
        // The cookie carries the id in hex; the digest is of that text, as `check` hashes it.
        let text = id.iter().fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        });
        t.push_back(Session {
            digest: Sha256::digest(text.as_bytes()).into(),
            token: token_digest.to_owned(),
            created: now,
            seen: now,
        });
        Some(text)
    }

    fn live(&self, s: &Session, token_digest: &str, now: Instant) -> bool {
        s.token == token_digest
            && now.saturating_duration_since(s.created) < self.limits.absolute
            && now.saturating_duration_since(s.seen) < self.limits.idle
    }

    /// Whether `presented` is a live session id for the current token; a live one counts as used.
    pub fn check(&self, presented: &str, token_digest: &str, now: Instant) -> bool {
        let want: [u8; 32] = Sha256::digest(presented.as_bytes()).into();
        let mut t = self.lock();
        t.retain(|s| self.live(s, token_digest, now));
        let mut found = false;
        for s in t.iter_mut() {
            if bool::from(s.digest.ct_eq(&want)) {
                s.seen = now;
                found = true;
            }
        }
        found
    }

    /// Ends the session `presented` names.
    pub fn end(&self, presented: &str) {
        let want: [u8; 32] = Sha256::digest(presented.as_bytes()).into();
        self.lock().retain(|s| !bool::from(s.digest.ct_eq(&want)));
    }

    /// Ends every session.
    pub fn clear(&self) {
        self.lock().clear();
    }

    /// How many sessions are kept.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The longest a wrong token waits.
const MAX_DELAY: Duration = Duration::from_secs(30);

/// What the request's cookie says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// No token has been issued: the only page names `nullrouter dashboard token`.
    NoToken,
    /// A token exists and the request holds no cookie for it.
    SignedOut,
    SignedIn,
}

/// The state of wrong tokens since the last right one (FR-010). It is global, not per client:
/// the dashboard is for one operator on one machine, so any run of wrong tokens slows them all.
#[derive(Debug, Default)]
pub struct Access {
    failures: AtomicU32,
}

impl Access {
    /// Counts a wrong token and says how long to make it wait: 1 s, then doubling to 30 s.
    pub fn wrong_token(&self) -> Duration {
        delay_for(self.failures.fetch_add(1, Ordering::SeqCst))
    }

    /// A right token ends the run.
    pub fn right_token(&self) {
        self.failures.store(0, Ordering::SeqCst);
    }
}

/// The wait after `failures` wrong tokens before this one.
fn delay_for(failures: u32) -> Duration {
    Duration::from_secs(1u64 << failures.min(5)).min(MAX_DELAY)
}

/// Where `headers` stand against the live sessions of the current token.
pub fn gate(engine: &Engine, sessions: &Sessions, headers: &HeaderMap, port: u16) -> Gate {
    let snapshot = engine.snapshot();
    let Some(digest) = snapshot.dashboard.digest.as_deref() else { return Gate::NoToken };
    let now = Instant::now();
    if cookie_values(headers, port).any(|presented| sessions.check(presented, digest, now)) {
        Gate::SignedIn
    } else {
        Gate::SignedOut
    }
}

/// The cookie value that is a live session id: the session a form post and a flash belong to.
pub(crate) fn session_cookie<'a>(
    engine: &Engine,
    sessions: &Sessions,
    headers: &'a HeaderMap,
    port: u16,
) -> Option<&'a str> {
    let snapshot = engine.snapshot();
    let digest = snapshot.dashboard.digest.as_deref()?;
    let now = Instant::now();
    cookie_values(headers, port).find(|presented| sessions.check(presented, digest, now))
}

/// Whether `presented` is the token whose digest is `digest`.
pub fn token_matches(digest: &str, presented: &str) -> bool {
    let got = DashboardToken::digest_of(presented);
    bool::from(got.as_bytes().ct_eq(digest.as_bytes()))
}

/// Every value of the dashboard cookie, from any `Cookie` header. All of them, not the first: a
/// server on another loopback port can set one with a longer path, which the browser sends first
/// (security-review.md L3).
fn cookie_values(headers: &HeaderMap, port: u16) -> impl Iterator<Item = &str> {
    let name = cookie_name(port);
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .filter_map(move |pair| pair.trim().strip_prefix(name.as_str())?.strip_prefix('='))
}

/// The SHA-256 (hex) of a session id: what a task records as its owner. The id never leaves the
/// cookie.
pub(crate) fn session_digest(id: &str) -> String {
    Sha256::digest(id.as_bytes()).iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The `Set-Cookie` value that signs a browser in with the session `id`.
pub fn set_cookie(id: &str, port: u16) -> String {
    let name = cookie_name(port);
    format!("{name}={id}; Path=/; HttpOnly; SameSite=Strict; Max-Age={MAX_AGE}")
}

/// The `Set-Cookie` value that removes the cookie (sign out).
pub fn clear_cookie(port: u16) -> String {
    let name = cookie_name(port);
    format!("{name}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0")
}

/// Whether a sign-in post may come from `origin`: no `Origin` (a command-line client) or one of
/// this dashboard's own. A browser form on another site sends its own, and is refused.
pub fn origin_ok(rule: &HostRule, origin: Option<&HeaderValue>) -> bool {
    match origin {
        None => true,
        Some(v) => v.to_str().ok().and_then(|o| o.strip_prefix("http://")).is_some_and(|host| rule.allows(host)),
    }
}

/// `raw` when it is a path on this dashboard, else `/`: it starts with one `/`, and holds no
/// backslash, control character or space, which a browser could read as another host or which
/// could split a header.
pub fn sanitize_next(raw: &str) -> String {
    let local = raw.starts_with('/')
        && !raw.starts_with("//")
        && !raw.chars().any(|c| c == '\\' || c == ' ' || c.is_control())
        && raw.is_ascii();
    if local { raw.to_owned() } else { "/".to_owned() }
}

/// `s` percent-encoded for use as one query value: everything but unreserved characters.
pub fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(b)),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    char::from(b).to_digit(16).and_then(|d| u8::try_from(d).ok())
}

/// Undoes [`encode_component`] and a form's `+` for a space; a bad escape stays as it is.
pub fn decode_component(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push((hi << 4) | lo);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `name=value` pairs of a form body or query string, decoded.
pub fn pairs(text: &str) -> Vec<(String, String)> {
    text.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode_component(k), decode_component(v))
        })
        .collect()
}

/// The first value named `name` in `pairs`.
pub fn field<'a>(pairs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    pairs.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wait_doubles_to_thirty_seconds_and_a_success_resets_it() {
        let a = Access::default();
        let waits: Vec<u64> = (0..8).map(|_| a.wrong_token().as_secs()).collect();
        assert_eq!(waits, [1, 2, 4, 8, 16, 30, 30, 30]);
        a.right_token();
        assert_eq!(a.wrong_token(), Duration::from_secs(1));
    }

    #[test]
    fn next_must_be_a_path_on_this_dashboard() {
        for ok in ["/", "/endpoint", "/providers?q=a&notices", "/usage/records/rq_1", "/a%20b"] {
            assert_eq!(sanitize_next(ok), ok);
        }
        for bad in [
            "",
            "endpoint",
            "//evil.example",
            "///evil.example",
            "https://evil.example/x",
            "/\\evil.example",
            "/a b",
            "/a\r\nSet-Cookie: x=y",
            "/é",
        ] {
            assert_eq!(sanitize_next(bad), "/", "{bad:?}");
        }
    }

    #[test]
    fn components_round_trip_and_forms_decode() {
        for s in ["/usage?before=rq_1&notices", "a b", "é/ü", "100%", "~-._"] {
            assert_eq!(decode_component(&encode_component(s)), s, "{s}");
        }
        assert_eq!(encode_component("/a?b=c&d"), "%2Fa%3Fb%3Dc%26d");
        let p = pairs("token=nrd_a%2Bb&next=%2Fusage%3Fbefore%3Drq_1&empty=&flag&plus=a+b&bad=%zz");
        assert_eq!(field(&p, "token"), Some("nrd_a+b"));
        assert_eq!(field(&p, "next"), Some("/usage?before=rq_1"));
        assert_eq!((field(&p, "empty"), field(&p, "flag")), (Some(""), Some("")));
        assert_eq!((field(&p, "plus"), field(&p, "bad")), (Some("a b"), Some("%zz")));
        assert_eq!(field(&p, "absent"), None);
    }

    #[test]
    fn a_cookie_is_found_among_others_and_checked_by_digest() {
        let token = "nrd_example";
        let digest = DashboardToken::digest_of(token);
        let mut h = HeaderMap::new();
        h.append(COOKIE, HeaderValue::from_static("theme=dark; nrd_session_1=nrd_example; other=1"));
        assert_eq!(cookie_values(&h, 1).collect::<Vec<_>>(), [token]);
        assert!(token_matches(&digest, token));
        assert!(!token_matches(&digest, "nrd_exampl"));
        assert!(!token_matches(&digest, ""));
        assert!(!token_matches("short", token));

        let mut other = HeaderMap::new();
        other.append(COOKIE, HeaderValue::from_static("nrd_session_1_x=nrd_example"));
        assert_eq!(cookie_values(&other, 1).count(), 0, "a longer cookie name is not ours");
        assert_eq!(cookie_values(&HeaderMap::new(), 1).count(), 0);
    }

    #[test]
    fn a_planted_cookie_sent_first_does_not_hide_the_right_one() {
        let mut h = HeaderMap::new();
        h.append(COOKIE, HeaderValue::from_static("nrd_session_1=x; nrd_session_1=nrd_example"));
        assert_eq!(cookie_values(&h, 1).collect::<Vec<_>>(), ["x", "nrd_example"]);
        let digest = DashboardToken::digest_of("nrd_example");
        assert!(cookie_values(&h, 1).any(|presented| token_matches(&digest, presented)));
    }

    #[test]
    fn the_cookie_is_the_one_the_contract_spells_out() {
        assert_eq!(set_cookie("abc", 1), "nrd_session_1=abc; Path=/; HttpOnly; SameSite=Strict; Max-Age=604800");
        assert_eq!(clear_cookie(1), "nrd_session_1=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
    }

    #[test]
    fn a_session_ends_idle_absolute_on_sign_out_on_a_new_token_and_is_capped() {
        let s = Sessions::default();
        let t0 = Instant::now();
        let id = s.create("tok", t0).unwrap();
        assert_eq!(id.len(), 64);
        assert!(s.check(&id, "tok", t0));
        assert!(!s.check("other", "tok", t0));
        // Use keeps it alive past the idle limit measured from the sign-in.
        let h = Duration::from_secs(3600);
        assert!(s.check(&id, "tok", t0 + 11 * h));
        assert!(s.check(&id, "tok", t0 + 22 * h));
        // Idle: 12 h unused.
        assert!(!s.check(&id, "tok", t0 + 35 * h));
        assert!(s.is_empty());
        // Absolute: 7 days, however busy.
        let id = s.create("tok", t0).unwrap();
        for k in 1..=16 {
            assert!(s.check(&id, "tok", t0 + 10 * k * h));
        }
        assert!(!s.check(&id, "tok", t0 + 7 * 24 * h));
        // Sign out, and a new token.
        let id = s.create("tok", t0).unwrap();
        s.end(&id);
        assert!(!s.check(&id, "tok", t0));
        let id = s.create("tok", t0).unwrap();
        assert!(!s.check(&id, "new", t0));
        assert!(!s.check(&id, "tok", t0), "a session of the old token does not come back");
        // The cap evicts the oldest.
        let first = s.create("tok", t0).unwrap();
        for _ in 0..MAX_SESSIONS {
            s.create("tok", t0).unwrap();
        }
        assert_eq!(s.len(), MAX_SESSIONS);
        assert!(!s.check(&first, "tok", t0));
    }

    #[test]
    fn only_this_dashboards_own_origins_post() {
        let rule = HostRule::new("127.0.0.1", 20130);
        let v = |s: &'static str| HeaderValue::from_static(s);
        assert!(origin_ok(&rule, None));
        for ok in ["http://127.0.0.1:20130", "http://localhost:20130", "http://[::1]:20130"] {
            assert!(origin_ok(&rule, Some(&v(ok))), "{ok}");
        }
        for bad in ["null", "http://evil.example", "https://127.0.0.1:20130", "http://127.0.0.1:20131", ""] {
            assert!(!origin_ok(&rule, Some(&v(bad))), "{bad:?}");
        }
    }
}
