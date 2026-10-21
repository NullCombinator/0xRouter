//! The routing decision (spec 006): warm first, then cold work by pace and deficit, then
//! pay-as-you-go as overflow.
//!
//! Everything under this module is pure. It does no I/O and never reads the clock: `now` is an
//! argument, so the simulated week (US6) can drive it with an injected clock. The `no_io` test
//! below keeps it that way.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, SystemTime};

use nullrouter_registry::schema::CacheMode;
use serde::{Deserialize, Serialize};

use crate::quota::fit::Source;

pub use meter::{AccountQuota, QuotaState};
pub use price::PriceSpec;
pub use router::{Router, Salt};

pub mod fingerprint;
pub mod ledger;
pub mod meter;
pub mod pace;
pub mod place;
pub mod price;
pub mod router;
pub mod view;
pub mod warm;

/// One account serving one model: what a placement chooses among.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CandidateKey {
    pub provider: String,
    pub account: String,
    pub model: String,
}

impl CandidateKey {
    pub fn new(provider: impl Into<String>, account: impl Into<String>, model: impl Into<String>) -> Self {
        Self { provider: provider.into(), account: account.into(), model: model.into() }
    }

    /// `provider/account`: the identity quota and deficits belong to.
    pub fn account_key(&self) -> String {
        format!("{}/{}", self.provider, self.account)
    }
}

impl fmt::Display for CandidateKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.provider, self.account)
    }
}

/// Subscription accounts are balanced by pace; pay-as-you-go accounts are overflow (FR-025).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    Subscription,
    Payg,
}

/// Where an account's quota comes from, as the routing view labels it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuotaSource {
    #[serde(rename = "polled")]
    Polled,
    #[serde(rename = "estimated")]
    Estimated,
    #[serde(rename = "pay-as-you-go")]
    PayAsYouGo,
}

impl QuotaSource {
    pub fn tier(self) -> Tier {
        match self {
            Self::PayAsYouGo => Tier::Payg,
            Self::Polled | Self::Estimated => Tier::Subscription,
        }
    }
}

/// Why a candidate was not an option for this request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhyNot {
    OutOfService,
    Cooling,
    Admission,
    ReserveFloor,
    PriorityZero,
    NotASubscription,
}

impl fmt::Display for WhyNot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OutOfService => "out of service",
            Self::Cooling => "cooling down",
            Self::Admission => "per-minute limit",
            Self::ReserveFloor => "reserve floor",
            Self::PriorityZero => "priority 0",
            Self::NotASubscription => "not a subscription",
        })
    }
}

/// Why an attempt went to the account it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlacementReason {
    Warm,
    ColdByDeficit,
    MovedForCapacity,
    LeftPayAsYouGo,
    Overflow,
    LastResort,
    Retry,
    Fallback,
}

/// Why a warm request left the account holding its cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovedBecause {
    WarmUnusable,
    RateLimited,
    ReserveFloor,
    LeftPayAsYouGo,
}

/// A request's agent had its prefix warm here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WarmHit {
    pub provider: String,
    pub account: String,
    pub model: String,
    pub prefix_tokens: u64,
    pub idle_s: f64,
    pub stayed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_because: Option<MovedBecause>,
}

/// One row of the decision table: a candidate and what the placement knew about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateRow {
    pub provider: String,
    pub account: String,
    pub model: String,
    pub tier: Tier,
    pub eligible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why_not: Option<WhyNot>,
    pub quota_source: QuotaSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
    pub priority: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weight: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share: Option<f64>,
    /// Tokens owed to this account before this placement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deficit_before: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_now: Option<f64>,
    /// Window name → number name → source, for the numbers the quota read that aren't declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meter_sources: Option<BTreeMap<String, BTreeMap<String, Source>>>,
}

