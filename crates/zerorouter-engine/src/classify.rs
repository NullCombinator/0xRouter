//! Failure classification, ported from 9router `checkFallbackError` and `ERROR_RULES`
//! (research R6), and the same-account retry budgets (R7).

use std::collections::BTreeMap;
use std::time::Duration;

use zerorouter_registry::schema::RetryOverride;

use crate::records::ErrorClass;

/// 9router `COOLDOWN.long`.
pub const LONG_MS: u64 = 2 * 60 * 1000;
/// 9router `COOLDOWN.short`.
pub const SHORT_MS: u64 = 5 * 1000;
/// 9router `TRANSIENT_COOLDOWN_MS`.
pub const TRANSIENT_MS: u64 = 30 * 1000;
/// 9router `BACKOFF_CONFIG`.
pub const BACKOFF_BASE_MS: u64 = 2000;
pub const BACKOFF_MAX_MS: u64 = 5 * 60 * 1000;
pub const BACKOFF_MAX_LEVEL: u8 = 15;

/// How long the failed account rests for this model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cooldown {
    None,
    Fixed(u64),
    /// Exponential: the account's backoff level goes up one.
    Backoff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub class: ErrorClass,
    /// Move to another account (or member); `false` returns the error to the client.
    pub fallback: bool,
    pub cooldown: Cooldown,
}

impl Verdict {
    /// The cooldown in ms and the new backoff level, from the account's current level.
    pub fn cooldown_ms(&self, level: u8) -> (u64, u8) {
        match self.cooldown {
            Cooldown::None => (0, level),
            Cooldown::Fixed(ms) => (ms, level),
            Cooldown::Backoff => {
                let next = (level + 1).min(BACKOFF_MAX_LEVEL);
                (backoff_ms(next), next)
            }
        }
    }
}

/// 9router `getQuotaCooldown`: 2000·2^(level−1), capped at 300 000.
pub fn backoff_ms(level: u8) -> u64 {
    let exp = u32::from(level.saturating_sub(1));
    BACKOFF_BASE_MS.saturating_mul(1u64.checked_shl(exp).unwrap_or(u64::MAX)).min(BACKOFF_MAX_MS)
}

/// The text rules, in priority order: substring, cooldown, class.
const TEXT_RULES: [(&str, Cooldown, ErrorClass); 8] = [
    ("no credentials", Cooldown::Fixed(LONG_MS), ErrorClass::Auth),
    ("request not allowed", Cooldown::Fixed(SHORT_MS), ErrorClass::Auth),
    ("improperly formed request", Cooldown::Fixed(LONG_MS), ErrorClass::RequestError),
    ("rate limit", Cooldown::Backoff, ErrorClass::RateLimited),
    ("too many requests", Cooldown::Backoff, ErrorClass::RateLimited),
    ("quota exceeded", Cooldown::Backoff, ErrorClass::RateLimited),
    ("capacity", Cooldown::Backoff, ErrorClass::RateLimited),
    ("overloaded", Cooldown::Backoff, ErrorClass::RateLimited),
];

/// An upstream HTTP error, matched as 9router's request path matches it:
/// `"[<status>]: <raw body>"`.
pub fn upstream(status: u16, raw_body: &str) -> Verdict {
    text(status, &format!("[{status}]: {raw_body}"))
}

/// `checkFallbackError(status, text)`: the text rules first, then the status rules.
pub fn text(status: u16, text: &str) -> Verdict {
    let lower = text.to_lowercase();
    if let Some(&(_, cooldown, class)) = TEXT_RULES.iter().find(|(t, ..)| lower.contains(t)) {
        return Verdict { class, fallback: true, cooldown };
    }
    let long = Cooldown::Fixed(LONG_MS);
    match status {
        401..=403 => Verdict { class: ErrorClass::Auth, fallback: true, cooldown: long },
        404 => Verdict { class: ErrorClass::NotFound, fallback: true, cooldown: long },
        429 => Verdict { class: ErrorClass::RateLimited, fallback: true, cooldown: Cooldown::Backoff },
        400..=499 => Verdict { class: ErrorClass::RequestError, fallback: false, cooldown: Cooldown::None },
        _ => Verdict { class: ErrorClass::Transient, fallback: true, cooldown: Cooldown::Fixed(TRANSIENT_MS) },
    }
}

/// A failure with no HTTP status (a network error, no headers in time, a stall, a break
/// before output): 9router counts these as 502, transient.
pub fn transport(class: ErrorClass) -> Verdict {
    Verdict { class, fallback: true, cooldown: Cooldown::Fixed(TRANSIENT_MS) }
}

/// Same-account retries for one failure (research R7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub retries: u32,
    pub delay: Duration,
}

/// The longest provider-indicated wait 0router retries after on the same account.
pub const MAX_INDICATED_WAIT: Duration = Duration::from_secs(5);

