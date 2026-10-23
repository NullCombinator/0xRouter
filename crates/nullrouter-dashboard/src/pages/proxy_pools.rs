//! Proxy Pools (spec 014 US2): the forms for outbound proxies, on the same ops the CLI runs. Add a
//! proxy by its address, with an optional username and a write-only password; send a provider's or
//! an account's traffic through one, or clear that; remove one, whose confirm window names what is
//! lost; and probe one as a task (`proxy fixed`). The page reads no view, so a proxy is named as the
//! operator types it.

use maud::{Markup, html};
use nullrouter_server::ops;

use super::{Body, Ctx, Failure, Refused, Req};
use crate::access::encode_component;
use crate::components::{Head, card, confirm_window, field, form, icon, secret_field, select};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] = &[];

/// Where every form here returns: this page.
const BACK: &str = "/proxy-pools";
/// What a use or a clear applies to (`scope`, as `proxy use` takes it).
const SCOPES: &[(&str, &str)] = &[("all", "all providers"), ("provider", "a provider"), ("account", "an account")];

pub fn wants(_req: &Req) -> Vec<Want> {
    Vec::new()
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let req = ctx.req;
    let content = html! {
        (add_card(req))
        (route_card(req))
        (remove_card())
        (probe_card(req))
    };
    let window = match req.get("window") {
        Some("remove") => remove_window(req),
        _ => None,
    };
    Ok(Body { content, side: None, window })
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

/// The value a refused form kept for `name`. The password never is kept (the flash leaves it out).
fn kept<'a>(r: Option<&'a Refused>, name: &str) -> Option<&'a str> {
    r?.kept.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// The scope drop-down of a use or a clear, with the refusal that names it beneath.
fn scope_field(selected: Option<&str>, error: Option<&str>) -> Markup {
    html! {
        (select("scope", "Applies to", SCOPES, Some(selected.unwrap_or("all"))))
        @if let Some(e) = error { span class="field__error" { (icon("error")) (e) } }
    }
}

/// "Add proxy": the name, the address, an optional username and the password, which is write-only.
fn add_card(req: &Req) -> Markup {
    let err = refused_form(req, "add");
    let fields: &[&str] = &["name", "url", "username", "password"];
    let head = Head::new("add", "Add proxy").subtitle("An outbound proxy by its address");
    card(
        Some(head),
        form(
            "/do/proxy/add",
            BACK,
            &req.token,
            html! {
                input type="hidden" name="_window" value="add";
                (field("name", "Name", kept(err, "name"), error_on(err, "name", fields)))
                (field("url", "Address", kept(err, "url"), error_on(err, "url", fields)))
                (field("username", "Username (optional)", kept(err, "username"), error_on(err, "username", fields)))
                (secret_field("password", "Password, with a username (write-only)", error_on(err, "password", fields)))
                button class="button button--primary" type="submit" { "Add proxy" }
            },
        ),
    )
}

/// "Route traffic": use a proxy for everything, a provider or an account (`none` stops using one),
/// or clear the choice that a provider or account has.
fn route_card(req: &Req) -> Markup {
    let (use_err, clear_err) = (refused_form(req, "use"), refused_form(req, "clear"));
    let use_fields: &[&str] = &["proxy", "scope", "target"];
    let clear_fields: &[&str] = &["scope", "target"];
    let head = Head::new("route", "Route traffic")
        .subtitle("Send a provider's or an account's requests through a proxy, or clear that choice");
    card(
        Some(head),
        html! {
            p class="hint" { "A target is a provider, or provider/account for an account. Scope all ignores it." }
            (form("/do/proxy/use", BACK, &req.token, html! {
                input type="hidden" name="_window" value="use";
                (field("proxy", "Proxy name, or none", kept(use_err, "proxy"), error_on(use_err, "proxy", use_fields)))
                (scope_field(kept(use_err, "scope"), error_on(use_err, "scope", use_fields)))
                (field("target", "Target", kept(use_err, "target"), error_on(use_err, "target", use_fields)))
                button class="button button--secondary" type="submit" { "Use proxy" }
            }))
            (form("/do/proxy/clear", BACK, &req.token, html! {
                input type="hidden" name="_window" value="clear";
                (scope_field(kept(clear_err, "scope"), error_on(clear_err, "scope", clear_fields)))
                (field("target", "Target", kept(clear_err, "target"), error_on(clear_err, "target", clear_fields)))
                button class="button button--secondary" type="submit" { "Clear choice" }
            }))
        },
    )
}

/// "Remove proxy": names the proxy and opens its confirm window (a GET, so it changes nothing).
fn remove_card() -> Markup {
    let head = Head::new("remove", "Remove proxy").subtitle("Only a proxy nothing uses; the next step asks first");
    card(
        Some(head),
        html! {
            form class="form" method="get" action=(BACK) {
                input type="hidden" name="window" value="remove";
                (field("name", "Proxy name", None, None))
                button class="button button--secondary" type="submit" { "Remove proxy" }
            }
        },
    )
}

/// The remove confirm window (`?window=remove&name=<n>`): what removing loses, the real form beside
/// Cancel. Nothing opens without a name. A refused remove reopens it: `_back` keeps the name, and
/// `_window` reopens the window.
fn remove_window(req: &Req) -> Option<Markup> {
    let name = req.get("name")?;
    let loss = ops::describe_loss("proxy.remove", name).unwrap_or_default();
    let back = format!("{BACK}?name={}", encode_component(name));
    let action = form(
        "/do/proxy/remove",
        &back,
        &req.token,
        html! {
            input type="hidden" name="_window" value="remove";
            input type="hidden" name="confirmed" value="on";
            input type="hidden" name="name" value=(name);
            button class="button button--primary" type="submit" { "Remove proxy" }
        },
    );
    Some(confirm_window("Remove proxy", &loss, BACK, action))
}

/// "Probe proxy": `proxy fixed` runs as a task; the page shows its progress and its line when it ends.
fn probe_card(req: &Req) -> Markup {
    let err = refused_form(req, "fixed");
    let fields: &[&str] = &["name"];
    let head = Head::new("lan", "Probe proxy")
        .subtitle("Checks that a proxy answers; traffic resumes through it when it does");
    card(
        Some(head),
        form(
            "/do/proxy/fixed",
            BACK,
            &req.token,
            html! {
                input type="hidden" name="_window" value="fixed";
                (field("name", "Proxy name", kept(err, "name"), error_on(err, "name", fields)))
                button class="button button--secondary" type="submit" { "Probe proxy" }
            },
        ),
    )
}
