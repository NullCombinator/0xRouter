//! Providers (contracts/dashboard-http.md "Providers"): the loaded providers in 9router's
//! sections, a window per provider, and the "Provider plugins" side panel. Every fact comes from
//! `providers`, `accounts list --long` and `plugins list --community`; the page adds none.
//!
//! The window shows each model with what `model <provider> <model>` shows: one `model` read in
//! list mode (`nullrouter model <provider>`) gives every model the provider declares.

use std::collections::BTreeSet;

use maud::{Markup, html};
use nullrouter_server::ops::{self, signin::TERMS_WARNING};
use serde_json::{Value, json};

use super::{Body, Ctx, Failure, Refused, Req};
use crate::access::encode_component;
use crate::components::{
    self, Tone, badge, confirm_window, disabled, empty, field, form, icon, kv, link_button, modal, name, prose,
    secret_field, select, status, textarea,
};
use crate::page::{ViewName, Want};

pub const VIEWS: &[ViewName] =
    &[ViewName::Providers, ViewName::Accounts, ViewName::Plugins, ViewName::Model, ViewName::Latency, ViewName::Check];

/// The hint of the one control that is not built yet (research R15).
pub const TEST_HINT: &str = "Not built yet: model tests come with their own slice.";
/// The windows that add a compatible provider: window id, style, title.
const COMPATIBLE: &[(&str, &str, &str)] =
    &[("add-anthropic", "anthropic", "Add Anthropic Compatible"), ("add-openai", "openai", "Add OpenAI Compatible")];
/// The fields of the add form, the first taking a refusal that names none of them.
const ADD_FIELDS: &[&str] = &["id", "name", "base_url", "models"];
/// Where the settings forms and windows return: this page, with its window closed.
const BACK: &str = "/providers";
/// The keys each settings form takes, as the CLI's `routing` and `connection` commands name them.
pub const OVERRIDE_KEYS: &str = "Keys: `cache_lifetime`, `reserve`, `price.input`, `price.output`, `price.cache_read`, `price.cache_write`, `window.<name>.capacity`, `window.<name>.length`, `window.<name>.reserve`, `window.<name>.weight.<class>`, `window.<name>.multiplier.<glob>`.";
pub const PLUGIN_KEYS: &str = "Keys: `window.<name>.weight.<class>` or `window.<name>.multiplier.<glob>`, with a number. Capacity is per account, so a plugin never takes it.";
pub const CONNECTION_KEYS: &str = "Keys: `connect-timeout`, `header-timeout`, `first-token-timeout`, `stall-timeout` (a duration such as `30s`), `reuse` and `http2` (`on` or `off`), `retries[.STATUS]` and `retry-wait[.STATUS]`. An empty model is the provider's own setting.";

/// The operator's own plugins (`source` is `user`).
pub const CUSTOM: &str = "Custom Providers";

/// 9router's sections, in its order, with the `category` values each lists. 9router puts its free
/// and free-tier providers under one heading.
pub const SECTIONS: &[(&str, &[&str])] = &[
    ("OAuth Providers", &["oauth"]),
    ("Free Tier Providers", &["freeTier", "free"]),
    ("API Key Providers", &["apikey"]),
    ("Web Cookie Providers", &["webCookie"]),
];

pub fn wants(req: &Req) -> Vec<Want> {
    let mut wants = vec![
        Want::new(ViewName::Providers, json!({})),
        // The settings panel lists hidden providers too, so Show can bring them back.
        Want::new(ViewName::Providers, json!({"all": true})),
        Want::new(ViewName::Accounts, json!({})),
        Want::new(ViewName::Plugins, json!({"community": true})),
        Want::new(ViewName::Check, json!({})),
    ];
    if let Some(id) = &req.window {
        wants.push(Want::new(ViewName::Model, json!({"provider": id})));
        wants.push(Want::new(ViewName::Latency, json!({})));
    }
    wants
}

pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    let providers = rows(ctx.json(ViewName::Providers));
    let accounts = rows(ctx.json(ViewName::Accounts));
    // A query window (`?window=signin&id=<p>`, `?window=remove-account&id=<p>&name=<n>`) is a confirm
    // window of its own; otherwise the provider window the path names is open.
    let window = match ctx.req.get("window") {
        Some("signin") => Some(signin_window(ctx, providers)?),
        Some("remove-account") => remove_window(ctx),
        Some("uninstall-plugin") => uninstall_window(ctx),
        Some(w) if COMPATIBLE.iter().any(|(id, ..)| *id == w) => Some(add_window(ctx, w)),
        _ => match &ctx.req.window {
            Some(token) => Some(window(ctx, providers, accounts, token)?),
            None => None,
        },
    };
    let q = ctx.req.get("q").map(str::to_lowercase);
    let shown: Vec<&Value> = providers.iter().filter(|p| q.as_deref().is_none_or(|q| matches(p, q))).collect();

    let content = html! {
        (search(ctx.req))
        (custom_section(ctx, &shown, accounts))
        @for (title, categories) in SECTIONS {
            @let mine: Vec<&Value> = shown.iter().copied().filter(|p| section_of(p) == *title && is_in(p, categories)).collect();
            @if !mine.is_empty() {
                (section(ctx, title, disabled("Test All", None, TEST_HINT), &mine, accounts))
            }
        }
        @if shown.is_empty() && q.is_some() {
            (empty("search", "No provider matches", "Search by provider id or alias."))
        }
    };
    Ok(Body { content, side: Some(side(ctx)), window })
}

