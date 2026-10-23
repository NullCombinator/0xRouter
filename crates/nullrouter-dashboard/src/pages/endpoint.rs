//! Endpoint & Key (contracts/dashboard-http.md "Endpoint & Key"): where agents connect, and one card
//! per agent key as `keys list` shows it. Every fact comes from `check` (the endpoint) and `keys`;
//! the page adds none. Spec 014 adds the forms: Add Agent opens the issue window, each card has its
//! tag, break and adapter forms, and Revoke opens the key's confirm window. There is no copy button
//! (it needs a script), and a key is never shown beyond the last four characters the CLI prints; a
//! new key is shown once by the flash at the top of the page, not here.

use maud::{Markup, html};
use nullrouter_server::ops::{self, adapters as adapter_ops};
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Req};
use crate::components::{
    Head, Tone, badge, card, confirm_window, empty, field, form, kv, link_button, modal, name, section_bar, select,
    side_panel, side_section, status, switch,
};
use crate::page::{ViewName, Want};
use crate::{access, landscape};

pub const VIEWS: &[ViewName] = &[
    ViewName::Check,
    ViewName::Keys,
    ViewName::Accounts,
    ViewName::Latency,
    ViewName::Usage,
    ViewName::Adapters,
    ViewName::Alerts,
];

pub const NO_AGENTS: &str = "No agent keys.";
pub const NO_AGENTS_HINT: &str = "Run `nullrouter keys issue <name>`.";
/// What the endpoint card adds when no server answered (`check` says the same).
pub const CONFIGURED: &str = "(configured; no server running)";

/// Where every form here returns: this page, with its window closed.
const BACK: &str = "/endpoint";
/// The break choices of the issue form: the empty value is the default, which the op reads as none.
const ISSUE_BREAKS: &[(&str, &str)] = &[("", "default"), ("restart", "restart"), ("error_event", "error_event")];
/// The break choices of a key card: `default` clears the key's own choice.
const SET_BREAKS: &[(&str, &str)] = &[("default", "default"), ("restart", "restart"), ("error_event", "error_event")];

pub fn wants(_req: &Req) -> Vec<Want> {
    vec![
        Want::new(ViewName::Check, json!({})),
        Want::new(ViewName::Keys, json!({})),
        Want::new(ViewName::Accounts, json!({})),
        Want::new(ViewName::Latency, json!({})),
        Want::new(ViewName::Usage, json!({"period": "today"})),
        Want::new(ViewName::Adapters, json!({})),
        Want::new(ViewName::Alerts, json!({})),
    ]
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let keys = ctx.json(ViewName::Keys).as_array().map_or(&[][..], Vec::as_slice);
    let content = html! {
        (endpoint(ctx.json(ViewName::Check)))
        (section_bar(&landscape::heading(ctx.page.as_of, ctx.tz), html! {}))
        (landscape::landscape(&landscape::Inputs {
            keys: ctx.json(ViewName::Keys),
            accounts: ctx.json(ViewName::Accounts),
            latency: ctx.json(ViewName::Latency),
            tz: ctx.tz,
            as_of: ctx.page.as_of,
        }))
        (section_bar("Agents", link_button(&ctx.req.href(BACK, &["done", "task", "key"], &[("window", "add")]), "Add Agent", Some("add"), true)))
        @if keys.is_empty() {
            (empty("key", NO_AGENTS, NO_AGENTS_HINT))
        } @else {
            div class="key-grid" {
                @for k in keys { (key_card(ctx, k)) }
            }
        }
    };
    Ok(Body { content, side: Some(adapters_panel(ctx)), window: window(ctx) })
}

/// The window open on top: Add Agent for `?window=add`, a key's confirm window for
/// `?window=revoke&key=<id>`, an adapter version's for `?window=clear-adapter` or `remove-adapter`
/// with `&harness=<h>&version=<v>`. Any other window, or one without what it names, opens nothing.
fn window(ctx: &Ctx<'_>) -> Option<Markup> {
    match ctx.req.get("window")? {
        "add" => Some(issue_window(ctx)),
        "revoke" => Some(revoke_window(ctx, ctx.req.get("key")?)),
        "clear-adapter" => Some(clear_window(ctx, ctx.req.get("harness")?, ctx.req.get("version")?)),
        "remove-adapter" => Some(remove_window(ctx, ctx.req.get("harness")?, ctx.req.get("version")?)),
        _ => None,
    }
}