/// The same-account budget for a failure. `status` is `None` for a transport failure
/// (counted as 502); `indicated` is the provider's `retry-after` or reset wait, if any.
pub fn budget(
    status: Option<u16>,
    verdict: &Verdict,
    indicated: Option<Duration>,
    overrides: &BTreeMap<String, RetryOverride>,
) -> Budget {
    let s = status.unwrap_or(502);
    if let Some(o) = overrides.get(&s.to_string()) {
        return Budget { retries: o.retries, delay: Duration::from_millis(o.delay_ms) };
    }
    let b = |retries, secs| Budget { retries, delay: Duration::from_secs(secs) };
    if !verdict.fallback {
        return b(0, 0);
    }
    if verdict.class == ErrorClass::RateLimited {
        return match indicated {
            Some(w) if w > MAX_INDICATED_WAIT => b(0, 0),
            Some(w) => Budget { retries: 1, delay: w },
            None => b(1, 2),
        };
    }
    match (s, verdict.class) {
        (_, ErrorClass::Auth | ErrorClass::NotFound | ErrorClass::RequestError) => b(0, 0),
        (502, _) => b(3, 3),
        (503, _) => b(3, 2),
        (504, _) => b(2, 3),
        (500..=599, _) => b(1, 2),
        _ => b(0, 0),
    }
}

/// The provider's indicated wait: `retry-after` (seconds or an HTTP date), else the
/// usual reset headers (`x-ratelimit-reset-requests`/`-tokens` as `1s`, `250ms`, `1m2s`;
/// `anthropic-ratelimit-*-reset` as an RFC 3339 time).
pub fn indicated_wait(headers: &reqwest::header::HeaderMap, now: std::time::SystemTime) -> Option<Duration> {
    let get = |n: &str| headers.get(n).and_then(|v| v.to_str().ok()).map(str::trim);
    if let Some(v) = get("retry-after-ms").and_then(|v| v.parse::<f64>().ok()) {
        return Some(Duration::from_millis(v.max(0.0) as u64));
    }
    if let Some(v) = get("retry-after") {
        if let Ok(secs) = v.parse::<f64>() {
            return Some(Duration::from_millis((secs.max(0.0) * 1000.0) as u64));
        }
        if let Ok(at) = httpdate::parse_http_date(v) {
            return Some(at.duration_since(now).unwrap_or_default());
        }
    }
    let spans = ["x-ratelimit-reset-requests", "x-ratelimit-reset-tokens"]
        .into_iter()
        .filter_map(|n| get(n).and_then(go_duration));
    let times = [
        "anthropic-ratelimit-requests-reset",
        "anthropic-ratelimit-tokens-reset",
        "anthropic-ratelimit-input-tokens-reset",
        "anthropic-ratelimit-output-tokens-reset",
    ]
    .into_iter()
    .filter_map(|n| get(n).and_then(crate::clock::parse_rfc3339))
    .map(|at| at.duration_since(now).unwrap_or_default());
    spans.chain(times).max()
}

/// `1m2.5s`, `250ms`, `3s`: OpenAI's reset header format.
fn go_duration(s: &str) -> Option<Duration> {
    let mut total = 0f64;
    let mut num = String::new();
    let mut rest = s;
    let mut any = false;
    while !rest.is_empty() {
        let c = rest.chars().next()?;
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
            rest = &rest[1..];
            continue;
        }
        let (unit, scale) =
            [("ms", 0.001), ("h", 3600.0), ("m", 60.0), ("s", 1.0)].into_iter().find(|(u, _)| rest.starts_with(u))?;
        total += num.parse::<f64>().ok()? * scale;
        num.clear();
        rest = &rest[unit.len()..];
        any = true;
    }
    (any && num.is_empty()).then(|| Duration::from_millis((total * 1000.0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn reset_headers_are_read() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let h = |pairs: &[(&'static str, &str)]| {
            let mut m = HeaderMap::new();
            for (k, v) in pairs {
                m.insert(*k, HeaderValue::from_str(v).unwrap());
            }
            m
        };
        assert_eq!(indicated_wait(&h(&[("retry-after", "3")]), now), Some(Duration::from_secs(3)));
        assert_eq!(
            indicated_wait(&h(&[("retry-after", "Tue, 14 Nov 2023 22:13:30 GMT")]), now),
            Some(Duration::from_secs(10))
        );
        assert_eq!(
            indicated_wait(&h(&[("x-ratelimit-reset-requests", "1m2.5s")]), now),
            Some(Duration::from_millis(62_500))
        );
        assert_eq!(indicated_wait(&h(&[("x-ratelimit-reset-tokens", "250ms")]), now), Some(Duration::from_millis(250)));
        assert_eq!(
            indicated_wait(&h(&[("anthropic-ratelimit-requests-reset", "2023-11-14T22:13:25Z")]), now),
            Some(Duration::from_secs(5))
        );
        assert_eq!(indicated_wait(&h(&[("retry-after", "soon")]), SystemTime::now()), None);
        assert_eq!(go_duration("3"), None);
    }
}
