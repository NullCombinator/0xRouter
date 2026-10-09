//! The plugin's `[routing]` declaration: prompt-cache behaviour, quota meters and a price schedule
//! (contracts/routing-schema.md, research R16). Data only: it holds no URL, header, credential or
//! code, so bundled and community plugins may both declare it.
//!
//! Parsing checks shapes (known keys, known enum values, units). The value rules (ranges,
//! uniqueness, which keys go together) are the gate's, in `validate/gate.rs`.

use std::fmt;
use std::time::Duration;

use indexmap::IndexMap;
use serde::de::{Deserializer, Error as _};
use serde::{Deserialize, Serialize, Serializer};

use super::duration::de_duration;
use super::enums::closed_enum;

closed_enum!(
    /// How the provider caches prompt prefixes.
    CacheMode, "cache mode" {
        /// Only up to the last marker the client set (Messages-style `cache_control`).
        Explicit = "explicit",
        /// Every prefix of at least `min_tokens` is cached without a marker.
        Automatic = "automatic",
        /// No prompt cache: nothing is warm.
        Disabled = "none",
    }
);

closed_enum!(
    /// What a quota meter counts.
    MeterUnit, "meter unit" {
        WeightedTokens = "weighted_tokens",
        Requests = "requests",
    }
);

closed_enum!(
    /// When an unreported window resets.
    ResetKind, "reset kind" {
        /// Cost inside the trailing `length` counts.
        Rolling = "rolling",
        /// At `anchor + n × length`.
        Fixed = "fixed",
        /// `length` after the first request following the previous window.
        FirstUse = "first_use",
    }
);

closed_enum!(
    /// A day of the week, for price schedules.
    Weekday, "weekday" {
        Mon = "mon",
        Tue = "tue",
        Wed = "wed",
        Thu = "thu",
        Fri = "fri",
        Sat = "sat",
        Sun = "sun",
    }
);

/// A percentage written `"5%"`. Stored as the number of percent (`5.0`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Percent(pub f64);

impl Percent {
    /// `"5%"` or `"12.5%"`. The `%` is required, so a bare number can't be read as a fraction.
    pub fn parse(s: &str) -> Result<Self, String> {
        let bad = || format!("{s:?} is not a percentage such as \"5%\"");
        let n = s.strip_suffix('%').ok_or_else(bad)?;
        let v: f64 = n.trim().parse().map_err(|_| bad())?;
        if v.is_finite() { Ok(Self(v)) } else { Err(bad()) }
    }

    /// The fraction, `0.05` for `5%`.
    pub fn fraction(self) -> f64 {
        self.0 / 100.0
    }
}

impl fmt::Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}%", self.0)
    }
}

impl<'de> Deserialize<'de> for Percent {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::parse(&String::deserialize(d)?).map_err(D::Error::custom)
    }
}

fn de_opt_duration<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    de_duration(d).map(Some)
}

/// The whole `[routing]` section. Every part is optional.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingDecl {
    pub cache: Option<CacheDecl>,
    /// `[[routing.window]]`: one meter per quota window the provider enforces.
    #[serde(default)]
    pub window: Vec<MeterDecl>,
    /// `[[routing.price]]`: first matching entry wins; per million tokens.
    #[serde(default)]
    pub price: Vec<PriceDecl>,
}

/// `[routing.cache]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheDecl {
    pub mode: CacheMode,
    /// How long an idle prefix stays cached. Default 5 minutes.
    #[serde(default, deserialize_with = "de_opt_duration")]
    pub lifetime: Option<Duration>,
    /// Smallest prefix an `automatic` cache keeps. Default 1024.
    pub min_tokens: Option<u64>,
}

/// Weights that turn a request's token counts into the cost a window is charged.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenWeights {
    #[serde(default = "one")]
    pub input: f64,
    #[serde(default = "one")]
    pub output: f64,
    #[serde(default = "one")]
    pub cache_read: f64,
    #[serde(default = "one")]
    pub cache_write: f64,
}

fn one() -> f64 {
    1.0
}

impl Default for TokenWeights {
    fn default() -> Self {
        Self { input: 1.0, output: 1.0, cache_read: 1.0, cache_write: 1.0 }
    }
}

/// `token_weights` in an operator's account override: each class is optional, and a class that
/// is absent keeps the declared weight.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartialTokenWeights {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

