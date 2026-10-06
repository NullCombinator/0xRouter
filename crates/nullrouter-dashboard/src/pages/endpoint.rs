//! Endpoint & Key (contracts/dashboard-http.md "Endpoint & Key"): where agents connect, and one
//! card per agent key as `keys list` shows it. Every fact comes from `check` (the endpoint) and
//! `keys`; the page adds none. There is no copy button (it needs a script), and a key is never
//! shown beyond the last four characters the CLI prints.

use maud::{Markup, html};
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Req};
use crate::components::{Head, card, disabled, empty, kv, name, section_bar, side_panel, side_section, slot};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[ViewName::Check, ViewName::Keys];

/// The hint beside the disabled "Add Agent" (research R15).
pub const ADD_HINT: &str = "In the CLI: `nullrouter keys issue <name>`";
pub const NO_AGENTS: &str = "No agent keys.";
pub const NO_AGENTS_HINT: &str = "Run `nullrouter keys issue <name>`.";
pub const ADAPTERS: &str = "Client-side adapters are not built yet.";
/// What the endpoint card adds when no server answered (`check` says the same).
pub const CONFIGURED: &str = "(configured; no server running)";

pub fn wants(_req: &Req) -> Vec<Want> {
    vec![Want::new(ViewName::Check, json!({})), Want::new(ViewName::Keys, json!({}))]
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let keys = ctx.json(ViewName::Keys).as_array().map_or(&[][..], Vec::as_slice);
    let content = html! {
        (endpoint(ctx.json(ViewName::Check)))
        (slot("Agent traffic"))
        (section_bar("Agents", disabled("Add Agent", Some("add"), ADD_HINT)))
        @if keys.is_empty() {
            (empty("key", NO_AGENTS, NO_AGENTS_HINT))
        } @else {
            div class="key-grid" {
                @for k in keys { (key_card(ctx, k)) }
            }
        }
    };
    let adapters = side_section("Adapters", html! { p class="hint" { (ADAPTERS) } });
    Ok(Body { content, side: Some(side_panel("extension", "Client adapters", adapters)), window: None })
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

/// One key as `keys list` shows it: name, id, `…last4`, created, revoked, break behaviour and
/// last used, with a place for the day's requests.
fn key_card(ctx: &Ctx<'_>, k: &Value) -> Markup {
    let text = |field: &str| k[field].as_str().unwrap_or_default();
    let revoked = k["revoked"].as_str();
    let class = if revoked.is_some() { "key-card key-card--revoked" } else { "key-card" };
    html! {
        article class=(class) {
            div class="key-card__head" {
                b class="key-card__name" { (name(text("name"))) }
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
            (slot("Requests today"))
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