/// The rows of `set` among the plugin rows.
fn of<'a>(plugins: &'a [Value], set: &'a str) -> impl Iterator<Item = &'a Value> {
    plugins.iter().filter(move |r| r["set"] == set)
}

fn rows(v: &Value) -> &[Value] {
    v.as_array().map_or(&[][..], Vec::as_slice)
}

/// The section a provider is listed under: the operator's own plugins together, the rest by
/// category.
pub fn section_of(p: &Value) -> &'static str {
    if p["source"] == "user" {
        return CUSTOM;
    }
    let category = p["category"].as_str().unwrap_or_default();
    SECTIONS.iter().find(|(_, cats)| cats.contains(&category)).map_or("Other Providers", |(title, _)| title)
}

/// Whether the provider's category is one of `categories` (the section titles repeat for
/// `free` and `freeTier`; this keeps an unknown category out of every named section).
fn is_in(p: &Value, categories: &[&str]) -> bool {
    p["category"].as_str().is_some_and(|c| categories.contains(&c))
}

/// Whether `q` (lower case) is in the provider's id or alias.
fn matches(p: &Value, q: &str) -> bool {
    ["id", "alias"].iter().any(|k| p[*k].as_str().is_some_and(|s| s.to_lowercase().contains(q)))
}

/// The `q` search, a `GET` form (research R4).
fn search(req: &Req) -> Markup {
    html! {
        form class="provider-grid__search" method="get" action=(req.base()) role="search" {
            @if req.notices_open() { input type="hidden" name="notices" value=""; }
            (icon("search"))
            input class="input" type="search" name="q" value=(req.get("q").unwrap_or_default())
                placeholder="Search providers..." aria-label="Search providers";
            button class="button button--secondary" type="submit" { "Search" }
        }
    }
}

/// "Custom Providers", always drawn: it holds the two disabled "Add … Compatible" controls.
fn custom_section(ctx: &Ctx<'_>, shown: &[&Value], accounts: &[Value]) -> Markup {
    let mine: Vec<&Value> = shown.iter().copied().filter(|p| section_of(p) == CUSTOM).collect();
    let controls = html! {
        @for (window, _, title) in COMPATIBLE {
            (link_button(&ctx.req.href(BACK, &["window", "id", "name", "done", "task"], &[("window", window)]), title, Some("add"), false))
        }
    };
    if mine.is_empty() {
        return html! {
            div class="provider-grid" {
                (components::section_bar(CUSTOM, controls))
                (empty("extension", "No custom providers", "Add one with the buttons above, or with a plugin file; see docs/plugins.md."))
            }
        };
    }
    section(ctx, CUSTOM, controls, &mine, accounts)
}

fn section(ctx: &Ctx<'_>, title: &str, controls: Markup, providers: &[&Value], accounts: &[Value]) -> Markup {
    html! {
        div class="provider-grid" {
            (components::section_bar(title, controls))
            div class="provider-grid__cards" {
                @for p in providers { (card(ctx, p, &accounts_of(accounts, p["id"].as_str().unwrap_or_default()))) }
            }
        }
    }
}

fn accounts_of<'a>(accounts: &'a [Value], provider: &str) -> Vec<&'a Value> {
    accounts.iter().filter(|a| a["provider"] == provider).collect()
}

fn card(ctx: &Ctx<'_>, p: &Value, accounts: &[&Value]) -> Markup {
    let id = p["id"].as_str().unwrap_or_default();
    let alias = p["alias"].as_str().unwrap_or(id);
    let href = ctx.req.href(&format!("/providers/{}", encode_component(id)), &["kind"], &[]);
    let class =
        if ctx.req.window.as_deref() == Some(id) { "provider-card provider-card--open" } else { "provider-card" };
    html! {
        a class=(class) href=(href) {
            span class="provider-card__logo" { (components::logo(ctx.logos.href(id), alias)) }
            span class="provider-card__text" {
                b class="provider-card__id" { (name(id)) }
                span class="provider-card__accounts" { (accounts_line(accounts)) }
            }
        }
    }
}

