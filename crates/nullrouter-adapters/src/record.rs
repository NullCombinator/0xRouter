//! What a harness adapter did on one call, as a record keeps it (data-model § AdapterRun).
//! It lives here, not in the engine, because the runner returns it and the engine depends on
//! this crate.

use serde::Serialize;

use crate::apply::{ContentChange, Rule as InvalidOutputRule};

/// What a harness adapter did on one call (data-model § AdapterRun). It holds paths, kinds and
/// reasons only: never a value, a preview or a length (FR-025).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdapterRun {
    pub harness: String,
    /// The reviewed version's id, or `builtin`.
    pub version: String,
    pub outcome: AdapterOutcome,
    pub changes: Vec<ContentChange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guardrail: Option<GuardrailEvent>,
    pub duration_us: u64,
}

impl AdapterRun {
    pub fn new(harness: &str, version: &str, outcome: AdapterOutcome) -> Self {
        Self {
            harness: harness.to_owned(),
            version: version.to_owned(),
            outcome,
            changes: Vec::new(),
            guardrail: None,
            duration_us: 0,
        }
    }

    /// Rewrites every client-derived string with `clean`. Paths carry the client's own key
    /// names, so the engine passes its redactor here.
    pub fn clean_with(&mut self, clean: impl Fn(&str) -> String) {
        for c in &mut self.changes {
            c.path = clean(&c.path);
        }
        if let Some(g) = &mut self.guardrail {
            for p in &mut g.paths {
                *p = clean(p);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AdapterOutcome {
    Ran,
    NotRun { reason: NotRunReason },
    Failed { reason: FailReason },
    /// The guardrail discarded the adapter's edits.
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotRunReason {
    NoApprovedVersion,
    Suspect,
    SourceMismatch,
    Rebuilding,
    RebuildFailed,
    Removed,
    NoSelectorMatch,
    MediaRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FailReason {
    Trap,
    Deadline,
    Memory,
    InvalidOutput { rule: InvalidOutputRule },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterDirection {
    Request,
    Response,
    Event,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuardrailRule {
    ToolCallAdded,
    ToolCallChanged,
    ToolDefAdded,
    ToolDefChanged,
    ToolResultAdded,
    ToolResultChanged,
    OpaqueAdded,
    UnplacedAdded,
}

/// A violation the guardrail caught, written with the run that caused it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuardrailEvent {
    pub direction: AdapterDirection,
    pub rule: GuardrailRule,
    /// The edit paths involved, at most 16.
    pub paths: Vec<String>,
    pub adapter: AdapterRef,
    /// RFC 3339.
    pub at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AdapterRef {
    pub harness: String,
    pub version: String,
}
