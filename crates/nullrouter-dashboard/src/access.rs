//! Who may see a page (contracts/dashboard-http.md "Access"): the dashboard token's cookie, the
//! delay a wrong token costs, the `next` a sign-in returns to, and the `Origin` of a sign-in post.
//!
//! The token is never stored: the snapshot holds its SHA-256, and a cookie is checked by hashing
//! what the browser sent and comparing the two digests in constant time.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::http::header::{COOKIE, HeaderMap, HeaderValue};
use nullrouter_engine::files::DashboardToken;
use nullrouter_engine::state::Engine;
use subtle::ConstantTimeEq;

use crate::headers::HostRule;

pub const COOKIE_NAME: &str = "nr_dashboard";

/// 400 days, the longest a browser keeps a cookie.
const MAX_AGE: u64 = 34_560_000;

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

/// Where `headers` stand against the current token.
pub fn gate(engine: &Engine, headers: &HeaderMap) -> Gate {
    let snapshot = engine.snapshot();
    let Some(digest) = snapshot.dashboard.digest.as_deref() else { return Gate::NoToken };
    match cookie_value(headers) {
        Some(presented) if token_matches(digest, &presented) => Gate::SignedIn,
        _ => Gate::SignedOut,
    }
}

/// Whether `presented` is the token whose digest is `digest`.
pub fn token_matches(digest: &str, presented: &str) -> bool {
    let got = DashboardToken::digest_of(presented);
    bool::from(got.as_bytes().ct_eq(digest.as_bytes()))
}

/// The dashboard cookie's value, from any `Cookie` header.
fn cookie_value(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|h| h.to_str().ok())
        .flat_map(|h| h.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(COOKIE_NAME)?.strip_prefix('='))
        .map(str::to_owned)
}

/// The `Set-Cookie` value that signs a browser in.
pub fn set_cookie(token: &str) -> String {
    format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={MAX_AGE}")
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
        h.append(COOKIE, HeaderValue::from_static("theme=dark; nr_dashboard=nrd_example; other=1"));
        assert_eq!(cookie_value(&h).as_deref(), Some(token));
        assert!(token_matches(&digest, token));
        assert!(!token_matches(&digest, "nrd_exampl"));
        assert!(!token_matches(&digest, ""));
        assert!(!token_matches("short", token));

        let mut other = HeaderMap::new();
        other.append(COOKIE, HeaderValue::from_static("nr_dashboard_x=nrd_example"));
        assert_eq!(cookie_value(&other), None, "a longer cookie name is not ours");
        assert_eq!(cookie_value(&HeaderMap::new()), None);
    }

    #[test]
    fn the_cookie_is_the_one_the_contract_spells_out() {
        assert_eq!(set_cookie("nrd_t"), "nr_dashboard=nrd_t; Path=/; HttpOnly; SameSite=Strict; Max-Age=34560000");
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