/// An account's state when it isn't plain active, in the words the card shows.
fn state_label(a: &Value) -> Option<&'static str> {
    match a["state"].as_str()? {
        "active" => a["state_text"].as_str().is_some_and(|t| t.starts_with("cooling")).then_some("cooling"),
        "needs_sign_in" => Some("needs sign-in"),
        "refused" => Some("refused"),
        "refreshing" => Some("refreshing"),
        "disabled" => Some("disabled"),
        _ => None,
    }
}

/// `N connections`, then how many are in each state that isn't active; `No connections`.
fn accounts_line(accounts: &[&Value]) -> Markup {
    if accounts.is_empty() {
        return html! { "No connections" };
    }
    let mut tally: Vec<(&'static str, usize)> = Vec::new();
    for label in accounts.iter().filter_map(|a| state_label(a)) {
        match tally.iter_mut().find(|(l, _)| *l == label) {
            Some((_, n)) => *n += 1,
            None => tally.push((label, 1)),
        }
    }
    let n = accounts.len();
    html! {
        (n) (if n == 1 { " connection" } else { " connections" })
        @for (label, count) in &tally { " · " (count) " " (status(label)) }
    }
}

/// The window open on `/providers/<id>`: 404 with the CLI's words for an id (or alias) no
/// loaded provider has.
fn window(ctx: &Ctx<'_>, providers: &[Value], accounts: &[Value], token: &str) -> Result<Markup, Failure> {
    let p = providers
        .iter()
        .find(|p| p["id"] == token || p["alias"] == token)
        .ok_or_else(|| Failure::not_found(format!("not found: unknown provider {token:?}")))?;
    let req = ctx.req;
    let id = p["id"].as_str().unwrap_or_default();
    let alias = p["alias"].as_str().unwrap_or(id);
    let listed = ctx.json(ViewName::Model);
    if listed["kind"] == "not_found" {
        return Err(Failure::not_found(format!("not found: {}", listed["error"].as_str().unwrap_or_default())));
    }
    let models = rows(listed);
    let chosen = req.get("kind");
    let shown: Vec<&Value> = models.iter().filter(|m| chosen.is_none_or(|k| in_kind(m, k))).collect();
    let mine = accounts_of(accounts, id);

    let body = html! {
        div class="provider-window" {
            div class="provider-window__head" {
                span class="provider-window__logo" { (components::logo(ctx.logos.href(id), alias)) }
                div class="provider-window__titles" {
                    h3 class="provider-window__name" { (id) }
                    div class="provider-window__facts" {
                        @if let Some(a) = p["alias"].as_str() { (kv("alias", html! { (a) })) }
                        (kv("category", html! { (p["category"].as_str().unwrap_or_default()) }))
                        (kv("source", html! { (p["source"].as_str().unwrap_or_default()) }))
                    }
                }
            }
            section class="provider-window__section" {
                h4 class="provider-window__heading" { "Accounts" }
                @if mine.is_empty() {
                    p class="provider-window__none" { "No connections" }
                } @else {
                    @for a in &mine { (account_row(ctx, a)) }
                }
                (add_account(ctx, id))
                (sign_in(ctx, p))
            }
            section class="provider-window__section" {
                h4 class="provider-window__heading" { "Models" }
                (kind_filter(req, providers, models, chosen))
                @if shown.is_empty() {
                    p class="provider-window__none" {
                        @if models.is_empty() { "This provider declares no models." } @else { "No models of this kind." }
                    }
                } @else {
                    div class="provider-window__models" { @for m in &shown { (model_row(ctx, id, &mine, m)) } }
                }
            }
            section class="provider-window__section" {
                h4 class="provider-window__heading" { "Last response" }
                (last_response(ctx, id))
            }
            section class="provider-window__section" {
                h4 class="provider-window__heading" { "Plugin meters" }
                (plugin_meters(ctx, id))
            }
            section class="provider-window__section" {
                h4 class="provider-window__heading" { "Connection" }
                (connection_settings(ctx, id))
            }
        }
    };
    Ok(modal(id, &req.close_href(), body))
}

/// The terms warning as the confirm window words it: the CLI's text without its `[y/N]` prompt.
fn terms_text() -> &'static str {
    TERMS_WARNING.trim_end().trim_end_matches("Continue? [y/N]").trim_end()
}

/// The sign-in confirm window (`?window=signin&id=<p>`): the terms warning and the account name.
/// Sign in posts `accept_terms`, so pressing it is the answer to the warning.
fn signin_window(ctx: &Ctx<'_>, providers: &[Value]) -> Result<Markup, Failure> {
    let id = ctx.req.get("id").unwrap_or_default();
    if !providers.iter().any(|p| p["id"] == id) {
        return Err(Failure::not_found(format!("not found: unknown provider {id:?}")));
    }
    // A refused sign-in reopens this window: `_window` names it, and `_back` keeps the provider.
    let back = format!("/providers?id={}", encode_component(id));
    let action = form(
        "/do/accounts/signin",
        &back,
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value="signin";
            input type="hidden" name="provider" value=(id);
            input type="hidden" name="accept_terms" value="on";
            (field("name", "Account name", None, None))
            button class="button button--primary" type="submit" { "Sign in" }
        },
    );
    let cancel = format!("/providers/{}", encode_component(id));
    Ok(confirm_window("Sign in", terms_text(), &cancel, action))
}

