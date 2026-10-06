//! The pages (contracts/dashboard-http.md "Routes"). Each module declares the views it reads as a
//! `const VIEWS`, asks for them with `wants`, and arranges them in `body`; the frame
//! ([`crate::frame`]) puts the result inside the sidebar and header. A page module only formats
//! what its views say (research R1).

use axum::http::StatusCode;
use jiff::tz::TimeZone;
use maud::Markup;
use serde_json::Value;

use crate::access;
use crate::page::{Page, ViewName, Want};

pub mod combo;
pub mod console_log;
pub mod endpoint;
pub mod providers;
pub mod proxy_pools;
pub mod quota;
pub mod settings;
pub mod signin;
pub mod usage;

/// A sidebar entry and its page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Id {
    Endpoint,
    Providers,
    Combo,
    Usage,
    Quota,
    ProxyPools,
    ConsoleLog,
    Settings,
}

impl Id {
    /// The sidebar, in 9router's order: five entries, then "System" and three more.
    pub const ALL: [Self; 8] = [
        Self::Endpoint,
        Self::Providers,
        Self::Combo,
        Self::Usage,
        Self::Quota,
        Self::ProxyPools,
        Self::ConsoleLog,
        Self::Settings,
    ];

    /// The first entry under the "System" heading.
    pub const SYSTEM: Self = Self::ProxyPools;

    pub const fn path(self) -> &'static str {
        match self {
            Self::Endpoint => "/endpoint",
            Self::Providers => "/providers",
            Self::Combo => "/combo",
            Self::Usage => "/usage",
            Self::Quota => "/quota",
            Self::ProxyPools => "/proxy-pools",
            Self::ConsoleLog => "/console-log",
            Self::Settings => "/settings",
        }
    }

    /// The sidebar label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Endpoint => "Endpoint & Key",
            Self::Providers => "Providers",
            Self::Combo => "Combo",
            Self::Usage => "Usage",
            Self::Quota => "Quota Tracker",
            Self::ProxyPools => "Proxy Pools",
            Self::ConsoleLog => "Console Log",
            Self::Settings => "Settings",
        }
    }

    /// The header's title and subtitle, as the mockups draw them.
    pub const fn title(self) -> (&'static str, &'static str) {
        match self {
            Self::Endpoint => ("Endpoint", "API endpoint configuration"),
            Self::Providers => ("Providers", "Manage your AI provider connections"),
            Self::Combo => ("Combo", "Model combos with fallback"),
            Self::Usage => ("Usage & Analytics", "Request records, token use and latency as recorded"),
            Self::Quota => ("Quota Tracker", "Track your API quota limits"),
            Self::ProxyPools => ("Proxy Pools", "Outbound proxy relays"),
            Self::ConsoleLog => ("Console Log", "Live server log"),
            Self::Settings => ("Settings", "Read-only view of your configuration"),
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            Self::Endpoint => "api",
            Self::Providers => "dns",
            Self::Combo => "layers",
            Self::Usage => "bar_chart",
            Self::Quota => "data_usage",
            Self::ProxyPools => "lan",
            Self::ConsoleLog => "terminal",
            Self::Settings => "settings",
        }
    }

    /// The `subject` of the `check` notices shown above this page's content (research R5);
    /// `None` for pages no notice concerns.
    pub const fn subject(self) -> Option<&'static str> {
        match self {
            Self::Endpoint => Some("endpoint"),
            Self::Providers => Some("providers"),
            Self::Combo => Some("combo"),
            Self::Usage => Some("usage"),
            Self::Quota => Some("quota"),
            Self::Settings => Some("settings"),
            Self::ProxyPools | Self::ConsoleLog => None,
        }
    }

    /// The views the page module declares (the twin test, T070, walks these).
    pub const fn views(self) -> &'static [ViewName] {
        match self {
            Self::Endpoint => endpoint::VIEWS,
            Self::Providers => providers::VIEWS,
            Self::Combo => combo::VIEWS,
            Self::Usage => usage::VIEWS,
            Self::Quota => quota::VIEWS,
            Self::ProxyPools => proxy_pools::VIEWS,
            Self::ConsoleLog => console_log::VIEWS,
            Self::Settings => settings::VIEWS,
        }
    }
}

/// A page request: the route, the window it opens (a provider or a record id), and the query.
#[derive(Debug, Clone)]
pub struct Req {
    pub id: Id,
    /// `/providers/<id>` or `/usage/records/<id>`: the window open on top of the page.
    pub window: Option<String>,
    pub query: Vec<(String, String)>,
}

