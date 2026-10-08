//! Running a harness adapter against one body. Built-in adapters are resolved from a static
//! table by name; third-party ones are WASM handles (wired in with the store, user story 2).

use std::borrow::Cow;
use std::sync::Arc;
use std::time::{Duration, Instant};

use nullrouter_adapter_kit::{Context, Edits, Part};
use nullrouter_sandbox::{CallError, Entry, LoadedModule, Redactor, SandboxEngine};
use nullrouter_wire::codec::{Style, request};
use serde::Serialize;
use serde_json::Value;

use crate::apply::{self, ContentChange, Rule};
use crate::builtin::hermes;
use crate::guard::{self, Verdict};
use crate::record::{
    AdapterDirection, AdapterOutcome, AdapterRef, AdapterRun, FailReason, GuardrailEvent, NotRunReason,
};
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

/// A loaded third-party module, and what its call path needs around it.
pub struct WasmModule {
    pub sandbox: Arc<SandboxEngine>,
    pub module: Arc<LoadedModule>,
    /// What the manifest says the adapter reads on a request.
    pub request_selectors: Vec<Selector>,
    pub request_deadline: Duration,
    /// Scrubs secrets from the module's log lines.
    pub redact: Redactor,
}

/// A third-party adapter as the runner sees it: the serving version, if there is one.
#[derive(Clone)]
pub struct WasmHandle {
    pub harness: String,
    pub version: String,
    /// `None`: no version serves, and the arm records `not_run`.
    pub module: Option<Arc<WasmModule>>,
    /// The key's client style, which the guardrail decodes with. Set for each request by
    /// [`AdapterRunner::with_client`]; without it an edit is never applied.
    pub client: Option<Arc<Style>>,
}

impl WasmHandle {
    /// A harness with no approved version.
    pub fn absent(harness: &str) -> Self {
        Self { harness: harness.to_owned(), version: "none".into(), module: None, client: None }
    }

    pub fn loaded(harness: &str, version: &str, module: WasmModule) -> Self {
        Self { harness: harness.to_owned(), version: version.to_owned(), module: Some(Arc::new(module)), client: None }
    }
}

impl std::fmt::Debug for WasmHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmHandle")
            .field("harness", &self.harness)
            .field("version", &self.version)
            .field("loaded", &self.module.is_some())
            .finish()
    }
}

/// What a module is given: the attempt's context and the parts its selectors matched.
#[derive(Serialize)]
struct CallInput<'a> {
    ctx: &'a Context,
    parts: &'a [Part],
}

/// A test adapter's request-side function.
#[cfg(feature = "testkit")]
pub type RequestFn = Arc<dyn Fn(&Context, &Value) -> Edits + Send + Sync>;

/// A test adapter: a closure over the context and body, declared selectors included. Only
/// built with the `testkit` feature, so no production path can reach it. `response` runs on a
/// whole client-style answer, `event` on each stream event; both read `response_selectors`.
#[cfg(feature = "testkit")]
#[derive(Clone)]
pub struct Fixture {
    pub name: String,
    pub selectors: Vec<Selector>,
    pub request: RequestFn,
    pub response_selectors: Vec<Selector>,
    pub response: Option<RequestFn>,
    pub event: Option<RequestFn>,
}

#[cfg(feature = "testkit")]
impl Default for Fixture {
    fn default() -> Self {
        Fixture {
            name: String::new(),
            selectors: Vec::new(),
            request: Arc::new(|_, _| Edits::default()),
            response_selectors: Vec::new(),
            response: None,
            event: None,
        }
    }
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