/// The remove confirm window (`?window=remove-account&id=<p>&name=<n>`): what removing loses, the
/// real form beside Cancel. Nothing opens without both the provider and the account name.
fn remove_window(ctx: &Ctx<'_>) -> Option<Markup> {
    let (id, account) = (ctx.req.get("id")?, ctx.req.get("name")?);
    let loss = ops::describe_loss("accounts.remove", &format!("{id}/{account}")).unwrap_or_default();
    // A refused remove reopens this window: `_back` names the account, and `_window` reopens it.
    let back = format!("/providers?id={}&name={}", encode_component(id), encode_component(account));
    let action = form(
        "/do/accounts/remove",
        &back,
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value="remove-account";
            input type="hidden" name="confirmed" value="on";
            input type="hidden" name="provider" value=(id);
            input type="hidden" name="name" value=(account);
            button class="button button--primary" type="submit" { "Remove account" }
        },
    );
    let cancel = format!("/providers/{}", encode_component(id));
    Some(confirm_window("Remove account", &loss, &cancel, action))
}

/// The add-account form: a name, the key (write-only, a password field) and the order it keeps.
fn add_account(ctx: &Ctx<'_>, id: &str) -> Markup {
    let back = ctx.req.href(&ctx.req.path(), &["done", "task"], &[]);
    // A refused add keeps its name and order and shows its refusal beside the field it names. Only
    // this form posts `order` (the secret is never kept), so only its refusal has it; a refusal with
    // no field is the name's (an account of that name exists, or the name is not valid).
    let refusal = ctx.req.refused.as_ref().filter(|r| r.kept.iter().any(|(k, _)| k == "order"));
    let kept = |k: &str| refusal.and_then(|r| r.kept.iter().find(|(n, _)| n == k)).map(|(_, v)| v.as_str());
    let error = |k: &str| refusal.filter(|r| r.field.as_deref().unwrap_or("name") == k).map(|r| r.text.as_str());
    form(
        "/do/accounts/add",
        &back,
        &ctx.req.token,
        html! {
            input type="hidden" name="provider" value=(id);
            (field("name", "Account name", kept("name"), error("name")))
            (secret_field("secret", "API key", error("secret")))
            (field("order", "Order", kept("order"), error("order")))
            button class="button button--primary" type="submit" { "Add account" }
        },
    )
}

/// Sign in, for a provider that declares sign-in (`signin` in the providers view). One with a
/// terms warning opens its confirm window; any other posts its account name directly.
fn sign_in(ctx: &Ctx<'_>, p: &Value) -> Markup {
    if p["signin"] != true {
        return html! {};
    }
    let id = p["id"].as_str().unwrap_or_default();
    if p["terms_warning"] == true {
        let href =
            ctx.req.href("/providers", &["window", "id", "name", "done", "task"], &[("window", "signin"), ("id", id)]);
        return link_button(&href, "Sign in", Some("key"), false);
    }
    let back = ctx.req.href(&ctx.req.path(), &["done", "task"], &[]);
    form(
        "/do/accounts/signin",
        &back,
        &ctx.req.token,
        html! {
            input type="hidden" name="provider" value=(id);
            (field("name", "Account name", None, None))
            button class="button button--secondary" type="submit" { "Sign in" }
        },
    )
}

/// An account's forms: priority, enable or disable (whichever the state doesn't already say),
/// exclusive on and off, and Remove, which opens its confirm window and changes nothing itself.
/// The view doesn't say whether an account is exclusive, so both choices are offered.
fn account_controls(ctx: &Ctx<'_>, a: &Value) -> Markup {
    let provider = a["provider"].as_str().unwrap_or_default();
    let account = a["name"].as_str().unwrap_or_default();
    let back = ctx.req.href(&ctx.req.path(), &["done", "task"], &[]);
    let token = &ctx.req.token;
    let priority = a["priority"].as_f64().map(|p| p.to_string());
    let remove = ctx.req.href(
        "/providers",
        &["window", "id", "name", "done", "task"],
        &[("window", "remove-account"), ("id", provider), ("name", account)],
    );
    html! {
        (form("/do/accounts/priority", &back, token, html! {
            input type="hidden" name="provider" value=(provider);
            input type="hidden" name="name" value=(account);
            (field("priority", "Priority", priority.as_deref(), None))
            button class="button button--secondary" type="submit" { "Save priority" }
        }))
        @if a["state"] == "disabled" {
            (form("/do/accounts/enable", &back, token, html! {
                input type="hidden" name="provider" value=(provider);
                input type="hidden" name="name" value=(account);
                button class="button button--secondary" type="submit" { "Enable" }
            }))
        } @else {
            (form("/do/accounts/disable", &back, token, html! {
                input type="hidden" name="provider" value=(provider);
                input type="hidden" name="name" value=(account);
                button class="button button--secondary" type="submit" { "Disable" }
            }))
        }
        (form("/do/accounts/exclusive", &back, token, html! {
            input type="hidden" name="provider" value=(provider);
            input type="hidden" name="name" value=(account);
            input type="hidden" name="on" value="on";
            button class="button button--secondary" type="submit" { "Make exclusive" }
        }))
        (form("/do/accounts/exclusive", &back, token, html! {
            input type="hidden" name="provider" value=(provider);
            input type="hidden" name="name" value=(account);
            button class="button button--secondary" type="submit" { "Clear exclusive" }
        }))
        (link_button(&remove, "Remove", None, false))
    }
}