impl CandidateRow {
    pub fn key(&self) -> CandidateKey {
        CandidateKey::new(&self.provider, &self.account, &self.model)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Warm,
    Cold,
    Overflow,
    None,
}

/// The amortization window a cold placement counted in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmortizationWindow {
    #[serde(with = "time_serde")]
    pub start: SystemTime,
    #[serde(with = "duration_serde")]
    pub length: Duration,
}

/// The placement decision recorded on a request: everything needed to recompute it (SC-006).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub kind: DecisionKind,
    #[serde(with = "time_serde")]
    pub at: SystemTime,
    pub amortization_window: AmortizationWindow,
    pub size_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warm: Option<WarmHit>,
    pub candidates: Vec<CandidateRow>,
    /// Indexes into `candidates`, in attempt order.
    pub order: Vec<usize>,
}

/// The attempt order a placement chose, with the reason for each step.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    pub decision: Decision,
    /// `(candidate, reason, rank)` in attempt order.
    pub steps: Vec<(CandidateKey, PlacementReason, usize)>,
    /// `(account key, share)` of the subscription accounts cold work was shared among.
    pub shares: Vec<(String, f64)>,
    /// The same for the pay-as-you-go accounts overflow was shared among.
    pub payg_shares: Vec<(String, f64)>,
}

/// The prompt cache of one candidate, with the operator's override applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheSpec {
    pub mode: CacheMode,
    pub lifetime: Duration,
    pub min_tokens: u64,
}

/// One account serving one model, as a placement sees it. Built from the engine state at the
/// moment of the decision; nothing in it is read again afterwards.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub key: CandidateKey,
    /// The operator's `order`: the last tie-break.
    pub order: i64,
    /// Never negative. 0 means never cold work.
    pub priority: f64,
    /// Why the account can't be used at all (slice 005 states), if so.
    pub out_of_service: Option<String>,
    /// Time left on a 429 or 5xx rest for this account and model (slice 003).
    pub cooling: Option<Duration>,
    pub quota: AccountQuota,
    pub cache: CacheSpec,
    pub price: PriceSpec,
    /// Carried into the decision row: the non-declared numbers in effect, per window.
    pub meter_sources: Option<BTreeMap<String, BTreeMap<String, Source>>>,
}

/// What `place` reads: one target's candidates and the request's facts.
#[derive(Debug, Clone)]
pub struct RoutingInput {
    /// The unified model name or `provider/model`; the key of its deficit ledgers.
    pub target: String,
    pub candidates: Vec<Candidate>,
    /// The amortization length for this target.
    pub amortization: Duration,
    /// The request's estimated input tokens (research R7).
    pub size_tokens: u64,
    /// The agent's longest warm prefix among the candidates, if any.
    pub warm: Option<WarmEntry>,
    /// Tokens owed to each account (`provider/account`) in this target's subscription ledger.
    pub deficits: std::collections::BTreeMap<String, f64>,
}

/// A warm store hit, as `place` takes it.
#[derive(Debug, Clone, PartialEq)]
pub struct WarmEntry {
    pub key: CandidateKey,
    pub hash: fingerprint::Hash,
    pub prefix_tokens: u64,
    pub last_used: SystemTime,
}

mod time_serde {
    use std::time::SystemTime;

    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer>(t: &SystemTime, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::quota::extract::rfc3339_millis(*t))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<SystemTime, D::Error> {
        let text = String::deserialize(d)?;
        crate::clock::parse_rfc3339(&text).ok_or_else(|| D::Error::custom(format!("{text:?} is not a time")))
    }
}

/// An optional time, written like [`time_serde`] and `null` when absent.
mod opt_time_serde {
    use std::time::SystemTime;

    use serde::Serializer;

    pub fn serialize<S: Serializer>(t: &Option<SystemTime>, s: S) -> Result<S::Ok, S::Error> {
        match t {
            Some(t) => s.serialize_str(&crate::quota::extract::rfc3339_millis(*t)),
            None => s.serialize_none(),
        }
    }
}

/// Durations are written as `5h`, `30m`, as in the operator's files.
mod duration_serde {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

