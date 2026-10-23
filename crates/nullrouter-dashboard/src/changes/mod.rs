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

/// The fields of one post, decoded, and the dashboard's own origin (`http://host:port`) the post
/// came to, when its `Host` is one this dashboard answers to.
pub struct Form {
    fields: HashMap<String, String>,
    origin: Option<String>,
}

impl Form {
    /// The field `name`, or `""` when it is absent.
    pub fn get(&self, name: &str) -> &str {
        self.fields.get(name).map_or("", String::as_str)
    }

    /// The field `name` when present and not empty.
    pub fn opt(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str).filter(|v| !v.is_empty())
    }

    /// A checkbox or switch: submitted as `on` when set.
    pub fn flag(&self, name: &str) -> bool {
        matches!(self.get(name), "on" | "true" | "1")
    }

    /// The dashboard origin the post came to, for a redirect that must return to it.
    pub fn origin(&self) -> Option<&str> {
        self.origin.as_deref()
    }

    /// A form from `(field, value)` pairs, for the handler tests.
    #[cfg(test)]
    pub fn from_pairs(pairs: &[(&str, &str)]) -> Self {
        let fields = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        Self { fields, origin: None }
    }
}

#[cfg(test)]
mod confirm_tests {
    use super::*;

    #[test]
    fn every_irreversible_op_has_a_window() {
        for op in nullrouter_server::ops::IRREVERSIBLE {
            assert!(CONFIRM_WINDOWS.iter().any(|(o, ..)| o == op), "{op}");
        }
        assert_eq!(CONFIRM_WINDOWS.len(), nullrouter_server::ops::IRREVERSIBLE.len());
    }

    #[test]
    fn the_window_url_carries_the_subject() {
        let f = Form::from_pairs(&[("provider", "anthropic"), ("name", "a b")]);
        assert_eq!(
            unconfirmed("accounts.remove", &f).as_deref(),
            Some("/providers?window=remove-account&id=anthropic&name=a%20b")
        );
        let f = Form::from_pairs(&[("confirmed", "on")]);
        assert_eq!(unconfirmed("accounts.remove", &f), None);
        assert_eq!(unconfirmed("plugins.private_endpoints", &Form::from_pairs(&[])), None);
        assert_eq!(
            unconfirmed("plugins.private_endpoints", &Form::from_pairs(&[("on", "on")])).as_deref(),
            Some("/settings?window=private-endpoints")
        );
    }
}

mod accounts;
mod adapters;
mod connection;
mod keys;
mod model_tests;
mod plugins;
mod proxy;
mod quota;
mod routing;
mod targets;

/// What a handler returns: the typed subject for the log, and the result.
pub type Ran = (String, Result<Done, OpError>);
/// An operation as the table holds it.
pub type Handler = fn(&Ctx, &Form) -> Ran;