/// Where this page's settings forms return: the page as it is, minus the flash and the window a
/// refusal named (the redirect adds that again).
fn back(req: &Req) -> String {
    req.href(&req.path(), &["done", "task", "window"], &[])
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

/// One account's routing overrides (`routing set`, `routing unset`): a key and its value, or a key
/// to remove. The keys are the CLI's; `window.<name>` names one of the provider's declared windows.
fn overrides(ctx: &Ctx<'_>, a: &Value) -> Markup {
    let req = ctx.req;
    let provider = a["provider"].as_str().unwrap_or_default();
    let account = a["name"].as_str().unwrap_or_default();
    let (set, unset) = (format!("routing-set-{account}"), format!("routing-unset-{account}"));
    let (set_err, unset_err) = (refused_form(req, &set), refused_form(req, &unset));
    let set_keys: &[&str] = &["key", "value"];
    html! {
        p class="hint" { (prose(OVERRIDE_KEYS)) }
        (form("/do/routing/set", &back(req), &req.token, html! {
            input type="hidden" name="_window" value=(set);
            input type="hidden" name="provider" value=(provider);
            input type="hidden" name="name" value=(account);
            (field("key", "Override key", kept(set_err, "key"), error_on(set_err, "key", set_keys)))
            (field("value", "Value", kept(set_err, "value"), error_on(set_err, "value", set_keys)))
            button class="button button--secondary" type="submit" { "Set override" }
        }))
        (form("/do/routing/unset", &back(req), &req.token, html! {
            input type="hidden" name="_window" value=(unset);
            input type="hidden" name="provider" value=(provider);
            input type="hidden" name="name" value=(account);
            (field("key", "Remove override", kept(unset_err, "key"), error_on(unset_err, "key", &["key"])))
            button class="button button--secondary" type="submit" { "Remove override" }
        }))
    }
}

/// The provider's plugin meters (`routing set-plugin`, `unset-plugin`): the per-window weights and
/// multipliers that every account of the plugin inherits.
fn plugin_meters(ctx: &Ctx<'_>, id: &str) -> Markup {
    let req = ctx.req;
    let (set_err, unset_err) = (refused_form(req, "plugin-set"), refused_form(req, "plugin-unset"));
    let set_keys: &[&str] = &["key", "value"];
    html! {
        p class="hint" { (prose(PLUGIN_KEYS)) }
        (form("/do/routing/set-plugin", &back(req), &req.token, html! {
            input type="hidden" name="_window" value="plugin-set";
            input type="hidden" name="provider" value=(id);
            (field("key", "Meter key", kept(set_err, "key"), error_on(set_err, "key", set_keys)))
            (field("value", "Value", kept(set_err, "value"), error_on(set_err, "value", set_keys)))
            button class="button button--secondary" type="submit" { "Set meter" }
        }))
        (form("/do/routing/unset-plugin", &back(req), &req.token, html! {
            input type="hidden" name="_window" value="plugin-unset";
            input type="hidden" name="provider" value=(id);
            (field("key", "Remove meter key", kept(unset_err, "key"), error_on(unset_err, "key", &["key"])))
            button class="button button--secondary" type="submit" { "Remove meter" }
        }))
    }
}

/// The provider's connection settings (`connection set`, `connection unset`), for the provider or,
/// with a model named, for that model.
fn connection_settings(ctx: &Ctx<'_>, id: &str) -> Markup {
    let req = ctx.req;
    let (set_err, unset_err) = (refused_form(req, "connection-set"), refused_form(req, "connection-unset"));
    let set_keys: &[&str] = &["key", "value", "model"];
    let unset_keys: &[&str] = &["key", "model"];
    html! {
        p class="hint" { (prose(CONNECTION_KEYS)) }
        (form("/do/connection/set", &back(req), &req.token, html! {
            input type="hidden" name="_window" value="connection-set";
            input type="hidden" name="provider" value=(id);
            (field("model", "Model (optional)", kept(set_err, "model"), error_on(set_err, "model", set_keys)))
            (field("key", "Setting", kept(set_err, "key"), error_on(set_err, "key", set_keys)))
            (field("value", "Value", kept(set_err, "value"), error_on(set_err, "value", set_keys)))
            button class="button button--secondary" type="submit" { "Set connection" }
        }))
        (form("/do/connection/unset", &back(req), &req.token, html! {
            input type="hidden" name="_window" value="connection-unset";
            input type="hidden" name="provider" value=(id);
            (field("model", "Model (optional)", kept(unset_err, "model"), error_on(unset_err, "model", unset_keys)))
            (field("key", "Remove setting", kept(unset_err, "key"), error_on(unset_err, "key", unset_keys)))
            button class="button button--secondary" type="submit" { "Remove setting" }
        }))
    }
}

/// The provider's last response in the last 24 hours (`latency`'s `last`): "resolved · <time>",
/// "failed · <time> · <status>", or that there was none.
pub const NO_RESPONSE: &str = "None in the last 24 hours";

fn last_response(ctx: &Ctx<'_>, id: &str) -> Markup {
    let row = rows(&ctx.json(ViewName::Latency)["providers"]).iter().find(|r| r["id"] == id);
    let last = row.map(|r| &r["last"]).filter(|l| l["at"].is_string());
    html! {
        @if let Some(l) = last {
            p {
                (l["result"].as_str().unwrap_or_default()) " · " (ctx.time(l["at"].as_str().unwrap_or_default()))
                @if let Some(status) = l["status"].as_u64() { " · " (status) }
            }
        } @else {
            p class="provider-window__none" { (NO_RESPONSE) }
        }
    }
}

/// One account as `accounts list --long` shows it: the columns, then a sign-in account's email,
/// tier and token times, then the command that signs in again.
fn account_row(ctx: &Ctx<'_>, a: &Value) -> Markup {
    let text = |k: &str| a[k].as_str().unwrap_or("-").to_owned();
    let priority = a["priority"].as_f64().map_or_else(|| "-".to_owned(), |p| p.to_string());
    let when = |k: &str| match a[k].as_str() {
        Some(t) => ctx.time(t),
        None => html! { "-" },
    };
    let command = format!("Run: `nullrouter accounts signin {} {}`", text("provider"), text("name"));
    html! {
        div class="provider-window__account" {
            div class="provider-window__account-head" {
                b class="provider-window__account-name" { (name(&text("name"))) }
                (status(&text("state_text")))
            }
            div class="provider-window__account-facts" {
                (kv("kind", html! { (text("kind")) }))
                (kv("order", html! { (a["order"].to_string()) }))
                (kv("priority", html! { (priority) }))
                (kv("secret", html! { (text("secret")) }))
                @if a["kind"] == "signin" {
                    (kv("email", html! { (text("email")) }))
                    (kv("tier", html! { (text("tier")) }))
                    (kv("expires", when("expires_at")))
                    (kv("refreshed", when("last_refresh_at")))
                }
            }
            @if matches!(a["state"].as_str(), Some("needs_sign_in" | "refused")) {
                p class="provider-window__account-run" { (prose(&command)) }
            }
            (account_controls(ctx, a))
            (overrides(ctx, a))
        }
    }
}

/// `All`, then one link per kind any loaded plugin declares, each with this provider's count.
fn kind_filter(req: &Req, providers: &[Value], models: &[Value], chosen: Option<&str>) -> Markup {
    let kinds: BTreeSet<&str> =
        providers.iter().flat_map(|p| rows(&p["capabilities"])).filter_map(Value::as_str).collect();
    let path = req.path();
    let all = req.href(&path, &["kind"], &[]);
    html! {
        nav class="kind-filter" aria-label="Model kinds" {
            (kind_link(&all, "All", models.len(), chosen.is_none()))
            @for kind in kinds.iter().copied() {
                @let href = req.href(&path, &[], &[("kind", kind)]);
                @let count = models.iter().filter(|m| in_kind(m, kind)).count();
                (kind_link(&href, kind, count, chosen == Some(kind)))
            }
        }
    }
}

/// `Registry::catalog`'s rule: a model is under `kind` when it has it, and untyped models are
/// listed under `llm`.
fn in_kind(m: &Value, kind: &str) -> bool {
    match m["kind"].as_str() {
        Some(k) => k == kind,
        None => kind == "llm",
    }
}

fn kind_link(href: &str, label: &str, count: usize, current: bool) -> Markup {
    let class = if current { "kind-filter__item kind-filter__item--current" } else { "kind-filter__item" };
    html! {
        a class=(class) href=(href) aria-current=[current.then_some("true")] {
            (label) " " span class="kind-filter__count" { (count) }
        }
    }
}

/// A model with what `model <provider> <model>` prints for it, its Test link (a test of
/// `provider/model`), and its verdict forms. A verdict names one of the provider's connections, so a
/// provider with none gets no verdict forms.
fn model_row(ctx: &Ctx<'_>, provider: &str, accounts: &[&Value], m: &Value) -> Markup {
    let list = |k: &str| m[k].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "));
    let fact = |label: &str, value: Option<String>| html! { @if let Some(v) = value { (kv(label, html! { (v) })) } };
    let id = m["model"].as_str().unwrap_or_default();
    let test = format!("/combo?window=test&target={}", encode_component(&format!("{provider}/{id}")));
    let connections: Vec<(&str, &str)> = accounts.iter().filter_map(|a| a["name"].as_str()).map(|n| (n, n)).collect();
    let back = ctx.req.href(&ctx.req.path(), &["done", "task"], &[]);
    let token = &ctx.req.token;
    html! {
        div class="model-row" {
            div class="model-row__head" {
                code class="code model-row__id" title=(id) { (id) }
                @if let Some(k) = m["kind"].as_str() { (badge(Tone::Primary, k)) } @else { (badge(Tone::Default, "untyped")) }
            }
            div class="model-row__facts" {
                (fact("name", m["name"].as_str().map(str::to_owned)))
                (fact("declared", m["declared"].as_bool().map(|d| d.to_string())))
                (fact("target format", m["target_format"].as_str().map(str::to_owned)))
                (fact("supported formats", list("supported_formats")))
                (fact("quota family", m["quota_family"].as_str().map(str::to_owned)))
                (fact("strip", list("strip")))
                (fact("upstream id", m["upstream_id"].as_str().map(str::to_owned)))
            }
            div class="model-row__actions" {
                (link_button(&test, "Test", Some("check"), false))
                @if connections.is_empty() {
                    p class="hint" { "No connection to give a verdict on this model." }
                } @else {
                    (form("/do/verdicts/mark", &back, token, html! {
                        input type="hidden" name="provider" value=(provider);
                        input type="hidden" name="model" value=(id);
                        (select("account", "Connection", &connections, None))
                        (field("note", "Note (optional)", None, None))
                        button class="button button--secondary" type="submit" { "Mark verdict" }
                    }))
                    (form("/do/verdicts/clear", &back, token, html! {
                        input type="hidden" name="provider" value=(provider);
                        input type="hidden" name="model" value=(id);
                        (select("account", "Connection", &connections, None))
                        button class="button button--secondary" type="submit" { "Clear verdict" }
                    }))
                }
            }
        }
    }
}

/// The add window of a compatible provider (`?window=add-openai` or `add-anthropic`): id, name,
/// base URL and models one per line. A refusal reopens it with the values kept; a private-address
/// refusal links to Settings, where the switch is.
fn add_window(ctx: &Ctx<'_>, window: &str) -> Markup {
    let (_, style, title) = COMPATIBLE.iter().find(|(id, ..)| *id == window).copied().unwrap_or_default();
    let err = refused_form(ctx.req, window);
    let url_error = error_on(err, "base_url", ADD_FIELDS);
    let private = err.is_some_and(|r| r.field.as_deref() == Some("base_url") && r.text.contains("private address"));
    let body = form(
        "/do/providers/add-compatible",
        BACK,
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value=(window);
            input type="hidden" name="style" value=(style);
            (field("id", "Id", kept(err, "id"), error_on(err, "id", ADD_FIELDS)))
            (field("name", "Name (optional)", kept(err, "name"), error_on(err, "name", ADD_FIELDS)))
            (field("base_url", "Base URL", kept(err, "base_url"), url_error))
            @if private { a class="button button--secondary" href="/settings" { "Open Settings" } }
            (textarea("models", "Models, one per line", kept(err, "models"), error_on(err, "models", ADD_FIELDS)))
            button class="button button--primary" type="submit" { "Add provider" }
        },
    );
    modal(title, BACK, body)
}

