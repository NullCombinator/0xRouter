//! The declarative quota extractor: JSON paths with `*`, alternatives, filters and name
//! templates turn a provider's report into quota windows (research R12).
//!
//! Pure: no I/O, no clock. A rule's `path` finds where windows sit; each match becomes at most
//! one window, read through the rule's value paths. The rules:
//!
//! - `path` segments are separated by `.`; a segment is a key, a key pattern with one `*`
//!   (`seven_day_*`, matching keys with a non-empty middle), or `key[*]` / `[*]` over array
//!   items. Each `*` binds `{1}`, `{2}`, … (the key part, or the 0-based item index). `.` is
//!   the document root.
//! - Value paths are relative to the match, with `first_of` alternatives: the first alternative
//!   that yields a number (or, for `resets_at`, a time) wins. JSON numbers and numeric strings
//!   count as numbers; with `unwrap_val`, `{ "val": n }` too.
//! - A window whose `used` and `remaining` both fail to resolve is skipped, as is one whose
//!   resolved `limit` is zero or less (9router's `> 0` checks: a zero cap is no window).
//! - Percent windows have `limit = 100`. `used` is shown as the provider reports it, floored at
//!   0 but not capped, so an account over its limit reads e.g. 101 % (spec SC-006: shown quota
//!   matches what the provider reports). `remaining` is floored at 0, and capped at 100 for
//!   percent windows. When two of `used`, `limit`, `remaining` are known the third is derived.
//! - Names come from the template; a hole that resolves to nothing or to blank text skips the
//!   window. Values are trimmed. Two windows with one name: the first (rule order) wins.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nullrouter_registry::schema::{QuotaUnit, ResetsFormat, ValuePath, WindowRule};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::clock;

/// One quota window as the provider reported it (data-model.md § Quota window).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuotaWindow {
    pub name: String,
    pub unit: QuotaUnit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining: Option<f64>,
    /// Written as RFC 3339 with milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none", with = "resets_serde")]
    pub resets_at: Option<SystemTime>,
}

mod resets_serde {
    use std::time::SystemTime;

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&super::rfc3339_millis(*t)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<SystemTime>, D::Error> {
        Ok(Option::<String>::deserialize(d)?.and_then(|s| crate::clock::parse_rfc3339(&s)))
    }
}

/// Every window `rules` find in `body`, in rule order.
pub fn extract(rules: &[WindowRule], body: &Value) -> Vec<QuotaWindow> {
    let mut out: Vec<QuotaWindow> = Vec::new();
    for rule in rules {
        let mut found = Vec::new();
        walk(body, &segments(&rule.path), Vec::new(), &mut found);
        for m in found {
            if !filter_holds(rule, m.value) {
                continue;
            }
            if let Some(w) = window(rule, &m)
                && !out.iter().any(|o| o.name == w.name)
            {
                out.push(w);
            }
        }
    }
    out
}

struct Match<'a> {
    value: &'a Value,
    binds: Vec<String>,
}

fn segments(path: &str) -> Vec<&str> {
    if path == "." { Vec::new() } else { path.split('.').collect() }
}

fn walk<'a>(v: &'a Value, segs: &[&str], binds: Vec<String>, out: &mut Vec<Match<'a>>) {
    let Some((seg, rest)) = segs.split_first() else {
        out.push(Match { value: v, binds });
        return;
    };
    if let Some(key) = seg.strip_suffix("[*]") {
        let arr = if key.is_empty() { Some(v) } else { v.get(key) };
        if let Some(Value::Array(items)) = arr {
            for (i, item) in items.iter().enumerate() {
                let mut b = binds.clone();
                b.push(i.to_string());
                walk(item, rest, b, out);
            }
        }
    } else if let Some((pre, suf)) = seg.split_once('*') {
        let Value::Object(map) = v else { return };
        if suf.contains('*') {
            return;
        }
        for (k, item) in map {
            if k.len() > pre.len() + suf.len() && k.starts_with(pre) && k.ends_with(suf) {
                let mut b = binds.clone();
                b.push(k[pre.len()..k.len() - suf.len()].to_owned());
                walk(item, rest, b, out);
            }
        }
    } else if let Some(next) = v.get(*seg) {
        walk(next, rest, binds, out);
    }
}

/// The value at a dotted path below `v` (`.` is `v`); `null` counts as absent.
fn get<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    if path == "." {
        return Some(v).filter(|v| !v.is_null());
    }
    let mut cur = v;
    for seg in path.split('.') {
        cur = match cur {
            Value::Object(m) => m.get(seg)?,
            Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur).filter(|v| !v.is_null())
}

/// A finite number: a JSON number or a numeric string; with `unwrap_val`, also `{ "val": n }`.
fn number(v: &Value, unwrap_val: bool) -> Option<f64> {
    let v = match v {
        Value::Object(o) if unwrap_val => o.get("val")?,
        v => v,
    };
    let n = match v {
        Value::Number(n) => n.as_f64()?,
        Value::String(s) if !s.trim().is_empty() => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

/// The first alternative of `path` that resolves below `v` to a non-null value (`.` is `v`).
pub fn value_at<'a>(v: &'a Value, path: &ValuePath) -> Option<&'a Value> {
    path.alternatives().find_map(|a| get(v, a))
}

/// The first alternative of `path` that resolves to a finite number (JSON number or numeric
/// string).
pub fn number_at(v: &Value, path: &ValuePath) -> Option<f64> {
    first_number(v, path, false)
}