/// The handler for the route `<area>/<op>` and its op id.
pub fn lookup(area: &str, op: &str) -> Option<(String, Handler)> {
    let id = format!("{area}.{}", op.replace('-', "_"));
    let handler: Handler = match id.as_str() {
        "accounts.add" => accounts::add,
        "accounts.priority" => accounts::priority,
        "accounts.enable" => accounts::enable,
        "accounts.disable" => accounts::disable,
        "accounts.exclusive" => accounts::exclusive,
        "accounts.remove" => accounts::remove,
        "accounts.signin" => accounts::signin,
        "accounts.signin_paste" => accounts::signin_paste,
        "keys.issue" => keys::issue,
        "keys.revoke" => keys::revoke,
        "keys.tag" => keys::tag,
        "keys.set_break" => keys::set_break,
        "keys.set_adapter" => keys::set_adapter,
        "behaviour.set_break" => routing::set_break,
        "routing.set" => routing::set,
        "routing.unset" => routing::unset,
        "routing.set_plugin" => routing::set_plugin,
        "routing.unset_plugin" => routing::unset_plugin,
        "routing.window" => routing::window,
        "connection.set" => connection::set,
        "connection.unset" => connection::unset,
        "proxy.add" => proxy::add,
        "proxy.remove" => proxy::remove,
        "proxy.use" => proxy::use_proxy,
        "proxy.clear" => proxy::clear,
        "proxy.fixed" => proxy::fixed,
        "quota.interval" => quota::interval,
        "quota.poll" => quota::poll,
        "quota.ack" => quota::ack,
        "unified.add" => targets::unified_add,
        "unified.set" => targets::unified_set,
        "unified.remove" => targets::unified_remove,
        "combos.add" => targets::combos_add,
        "combos.set" => targets::combos_set,
        "combos.remove" => targets::combos_remove,
        "test.run" => model_tests::run,
        "verdicts.mark" => model_tests::mark,
        "verdicts.clear" => model_tests::clear,
        "verdicts.settings" => model_tests::settings,
        "plugins.install" => plugins::install,
        "plugins.uninstall" => plugins::uninstall,
        "plugins.private_endpoints" => plugins::private_endpoints,
        "providers.add_compatible" => plugins::add_compatible,
        "providers.enable" => plugins::enable,
        "providers.disable" => plugins::disable,
        "providers.hide" => plugins::hide,
        "providers.show" => plugins::show,
        "adapters.install" => adapters::install,
        "adapters.review" => adapters::review,
        "adapters.build" => adapters::build,
        "adapters.approve" => adapters::approve,
        "adapters.reject" => adapters::reject,
        "adapters.clear" => adapters::clear,
        "adapters.rebuild" => adapters::rebuild,
        "adapters.remove" => adapters::remove,
        "adapters.review_settings" => adapters::review_settings,
        "catalogue.install" => adapters::catalogue_install,
        "alerts.ack" => adapters::ack,
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

/// `POST /signout`: the same Origin and `_t` checks as a change, then the session ends, the cookie
/// is cleared and the browser goes to sign in.
pub async fn signout(State(shared): State<Arc<Shared>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(body) = to_bytes(body, 8 * 1024).await else {
        return plain(StatusCode::PAYLOAD_TOO_LARGE, "That form is too large.\n");
    };
    let fields: HashMap<String, String> = access::pairs(&String::from_utf8_lossy(&body)).into_iter().rev().collect();
    let session = match shared.forms.check_post(&shared.engine, &shared.sessions, &shared.rule, &parts.headers, &fields)
    {
        Ok(s) => s,
        Err(Refusal::SignIn) => return see_other("/signin"),
        Err(Refusal::Foreign) => return plain(StatusCode::FORBIDDEN, "This change didn't come from the dashboard.\n"),
    };
    shared.sessions.end(&session);
    let mut response = see_other("/signin");
    if let Ok(cookie) = header::HeaderValue::try_from(access::clear_cookie(shared.rule.port())) {
        response.headers_mut().insert(header::SET_COOKIE, cookie);
    }
    response
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
    let session = match shared.forms.check_post(&shared.engine, &shared.sessions, &shared.rule, &parts.headers, &fields)
    {
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
    // The origin a sign-in returns to: the Host the browser used, when this dashboard answers to it.
    let origin = parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .filter(|h| shared.rule.allows(h))
        .map(|h| format!("http://{h}"));
    let form = Form { fields, origin };

    // An irreversible op does nothing until the post says it was confirmed (FR-009): the home is
    // untouched and the browser goes to the op's confirm window, which posts `confirmed=on`.
    if let Some(to) = unconfirmed(&handler_id, &form) {
        let entry = log::Entry {
            at: now(),
            op: handler_id.clone(),
            subject: String::new(),
            result: Outcome::Refused,
            text: "not confirmed".into(),
        };
        if let Err(e) = log::append(shared.engine.home(), &entry) {
            tracing::warn!("the dashboard change log could not be written: {e}");
        }
        return see_other(&to);
    }

    // A pasted code goes only to a task this session started (security-review.md I3). The refusal
    // is the one a task that isn't waiting gives, so a foreign id learns nothing.
    if handler_id == "accounts.signin_paste" {
        let owned = form
            .get("task")
            .parse::<nullrouter_engine::tasks::TaskId>()
            .ok()
            .and_then(|id| shared.engine.tasks.get(&id))
            .is_some_and(|t| t.owner.as_ref().is_some_and(|o| *o != access::session_digest(&session)));
        if owned {
            let text = format!("{}: not waiting for a code", form.get("task"));
            let err = OpError { text, exit: 1, field: None };
            let a = Answer { session: &session, op: &handler_id, back: &back, window: window.as_deref(), kept };
            return answer(&shared, &a, format!("task {}", form.get("task")), Err(err));
        }
    }

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

/// Where the button of each irreversible op (`ops::IRREVERSIBLE`) opens its confirm window: the
/// page, the window, and the window's parameters as `(query name, form field)`.
type Window = (&'static str, &'static str, &'static str, &'static [(&'static str, &'static str)]);
const CONFIRM_WINDOWS: [Window; 9] = [
    ("accounts.remove", "/providers", "remove-account", &[("id", "provider"), ("name", "name")]),
    ("keys.revoke", "/endpoint", "revoke", &[("key", "key")]),
    ("plugins.uninstall", "/providers", "uninstall-plugin", &[("id", "id")]),
    ("proxy.remove", "/proxy-pools", "remove", &[("name", "name")]),
    ("adapters.remove", "/endpoint", "remove-adapter", &[("harness", "harness"), ("version", "version")]),
    ("adapters.clear", "/endpoint", "clear-adapter", &[("harness", "harness"), ("version", "version")]),
    ("unified.remove", "/combo", "remove-unified", &[("name", "name")]),
    ("combos.remove", "/combo", "remove-combo", &[("name", "name")]),
    ("plugins.private_endpoints", "/settings", "private-endpoints", &[]),
];

/// The confirm window to send the browser to when `op` is irreversible and `form` is not
/// confirmed. Turning private endpoints off needs no confirmation; only turning them on does.
fn unconfirmed(op: &str, form: &Form) -> Option<String> {
    if form.flag("confirmed") || (op == "plugins.private_endpoints" && !form.flag("on")) {
        return None;
    }
    let (_, page, window, params) = CONFIRM_WINDOWS.iter().find(|(o, ..)| *o == op)?;
    let mut to = format!("{page}?window={window}");
    for (name, field) in *params {
        to.push('&');
        to.push_str(name);
        to.push('=');
        to.push_str(&access::encode_component(form.get(field)));
    }
    Some(to)
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
        Ok(Done { status: Status::Started { task }, .. }) => {
            shared.engine.tasks.set_owner(&task, access::session_digest(session));
            join(back, &[("task", task.as_str())])
        }
        Ok(done) => {
            let kind = match (done.once, done.status) {
                (Some(once), _) => FlashKind::Once { label: once.label, value: once.value },
                (None, Status::SavedNotApplied { reason }) => {
                    // The ops put the reason in `text` too when the reload was refused: say it once.
                    let text = if done.text == reason {
                        format!("saved but not applied: {reason}")
                    } else {
                        format!("{}: saved but not applied: {reason}", done.text)
                    };
                    FlashKind::Done { text }
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