impl PartialTokenWeights {
    /// The four classes in `TokenClass` order, each with its value when one is set.
    pub fn classes(&self) -> [(&'static str, Option<f64>); 4] {
        [
            ("input", self.input),
            ("output", self.output),
            ("cache_read", self.cache_read),
            ("cache_write", self.cache_write),
        ]
    }
}

/// One `[[routing.window]]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeterDecl {
    /// Matches a `[quota]` window name; `*` matches any run of characters.
    pub name: String,
    #[serde(deserialize_with = "de_duration")]
    pub length: Duration,
    pub unit: MeterUnit,
    /// In `unit`. Absent: assumed from peers and shown as such.
    pub capacity: Option<f64>,
    /// Only with `weighted_tokens`. Absent: every weight is 1.
    pub token_weights: Option<TokenWeights>,
    /// Model glob → factor on the cost. The first matching glob applies.
    #[serde(default)]
    pub model_multiplier: IndexMap<String, f64>,
    /// The share of capacity kept back from cold work. Default 5%.
    pub reserve: Option<Percent>,
    /// Only for a window no `[quota]` rule names.
    pub reset: Option<ResetKind>,
    /// `HH:MM±hh:mm`, or `D HH:MM±hh:mm` for weekly or monthly windows. Needs `reset = "fixed"`.
    pub anchor: Option<String>,
}

/// The reserve floor of a window that declares none.
pub const DEFAULT_RESERVE: Percent = Percent(5.0);
/// How long a prefix stays cached when a plugin says nothing.
pub const DEFAULT_CACHE_LIFETIME: Duration = Duration::from_secs(300);
/// The smallest prefix an `automatic` cache keeps, when a plugin says nothing.
pub const DEFAULT_MIN_TOKENS: u64 = 1024;

impl MeterDecl {
    /// The reserve floor: the window's own, else 5%.
    pub fn reserve_or_default(&self) -> Percent {
        self.reserve.unwrap_or(DEFAULT_RESERVE)
    }

    /// Windows shorter than an hour only admit or refuse requests; they don't pace.
    pub fn is_admission(&self) -> bool {
        self.length < Duration::from_secs(3600)
    }
}

/// The cache behaviour in effect, defaults filled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveCache {
    pub mode: CacheMode,
    pub lifetime: Duration,
    /// Only meaningful for `automatic`.
    pub min_tokens: u64,
}

/// A plugin's routing declaration with the contract's defaults applied: `[routing.cache]` missing
/// is `automatic`, 5 minutes, 1024 tokens; no meters and no prices stay empty (the engine paces a
/// reported window in its own unit, and ranks an unpriced account as price 1).
#[derive(Debug, Clone, Copy)]
pub struct EffectiveRouting<'a> {
    pub cache: EffectiveCache,
    pub windows: &'a [MeterDecl],
    pub prices: &'a [PriceDecl],
}

impl<'a> EffectiveRouting<'a> {
    pub fn of(decl: Option<&'a RoutingDecl>) -> Self {
        let cache = match decl.and_then(|d| d.cache.as_ref()) {
            Some(c) => EffectiveCache {
                mode: c.mode,
                lifetime: c.lifetime.unwrap_or(DEFAULT_CACHE_LIFETIME),
                min_tokens: c.min_tokens.unwrap_or(DEFAULT_MIN_TOKENS),
            },
            None => EffectiveCache {
                mode: CacheMode::Automatic,
                lifetime: DEFAULT_CACHE_LIFETIME,
                min_tokens: DEFAULT_MIN_TOKENS,
            },
        };
        Self { cache, windows: decl.map_or(&[], |d| &d.window), prices: decl.map_or(&[], |d| &d.price) }
    }
}

/// One `[[routing.price]]`, per million tokens.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceDecl {
    /// Absent: the default price (at most one, and it comes last).
    pub when: Option<PriceWhen>,
    pub input: f64,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

/// When a price applies. Every part that is present must hold.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceWhen {
    #[serde(default)]
    pub days: Vec<Weekday>,
    /// `HH:MM`. `to` before `from` wraps past midnight.
    pub from: Option<String>,
    pub to: Option<String>,
    /// `±hh:mm` from UTC. Default `+00:00`.
    pub offset: Option<String>,
}