/// The first alternative of `path` that resolves to non-blank text (a string, or a number
/// written out), trimmed.
pub fn text_at(v: &Value, path: &ValuePath) -> Option<String> {
    path.alternatives().find_map(|a| match get(v, a)? {
        Value::String(s) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn first_number(v: &Value, path: &ValuePath, unwrap_val: bool) -> Option<f64> {
    path.alternatives().find_map(|a| get(v, a).and_then(|x| number(x, unwrap_val)))
}

fn filter_holds(rule: &WindowRule, v: &Value) -> bool {
    rule.filter.iter().all(|(k, want)| {
        let Some(got) = get(v, k) else { return false };
        match want {
            toml::Value::String(s) => got.as_str() == Some(s.as_str()),
            toml::Value::Integer(i) => got.as_f64() == Some(*i as f64),
            toml::Value::Boolean(b) => got.as_bool() == Some(*b),
            _ => false,
        }
    })
}

fn window(rule: &WindowRule, m: &Match<'_>) -> Option<QuotaWindow> {
    let num = |p: &Option<ValuePath>| p.as_ref().and_then(|p| first_number(m.value, p, rule.unwrap_val));
    let (mut used, mut remaining) = (num(&rule.used), num(&rule.remaining));
    if used.is_none() && remaining.is_none() {
        return None;
    }
    let percent = rule.unit == QuotaUnit::Percent;
    let limit = if percent { Some(100.0) } else { num(&rule.limit) };
    if limit.is_some_and(|l| l <= 0.0) {
        return None;
    }
    used = used.map(|u| u.max(0.0));
    remaining = remaining.map(|r| if percent { r.clamp(0.0, 100.0) } else { r.max(0.0) });
    match (used, limit, remaining) {
        (Some(u), Some(l), None) => remaining = Some((l - u).max(0.0)),
        (None, Some(l), Some(r)) => used = Some((l - r).max(0.0)),
        _ => {}
    }
    let resets_at = rule
        .resets_at
        .as_ref()
        .and_then(|p| p.alternatives().find_map(|a| get(m.value, a).and_then(|x| parse_reset(x, rule.resets_format))));
    Some(QuotaWindow { name: render_name(&rule.name, m)?, unit: rule.unit, used, limit, remaining, resets_at })
}

/// Fills `{N}`, `{path}` and `{path|lower}`; `None` when a hole is empty or unresolved.
fn render_name(template: &str, m: &Match<'_>) -> Option<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}')? + open;
        let hole = &rest[open + 1..close];
        let (path, lower) = match hole.strip_suffix("|lower") {
            Some(p) => (p, true),
            None => (hole, false),
        };
        let text = match path.parse::<usize>() {
            Ok(n) => m.binds.get(n.checked_sub(1)?)?.clone(),
            Err(_) => match get(m.value, path)? {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                _ => return None,
            },
        };
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        out.push_str(&if lower { text.to_lowercase() } else { text.to_owned() });
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out.trim().to_owned()).filter(|s| !s.is_empty())
}

/// JavaScript's largest `Date`, in milliseconds.
const MAX_DATE_MS: f64 = 8.64e15;

/// A reset time, truncated to milliseconds like 9router's `parseResetTime`
/// (`ref/9router/open-sse/services/usage/shared.js:15-43`). `auto`: numbers and all-digit
/// strings are epoch seconds below 1e12, else milliseconds; other strings are RFC 3339 (or a
/// bare `YYYY-MM-DD`, read as UTC midnight). Empty, `null`, zero and anything else: no reset.
pub fn parse_reset(v: &Value, format: ResetsFormat) -> Option<SystemTime> {
    let epoch = matches!(format, ResetsFormat::Auto | ResetsFormat::EpochS | ResetsFormat::EpochMs);
    match v {
        Value::Number(n) if epoch => from_epoch(n.as_f64()?, format),
        Value::String(s) if epoch && !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
            from_epoch(s.parse().ok()?, format)
        }
        Value::String(s) if matches!(format, ResetsFormat::Auto | ResetsFormat::Rfc3339) => parse_date(s),
        _ => None,
    }
}

fn from_epoch(x: f64, format: ResetsFormat) -> Option<SystemTime> {
    if !x.is_finite() || x <= 0.0 {
        return None;
    }
    let ms = match format {
        ResetsFormat::EpochS => x * 1000.0,
        ResetsFormat::EpochMs => x,
        _ if x < 1e12 => x * 1000.0,
        _ => x,
    }
    .trunc();
    (ms <= MAX_DATE_MS).then(|| UNIX_EPOCH + Duration::from_millis(ms as u64))
}

fn parse_date(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let t = clock::parse_rfc3339(s).or_else(|| {
        let b = s.as_bytes();
        (b.len() == 10 && b[4] == b'-' && b[7] == b'-').then(|| clock::parse_rfc3339(&format!("{s}T00:00:00Z")))?
    })?;
    let ms = t.duration_since(UNIX_EPOCH).ok()?.as_millis();
    Some(UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).ok()?))
}

/// `t` as JavaScript's `toISOString` writes it: `2026-10-07T00:00:00.000Z`, years past 9999
/// as `+033658`.
pub fn rfc3339_millis(t: SystemTime) -> String {
    let d = t.duration_since(UNIX_EPOCH).unwrap_or_default();
    let base = clock::rfc3339(UNIX_EPOCH + Duration::from_secs(d.as_secs()));
    let (date, time) = base.split_once('T').unwrap_or((&base, ""));
    let (year, md) = date.split_once('-').unwrap_or((date, ""));
    let year = if year.len() > 4 { format!("+{year:0>6}") } else { year.to_owned() };
    let hms = time.strip_suffix('Z').unwrap_or(time);
    format!("{year}-{md}T{hms}.{:03}Z", d.subsec_millis())
}
