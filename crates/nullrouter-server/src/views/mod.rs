//! The read model (spec 008): one answer per read, shared by the CLI and, later, the dashboard.
//! See `specs/008-read-model/contracts/read-model.md`.
//!
//! A view is a sync function of the operator home's files and the live answers of the operator
//! ops it needs. A caller fetches those answers first, by [`fetch_socket`] (the CLI) or
//! [`fetch_in_process`] (inside the server), so the two routes differ only in transport.

use std::collections::BTreeMap;
use std::sync::Arc;

use nullrouter_engine::state::Engine;
use nullrouter_registry::{OperatorHome, RegistryHandle};
use serde_json::{Value, json};

pub mod accounts;
pub mod keys;
pub mod model;
pub mod providers;
pub mod quota;
pub mod records;
pub mod routing;

use crate::operator::{self, CallError};

/// What a view answers: the value `--json` prints, and the facts only the text shows.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    pub json: Value,
    /// Facts the text rendering needs and `json` lacks (research R3); `Null` when there are none.
    pub extra: Value,
}

impl View {
    pub fn new(json: Value) -> Self {
        Self { json, extra: Value::Null }
    }
}

/// A read that failed: the exact message and exit code the CLI gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewError {
    pub message: String,
    pub code: u8,
}

impl ViewError {
    pub fn new(code: u8, message: impl Into<String>) -> Self {
        Self { message: message.into(), code }
    }

    /// Exit code 1: an invalid input or file.
    pub fn failed(message: impl std::fmt::Display) -> Self {
        Self::new(1, message.to_string())
    }
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The live input of a view: for each op it needs, the server's answer, or `None` for no server.
/// A transport failure is an answer `{"ok":false,"error":"operator socket: …"}`.
#[derive(Debug, Default)]
pub struct Live {
    pub answers: BTreeMap<&'static str, Option<Value>>,
    /// Whether a server is running (it names unfinished records "in flight" or "cut short").
    pub running: bool,
}

impl Live {
    /// No server, no answers.
    pub fn none() -> Self {
        Self::default()
    }

    /// The raw answer to `op`; `None` with no server, or when the view didn't ask for it.
    pub fn answer(&self, op: &str) -> Option<&Value> {
        self.answers.get(op).and_then(Option::as_ref)
    }

    /// The answer to `op` when it is `ok`; a refusal is the error the CLI prints for it.
    pub fn ok(&self, op: &str) -> Result<Option<&Value>, ViewError> {
        match self.answer(op) {
            Some(a) if a["ok"] == true => Ok(Some(a)),
            Some(a) => Err(ViewError::failed(a["error"].as_str().unwrap_or("the server refused the request"))),
            None => Ok(None),
        }
    }
}

/// Opens the registry from the home's files. A failure is the startup error the CLI prints, so
/// every registry-backed view reports it the same way.
pub fn open_registry(home: &OperatorHome) -> Result<RegistryHandle, ViewError> {
    RegistryHandle::open(home.clone()).map_err(|e| ViewError::failed(format!("startup failed:\n{e}")))
}

/// The request for `op`, built from the view's arguments.
pub fn request(op: &str, args: &Value) -> Value {
    let keys: &[&str] = match op {
        "records.get" => &["id"],
        "routing.view" => &["target"],
        "quota.list" => &["provider", "name"],
        _ => &[],
    };
    let mut req = json!({ "op": op });
    for k in keys {
        req[*k] = args.get(*k).cloned().unwrap_or(Value::Null);
    }
    req
}

/// Whether a server answers on the home's operator socket.
pub fn server_runs(home: &OperatorHome) -> bool {
    std::os::unix::net::UnixStream::connect(operator::socket_path(home)).is_ok()
}

/// The CLI route: one `operator::call` per op in `needs`. A refused connection is "no server".
/// `running` is set by connecting to the socket, even when `needs` is empty.
pub fn fetch_socket(home: &OperatorHome, needs: &[&'static str], args: &Value) -> Live {
    let mut live = Live { running: server_runs(home), ..Live::default() };
    for op in needs {
        let answer = match operator::call(home, &request(op, args)) {
            Ok(a) => Some(a),
            Err(CallError::NoServer(_)) => None,
            Err(e) => Some(json!({"ok": false, "error": e.to_string()})),
        };
        live.answers.insert(op, answer);
    }
    live
}

/// The in-server route: `operator::handle` per op, the function the socket itself calls.
pub async fn fetch_in_process(engine: &Arc<Engine>, needs: &[&'static str], args: &Value) -> Live {
    let mut live = Live { running: true, ..Live::default() };
    for op in needs {
        live.answers.insert(op, Some(operator::handle(engine, &request(op, args)).await));
    }
    live
}

/// Fetches in-process, then builds on the blocking pool: views read files, and the async
/// executor must not block on them.
pub async fn run_in_process<F>(
    engine: &Arc<Engine>,
    needs: &[&'static str],
    args: &Value,
    build: F,
) -> Result<View, ViewError>
where
    F: FnOnce(&Live) -> Result<View, ViewError> + Send + 'static,
{
    let live = fetch_in_process(engine, needs, args).await;
    tokio::task::spawn_blocking(move || build(&live))
        .await
        .unwrap_or_else(|e| Err(ViewError::failed(format!("the read failed: {e}"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_carry_the_views_arguments() {
        let args = json!({"target": "sonnet", "id": "rq_1", "provider": "xai", "name": null, "other": 1});
        assert_eq!(request("routing.view", &args), json!({"op": "routing.view", "target": "sonnet"}));
        assert_eq!(request("records.get", &args), json!({"op": "records.get", "id": "rq_1"}));
        assert_eq!(request("quota.list", &args), json!({"op": "quota.list", "provider": "xai", "name": null}));
        assert_eq!(request("accounts.state", &args), json!({"op": "accounts.state"}));
    }

    #[test]
    fn a_refusal_is_the_error_the_cli_prints() {
        let mut live = Live::none();
        live.answers.insert("quota.list", Some(json!({"ok": false, "error": "nope"})));
        live.answers.insert("accounts.state", Some(json!({"ok": true, "accounts": []})));
        live.answers.insert("routing.view", None);
        assert_eq!(live.ok("quota.list"), Err(ViewError::failed("nope")));
        assert!(live.ok("accounts.state").unwrap().is_some());
        assert_eq!(live.ok("routing.view"), Ok(None), "no server");
        assert_eq!(live.ok("not asked"), Ok(None));
    }

    #[test]
    fn no_socket_means_no_server_and_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        let live = fetch_socket(&home, &["accounts.state"], &Value::Null);
        assert!(!live.running);
        assert_eq!(live.answers["accounts.state"], None);
    }
}