    /// This runner for a request in the client style `client`. Only a third-party adapter uses
    /// it, for the guardrail.
    pub fn with_client(mut self, client: Arc<Style>) -> Self {
        if let AdapterRunner::Wasm(w) = &mut self {
            w.client = Some(client);
        }
        self
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
    pub async fn run_request<'a>(&self, ctx: &Context, body: &'a Value) -> RunOutcome<'a> {
        let started = Instant::now();
        let (selectors, edits) = match self {
            AdapterRunner::Builtin(b) => {
                let selectors = b.request_selectors();
                if selector::extract(body, &selectors).is_empty() {
                    return self.not_run(body, NotRunReason::NoSelectorMatch);
                }
                (selectors, b.on_request(ctx, body))
            }
            AdapterRunner::Wasm(w) => return self.wasm_request(w, ctx, body).await,
            #[cfg(feature = "testkit")]
            AdapterRunner::Fixture(f) => {
                if selector::extract(body, &f.selectors).is_empty() {
                    return self.not_run(body, NotRunReason::NoSelectorMatch);
                }
                (f.selectors.clone(), (f.request)(ctx, body))
            }
        };
        self.finish(body, &selectors, edits, started)
    }

    /// The request side of a third-party adapter: the selected parts go to the sandbox, the
    /// edits that come back are checked and applied to a copy, and the guardrail sees the copy.
    /// Whatever goes wrong, the original body goes on (FR-018).
    async fn wasm_request<'a>(&self, w: &WasmHandle, ctx: &Context, body: &'a Value) -> RunOutcome<'a> {
        let started = Instant::now();
        let Some(m) = &w.module else { return self.not_run(body, NotRunReason::NoApprovedVersion) };
        let parts = selector::extract(body, &m.request_selectors);
        if parts.is_empty() {
            return self.not_run(body, NotRunReason::NoSelectorMatch);
        }
        let Ok(input) = serde_json::to_vec(&CallInput { ctx, parts: &parts }) else {
            return self.failed(body, FailReason::InvalidOutput { rule: Rule::NotJson }, started);
        };
        let called = nullrouter_sandbox::call(
            &m.sandbox,
            &m.module,
            Entry::Request,
            &input,
            m.request_deadline,
            m.redact.clone(),
        )
        .await;
        let edits = match called {
            Ok(None) => Edits::default(),
            Ok(Some(bytes)) => match apply::parse_output(&bytes) {
                Ok(edits) => edits,
                Err(e) => return self.failed(body, FailReason::InvalidOutput { rule: e.rule }, started),
            },
            Err(e) => return self.failed(body, fail_reason(&e), started),
        };
        let mut out = self.finish(body, &m.request_selectors, edits, started);
        if out.run.outcome != AdapterOutcome::Ran || !matches!(out.body, Cow::Owned(_)) {
            return out;
        }
        let verdict = match &w.client {
            Some(client) => match request::decode(client, body) {
                Ok(before) => guard::check_request(client, &before, &out.body),
                Err(_) => Verdict::Undecodable,
            },
            None => {
                tracing::error!("adapter {} ran with no client style; its edits are dropped", w.harness);
                Verdict::Undecodable
            }
        };
        match verdict {
            Verdict::Ok => {}
            Verdict::Undecodable => {
                out.run.outcome =
                    AdapterOutcome::Failed { reason: FailReason::InvalidOutput { rule: Rule::Undecodable } };
            }
            Verdict::Violation { rule, paths } => {
                out.run.outcome = AdapterOutcome::Blocked;
                out.run.guardrail = Some(GuardrailEvent {
                    direction: AdapterDirection::Request,
                    rule,
                    paths,
                    adapter: AdapterRef { harness: w.harness.clone(), version: w.version.clone() },
                    at: jiff::Timestamp::now().to_string(),
                });
            }
        }
        if out.run.outcome != AdapterOutcome::Ran {
            out.run.changes.clear();
            out.body = Cow::Borrowed(body);
        }
        out.run.duration_us = micros(started);
        out
    }

    /// A run that failed: the original body goes on.
    fn failed<'a>(&self, body: &'a Value, reason: FailReason, started: Instant) -> RunOutcome<'a> {
        let (h, v) = self.identity();
        let mut run = AdapterRun::new(h, v, AdapterOutcome::Failed { reason });
        run.duration_us = micros(started);
        RunOutcome { body: Cow::Borrowed(body), run }
    }

    /// Checks `edits` against `selectors`, applies them to a copy, and reports the run.
    fn finish<'a>(&self, body: &'a Value, selectors: &[Selector], edits: Edits, started: Instant) -> RunOutcome<'a> {
        let (h, v) = self.identity();
        let mut run = AdapterRun::new(h, v, AdapterOutcome::Ran);
        let out = match apply::check(body, selectors, &edits.edits) {
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
        run.duration_us = micros(started);
        RunOutcome { body: out, run }
    }

    /// Whether the adapter reads responses at all. Until it does, a pass-through frame is never
    /// parsed for it (research R2).
    pub fn reads_responses(&self) -> bool {
        match self {
            AdapterRunner::Builtin(_) | AdapterRunner::Wasm(_) => false,
            #[cfg(feature = "testkit")]
            AdapterRunner::Fixture(f) => !f.response_selectors.is_empty() && (f.response.is_some() || f.event.is_some()),
        }
    }

    /// Runs the response side on a whole client-style answer.
    pub async fn run_response<'a>(&self, ctx: &Context, body: &'a Value) -> RunOutcome<'a> {
        self.run_answer(ctx, body, false).await
    }

    /// Runs the response side on one client-style stream event.
    pub async fn run_event<'a>(&self, ctx: &Context, event: &'a Value) -> RunOutcome<'a> {
        self.run_answer(ctx, event, true).await
    }

    async fn run_answer<'a>(&self, ctx: &Context, body: &'a Value, event: bool) -> RunOutcome<'a> {
        let started = Instant::now();
        match self {
            #[cfg(feature = "testkit")]
            AdapterRunner::Fixture(f) => {
                let Some(call) = (if event { &f.event } else { &f.response }) else {
                    return self.not_run(body, NotRunReason::NoSelectorMatch);
                };
                if selector::extract(body, &f.response_selectors).is_empty() {
                    return self.not_run(body, NotRunReason::NoSelectorMatch);
                }
                let edits = call(ctx, body);
                self.finish(body, &f.response_selectors, edits, started)
            }
            // hermes has no response side.
            _ => self.not_run(body, NotRunReason::NoSelectorMatch),
        }
    }
}

fn micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// The record's reason for a call that produced no output.
fn fail_reason(e: &CallError) -> FailReason {
    match e {
        CallError::Deadline => FailReason::Deadline,
        CallError::Memory => FailReason::Memory,
        CallError::Trap(_) => FailReason::Trap,
        CallError::InputTooLarge => FailReason::InvalidOutput { rule: Rule::InputTooLarge },
        CallError::InvalidOutput(why) => FailReason::InvalidOutput {
            rule: match *why {
                "output over the size limit" => Rule::OutputTooLarge,
                "input too large" => Rule::InputTooLarge,
                // The module's answer pointed outside its memory, or it had none to read.
                _ => Rule::NotJson,
            },
        },
    }
}

/// What a record keeps of a run's changes, for callers that only hold edits.
pub fn changes_of(edits: &Edits) -> Vec<ContentChange> {
    apply::changes(&edits.edits)
}
