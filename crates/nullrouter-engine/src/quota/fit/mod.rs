//! The quota fit (spec 012): learns each polled account's real meter from its poll history,
//! keeps outside use out of the evidence, and hands routing a precomputed meter in effect.
//!
//! The fit sits beside routing, not inside it (research R1). Rows come from the history that
//! already exists (R2), the model and its noise are R3 and R4, the one always-valid test is R5,
//! and classification, separability, split-off, breaks, alerts and epochs are R6–R12.

pub mod breaks;
pub mod classify;
pub mod linalg;
pub mod model;
pub mod outside;
pub mod rows;
pub mod split;
pub mod store;
pub mod test;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// One of the four token classes a `weighted_tokens` window charges differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TokenClass {
    Input,
    Output,
    CacheRead,
    CacheWrite,
}

impl TokenClass {
    pub const ALL: [TokenClass; 4] = [Self::Input, Self::Output, Self::CacheRead, Self::CacheWrite];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
            Self::CacheRead => "cache_read",
            Self::CacheWrite => "cache_write",
        }
    }
}

impl FromStr for TokenClass {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Self::ALL.into_iter().find(|c| c.as_str() == s).ok_or_else(|| format!("unknown token class \"{s}\""))
    }
}

/// A name for one number of one window's meter (data-model § Meter number). `Capacity` is per
/// account; weights and multipliers are pooled per plugin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MeterNumber {
    Capacity,
    Weight(TokenClass),
    /// The glob as the plugin declares it.
    Multiplier(String),
}

impl fmt::Display for MeterNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity => f.write_str("capacity"),
            Self::Weight(c) => write!(f, "weight.{}", c.as_str()),
            Self::Multiplier(g) => write!(f, "multiplier.{g}"),
        }
    }
}

impl FromStr for MeterNumber {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        if s == "capacity" {
            Ok(Self::Capacity)
        } else if let Some(c) = s.strip_prefix("weight.") {
            c.parse().map(Self::Weight)
        } else if let Some(g) = s.strip_prefix("multiplier.").filter(|g| !g.is_empty()) {
            Ok(Self::Multiplier(g.to_owned()))
        } else {
            Err(format!("unknown meter number \"{s}\""))
        }
    }
}

impl Serialize for MeterNumber {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MeterNumber {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Where the value in effect for a number came from (FR-012), strongest first. A plugin
/// override never applies to `Capacity` (clarify Q5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    AccountOverride,
    PluginOverride,
    Fit,
    Declared,
}