/// The issue form in its window: name, harness tag, break behaviour and adapter. The key it makes
/// is shown once by the flash the redirect returns with.
fn issue_window(ctx: &Ctx<'_>) -> Markup {
    let body = form(
        "/do/keys/issue",
        BACK,
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value="add";
            (field("name", "Name", None, None))
            (field("harness", "Harness tag", None, None))
            (select("break", "Break behaviour", ISSUE_BREAKS, Some("")))
            (field("adapter", "Adapter", None, None))
            button class="button button--primary" type="submit" { "Issue key" }
        },
    );
    modal("Add Agent", BACK, body)
}

/// The confirm window of one key: what revoking loses, the real form beside Cancel.
fn revoke_window(ctx: &Ctx<'_>, key: &str) -> Markup {
    let loss = ops::describe_loss("keys.revoke", key).unwrap_or_default();
    // A refused revoke returns to this window: `_back` names the key, and `_window` reopens it.
    let back = format!("{BACK}?key={}", access::encode_component(key));
    let action = form(
        "/do/keys/revoke",
        &back,
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value="revoke";
            input type="hidden" name="confirmed" value="on";
            input type="hidden" name="key" value=(key);
            button class="button button--primary" type="submit" { "Revoke key" }
        },
    );
    confirm_window("Revoke key", &loss, BACK, action)
}

/// The confirm window of clearing a suspect adapter version: what clearing does, the real form
/// beside Cancel. A refused clear reopens it: `_back` names the version.
fn clear_window(ctx: &Ctx<'_>, harness: &str, version: &str) -> Markup {
    let loss = adapter_ops::clear_question(harness, version);
    let action = form(
        "/do/adapters/clear",
        &adapter_back(harness, version),
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value="clear-adapter";
            input type="hidden" name="confirmed" value="on";
            input type="hidden" name="harness" value=(harness);
            input type="hidden" name="version" value=(version);
            button class="button button--primary" type="submit" { "Clear version" }
        },
    );
    confirm_window("Clear version", &loss, BACK, action)
}

/// The confirm window of removing an adapter version. The active version needs the extra switch,
/// which the adapters view says (`active`).
fn remove_window(ctx: &Ctx<'_>, harness: &str, version: &str) -> Markup {
    let loss = adapter_ops::remove_question(harness, Some(version));
    let active = ctx
        .json(ViewName::Adapters)
        .as_array()
        .into_iter()
        .flatten()
        .any(|h| h["harness"] == harness && h["active"] == version);
    let action = form(
        "/do/adapters/remove",
        &adapter_back(harness, version),
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value="remove-adapter";
            input type="hidden" name="confirmed" value="on";
            input type="hidden" name="harness" value=(harness);
            input type="hidden" name="version" value=(version);
            @if active { (switch("force", "Remove even though it is the active version", false)) }
            button class="button button--primary" type="submit" { "Remove version" }
        },
    );
    confirm_window("Remove version", &loss, BACK, action)
}

/// Where a refused clear or remove returns: the page, with the version named for the window.
fn adapter_back(harness: &str, version: &str) -> String {
    format!("{BACK}?harness={}&version={}", access::encode_component(harness), access::encode_component(version))
}

