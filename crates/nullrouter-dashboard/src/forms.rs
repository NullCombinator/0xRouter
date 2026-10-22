//! The write side's guards (specs/014-functional-dashboard, research R2/R3): the form token, the
//! checks every POST passes before any change, the `_back` sanitizer, and the flash store that
//! carries a result across the redirect.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use axum::http::header::{ORIGIN, REFERER};
use nullrouter_engine::state::Engine;
use nullrouter_registry::SecretString;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

use crate::access;
use crate::headers::HostRule;
use crate::pages::Id;

const BLOCK: usize = 64;
const FLASH_TTL: Duration = Duration::from_secs(300);
const FLASH_MAX: usize = 1000;
/// Field names that never travel in a flash (contracts/dashboard-forms.md "Write-only fields").
const WRITE_ONLY: [&str; 3] = ["secret", "password", "code"];

/// Why a post was turned away before any change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No valid session: `303 /signin`.
    SignIn,
    /// Wrong `Origin`/`Referer` or `_t`: `403` with [`Refusal::FOREIGN_TEXT`].
    Foreign,
}

impl Refusal {
    pub const FOREIGN_TEXT: &'static str = "This change didn't come from the dashboard.";
}

/// What a flash says.
#[derive(Debug)]
pub enum FlashKind {
    Done {
        text: String,
    },
    /// A value shown once (a new key).
    Once {
        label: String,
        value: SecretString,
    },
    Refused {
        text: String,
        field: Option<String>,
        kept: Vec<(String, String)>,
    },
}

struct Entry {
    id: String,
    session: String,
    kind: FlashKind,
    created: Instant,
}

/// Process-wide form state: the token key and the flashes. Built once per dashboard.
pub struct Forms {
    key: [u8; 32],
    flashes: Mutex<VecDeque<Entry>>,
}

impl Default for Forms {
    fn default() -> Self {
        Self::new()
    }
}

