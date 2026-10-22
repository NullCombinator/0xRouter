//! `POST /do/<area>/<op>` (specs/014-functional-dashboard, contracts/dashboard-forms.md "Every
//! form"): the checks, the typed op, the change-log line, then `303` with the flash.
//!
//! Handlers live in a table keyed by the route's `<area>/<op>`; an op id has `.` where its URL has
//! `-` (`keys/set-break` is `keys.set_break`). A handler is a plain function from the parsed form
//! to its subject and the op's result; it is sync and runs on the blocking pool under the
//! dashboard guard, so a stalled op answers with a refusal and never holds a request open.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use jiff::Zoned;
use nullrouter_server::ops::{Ctx, Done, OpError, Status};

use crate::forms::{FlashKind, Refusal};
use crate::log::{self, Outcome};
use crate::pages::signin::see_other;
use crate::{Shared, access};

/// The most a form body may hold: a model list is the largest thing a form carries.
const MAX_BODY: usize = 256 * 1024;

/// The fields of one post, decoded.
pub struct Form(HashMap<String, String>);

impl Form {
    /// The field `name`, or `""` when it is absent.
    pub fn get(&self, name: &str) -> &str {
        self.0.get(name).map_or("", String::as_str)
    }

    /// The field `name` when present and not empty.
    pub fn opt(&self, name: &str) -> Option<&str> {
        self.0.get(name).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// A checkbox or switch: submitted as `on` when set.
    pub fn flag(&self, name: &str) -> bool {
        matches!(self.get(name), "on" | "true" | "1")
    }
}

/// What a handler returns: the typed subject for the log, and the result.
pub type Ran = (String, Result<Done, OpError>);
/// An operation as the table holds it.
pub type Handler = fn(&Ctx, &Form) -> Ran;

/// The handler for the route `<area>/<op>` and its op id.
pub fn lookup(area: &str, op: &str) -> Option<(String, Handler)> {
    let id = format!("{area}.{}", op.replace('-', "_"));
    let handler: Handler = match id.as_str() {
        #[cfg(any(test, feature = "testkit"))]
        "debug.noop" => testkit::noop,
        _ => return None,
    };
    Some((id, handler))
}

#[cfg(any(test, feature = "testkit"))]
mod testkit {
    use nullrouter_registry::SecretString;
    use nullrouter_server::ops::Once;

    use super::{Done, Form, OpError, Ran, Status};
    use nullrouter_server::ops::Ctx;