/// "Client adapters": each harness with its versions and the steps their states allow, the open
/// alerts, and the forms that install an adapter and set the review model. Every fact is
/// `adapters list` or `alerts list`; every form runs the op the CLI command runs.
fn adapters_panel(ctx: &Ctx<'_>) -> Markup {
    let harnesses = ctx.json(ViewName::Adapters).as_array().map_or(&[][..], Vec::as_slice);
    let alerts = ctx.json(ViewName::Alerts).as_array().map_or(&[][..], Vec::as_slice);
    let token = &ctx.req.token;
    let sections = html! {
        (side_section("Adapters", html! {
            @for h in harnesses { (harness_block(ctx, h)) }
        }))
        (side_section("Alerts", html! {
            @if alerts.is_empty() {
                p class="hint" { "No open alerts." }
            } @else {
                @for a in alerts { (alert_row(ctx, a)) }
                (form("/do/alerts/ack", BACK, token, html! {
                    input type="hidden" name="id" value="all";
                    button class="button button--secondary" type="submit" { "Acknowledge all" }
                }))
            }
        }))
        (side_section("Install adapter", html! {
            (form("/do/adapters/install", BACK, token, html! {
                (field("path", "Package path (a directory or a .tar.gz)", None, None))
                button class="button button--secondary" type="submit" { "Install from path" }
            }))
            (form("/do/catalogue/install", BACK, token, html! {
                (field("harness", "Harness", None, None))
                (field("version", "Version (optional)", None, None))
                button class="button button--secondary" type="submit" { "Install from catalogue" }
            }))
        }))
        (side_section("Review settings", html! {
            (form("/do/adapters/review-settings", BACK, token, html! {
                (field("model", "Review model", None, None))
                (field("budget", "Budget (input tokens)", None, None))
                (field("reserve_output", "Reserve for the answer (optional)", None, None))
                (switch("clear", "Clear the review model", false))
                button class="button button--secondary" type="submit" { "Save review settings" }
            }))
        }))
    };
    side_panel("extension", "Client adapters", sections)
}

/// One harness: hermes as built-in, a third-party one with its active version, its open alerts and
/// its versions.
fn harness_block(ctx: &Ctx<'_>, h: &Value) -> Markup {
    let harness = h["harness"].as_str().unwrap_or_default();
    if h["built_in"] == true {
        return html! { div class="adapter" { b class="adapter__name" { (name(harness)) } (badge(Tone::Default, "built-in")) } };
    }
    let versions = h["versions"].as_array().map_or(&[][..], Vec::as_slice);
    html! {
        div class="adapter" {
            b class="adapter__name" { (name(harness)) }
            (kv("active", html! { (h["active"].as_str().unwrap_or("none")) }))
            (kv("open alerts", html! { (h["alerts"].as_u64().unwrap_or(0)) }))
            @for v in versions { (version_row(ctx, harness, v)) }
        }
    }
}

/// One version: id, semver, state and flags, then the steps its state allows, each a POST that
/// returns here. Clear and Remove open their confirm windows and change nothing by themselves.
fn version_row(ctx: &Ctx<'_>, harness: &str, v: &Value) -> Markup {
    let id = v["id"].as_str().unwrap_or_default();
    let state = v["state"].as_str().unwrap_or_default();
    let token = &ctx.req.token;
    let step = |action: &str, label: &str, note: bool| {
        form(
            action,
            BACK,
            token,
            html! {
                input type="hidden" name="harness" value=(harness);
                input type="hidden" name="version" value=(id);
                @if note { (field("note", "Note (optional)", None, None)) }
                button class="button button--secondary" type="submit" { (label) }
            },
        )
    };
    let window =
        |w: &str| ctx.req.href(BACK, &["done", "task", "key"], &[("window", w), ("harness", harness), ("version", id)]);
    html! {
        div class="adapter-version" {
            code class="code" { (id) }
            span { (v["semver"].as_str().unwrap_or_default()) }
            (status(state))
            @if v["rebuilding"] == true { (badge(Tone::Info, "rebuilding")) }
            @if v["rebuild_failed"] == true { (badge(Tone::Error, "rebuild_failed")) }
            @match state {
                "queued" => { (step("/do/adapters/build", "Build", false)) }
                "quarantined" => { (step("/do/adapters/review", "Review", false)) }
                "reported" => {
                    (step("/do/adapters/approve", "Approve", true))
                    (step("/do/adapters/reject", "Reject", true))
                }
                "suspect" => { (link_button(&window("clear-adapter"), "Clear", None, false)) }
                _ => {}
            }
            (link_button(&window("remove-adapter"), "Remove", None, false))
        }
    }
}

/// One alert as `alerts list` shows it, with its Acknowledge form.
fn alert_row(ctx: &Ctx<'_>, a: &Value) -> Markup {
    let text = |k: &str| a[k].as_str().unwrap_or_default();
    html! {
        div class="alert-row" {
            code class="code" { (text("id")) }
            span { (text("kind")) }
            span { (text("harness")) "@" (text("version")) }
            span { (text("detail")) }
            (ctx.time(text("at")))
            (form("/do/alerts/ack", BACK, &ctx.req.token, html! {
                input type="hidden" name="id" value=(text("id"));
                button class="button button--secondary" type="submit" { "Acknowledge" }
            }))
        }
    }
}

