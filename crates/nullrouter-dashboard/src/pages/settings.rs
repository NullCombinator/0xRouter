//! Settings (contracts/dashboard-http.md "Settings"): what `behaviour show`, `check` and
//! `dashboard status` say, read-only. Changing any of it is a CLI command or an edit of a file in
//! the home; the page only names which. The dashboard token is never here: its view carries only
//! the time it was issued.

use maud::{Markup, html};
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Req};
use crate::components::{Head, card, inset, kv, prose};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Behaviour, ViewName::Check, ViewName::Dashboard];

pub const CHANGE_TOKEN: &str = "Change it with `nullrouter dashboard token`.";

pub fn wants(_req: &Req) -> Vec<Want> {
    vec![
        Want::new(ViewName::Behaviour, json!({})),
        Want::new(ViewName::Check, json!({})),
        Want::new(ViewName::Dashboard, json!({})),
    ]
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let content = html! {
        (routing(ctx.json(ViewName::Behaviour)))
        (local_mode(ctx.json(ViewName::Check)))
        (dashboard(ctx, ctx.json(ViewName::Dashboard)))
    };
    Ok(Body::new(content))
}

/// "Routing": each `[pipeline]` setting as `behaviour show` prints it, `(default)` where the value
/// is the built-in one.
fn routing(behaviour: &Value) -> Markup {
    let head = Head::new("route", "Routing").subtitle("Same as nullrouter behaviour show");
    card(
        Some(head),
        html! {
            div class="settings-list" {
                @for (setting, v) in behaviour.as_object().into_iter().flatten() {
                    @let tag = if v["default"] == true { " (default)" } else { "" };
                    (kv(setting, html! { (v["value"].as_str().unwrap_or_default()) (tag) }))
                }
            }
        },
    )
}

/// "Local mode": the operator home, where every file lives.
fn local_mode(check: &Value) -> Markup {
    let head = Head::new("folder", "Local mode").subtitle("Where your data lives");
    let home = check["home"].as_str().unwrap_or_default();
    card(Some(head), inset(html! { code class="settings-list__path" { (home) } }))
}

/// What `dashboard status` says first, in its words without the `dashboard:` label.
pub fn state(v: &Value) -> String {
    let (listen, enabled) = (v["listen"].as_str().unwrap_or_default(), v["enabled"] == true);
    match (v["server"].as_str(), enabled) {
        (Some("running"), false) => "off (config.toml [dashboard] enabled = false)".to_owned(),
        (Some("running"), true) if v["serving"] == true => format!("on, listening on {listen}"),
        (Some("running"), true) => format!("on, not listening: {}", v["error"].as_str().unwrap_or(listen)),
        (_, true) => format!("no server running; config.toml: on, {listen}"),
        (_, false) => "no server running; config.toml: off".to_owned(),
    }
}

/// "Dashboard": on or off, the address, serving or why not, and when the token was issued.
fn dashboard(ctx: &Ctx<'_>, v: &Value) -> Markup {
    let head = Head::new("monitor", "Dashboard").subtitle("Same as nullrouter dashboard status");
    card(
        Some(head),
        html! {
            div class="settings-list" {
                (kv("Status", html! { (state(v)) }))
                (kv("Address", html! { code class="code" { (v["listen"].as_str().unwrap_or_default()) } }))
                (kv("Token issued", match v["token_issued"].as_str() {
                    Some(at) => ctx.time(at),
                    None => html! { "none" },
                }))
                p class="settings-list__note" { (prose(CHANGE_TOKEN)) }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(server: &str, enabled: bool, serving: Value, error: Value) -> Value {
        json!({"enabled": enabled, "listen": "127.0.0.1:20130", "server": server, "serving": serving, "error": error})
    }

    #[test]
    fn each_state_is_worded_as_dashboard_status_words_it() {
        for (value, want) in [
            (v("running", true, json!(true), Value::Null), "on, listening on 127.0.0.1:20130"),
            (
                v("running", true, json!(false), json!("127.0.0.1:20130: in use")),
                "on, not listening: 127.0.0.1:20130: in use",
            ),
            (v("running", false, json!(false), Value::Null), "off (config.toml [dashboard] enabled = false)"),
            (v("none", true, Value::Null, Value::Null), "no server running; config.toml: on, 127.0.0.1:20130"),
            (v("none", false, Value::Null, Value::Null), "no server running; config.toml: off"),
        ] {
            assert_eq!(state(&value), want);
        }
    }
}
