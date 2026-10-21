//! Endpoint & Key (contracts/dashboard-http.md "Endpoint & Key"): where agents connect, and one
//! card per agent key as `keys list` shows it. Every fact comes from `check` (the endpoint) and
//! `keys`; the page adds none. There is no copy button (it needs a script), and a key is never
//! shown beyond the last four characters the CLI prints.

use maud::{Markup, html};
use nullrouter_adapters::BUILTIN;
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Req};
use crate::components::{
    self, Head, Tone, agent_colour, badge, bare_card, card, disabled_button, disabled_switch, empty, icon, inset, name, prose,
    section_bar, section_bar_noted, side_panel, side_row, side_section,
};
use crate::landscape;
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] =
    &[ViewName::Check, ViewName::Keys, ViewName::Accounts, ViewName::Latency, ViewName::Usage];

/// The hint beside the disabled "Add Agent" (research R15).
pub const ADD_HINT: &str = "In the CLI: `nullrouter keys issue <name>`";
pub const NO_AGENTS: &str = "No agent keys.";
pub const NO_AGENTS_HINT: &str = "Run `nullrouter keys issue <name>`.";
/// Where the third-party adapters are listed today.
pub const ADAPTERS: &str = "In the CLI: `nullrouter adapters list`.";
/// What the panel says about adapters.
pub const HOW: &str = "An adapter handles one client harness's quirks. A new version goes live only after you review \
                       it, and the previous version keeps serving meanwhile.";
/// How an adapter is added today.
pub const ADD_ADAPTER: &str = "In the CLI: `nullrouter adapters install <package>`. The review follows, and a version \
                               goes live when you run `nullrouter adapters approve <harness> <version>`.";
/// The hint on an adapter's switch.
pub const SWITCH_HINT: &str = "Not built yet: a built-in adapter serves every key of its harness.";
/// The id of the "Client adapter settings" window: the checkbox the panel's gear toggles.
pub const SETTINGS: &str = "adapter-settings";
/// What the traffic card says under its title.
pub const TRAFFIC: &str = "Each colour is one agent: one pipe per agent into the router, one per provider out of it. \
                           The gauge on a pipe is its latency.";
/// What the endpoint card adds when no server answered (`check` says the same).
pub const CONFIGURED: &str = "(configured; no server running)";

pub fn wants(_req: &Req) -> Vec<Want> {
    vec![
        Want::new(ViewName::Check, json!({})),
        Want::new(ViewName::Keys, json!({})),
        Want::new(ViewName::Accounts, json!({})),
        Want::new(ViewName::Latency, json!({})),
        Want::new(ViewName::Usage, json!({"period": "today"})),
    ]
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let keys = ctx.json(ViewName::Keys).as_array().map_or(&[][..], Vec::as_slice);
    let heading = landscape::heading(ctx.page.as_of, ctx.tz);
    let content = html! {
        (bare_card(Head::new("hub", &heading).subtitle(TRAFFIC), landscape::landscape(&landscape::Inputs {
            keys: ctx.json(ViewName::Keys),
            accounts: ctx.json(ViewName::Accounts),
            latency: ctx.json(ViewName::Latency),
            tz: ctx.tz,
            as_of: ctx.page.as_of,
            logos: ctx.logos,
        })))
        (endpoint(ctx.json(ViewName::Check)))
        div class="key-grid" {
            (section_bar_noted("Agents", prose(ADD_HINT), disabled_button("Add Agent", Some("add"), true)))
            @if keys.is_empty() {
                (card(None, empty("key", NO_AGENTS, NO_AGENTS_HINT)))
            } @else {
                div class="key-grid__cards" {
                    @for (n, k) in keys.iter().enumerate() { (key_card(ctx, n, k)) }
                }
            }
        }
    };
    Ok(Body { content, side: Some(side()), window: None })
}

/// "Client adapters": the built-in ones, where the third-party ones are listed, and how an
/// adapter goes live; the gear opens the settings window.
fn side() -> Markup {
    let tile = || html! { span class="plugin-row__tile" { (icon("terminal")) } };
    let sections = html! {
        (side_section(&format!("Built in · {}", BUILTIN.len()), html! {
            @for h in BUILTIN { (side_row(tile(), h, "core code", badge(Tone::Success, "built-in"))) }
        }))
        (side_section("Third party · sandboxed", html! { p class="side-section__text" { (prose(ADAPTERS)) } }))
        (side_section("How adapters work", html! { p class="side-section__text" { (HOW) } }))
    };
    let settings = html! {
        table class="settings-table" {
            thead { tr { th { "Adapter" } th { "Source" } th { "State" } th { "Enabled" } } }
            tbody {
                @for h in BUILTIN {
                    tr {
                        td { b { (name(h)) } }
                        td class="settings-table__muted" { "core code" }
                        td { (badge(Tone::Success, "built-in")) }
                        td { (disabled_switch("Enabled", SWITCH_HINT)) }
                    }
                }
            }
        }
        p class="settings-table__note" { (SWITCH_HINT) " " (prose(ADAPTERS)) }
        (section_bar("Add an adapter", html! {}))
        (inset(html! { p class="settings-table__note" { (prose(ADD_ADAPTER)) } }))
    };
    html! {
        (side_panel("terminal", "Client adapters", "Search adapters...", SETTINGS, sections))
        (components::settings(SETTINGS, "Client adapter settings", settings))
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
                span class="endpoint-url__label" { "Local" }
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

/// One key as `keys list` shows it (name, harness, `…last4`, id, created, revoked, break
/// behaviour, last used) with the day's requests, in the mockups' agent card: the agent's colour
/// (its place in `keys list`, as the landscape colours it), the name and harness, the key, then
/// the facts in short lines.
fn key_card(ctx: &Ctx<'_>, n: usize, k: &Value) -> Markup {
    let text = |field: &str| k[field].as_str().unwrap_or_default();
    let revoked = k["revoked"].as_str();
    let class = if revoked.is_some() { "key-card key-card--revoked" } else { "key-card" };
    html! {
        article class=(class) {
            span class=(format!("key-card__avatar key-card__avatar--{}", agent_colour(n))) aria-hidden="true" {}
            div class="key-card__body" {
                div class="key-card__head" {
                    b class="key-card__name" { (name(text("name"))) }
                    @if let Some(tag) = k["harness"].as_str() { (badge(Tone::Primary, tag)) }
                }
                code class="key-card__key" { (text("key")) " · " (text("id")) }
                @if let Some(at) = revoked {
                    p class="key-card__state" { "revoked " (ctx.time(at)) }
                }
                p class="key-card__line" {
                    "last used " (match k["last_used"].as_str() {
                        Some(at) => ctx.time(at),
                        None => html! { "never" },
                    })
                    " · requests today " (today(ctx.json(ViewName::Usage), text("id")))
                }
                p class="key-card__line" {
                    "created " (ctx.time(text("created"))) " · break " (k["break"].as_str().unwrap_or("default"))
                }
            }
        }
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
