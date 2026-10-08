//! Running a harness adapter against one body. Built-in adapters are resolved from a static
//! table by name; third-party ones are WASM handles (wired in with the store, user story 2).

use std::borrow::Cow;
#[cfg(feature = "testkit")]
use std::sync::Arc;
use std::time::Instant;

use nullrouter_adapter_kit::{Context, Edits};
use serde_json::Value;

use crate::apply::{self, ContentChange};
use crate::builtin::hermes;
use crate::record::{AdapterOutcome, AdapterRun, FailReason, NotRunReason};
use crate::selector::{self, Selector};
use crate::HarnessName;

/// The built-in adapters, a closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Hermes,
}

impl Builtin {
    pub fn named(name: &HarnessName) -> Option<Builtin> {
        match name.as_str() {
            hermes::NAME => Some(Builtin::Hermes),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Builtin::Hermes => hermes::NAME,
        }
    }

    fn request_selectors(self) -> Vec<Selector> {
        match self {
            Builtin::Hermes => hermes::request_selectors(),
        }
    }

    fn on_request(self, ctx: &Context, body: &Value) -> Edits {
        match self {
            Builtin::Hermes => hermes::on_request(ctx, body),
        }
    }
}

/// A loaded third-party module. Filled in by the store; until then it never runs.
#[derive(Debug, Clone)]
pub struct WasmHandle {
    pub harness: String,
    pub version: String,
}

/// A test adapter: a closure over the context and body, declared selectors included. Only
/// built with the `testkit` feature, so no production path can reach it.
#[cfg(feature = "testkit")]
#[derive(Clone)]
pub struct Fixture {
    pub name: String,
    pub selectors: Vec<Selector>,
    pub request: Arc<dyn Fn(&Context, &Value) -> Edits + Send + Sync>,
}

#[cfg(feature = "testkit")]
impl std::fmt::Debug for Fixture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fixture").field("name", &self.name).finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum AdapterRunner {
    Builtin(Builtin),
    Wasm(WasmHandle),
    #[cfg(feature = "testkit")]
    Fixture(Fixture),
}

/// The body to go on with, and what happened.
#[derive(Debug)]
pub struct RunOutcome<'a> {
    pub body: Cow<'a, Value>,
    pub run: AdapterRun,
}

impl AdapterRunner {
    /// The runner for a key's harness, if it is a built-in one.
    pub fn builtin(name: &HarnessName) -> Option<AdapterRunner> {
        Builtin::named(name).map(AdapterRunner::Builtin)
    }

    fn identity(&self) -> (&str, &str) {
        match self {
            AdapterRunner::Builtin(b) => (b.name(), "builtin"),
            AdapterRunner::Wasm(w) => (&w.harness, &w.version),
            #[cfg(feature = "testkit")]
            AdapterRunner::Fixture(f) => (&f.name, "fixture"),
        }
    }

    /// A run that did nothing, for `reason`.
    pub fn not_run<'a>(&self, body: &'a Value, reason: NotRunReason) -> RunOutcome<'a> {
        let (h, v) = self.identity();
        RunOutcome { body: Cow::Borrowed(body), run: AdapterRun::new(h, v, AdapterOutcome::NotRun { reason }) }
    }

    /// Runs the request side on the client-style body. Edits are checked, then applied to a
    /// copy; the body comes back borrowed when nothing changed.
    pub fn run_request<'a>(&self, ctx: &Context, body: &'a Value) -> RunOutcome<'a> {
        let started = Instant::now();
        let (selectors, edits) = match self {
            AdapterRunner::Builtin(b) => {
                let selectors = b.request_selectors();
                if selector::extract(body, &selectors).is_empty() {
                    return self.not_run(body, NotRunReason::NoSelectorMatch);
                }
                (selectors, b.on_request(ctx, body))
            }
            AdapterRunner::Wasm(_) => return self.not_run(body, NotRunReason::NoApprovedVersion),
            #[cfg(feature = "testkit")]
            AdapterRunner::Fixture(f) => {
                if selector::extract(body, &f.selectors).is_empty() {
                    return self.not_run(body, NotRunReason::NoSelectorMatch);
                }
                (f.selectors.clone(), (f.request)(ctx, body))
            }
        };
        let (h, v) = self.identity();
        let mut run = AdapterRun::new(h, v, AdapterOutcome::Ran);
        let out = match apply::check(body, &selectors, &edits.edits) {
            Err(e) => {
                run.outcome = AdapterOutcome::Failed { reason: FailReason::InvalidOutput { rule: e.rule } };
                Cow::Borrowed(body)
            }
            Ok(()) if edits.is_empty() => Cow::Borrowed(body),
            Ok(()) => {
                run.changes = apply::changes(&edits.edits);
                Cow::Owned(apply::apply(body, &edits.edits))
            }
        };
        run.duration_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        RunOutcome { body: out, run }
    }

    /// hermes has no response side, so the response runs report that no selector matched.
    pub fn run_response<'a>(&self, _ctx: &Context, body: &'a Value) -> RunOutcome<'a> {
        self.not_run(body, NotRunReason::NoSelectorMatch)
    }

    pub fn run_event<'a>(&self, _ctx: &Context, event: &'a Value) -> RunOutcome<'a> {
        self.not_run(event, NotRunReason::NoSelectorMatch)
    }
}

/// What a record keeps of a run's changes, for callers that only hold edits.
pub fn changes_of(edits: &Edits) -> Vec<ContentChange> {
    apply::changes(&edits.edits)
}