/// The uninstall confirm window (`?window=uninstall-plugin&id=<p>`): what uninstalling loses, the
/// real form beside Cancel. Nothing opens without an id.
fn uninstall_window(ctx: &Ctx<'_>) -> Option<Markup> {
    let id = ctx.req.get("id")?;
    let loss = ops::describe_loss("plugins.uninstall", id).unwrap_or_default();
    // A refused uninstall reopens this window: `_back` names the plugin, `_window` reopens it.
    let back = format!("{BACK}?id={}", encode_component(id));
    let action = form(
        "/do/plugins/uninstall",
        &back,
        &ctx.req.token,
        html! {
            input type="hidden" name="_window" value="uninstall-plugin";
            input type="hidden" name="confirmed" value="on";
            input type="hidden" name="id" value=(id);
            button class="button button--primary" type="submit" { "Uninstall provider" }
        },
    );
    Some(confirm_window("Uninstall provider", &loss, BACK, action))
}

/// "Provider plugins": what `plugins list --community` lists, each with its Enabled and Show on
/// page switches, an Install form for the community plugins that fit, and Uninstall (confirm) for
/// the installed ones.
fn side(ctx: &Ctx<'_>) -> Markup {
    let plugins = rows(ctx.json(ViewName::Plugins));
    let all = ctx.all(ViewName::Providers).find(|(args, _)| args["all"] == true).map_or(&[][..], |(_, v)| rows(v));
    let installed: Vec<&Value> = of(plugins, "community").filter(|r| r["status"] == "installed").collect();
    let not_installed = of(plugins, "community").filter(|r| r["status"] != "installed").count();
    let fits: Vec<(&str, &str)> = of(plugins, "community")
        .filter(|r| r["status"] == "fits")
        .filter_map(|r| r["id"].as_str())
        .map(|id| (id, id))
        .collect();
    let is_installed = |r: &Value| installed.iter().any(|i| i["id"] == r["id"]);
    let bundled: Vec<&Value> = of(plugins, "bundled").collect();
    let own: Vec<&Value> = of(plugins, "user").filter(|r| !is_installed(r)).collect();
    // An installed community plugin is loaded as a user plugin: that row says how it loaded.
    let state_of = |r: &Value| -> String {
        of(plugins, "user")
            .find(|u| u["id"] == r["id"])
            .and_then(|u| u["status"].as_str())
            .or_else(|| r["status"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let row = |r: &Value, state: &str, uninstall: bool| plugin_row(ctx, r, state, all, uninstall);
    let back = ctx.req.href(&ctx.req.path(), &["done", "task"], &[]);
    let sections = html! {
        (components::side_section(&format!("Bundled · {}", bundled.len()), html! {
            @for r in &bundled { (row(r, r["status"].as_str().unwrap_or_default(), false)) }
        }))
        (components::side_section(&format!("Installed from community · {}", installed.len()), html! {
            @for r in &installed { (row(r, &state_of(r), true)) }
        }))
        @if !own.is_empty() {
            (components::side_section(&format!("Your own · {}", own.len()), html! {
                @for r in &own { (row(r, r["status"].as_str().unwrap_or_default(), false)) }
            }))
        }
        (components::side_section(&format!("Community · not installed: {not_installed} available."), html! {
            @if fits.is_empty() {
                p class="hint" { "No community plugin is left to install." }
            } @else {
                (form("/do/plugins/install", &back, &ctx.req.token, html! {
                    (select("id", "Plugin", &fits, None))
                    button class="button button--secondary" type="submit" { "Install" }
                }))
            }
        }))
    };
    components::side_panel("extension", "Provider plugins", sections)
}

/// One plugin: its source and state as `plugins list` shows them, and, for a plugin the registry
/// loaded, its Enabled and Show on page switches (each posts the opposite of what the providers
/// view says) and, for a community plugin, Uninstall.
fn plugin_row(ctx: &Ctx<'_>, r: &Value, state: &str, all: &[Value], uninstall: bool) -> Markup {
    let id = r["id"].as_str().unwrap_or_default();
    let settings = all.iter().find(|p| p["id"] == id);
    let back = ctx.req.href(&ctx.req.path(), &["done", "task"], &[]);
    let token = &ctx.req.token;
    let switch_form = |action: &str, label: &str| {
        form(
            action,
            &back,
            token,
            html! {
                input type="hidden" name="id" value=(id);
                button class="button button--secondary" type="submit" { (label) }
            },
        )
    };
    let remove =
        ctx.req.href(BACK, &["window", "id", "name", "done", "task"], &[("window", "uninstall-plugin"), ("id", id)]);
    html! {
        div class="plugin-row" {
            span class="plugin-row__logo" { (components::logo(ctx.logos.href(id), id)) }
            div class="plugin-row__text" {
                b class="plugin-row__id" { (name(id)) }
                span class="plugin-row__source" { (r["set"].as_str().unwrap_or_default()) }
            }
            (status(state))
            @if let Some(p) = settings {
                div class="plugin-row__switches" {
                    span class="hint" { "Enabled: " (if p["enabled"] == false { "no" } else { "yes" }) }
                    @if p["enabled"] == false {
                        (switch_form("/do/providers/enable", "Enable"))
                    } @else {
                        (switch_form("/do/providers/disable", "Disable"))
                    }
                    span class="hint" { "Show on page: " (if p["hidden"] == true { "no" } else { "yes" }) }
                    @if p["hidden"] == true {
                        (switch_form("/do/providers/show", "Show on page"))
                    } @else {
                        (switch_form("/do/providers/hide", "Hide from page"))
                    }
                }
            }
            @if uninstall { (link_button(&remove, "Uninstall", None, false)) }
        }
    }
}