/// `HH:MM` as minutes since midnight.
pub fn parse_clock(s: &str) -> Result<u32, String> {
    let bad = || format!("{s:?} is not a time such as \"16:30\"");
    let (h, m) = s.split_once(':').ok_or_else(bad)?;
    if h.len() != 2 || m.len() != 2 {
        return Err(bad());
    }
    let (h, m): (u32, u32) = (h.parse().map_err(|_| bad())?, m.parse().map_err(|_| bad())?);
    if h > 23 || m > 59 { Err(bad()) } else { Ok(h * 60 + m) }
}

/// `±hh:mm` as signed minutes east of UTC.
pub fn parse_offset(s: &str) -> Result<i32, String> {
    let bad = || format!("{s:?} is not a UTC offset such as \"+00:00\" or \"-05:30\"");
    let sign = match s.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Err(bad()),
    };
    let t = parse_clock(&s[1..]).map_err(|_| bad())?;
    if t > 14 * 60 { Err(bad()) } else { Ok(sign * t as i32) }
}

/// A fixed-window anchor: an optional day and a clock time with an offset, `D HH:MM±hh:mm`.
/// `day` is 0 (Monday) to 6 for weekly anchors or 1 to 31 for monthly ones; the engine reads it
/// against the window's length. Returns `(day, minutes_since_midnight_utc)`.
pub fn parse_anchor(s: &str) -> Result<(Option<u32>, i32), String> {
    let bad = || format!("{s:?} is not an anchor such as \"00:00+00:00\" or \"3 09:00+00:00\"");
    let (day, rest) = match s.split_once(' ') {
        Some((d, r)) => (Some(d.parse::<u32>().map_err(|_| bad())?), r),
        None => (None, s),
    };
    let at = rest.find(['+', '-']).ok_or_else(bad)?;
    let clock = parse_clock(&rest[..at]).map_err(|_| bad())?;
    let offset = parse_offset(&rest[at..]).map_err(|_| bad())?;
    Ok((day, clock as i32 - offset))
}

/// `*` matches any run of characters, including none. Everything else matches itself.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if let Some(sp) = star {
            pi = sp + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

// The value rules below are shared by the plugin gate and by the operator's account overrides, so
// both refuse the same values with the same words. Each returns the rule that was broken.

/// `cache.lifetime`: more than zero, at most 24 hours.
pub fn check_lifetime(d: Duration) -> Result<(), String> {
    if d.is_zero() || d > Duration::from_secs(24 * 3600) {
        Err("must be more than 0 and at most 24h".into())
    } else {
        Ok(())
    }
}

/// `reserve`: 0 to 50 percent.
pub fn check_reserve(p: Percent) -> Result<(), String> {
    if (0.0..=50.0).contains(&p.0) { Ok(()) } else { Err("reserve must be between 0% and 50%".into()) }
}

/// `capacity`: more than zero.
pub fn check_capacity(c: f64) -> Result<(), String> {
    if c.is_finite() && c > 0.0 { Ok(()) } else { Err("capacity must be more than 0".into()) }
}

/// `window[].length`: more than zero.
pub fn check_length(d: Duration) -> Result<(), String> {
    if d.is_zero() { Err("length must be more than 0".into()) } else { Ok(()) }
}

/// A price per million tokens: not negative.
pub fn check_price_value(v: f64) -> Result<(), String> {
    if v.is_finite() && v >= 0.0 { Ok(()) } else { Err("a price must be 0 or more".into()) }
}

/// One `token_weights` class (`class` names it in the wording): 0 or more.
pub fn check_weight(class: &str, v: f64) -> Result<(), String> {
    if v.is_finite() && v >= 0.0 { Ok(()) } else { Err(format!("{class} must be 0 or more")) }
}

/// `token_weights` only applies to `weighted_tokens`.
pub fn check_weights_unit(unit: MeterUnit) -> Result<(), String> {
    if unit == MeterUnit::WeightedTokens { Ok(()) } else { Err("only applies to unit \"weighted_tokens\"".into()) }
}

/// A `model_multiplier` factor: more than 0. `glob` names it in the wording.
pub fn check_multiplier(glob: &str, f: f64) -> Result<(), String> {
    if f.is_finite() && f > 0.0 { Ok(()) } else { Err(format!("{glob:?}: a factor must be more than 0")) }
}

