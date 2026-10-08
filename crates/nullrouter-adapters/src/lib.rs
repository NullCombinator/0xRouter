//! Harness adapters: the built-in hermes adapter and the pipeline for third-party ones.

pub mod alerts;
pub mod apply;
pub mod builder_client;
pub mod builtin;
pub mod catalogue;
pub mod fingerprint;
pub mod gate;
pub mod guard;
pub mod loader;
pub mod record;
pub mod review;
pub mod runner;
pub mod scramble;
pub mod selector;
pub mod store;
#[cfg(feature = "testkit")]
pub mod testkit;

use std::fmt;

use serde::{Deserialize, Serialize};

/// Adapters that ship in the core.
pub const BUILTIN: &[&str] = &["hermes"];
/// Names kept for harnesses that are handled elsewhere or are not ours to adapt.
pub const RESERVED: &[&str] = &["opencode", "grok-build", "zcode"];

/// The name of a harness a key is bound to: `^[a-z][a-z0-9-]{1,31}$`, and not reserved.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct HarnessName(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HarnessNameError {
    #[error("harness names are 2-32 characters: a lowercase letter, then lowercase letters, digits or '-' ({0:?})")]
    Malformed(String),
    #[error("{0} is a reserved harness name")]
    Reserved(String),
}

impl HarnessName {
    pub fn new(s: &str) -> Result<Self, HarnessNameError> {
        let b = s.as_bytes();
        let ok = (2..=32).contains(&b.len())
            && b[0].is_ascii_lowercase()
            && b[1..].iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
        if !ok {
            return Err(HarnessNameError::Malformed(s.to_owned()));
        }
        if RESERVED.contains(&s) {
            return Err(HarnessNameError::Reserved(s.to_owned()));
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_builtin(&self) -> bool {
        BUILTIN.contains(&self.0.as_str())
    }
}

impl fmt::Display for HarnessName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HarnessName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}