impl Req {
    /// The route for `path`, or `None` for a path no page answers.
    pub fn parse(path: &str, query: &str) -> Option<Self> {
        let query = access::pairs(query);
        let (id, window) = match path.trim_end_matches('/') {
            "/endpoint" => (Id::Endpoint, None),
            "/providers" => (Id::Providers, None),
            "/combo" => (Id::Combo, None),
            "/usage" => (Id::Usage, None),
            "/quota" => (Id::Quota, None),
            "/proxy-pools" => (Id::ProxyPools, None),
            "/console-log" => (Id::ConsoleLog, None),
            "/settings" => (Id::Settings, None),
            p => {
                let (id, rest) = if let Some(rest) = p.strip_prefix("/providers/") {
                    (Id::Providers, rest)
                } else if let Some(rest) = p.strip_prefix("/usage/records/") {
                    (Id::Usage, rest)
                } else {
                    return None;
                };
                let rest = access::decode_component(rest);
                if rest.is_empty() || rest.contains('/') {
                    return None;
                }
                (id, Some(rest))
            }
        };
        Some(Self { id, window, query })
    }

    /// The first value of `name` in the query, when it isn't empty.
    pub fn get(&self, name: &str) -> Option<&str> {
        access::field(&self.query, name).filter(|v| !v.is_empty())
    }

    /// Whether the housekeeping panel is open (`?notices`).
    pub fn notices_open(&self) -> bool {
        self.query.iter().any(|(k, _)| k == "notices")
    }

    /// The page's own path (without the window).
    pub fn base(&self) -> &'static str {
        self.id.path()
    }

    /// `path` with this request's query, minus `drop` and plus `set` (an empty value is written
    /// as a bare name, as `?notices` is).
    pub fn href(&self, path: &str, drop: &[&str], set: &[(&str, &str)]) -> String {
        let mut parts: Vec<String> = self
            .query
            .iter()
            .filter(|(k, _)| !drop.contains(&k.as_str()) && !set.iter().any(|(s, _)| s == k))
            .map(|(k, v)| pair(k, v))
            .collect();
        parts.extend(set.iter().map(|(k, v)| pair(k, v)));
        if parts.is_empty() { path.to_owned() } else { format!("{path}?{}", parts.join("&")) }
    }

    /// Where the open window's close link goes: the page with the same query.
    pub fn close_href(&self) -> String {
        self.href(self.base(), &[], &[])
    }

    /// The round button's link: the same address with `notices` toggled.
    pub fn toggle_notices_href(&self, path: &str) -> String {
        if self.notices_open() { self.href(path, &["notices"], &[]) } else { self.href(path, &[], &[("notices", "")]) }
    }

    /// The path this request was for, window included.
    pub fn path(&self) -> String {
        match (&self.window, self.id) {
            (Some(w), Id::Providers) => format!("/providers/{}", access::encode_component(w)),
            (Some(w), Id::Usage) => format!("/usage/records/{}", access::encode_component(w)),
            _ => self.base().to_owned(),
        }
    }
}

fn pair(k: &str, v: &str) -> String {
    if v.is_empty() {
        access::encode_component(k)
    } else {
        format!("{}={}", access::encode_component(k), access::encode_component(v))
    }
}

/// What a page puts in the frame.
#[derive(Default)]
pub struct Body {
    pub content: Markup,
    /// The right side panel (Endpoint & Key, Providers).
    pub side: Option<Markup>,
    /// The window open on top (`/providers/<id>`, `/usage/records/<id>`).
    pub window: Option<Markup>,
}

impl Body {
    pub fn new(content: Markup) -> Self {
        Self { content, side: None, window: None }
    }
}

/// A page that can't be shown: a window whose id doesn't exist (404, the CLI's message).
#[derive(Debug, Clone)]
pub struct Failure {
    pub status: StatusCode,
    pub message: String,
}

impl Failure {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self { status: StatusCode::NOT_FOUND, message: message.into() }
    }
}

/// What a page module's `body` reads from.
pub struct Ctx<'a> {
    pub req: &'a Req,
    pub page: &'a Page,
    pub tz: &'a TimeZone,
    /// Provider logo addresses (not a view: the bytes are the registry's, research R10).
    pub logos: &'a crate::logos::Index,
}

