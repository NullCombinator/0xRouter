//! The operations the CLI and the dashboard share (spec 014, data-model.md § Operation): each
//! one changes files or the running engine and says what happened. The CLI prints the result;
//! the dashboard shows it.

pub mod config_text;

use std::sync::Arc;

use nullrouter_engine::state::Engine;
pub use nullrouter_engine::tasks::TaskId;
use nullrouter_registry::{OperatorHome, SecretString};
use serde_json::json;
use tracing::{error, info, warn};

use crate::operator::{self, CallError};

/// Where an operation runs: the operator's home, and the engine when it runs inside `serve`.
pub struct Ctx {
    pub home: OperatorHome,
    /// `Some` inside the server, which reloads in-process; `None` in the CLI, which asks the
    /// running server over the operator socket.
    pub engine: Option<Arc<Engine>>,
}

/// What an operation returns to its caller.
#[derive(Debug)]
pub struct Done {
    pub status: Status,
    pub text: String,
    /// A secret shown once, such as a client key just issued.
    pub once: Option<Once>,
}

/// Whether a change is live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Saved and live.
    Applied,
    /// Saved, but the running server did not take it.
    SavedNotApplied { reason: String },
    /// A long-running task was started; its progress is read by `task`.
    Started { task: TaskId },
}

/// A secret shown once: its label and the value. Its `Debug` prints `***`.
#[derive(Debug)]
pub struct Once {
    pub label: String,
    pub value: SecretString,
}

/// A failed operation: the message, the CLI exit code (1 invalid, 3 refused by the gate,
/// 4 no server, 5 sign-in ended), and the form field it belongs to, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpError {
    pub text: String,
    pub exit: u8,
    pub field: Option<&'static str>,
}

/// What a reload produced: the text to print and the notes it raised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// `applied`, or `saved; applies at next start` with no server to tell.
    pub text: &'static str,
    /// Unified models whose members' limits differ, as the server reports them.
    pub notes: Vec<String>,
}

/// Asks the running server to reload the saved files. Inside the server the engine reloads
/// in-process; otherwise the operator socket's `reload` is sent. `Err` carries why the running
/// server kept its previous state or was not told.
pub fn apply(ctx: &Ctx) -> Result<Applied, String> {
    match &ctx.engine {
        Some(engine) => reload_in_process(engine),
        None => reload_by_socket(&ctx.home),
    }
}

fn reload_in_process(engine: &Engine) -> Result<Applied, String> {
    match engine.reload_blocking() {
        Ok(r) => {
            for a in &r.unused_accounts {
                warn!("account {a} names a provider that isn't loaded");
            }
            let notes: Vec<String> = engine.snapshot().registry.report().notes.iter().map(ToString::to_string).collect();
            for n in &notes {
                info!("{n}");
            }
            Ok(Applied { text: "applied", notes })
        }
        Err(e) => {
            let error_text = engine.snapshot().redactor.redact(&e.to_string()).into_owned();
            error!("reload failed, the previous state stays: {error_text}");
            Err(format!("saved, but the running server kept its previous state: {error_text}"))
        }
    }
}

fn reload_by_socket(home: &OperatorHome) -> Result<Applied, String> {
    match operator::call(home, &json!({"op": "reload"})) {
        Ok(a) if a["ok"] == true => {
            let notes: Vec<String> =
                a["notes"].as_array().into_iter().flatten().filter_map(|n| n.as_str()).map(str::to_owned).collect();
            Ok(Applied { text: "applied", notes })
        }
        Ok(a) => Err(format!(
            "saved, but the running server kept its previous state: {}",
            a["error"].as_str().unwrap_or("unknown error")
        )),
        Err(CallError::NoServer(_)) => Ok(Applied { text: "saved; applies at next start", notes: Vec::new() }),
        Err(e) => Err(format!("saved, but the running server wasn't told: {e}")),
    }
}
