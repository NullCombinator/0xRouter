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
pub mod connection;
pub mod cooldown;
pub mod files;
pub mod forwarding;
pub mod identity;
pub mod inband;
pub mod internal;
pub mod jobs;
pub mod journal;
pub mod live;
pub mod keys;
pub mod maintenance;
pub mod models_live;
pub mod phases;
pub mod plan;
pub mod quota;
pub mod records;
pub mod redact;
pub mod response_side;
pub mod review_queue;
pub mod route;
pub mod routing;
pub mod signin;
pub mod state;
pub mod status;
pub mod tests;
pub mod timing;
pub mod tokens;
pub mod upstream;
pub mod verdict;

#[cfg(feature = "testkit")]
pub mod testkit;