    pub fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        let secs = d.as_secs();
        let text = match () {
            () if secs != 0 && secs.is_multiple_of(86_400) => format!("{}d", secs / 86_400),
            () if secs != 0 && secs.is_multiple_of(3600) => format!("{}h", secs / 3600),
            () if secs != 0 && secs.is_multiple_of(60) => format!("{}m", secs / 60),
            () => format!("{secs}s"),
        };
        s.serialize_str(&text)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        nullrouter_registry::schema::parse_duration(&String::deserialize(d)?).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod no_io {
    /// Source text of every pure routing file, paired with its name for the failure message.
    const SOURCES: [(&str, &str); 8] = [
        ("fingerprint.rs", include_str!("fingerprint.rs")),
        ("ledger.rs", include_str!("ledger.rs")),
        ("meter.rs", include_str!("meter.rs")),
        ("pace.rs", include_str!("pace.rs")),
        ("place.rs", include_str!("place.rs")),
        ("price.rs", include_str!("price.rs")),
        ("view.rs", include_str!("view.rs")),
        ("warm.rs", include_str!("warm.rs")),
    ];

    #[test]
    fn routing_files_do_no_io_and_read_no_clock() {
        // This file is not in `SOURCES`, so naming the needles here is safe.
        let needles = ["std::fs", "std::net", "tokio", "SystemTime::now"];
        for (name, src) in SOURCES {
            for needle in needles {
                assert!(!src.contains(needle), "routing/{name} mentions `{needle}`; routing is pure");
            }
        }
    }
}

#[cfg(test)]
mod types_tests {
    use super::*;

    #[test]
    fn decision_serializes_as_the_journal_contract() {
        let at = crate::clock::parse_rfc3339("2026-10-04T09:12:03.120Z").unwrap();
        let start = crate::clock::parse_rfc3339("2026-10-04T05:00:00Z").unwrap();
        let d = Decision {
            kind: DecisionKind::Cold,
            at,
            amortization_window: AmortizationWindow { start, length: Duration::from_secs(5 * 3600) },
            size_tokens: 18_400,
            warm: None,
            candidates: vec![CandidateRow {
                provider: "anthropic".into(),
                account: "max".into(),
                model: "claude-sonnet-4-5".into(),
                tier: Tier::Subscription,
                eligible: true,
                why_not: None,
                quota_source: QuotaSource::Polled,
                pace: Some(1.42),
                rate: Some(5210.0),
                priority: 1.0,
                weight: Some(7398.2),
                share: Some(0.61),
                deficit_before: Some(91_200),
                price_now: None,
                meter_sources: None,
            }],
            order: vec![0],
        };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["kind"], "cold");
        assert_eq!(v["at"], "2026-10-04T09:12:03.120Z");
        assert_eq!(v["amortization_window"]["length"], "5h");
        assert_eq!(v["candidates"][0]["tier"], "subscription");
        assert_eq!(v["candidates"][0]["quota_source"], "polled");
        assert!(v["candidates"][0].get("why_not").is_none() && v["candidates"][0].get("price_now").is_none());
        let back: Decision = serde_json::from_value(v).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn wire_names() {
        let s = |v: &dyn erased::Ser| v.json();
        assert_eq!(s(&QuotaSource::PayAsYouGo), "\"pay-as-you-go\"");
        assert_eq!(s(&Tier::Payg), "\"payg\"");
        assert_eq!(s(&WhyNot::PriorityZero), "\"priority_zero\"");
        assert_eq!(s(&PlacementReason::LeftPayAsYouGo), "\"left_pay_as_you_go\"");
        assert_eq!(s(&MovedBecause::WarmUnusable), "\"warm_unusable\"");
        assert_eq!(QuotaSource::Estimated.tier(), Tier::Subscription);
    }

    mod erased {
        pub trait Ser {
            fn json(&self) -> String;
        }
        impl<T: serde::Serialize> Ser for T {
            fn json(&self) -> String {
                serde_json::to_string(self).unwrap()
            }
        }
    }
}