impl CacheDecl {
    /// The rules for one `[routing.cache]`: `(key, rule)` per broken rule.
    pub fn problems(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(Err(e)) = self.lifetime.map(check_lifetime) {
            out.push(("lifetime", e));
        }
        if self.min_tokens.is_some() && self.mode != CacheMode::Automatic {
            out.push(("min_tokens", "only applies to mode \"automatic\"".into()));
        }
        out
    }
}

impl MeterDecl {
    /// The rules for one `[[routing.window]]`: `(key, rule)` per broken rule. `reported` says a
    /// `[quota]` rule names this window, which then reports its own reset, so `reset` and
    /// `anchor` don't apply.
    pub fn problems(&self, reported: bool) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if self.name.is_empty() || self.name.chars().count() > 64 {
            out.push(("name", "must be 1 to 64 characters".into()));
        }
        if let Err(e) = check_length(self.length) {
            out.push(("length", e));
        }
        if let Some(Err(e)) = self.capacity.map(check_capacity) {
            out.push(("capacity", e));
        }
        if let Some(w) = &self.token_weights {
            if let Err(e) = check_weights_unit(self.unit) {
                out.push(("token_weights", e));
            }
            for (k, v) in [
                ("input", w.input),
                ("output", w.output),
                ("cache_read", w.cache_read),
                ("cache_write", w.cache_write),
            ] {
                if let Err(e) = check_weight(k, v) {
                    out.push(("token_weights", e));
                }
            }
        }
        for (glob, f) in &self.model_multiplier {
            if let Err(e) = check_multiplier(glob, *f) {
                out.push(("model_multiplier", e));
            }
        }
        if let Some(Err(e)) = self.reserve.map(check_reserve) {
            out.push(("reserve", e));
        }
        if reported {
            let why = "the provider reports this window's reset; `reset` and `anchor` are for unreported windows";
            if self.reset.is_some() {
                out.push(("reset", why.into()));
            }
            if self.anchor.is_some() {
                out.push(("anchor", why.into()));
            }
        } else {
            if self.reset == Some(ResetKind::Fixed) && self.anchor.is_none() {
                out.push(("reset", "\"fixed\" needs an anchor such as \"00:00+00:00\"".into()));
            }
            if self.reset != Some(ResetKind::Fixed) && self.anchor.is_some() {
                out.push(("anchor", "an anchor needs reset = \"fixed\"".into()));
            }
            if let Some(Err(e)) = self.anchor.as_deref().map(parse_anchor) {
                out.push(("anchor", e));
            }
        }
        out
    }
}