/// HMAC-SHA256 (RFC 2104).
fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    inner.update(msg);
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn base32(bytes: &[u8]) -> String {
    const A: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let (mut out, mut acc, mut bits) = (String::new(), 0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(A[((acc >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(A[((acc << (5 - bits)) & 31) as usize]));
    }
    out
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // The OS random source failing is unrecoverable for a token key; a zero key would be forgeable.
    getrandom::fill(&mut b).expect("OS random source");
    b
}

impl Forms {
    pub fn new() -> Self {
        Self { key: random::<32>(), flashes: Mutex::new(VecDeque::new()) }
    }

    /// The form token for `session` (the cookie value): nothing is stored.
    pub fn token(&self, session: &str) -> String {
        hex_of(&hmac_sha256(&self.key, session.as_bytes()))[..32].to_owned()
    }

    /// The checks of contracts/dashboard-forms.md after the Host check: session, then
    /// `Origin` (or `Referer` when `Origin` is absent), then `_t`. Returns the session.
    pub fn check_post(
        &self,
        engine: &Engine,
        rule: &HostRule,
        headers: &HeaderMap,
        form: &HashMap<String, String>,
    ) -> Result<String, Refusal> {
        let session = access::session_cookie(engine, headers).ok_or(Refusal::SignIn)?;
        self.check_rest(session, rule, headers, form)?;
        Ok(session.to_owned())
    }

    fn check_rest(
        &self,
        session: &str,
        rule: &HostRule,
        headers: &HeaderMap,
        form: &HashMap<String, String>,
    ) -> Result<(), Refusal> {
        let from_own_origin = if let Some(o) = headers.get(ORIGIN) {
            o.to_str().ok().and_then(|o| o.strip_prefix("http://")).is_some_and(|h| rule.allows(h))
        } else {
            headers
                .get(REFERER)
                .and_then(|r| r.to_str().ok())
                .and_then(|r| r.strip_prefix("http://"))
                .map(|r| r.split(['/', '?', '#']).next().unwrap_or(""))
                .is_some_and(|h| rule.allows(h))
        };
        if !from_own_origin {
            return Err(Refusal::Foreign);
        }
        let expected = self.token(session);
        let ok = form.get("_t").is_some_and(|t| bool::from(t.as_bytes().ct_eq(expected.as_bytes())));
        if ok { Ok(()) } else { Err(Refusal::Foreign) }
    }

    /// Stores a flash for `session` and returns its id.
    pub fn put(&self, session: &str, kind: FlashKind) -> String {
        self.put_at(session, kind, Instant::now())
    }

    pub(crate) fn put_at(&self, session: &str, mut kind: FlashKind, now: Instant) -> String {
        if let FlashKind::Refused { kept, .. } = &mut kind {
            kept.retain(|(k, _)| !WRITE_ONLY.contains(&k.as_str()));
        }
        let id = base32(&random::<16>());
        let mut q = self.flashes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        q.retain(|e| now.saturating_duration_since(e.created) < FLASH_TTL);
        while q.len() >= FLASH_MAX {
            q.pop_front();
        }
        q.push_back(Entry { id: id.clone(), session: session.to_owned(), kind, created: now });
        id
    }

    /// Reads and removes a flash; another session's, an expired or an unknown id gives `None`.
    pub fn take(&self, session: &str, id: &str) -> Option<FlashKind> {
        self.take_at(session, id, Instant::now())
    }

    pub(crate) fn take_at(&self, session: &str, id: &str, now: Instant) -> Option<FlashKind> {
        let mut q = self.flashes.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let pos = q.iter().position(|e| e.id == id && e.session == session)?;
        let e = q.remove(pos)?;
        (now.saturating_duration_since(e.created) < FLASH_TTL).then_some(e.kind)
    }
}

/// `raw` when it is a dashboard page path (optionally with a query), else `/endpoint`.
pub fn back(raw: &str) -> String {
    const DEFAULT: &str = "/endpoint";
    if access::sanitize_next(raw) != raw {
        return DEFAULT.to_owned();
    }
    let (path, query) = raw.split_once('?').map_or((raw, None), |(p, q)| (p, Some(q)));
    let known = Id::ALL.iter().any(|id| id.path() == path)
        || path.strip_prefix("/providers/").is_some_and(|r| !r.is_empty() && !r.contains('/'))
        || path.strip_prefix("/usage/records/").is_some_and(|r| !r.is_empty() && !r.contains('/'));
    match (known, query) {
        (true, Some(q)) if !q.contains('#') => format!("{path}?{q}"),
        (true, None) => path.to_owned(),
        _ => DEFAULT.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn form(t: &str) -> HashMap<String, String> {
        HashMap::from([("_t".to_owned(), t.to_owned())])
    }

    #[test]
    fn hmac_matches_rfc_4231_case_2() {
        let mac = hmac_sha256(b"Jefe", b"what do ya want for nothing?");
        assert_eq!(hex_of(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    #[test]
    fn hmac_with_a_long_key_hashes_it_first() {
        // RFC 4231 case 6: 131 bytes of 0xaa.
        let mac = hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First");
        assert_eq!(hex_of(&mac), "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54");
    }

    #[test]
    fn token_is_32_hex_per_session_and_per_process() {
        let (a, b) = (Forms::new(), Forms::new());
        let t = a.token("s1");
        assert_eq!(t.len(), 32);
        assert!(t.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(t, a.token("s1"));
        assert_ne!(t, a.token("s2"));
        assert_ne!(t, b.token("s1"));
    }

    #[test]
    fn checks_origin_then_token() {
        let f = Forms::new();
        let rule = HostRule::new("127.0.0.1", 20130);
        let good = f.token("s");
        let mut h = HeaderMap::new();
        assert_eq!(f.check_rest("s", &rule, &h, &form(&good)), Err(Refusal::Foreign), "no origin, no referer");
        h.insert(ORIGIN, HeaderValue::from_static("http://127.0.0.1:20130"));
        assert_eq!(f.check_rest("s", &rule, &h, &form(&good)), Ok(()));
        assert_eq!(f.check_rest("s", &rule, &h, &form("0".repeat(32).as_str())), Err(Refusal::Foreign));
        assert_eq!(f.check_rest("s", &rule, &h, &HashMap::new()), Err(Refusal::Foreign));
        assert_eq!(f.check_rest("other", &rule, &h, &form(&good)), Err(Refusal::Foreign));
        h.insert(ORIGIN, HeaderValue::from_static("http://evil.example"));
        assert_eq!(f.check_rest("s", &rule, &h, &form(&good)), Err(Refusal::Foreign));
        // Referer only counts when Origin is absent.
        h.insert(REFERER, HeaderValue::from_static("http://localhost:20130/endpoint"));
        assert_eq!(f.check_rest("s", &rule, &h, &form(&good)), Err(Refusal::Foreign));
        h.remove(ORIGIN);
        assert_eq!(f.check_rest("s", &rule, &h, &form(&good)), Ok(()));
        h.insert(REFERER, HeaderValue::from_static("http://localhost:20130.evil.example/x"));
        assert_eq!(f.check_rest("s", &rule, &h, &form(&good)), Err(Refusal::Foreign));
    }

    #[test]
    fn back_accepts_only_dashboard_pages() {
        for ok in [
            "/endpoint",
            "/providers",
            "/settings",
            "/providers/anthropic",
            "/usage?before=rq_1",
            "/usage/records/rq_1",
        ] {
            assert_eq!(back(ok), ok);
        }
        for bad in [
            "",
            "/",
            "//evil.example",
            "https://evil.example",
            "/nope",
            "/providers/a/b",
            "/endpoint\r\nX: y",
            "/\\evil",
            "/usage/records/",
        ] {
            assert_eq!(back(bad), "/endpoint", "{bad:?}");
        }
    }

    #[test]
    fn a_flash_is_for_its_session_and_read_once() {
        let f = Forms::new();
        let id = f.put("s1", FlashKind::Done { text: "ok".into() });
        assert_eq!(id.len(), 26);
        assert!(f.take("s2", &id).is_none());
        assert!(matches!(f.take("s1", &id), Some(FlashKind::Done { text }) if text == "ok"));
        assert!(f.take("s1", &id).is_none());
    }

    #[test]
    fn a_flash_expires_after_five_minutes() {
        let f = Forms::new();
        let t0 = Instant::now();
        let id = f.put_at("s", FlashKind::Done { text: "x".into() }, t0);
        assert!(f.take_at("s", &id, t0 + Duration::from_secs(300)).is_none());
        let id = f.put_at("s", FlashKind::Done { text: "x".into() }, t0);
        assert!(f.take_at("s", &id, t0 + Duration::from_secs(299)).is_some());
    }

    #[test]
    fn the_store_keeps_the_newest_thousand() {
        let f = Forms::new();
        let ids: Vec<String> = (0..1001).map(|i| f.put("s", FlashKind::Done { text: i.to_string() })).collect();
        assert!(f.take("s", &ids[0]).is_none());
        assert!(f.take("s", &ids[1]).is_some());
        assert!(f.take("s", &ids[1000]).is_some());
    }

    #[test]
    fn write_only_fields_never_stay_in_a_flash() {
        let f = Forms::new();
        let kept = ["name", "secret", "password", "code", "label"].map(|k| (k.to_owned(), "v".to_owned())).to_vec();
        let id = f.put("s", FlashKind::Refused { text: "no".into(), field: Some("name".into()), kept });
        let Some(FlashKind::Refused { kept, .. }) = f.take("s", &id) else { panic!("refused") };
        assert_eq!(kept.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(), ["name", "label"]);
    }

    #[test]
    fn a_once_value_does_not_print() {
        let k = FlashKind::Once { label: "Key".into(), value: SecretString::new("nr_secret") };
        assert!(!format!("{k:?}").contains("nr_secret"));
    }
}
