//! The 0router request engine (spec 003).
//!
//! Holds the operator state (accounts, agent keys, config) as one swappable snapshot, runs
//! the attempt loop (classification, retry, fallback, stay-warm), talks to upstreams, and
//! keeps the request records. It has no HTTP-server types; `nullrouter-server` sits on top.

pub mod accounts;
pub mod attempt;
pub mod breaks;
pub mod classify;
pub mod clock;
pub mod cooldown;
pub mod files;
pub mod forwarding;
pub mod identity;
pub mod inband;
pub mod jobs;
pub mod keys;
pub mod maintenance;
pub mod plan;
pub mod quota;
pub mod records;
pub mod redact;
pub mod signin;
pub mod state;
pub mod tokens;
pub mod upstream;

#[cfg(feature = "testkit")]
pub mod testkit;
