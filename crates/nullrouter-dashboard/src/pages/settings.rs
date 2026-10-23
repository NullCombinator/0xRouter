//! Settings (contracts/dashboard-http.md "Settings"): what `behaviour show`, `check` and
//! `dashboard status` say, and the two changes the dashboard makes on the same ops the CLI runs: the
//! default break behaviour and the amortization window. The dashboard token is never here: its view
//! carries only the time it was issued.

use maud::{Markup, html};
use nullrouter_server::ops;
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Refused, Req};
use crate::components::{
    Head, Tone, badge, card, confirm_window, field, form, icon, inset, kv, link_button, prose, select,
};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Behaviour, ViewName::Check, ViewName::Dashboard, ViewName::Routing];

pub const CHANGE_TOKEN: &str = "Change it with `nullrouter dashboard token`.";

/// Where every form here returns: this page.
const BACK: &str = "/settings";
/// The break behaviours the default takes (`behaviour set-break`).
const BREAKS: &[(&str, &str)] = &[("restart", "restart"), ("error_event", "error_event")];

pub fn wants(_req: &Req) -> Vec<Want> {
    vec![
        Want::new(ViewName::Behaviour, json!({})),
        Want::new(ViewName::Check, json!({})),
        Want::new(ViewName::Dashboard, json!({})),
        Want::new(ViewName::Routing, json!({"target": null})),
    ]
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let content = html! {
        (routing(ctx.json(ViewName::Behaviour)))
        (changes(ctx))
        (private_endpoints(ctx))
        (local_mode(ctx.json(ViewName::Check)))
        (dashboard(ctx, ctx.json(ViewName::Dashboard)))
    };
    let window = match ctx.req.get("window") {
        Some("private-endpoints") => Some(private_window(ctx)),
        _ => None,
    };
    Ok(Body { content, side: None, window })
}

/// "Private endpoints": whether plugins may reach this machine and the local network, as `check`
/// says it. Turning it on opens a confirm window (`?window=private-endpoints`); turning it off
/// posts at once.
fn private_endpoints(ctx: &Ctx<'_>) -> Markup {
    let on = ctx.json(ViewName::Check)["allow_private_endpoints"] == true;
    let head = Head::new("settings", "Private endpoints").subtitle("Same as nullrouter plugins private-endpoints");
    let turn_on = ctx.req.href(BACK, &["done", "task", "window"], &[("window", "private-endpoints")]);
    card(
        Some(head),
        html! {
            (kv("Private endpoints", badge(if on { Tone::Warning } else { Tone::Default }, if on { "on" } else { "off" })))
            @if on {
                (form("/do/plugins/private-endpoints", BACK, &ctx.req.token, html! {
                    button class="button button--secondary" type="submit" { "Turn off private endpoints" }
                }))
            } @else {
                (link_button(&turn_on, "Turn on private endpoints", None, false))
            }
        },
    )
}

/// The confirm window of turning private endpoints on: what it allows, the real form beside Cancel.
fn private_window(ctx: &Ctx<'_>) -> Markup {
    let loss = ops::describe_loss("plugins.private_endpoints", "on").unwrap_or_default();
    let action = form(
        "/do/plugins/private-endpoints",
        BACK,
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value="private-endpoints";
            input type="hidden" name="confirmed" value="on";
            input type="hidden" name="on" value="on";
            button class="button button--primary" type="submit" { "Turn on private endpoints" }
        },
    );
    confirm_window("Turn on private endpoints", &loss, BACK, action)
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

/// The refusal of the form `id`: a refused change's redirect names its form by `window`, so only
/// that form shows it.
fn refused_form<'a>(req: &'a Req, id: &str) -> Option<&'a Refused> {
    req.refused.as_ref().filter(|_| req.get("window") == Some(id))
}

/// The text of `r` when it belongs to the field `name`: the field it names, or the form's first
/// field when it names none of `fields`.
fn error_on<'a>(r: Option<&'a Refused>, name: &str, fields: &[&str]) -> Option<&'a str> {
    let r = r?;
    let owner = r.field.as_deref().filter(|f| fields.contains(f)).or(fields.first().copied());
    (owner == Some(name)).then_some(r.text.as_str())
}

/// The value a refused form kept for `name`.
fn kept<'a>(r: Option<&'a Refused>, name: &str) -> Option<&'a str> {
    r?.kept.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// "Change": the default break behaviour and the amortization window, each a form that returns here.
fn changes(ctx: &Ctx<'_>) -> Markup {
    let req = ctx.req;
    let head = Head::new("settings", "Change").subtitle("Same as nullrouter behaviour set-break and routing window");
    let (break_err, window_err) = (refused_form(req, "break"), refused_form(req, "amortization"));
    let now_break = ctx.json(ViewName::Behaviour)["break_behaviour"]["value"].as_str().unwrap_or("restart");
    let now_window = ctx.json(ViewName::Routing)["amortization"]["length"].as_str().unwrap_or_default();
    let verdict_err = refused_form(req, "verdicts-settings");
    let verdict_fields: &[&str] = &["setting", "type", "value"];
    card(
        Some(head),
        html! {
            (form("/do/behaviour/set-break", BACK, &req.token, html! {
                input type="hidden" name="_window" value="break";
                (select("behaviour", "Default break behaviour", BREAKS, Some(kept(break_err, "behaviour").unwrap_or(now_break))))
                @if let Some(e) = error_on(break_err, "behaviour", &["behaviour"]) {
                    span class="field__error" { (icon("error")) (e) }
                }
                p class="hint" { "What a stream that breaks after output does, for every key without its own choice." }
                button class="button button--secondary" type="submit" { "Save break behaviour" }
            }))
            (form("/do/routing/window", BACK, &req.token, html! {
                input type="hidden" name="_window" value="amortization";
                (field(
                    "duration",
                    "Amortization window",
                    Some(kept(window_err, "duration").unwrap_or(now_window)),
                    error_on(window_err, "duration", &["duration"]),
                ))
                p class="hint" { (prose("A duration such as `5h`, or `default` for the built-in window.")) }
                button class="button button--secondary" type="submit" { "Save amortization window" }
            }))
            (form("/do/verdicts/settings", BACK, &req.token, html! {
                input type="hidden" name="_window" value="verdicts-settings";
                (field("setting", "Test setting", kept(verdict_err, "setting"), error_on(verdict_err, "setting", verdict_fields)))
                (field("type", "Model type (optional)", kept(verdict_err, "type"), error_on(verdict_err, "type", verdict_fields)))
                (field("value", "Value", kept(verdict_err, "value"), error_on(verdict_err, "value", verdict_fields)))
                p class="hint" { "The settings and their values are listed by " code { "nullrouter verdicts settings" } "." }
                button class="button button--secondary" type="submit" { "Save test settings" }
            }))
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