    /// Changes nothing. `fail` refuses, `saved` is saved-not-applied, `once` shows a value once.
    pub fn noop(_ctx: &Ctx, form: &Form) -> Ran {
        let subject = form.opt("subject").unwrap_or("noop").to_owned();
        if let Some(text) = form.opt("fail") {
            return (subject, Err(OpError { text: text.to_owned(), exit: 1, field: Some("subject") }));
        }
        let status = match form.opt("saved") {
            Some(reason) => Status::SavedNotApplied { reason: reason.to_owned() },
            None => Status::Applied,
        };
        let once = form.opt("once").map(|v| Once { label: "Value".into(), value: SecretString::new(v) });
        (subject, Ok(Done { status, text: "Nothing changed.".into(), once }))
    }
}

fn plain(status: StatusCode, text: &'static str) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], text).into_response()
}

/// `back` with `params` joined on, whether or not it already has a query.
fn join(back: &str, params: &[(&str, &str)]) -> String {
    let mut out = back.to_owned();
    for (k, v) in params {
        out.push(if out.contains('?') { '&' } else { '?' });
        out.push_str(k);
        out.push('=');
        out.push_str(&access::encode_component(v));
    }
    out
}

fn now() -> String {
    Zoned::now().strftime("%Y-%m-%dT%H:%M:%S%:z").to_string()
}

pub async fn dispatch(State(shared): State<Arc<Shared>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Some(route) = parts.uri.path().strip_prefix("/do/") else {
        return plain(StatusCode::NOT_FOUND, "No such change.\n");
    };
    let Ok(body) = to_bytes(body, MAX_BODY).await else {
        return plain(StatusCode::PAYLOAD_TOO_LARGE, "That form is too large.\n");
    };
    let fields: HashMap<String, String> = access::pairs(&String::from_utf8_lossy(&body)).into_iter().rev().collect();
    let session = match shared.forms.check_post(&shared.engine, &shared.rule, &parts.headers, &fields) {
        Ok(s) => s,
        Err(Refusal::SignIn) => return see_other("/signin"),
        Err(Refusal::Foreign) => {
            return (
                StatusCode::FORBIDDEN,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                format!("{}\n", Refusal::FOREIGN_TEXT),
            )
                .into_response();
        }
    };
    let Some((handler_id, handler)) =
        route.split_once('/').and_then(|(a, o)| (!o.contains('/')).then(|| lookup(a, o)).flatten())
    else {
        return plain(StatusCode::NOT_FOUND, "No such change.\n");
    };

    let back = crate::forms::back(fields.get("_back").map_or("", String::as_str));
    let window = fields.get("_window").filter(|w| !w.is_empty()).cloned();
    let mut kept: Vec<(String, String)> =
        fields.iter().filter(|(k, _)| !k.starts_with('_')).map(|(k, v)| (k.clone(), v.clone())).collect();
    kept.sort();
    let form = Form(fields);

    let engine = shared.engine.clone();
    let ran = shared
        .guard
        .run(move || async move {
            tokio::task::spawn_blocking(move || {
                let ctx = Ctx { home: engine.home().clone(), engine: Some(engine) };
                handler(&ctx, &form)
            })
            .await
        })
        .await;
    let refused = |text: String| (String::new(), Err(OpError { text, exit: 1, field: None }));
    let (subject, result) = match ran {
        Ok(Ok(done)) => done,
        Ok(Err(_)) => refused("The change stopped unexpectedly.".into()),
        Err(e) => refused(e.text()),
    };
    answer(
        &shared,
        &Answer { session: &session, op: &handler_id, back: &back, window: window.as_deref(), kept },
        subject,
        result,
    )
}

/// What a post's answer needs besides the result.
struct Answer<'a> {
    session: &'a str,
    op: &'a str,
    back: &'a str,
    window: Option<&'a str>,
    kept: Vec<(String, String)>,
}

fn answer(shared: &Shared, a: &Answer<'_>, subject: String, result: Result<Done, OpError>) -> Response {
    let Answer { session, op, back, window, ref kept } = *a;
    let (outcome, text) = match &result {
        Ok(d) => (
            match d.status {
                Status::Applied => Outcome::Done,
                Status::SavedNotApplied { .. } => Outcome::SavedNotApplied,
                Status::Started { .. } => Outcome::Started,
            },
            d.text.clone(),
        ),
        Err(e) => (Outcome::Refused, e.text.clone()),
    };
    let entry = log::Entry { at: now(), op: op.to_owned(), subject, result: outcome, text };
    if let Err(e) = log::append(shared.engine.home(), &entry) {
        tracing::warn!("the dashboard change log could not be written: {e}");
    }
    let to = match result {
        Ok(Done { status: Status::Started { task }, .. }) => join(back, &[("task", task.as_str())]),
        Ok(done) => {
            let kind = match (done.once, done.status) {
                (Some(once), _) => FlashKind::Once { label: once.label, value: once.value },
                (None, Status::SavedNotApplied { reason }) => {
                    FlashKind::Done { text: format!("{}: saved but not applied: {reason}", done.text) }
                }
                (None, _) => FlashKind::Done { text: done.text },
            };
            join(back, &[("done", &shared.forms.put(session, kind))])
        }
        Err(e) => {
            let id = shared.forms.put(
                session,
                FlashKind::Refused { text: e.text, field: e.field.map(str::to_owned), kept: kept.clone() },
            );
            match window {
                Some(w) => join(back, &[("window", w), ("done", &id)]),
                None => join(back, &[("done", &id)]),
            }
        }
    };
    see_other(&to)
}