impl PriceDecl {
    /// The rules for one `[[routing.price]]`: `(key, rule)` per broken rule.
    pub fn problems(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        for (k, v) in [
            ("input", Some(self.input)),
            ("output", self.output),
            ("cache_read", self.cache_read),
            ("cache_write", self.cache_write),
        ] {
            if let Some(Err(e)) = v.map(check_price_value) {
                out.push((k, e));
            }
        }
        if let Some(w) = &self.when {
            for (k, v) in [("from", &w.from), ("to", &w.to)] {
                if let Some(Err(e)) = v.as_deref().map(parse_clock) {
                    out.push((k, e));
                }
            }
            if let Some(Err(e)) = w.offset.as_deref().map(parse_offset) {
                out.push(("offset", e));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_contract_example() {
        let text = r#"
            [cache]
            mode = "explicit"
            lifetime = "5m"

            [[window]]
            name = "5-hour"
            length = "5h"
            unit = "weighted_tokens"
            capacity = 9_000_000
            token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }
            model_multiplier = { "claude-opus-*" = 5.0 }
            reserve = "5%"

            [[window]]
            name = "per-minute"
            length = "1m"
            unit = "requests"
            capacity = 50

            [[price]]
            when = { days = ["mon", "tue"], from = "16:30", to = "00:30", offset = "+00:00" }
            input = 0.135

            [[price]]
            input = 0.27
        "#;
        let d: RoutingDecl = toml::from_str(text).unwrap();
        assert_eq!(d.cache.as_ref().unwrap().mode, CacheMode::Explicit);
        assert_eq!(d.window.len(), 2);
        assert!(!d.window[0].is_admission() && d.window[1].is_admission());
        assert_eq!(d.window[0].reserve, Some(Percent(5.0)));
        assert_eq!(d.window[0].token_weights.unwrap().output, 5.0);
        assert_eq!(d.price[0].when.as_ref().unwrap().days, [Weekday::Mon, Weekday::Tue]);
        assert!(d.price[1].when.is_none());
    }

    #[test]
    fn defaults_when_absent() {
        let e = EffectiveRouting::of(None);
        assert_eq!(
            e.cache,
            EffectiveCache { mode: CacheMode::Automatic, lifetime: DEFAULT_CACHE_LIFETIME, min_tokens: 1024 }
        );
        assert!(e.windows.is_empty() && e.prices.is_empty());
        let d: RoutingDecl = toml::from_str("[cache]\nmode = \"explicit\"").unwrap();
        assert_eq!(EffectiveRouting::of(Some(&d)).cache.lifetime, Duration::from_secs(300));
        let w: RoutingDecl = toml::from_str("[[window]]\nname = \"w\"\nlength = \"1h\"\nunit = \"requests\"").unwrap();
        assert_eq!(w.window[0].reserve_or_default(), Percent(5.0));
    }

    #[test]
    fn unknown_keys_and_values_are_refused() {
        for bad in [
            "[cache]\nmode = \"sometimes\"",
            "[cache]\nmode = \"none\"\nttl = \"5m\"",
            "[[window]]\nname = \"w\"\nlength = \"5h\"\nunit = \"credits\"",
            "[[window]]\nname = \"w\"\nlength = \"5h\"\nunit = \"requests\"\nreserve = \"5\"",
            "[[window]]\nname = \"w\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ntoken_weights = { thinking = 2.0 }",
            "[[price]]\noutput = 1.0",
            "[[price]]\ninput = 1.0\nwhen = { days = [\"funday\"] }",
            "url = \"https://example.com\"",
        ] {
            assert!(toml::from_str::<RoutingDecl>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn globs() {
        assert!(glob_match("weekly *", "weekly Opus") && glob_match("*", "") && glob_match("a*c", "abbc"));
        assert!(!glob_match("weekly *", "weekly") && !glob_match("a*c", "abd") && glob_match("5-hour", "5-hour"));
    }

    #[test]
    fn value_rules() {
        assert!(check_lifetime(Duration::from_secs(24 * 3600)).is_ok());
        assert!(check_lifetime(Duration::from_secs(25 * 3600)).is_err() && check_lifetime(Duration::ZERO).is_err());
        assert!(check_reserve(Percent(50.0)).is_ok() && check_reserve(Percent(60.0)).is_err());
        assert!(check_capacity(0.0).is_err() && check_capacity(1.0).is_ok());
        let w: RoutingDecl = toml::from_str(
            "[[window]]\nname = \"w\"\nlength = \"1d\"\nunit = \"requests\"\nreset = \"fixed\"\ntoken_weights = { input = 2.0 }",
        )
        .unwrap();
        let keys: Vec<_> = w.window[0].problems(false).into_iter().map(|(k, _)| k).collect();
        assert_eq!(keys, ["token_weights", "reset"]);
        assert_eq!(w.window[0].problems(true).iter().map(|(k, _)| *k).collect::<Vec<_>>(), ["token_weights", "reset"]);
    }

    #[test]
    fn percent_needs_its_sign() {
        assert_eq!(Percent::parse("5%"), Ok(Percent(5.0)));
        assert_eq!(Percent::parse("12.5%").unwrap().fraction(), 0.125);
        for bad in ["5", "%", "five%", "inf%"] {
            assert!(Percent::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn clocks_offsets_and_anchors() {
        assert_eq!(parse_clock("16:30"), Ok(990));
        assert!(parse_clock("24:00").is_err() && parse_clock("9:00").is_err() && parse_clock("09:60").is_err());
        assert_eq!(parse_offset("+00:00"), Ok(0));
        assert_eq!(parse_offset("-05:30"), Ok(-330));
        assert!(parse_offset("00:00").is_err() && parse_offset("+15:00").is_err());
        assert_eq!(parse_anchor("00:00+00:00"), Ok((None, 0)));
        assert_eq!(parse_anchor("3 09:00+02:00"), Ok((Some(3), 420)));
        assert!(parse_anchor("09:00").is_err() && parse_anchor("x 09:00+00:00").is_err());
    }
}