impl Ctx<'_> {
    /// The JSON of the first view fetched under `name`; `Null` when the page didn't ask for it.
    pub fn json(&self, name: ViewName) -> &Value {
        self.page.view(name).map_or(&Value::Null, |v| &v.json)
    }

    /// The `extra` of the first view fetched under `name`.
    pub fn extra(&self, name: ViewName) -> &Value {
        self.page.view(name).map_or(&Value::Null, |v| &v.extra)
    }

    /// Every fetched view named `name`, with its arguments (`model` is fetched once per model).
    pub fn all(&self, name: ViewName) -> impl Iterator<Item = (&Value, &Value)> {
        self.page.views.iter().filter(move |f| f.view == name).map(|f| (&f.args, &f.value.json))
    }

    /// An instant as `<time>` in this machine's zone.
    pub fn time(&self, rfc3339: &str) -> Markup {
        crate::time::time_element(rfc3339, self.tz)
    }
}

/// The views a request needs: the page's own, then `check` for its notices and, with the panel
/// open, `check` and `accounts` for the panel.
pub fn wants(req: &Req) -> Vec<Want> {
    let mut wants = match req.id {
        Id::Endpoint => endpoint::wants(req),
        Id::Providers => providers::wants(req),
        Id::Combo => combo::wants(req),
        Id::Usage => usage::wants(req),
        Id::Quota => quota::wants(req),
        Id::ProxyPools => proxy_pools::wants(req),
        Id::ConsoleLog => console_log::wants(req),
        Id::Settings => settings::wants(req),
    };
    let has = |w: &[Want], v: ViewName| w.iter().any(|x| x.view == v && x.args == serde_json::json!({}));
    if (req.id.subject().is_some() || req.notices_open()) && !has(&wants, ViewName::Check) {
        wants.push(Want::new(ViewName::Check, serde_json::json!({})));
    }
    if req.notices_open() && !has(&wants, ViewName::Accounts) {
        wants.push(Want::new(ViewName::Accounts, serde_json::json!({})));
    }
    wants
}

/// The page's body from its views.
pub fn body(ctx: &Ctx<'_>) -> Result<Body, Failure> {
    match ctx.req.id {
        Id::Endpoint => endpoint::body(ctx),
        Id::Providers => providers::body(ctx),
        Id::Combo => combo::body(ctx),
        Id::Usage => usage::body(ctx),
        Id::Quota => quota::body(ctx),
        Id::ProxyPools => proxy_pools::body(ctx),
        Id::ConsoleLog => console_log::body(ctx),
        Id::Settings => settings::body(ctx),
    }
}

/// The `check` notices whose subject is `subject`, as `(level, text)`.
pub fn notices_for<'a>(check: &'a Value, subject: &str) -> Vec<(&'a str, &'a str)> {
    check["notices"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| n["subject"] == subject)
        .map(|n| (n["level"].as_str().unwrap_or("note"), n["text"].as_str().unwrap_or_default()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_and_windows_parse() {
        let r = Req::parse("/providers/anthropic", "kind=chat&notices").unwrap();
        assert_eq!((r.id, r.window.as_deref()), (Id::Providers, Some("anthropic")));
        assert_eq!(r.get("kind"), Some("chat"));
        assert!(r.notices_open());
        let r = Req::parse("/usage/records/rq_01", "").unwrap();
        assert_eq!((r.id, r.window.as_deref()), (Id::Usage, Some("rq_01")));
        assert!(Req::parse("/usage/records/", "").is_none());
        assert!(Req::parse("/providers/a/b", "").is_none());
        assert!(Req::parse("/nope", "").is_none());
        assert_eq!(Req::parse("/quota/", "").unwrap().id, Id::Quota);
    }

    #[test]
    fn the_round_button_toggles_notices_and_keeps_the_rest() {
        let r = Req::parse("/quota", "provider=xai").unwrap();
        assert_eq!(r.toggle_notices_href("/quota"), "/quota?provider=xai&notices");
        let open = Req::parse("/quota", "provider=xai&notices").unwrap();
        assert_eq!(open.toggle_notices_href("/quota"), "/quota?provider=xai");
        assert_eq!(open.close_href(), "/quota?provider=xai&notices");
    }

    #[test]
    fn query_values_are_encoded() {
        let r = Req::parse("/providers", "q=a%26b").unwrap();
        assert_eq!(r.get("q"), Some("a&b"));
        assert_eq!(r.href("/providers", &[], &[]), "/providers?q=a%26b");
    }

    #[test]
    fn every_page_but_two_has_a_subject_and_the_sidebar_has_eight_entries() {
        assert_eq!(Id::ALL.len(), 8);
        assert_eq!(Id::ALL.iter().filter(|i| i.subject().is_none()).count(), 2);
        assert_eq!(Id::ALL.iter().position(|i| *i == Id::SYSTEM), Some(5));
    }
}