/// The "API Endpoint" card: the URL as selectable text, and the note when it is only configured.
fn endpoint(check: &Value) -> Markup {
    let url = check["endpoint"].as_str().unwrap_or_default();
    let head = Head::new("api", "API Endpoint").subtitle("Point your agents here");
    card(
        Some(head),
        html! {
            div class="endpoint-url" {
                code class="endpoint-url__text" { (url) }
                @if check["endpoint_source"] == "config" { span class="endpoint-url__note" { (CONFIGURED) } }
            }
        },
    )
}

/// A key's requests so far today: its row in `usage --period today`, else 0.
fn today(usage: &Value, id: &str) -> u64 {
    usage["agents"]
        .as_array()
        .and_then(|a| a.iter().find(|r| r["id"] == id))
        .and_then(|r| r["requests"].as_u64())
        .unwrap_or(0)
}

/// One key as `keys list` shows it: name, id, `…last4`, created, revoked, break behaviour and
/// last used, with a place for the day's requests, then the key's own forms.
fn key_card(ctx: &Ctx<'_>, k: &Value) -> Markup {
    let text = |field: &str| k[field].as_str().unwrap_or_default();
    let revoked = k["revoked"].as_str();
    let class = if revoked.is_some() { "key-card key-card--revoked" } else { "key-card" };
    html! {
        article class=(class) {
            div class="key-card__head" {
                b class="key-card__name" { (name(text("name"))) }
                @if let Some(tag) = k["harness"].as_str() { (badge(Tone::Primary, tag)) }
            }
            div class="key-card__rows" {
                (kv("id", html! { code class="code" { (text("id")) } }))
                (kv("key", html! { code class="code" { (text("key")) } }))
                (kv("created", ctx.time(text("created"))))
                @if let Some(at) = revoked { (kv("revoked", ctx.time(at))) }
                (kv("break", html! { (k["break"].as_str().unwrap_or("default")) }))
                (kv("last used", match k["last_used"].as_str() {
                    Some(at) => ctx.time(at),
                    None => html! { "never" },
                }))
            }
            (kv("requests today", html! { (today(ctx.json(ViewName::Usage), text("id"))) }))
            @if revoked.is_none() { (controls(ctx, k)) }
        }
    }
}

/// A key's forms: tag, break and adapter, each a POST that returns here, and Revoke, which opens
/// the confirm window (`?window=revoke&key=<id>`) and changes nothing by itself.
fn controls(ctx: &Ctx<'_>, k: &Value) -> Markup {
    let id = k["id"].as_str().unwrap_or_default();
    let token = &ctx.req.token;
    let revoke = ctx.req.href(BACK, &["done", "task"], &[("window", "revoke"), ("key", id)]);
    html! {
        (form("/do/keys/tag", BACK, token, html! {
            input type="hidden" name="key" value=(id);
            (field("text", "Tag", k["harness"].as_str(), None))
            button class="button button--secondary" type="submit" { "Save tag" }
        }))
        (form("/do/keys/set-break", BACK, token, html! {
            input type="hidden" name="key" value=(id);
            (select("behaviour", "Break", SET_BREAKS, Some(k["break"].as_str().unwrap_or("default"))))
            button class="button button--secondary" type="submit" { "Save break" }
        }))
        (form("/do/keys/set-adapter", BACK, token, html! {
            input type="hidden" name="key" value=(id);
            (field("harness", "Adapter", k["adapter"].as_str(), None))
            button class="button button--secondary" type="submit" { "Save adapter" }
        }))
        (link_button(&revoke, "Revoke", None, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_configured_note_is_only_for_an_address_no_server_bound() {
        let config = endpoint(&json!({"endpoint": "http://127.0.0.1:20129/v1", "endpoint_source": "config"}));
        let config = config.into_string();
        assert!(config.contains("http://127.0.0.1:20129/v1") && config.contains(CONFIGURED), "{config}");
        let served = endpoint(&json!({"endpoint": "http://127.0.0.1:9000/v1", "endpoint_source": "server"}));
        let served = served.into_string();
        assert!(served.contains("http://127.0.0.1:9000/v1") && !served.contains("configured"), "{served}");
        assert!(!served.contains("<button") && !served.contains("<script"), "no copy button: {served}");
    }
}
