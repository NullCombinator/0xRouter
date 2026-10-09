//! Provider accounts (`accounts.toml`, data-model § ProviderAccount).
//!
//! A secret is bound to the hosts its provider sent traffic to when the account was added.
//! If a replacing plugin points the provider elsewhere, the secret is withheld until the
//! operator adds the account again (slice 002 FR-012a rule, applied to operator secrets).
//!
//! Schema 2 (spec 005, research R5) adds sign-in accounts: `kind = "signin"`, no secret in
//! this file; the tokens and the hosts they are bound to live in `tokens.toml`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use indexmap::IndexMap;
use nullrouter_registry::schema::{
    PartialTokenWeights, Percent, check_capacity, check_length, check_lifetime, check_multiplier, check_price_value,
    check_reserve, check_weight, check_weights_unit, parse_duration,
};
use nullrouter_registry::{ProviderEntity, Registry, SecretString};
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::files::{self, FileError};
use crate::records::ErrorClass;
use crate::tokens::{AccountState, TokenCells, TokenView};

pub const FILE: &str = "accounts.toml";
/// The schema `to_toml` writes. Schema 1 still loads (every account a `key` account).
pub const SCHEMA: u32 = 2;
/// How often an account's quota is polled when it sets no interval (research R11).
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(600);
/// The shortest polling interval; a shorter `poll_interval` is raised to it.
pub const MIN_POLL_INTERVAL: Duration = Duration::from_secs(120);

/// What an account authenticates with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// An API key from `secret` (slice 003).
    #[default]
    Key,
    /// Tokens from `nullrouter accounts signin`, kept in `tokens.toml`.
    Signin,
}

/// Where a secret comes from. Only the source is ever shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    Literal,
    Env(String),
}

/// The priority an account has unless the operator sets one.
pub const DEFAULT_PRIORITY: f64 = 1.0;

/// An account's routing overrides (`[account.routing]`, research R13): each beats the plugin's
/// declaration for this account only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoutingOverrides {
    /// How long an idle prefix stays warm on this account.
    pub cache_lifetime: Option<Duration>,
    /// The reserve floor of every window.
    pub reserve: Option<Percent>,
    /// A flat price that replaces the plugin's whole schedule.
    pub price: Option<PriceOverride>,
    /// Per window name: its capacity, length or reserve.
    pub window: BTreeMap<String, WindowOverride>,
}

/// A flat price per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriceOverride {
    pub input: f64,
    pub output: Option<f64>,
    pub cache_read: Option<f64>,
    pub cache_write: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowOverride {
    pub capacity: Option<f64>,
    pub length: Option<Duration>,
    pub reserve: Option<Percent>,
    /// Replaces the declared weights of the classes set (a `requests` window has none).
    pub token_weights: Option<PartialTokenWeights>,
    /// Model glob → factor. Each glob must be one the window's declaration has.
    pub model_multiplier: IndexMap<String, f64>,
}

impl RoutingOverrides {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The first value that breaks a gate rule, worded as the gate words it. The same rules as a
    /// plugin's `[routing]` (`nullrouter_registry::schema`).
    pub fn problem(&self) -> Option<String> {
        let mut found: Vec<String> = Vec::new();
        let mut note = |at: &str, r: Result<(), String>| {
            if let Err(e) = r {
                found.push(format!("{at}: {e}"));
            }
        };
        if let Some(d) = self.cache_lifetime {
            note("cache_lifetime", check_lifetime(d));
        }
        if let Some(p) = self.reserve {
            note("reserve", check_reserve(p));
        }
        if let Some(p) = &self.price {
            for (k, v) in [
                ("input", Some(p.input)),
                ("output", p.output),
                ("cache_read", p.cache_read),
                ("cache_write", p.cache_write),
            ] {
                if let Some(v) = v {
                    note(&format!("price.{k}"), check_price_value(v));
                }
            }
        }
        for (name, w) in &self.window {
            if let Some(c) = w.capacity {
                note(&format!("window.{name}.capacity"), check_capacity(c));
            }
            if let Some(l) = w.length {
                note(&format!("window.{name}.length"), check_length(l));
            }
            if let Some(r) = w.reserve {
                note(&format!("window.{name}.reserve"), check_reserve(r));
            }
            if let Some(tw) = &w.token_weights {
                for (k, v) in tw.classes() {
                    if let Some(v) = v {
                        note(&format!("window.{name}.token_weights"), check_weight(k, v));
                    }
                }
            }
            for (glob, f) in &w.model_multiplier {
                note(&format!("window.{name}.model_multiplier"), check_multiplier(glob, *f));
            }
        }
        found.into_iter().next()
    }

    /// Names of overridden windows that the provider's declaration doesn't have.
    pub fn unknown_windows<'a>(&'a self, declared: &[nullrouter_registry::schema::MeterDecl]) -> Vec<&'a str> {
        self.window.keys().filter(|n| !declared.iter().any(|d| d.name == **n)).map(String::as_str).collect()
    }

    /// The first refusal that needs the provider's declared windows: `token_weights` on a window
    /// whose unit isn't `weighted_tokens`, or a `model_multiplier` glob the window doesn't declare.
    /// Worded as the gate words the plugin's own declaration. Overridden windows the declaration
    /// doesn't have are left to [`Self::unknown_windows`].
    pub fn meter_problem(&self, provider: &str, declared: &[nullrouter_registry::schema::MeterDecl]) -> Option<String> {
        for (name, w) in &self.window {
            let Some(d) = declared.iter().find(|d| d.name == *name) else { continue };
            if w.token_weights.is_some()
                && let Err(e) = check_weights_unit(d.unit)
            {
                return Some(format!("routing.window.{name}.token_weights: {e}"));
            }
            if let Some(glob) = w.model_multiplier.keys().find(|g| !d.model_multiplier.contains_key(*g)) {
                return Some(format!(
                    "routing.window.{name}.model_multiplier: {provider} declares no glob {glob:?} for window {name}"
                ));
            }
        }
        None
    }
}

#[derive(Debug)]
pub struct Account {
    pub provider: String,
    pub name: String,
    pub kind: AccountKind,
    /// Where a `key` account's secret comes from. `Literal` (with no secret) for a
    /// sign-in account, which has none here.
    pub source: SecretSource,
    /// `None` when the source is an unset environment variable, and for sign-in accounts.
    pub secret: Option<SecretString>,
    pub order: i64,
    pub disabled: bool,
    /// The quota polling interval as written; see [`Account::poll_interval`].
    pub poll_interval: Option<Duration>,
    /// Routing weight multiplier: a number of at least 0, default 1. 0 means never cold work.
    pub priority: f64,
    /// Overrides of the plugin's `[routing]` declaration for this account.
    pub routing: RoutingOverrides,
    /// Hosts a `key` account's secret may be sent to. Empty in a hand-written file: bound
    /// to the provider's hosts at load. Always empty for a sign-in account: its tokens
    /// carry their own hosts.
    pub hosts: BTreeSet<String>,
    /// A proxy name from `proxies.toml`, or `"none"` for a direct connection whatever the
    /// provider or `[connection]` says. `None`: inherit.
    pub proxy: Option<String>,
    /// FR-023: from when the operator declared this account in exclusive use (no traffic
    /// from 0router). `None`: not declared. Set only where the provider reports quota.
    pub exclusive_use: Option<SystemTime>,
}

impl Account {
    /// A `key` account (slice 003 shape), for callers that build one.
    pub fn key(
        provider: impl Into<String>,
        name: impl Into<String>,
        source: SecretSource,
        secret: Option<SecretString>,
        order: i64,
        hosts: BTreeSet<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            name: name.into(),
            kind: AccountKind::Key,
            source,
            secret,
            order,
            disabled: false,
            poll_interval: None,
            priority: DEFAULT_PRIORITY,
            routing: RoutingOverrides::default(),
            hosts,
            proxy: None,
            exclusive_use: None,
        }
    }

    /// A sign-in account; its tokens are written to `tokens.toml` separately.
    pub fn signin(provider: impl Into<String>, name: impl Into<String>, order: i64) -> Self {
        Self {
            kind: AccountKind::Signin,
            ..Self::key(provider, name, SecretSource::Literal, None, order, BTreeSet::new())
        }
    }

    pub fn is_signin(&self) -> bool {
        self.kind == AccountKind::Signin
    }

    /// The refusal for exclusive use on this account (FR-023), or `None` when `provider`
    /// reports quota for it. Load checks accounts that carry a declaration; the CLI checks
    /// before it sets one.
    pub fn exclusive_use_problem(&self, provider: &ProviderEntity) -> Option<String> {
        crate::quota::poll::reported(provider, self).is_none().then(|| {
            format!(
                "account {}/{}: exclusive use needs quota polls; {} reports no quota for this account",
                self.provider, self.name, self.provider
            )
        })
    }

    /// The effective quota polling interval: the account's own (never below
    /// [`MIN_POLL_INTERVAL`]), else [`DEFAULT_POLL_INTERVAL`].
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval.map_or(DEFAULT_POLL_INTERVAL, |d| d.max(MIN_POLL_INTERVAL))
    }

    /// `…last4` for a literal, `env:VAR` otherwise; for listings.
    pub fn shown_secret(&self) -> String {
        match (&self.source, &self.secret) {
            (SecretSource::Env(v), _) => format!("env:{v}"),
            (SecretSource::Literal, Some(s)) if s.len() >= 8 => s.with_exposed(|s| format!("…{}", last4(s))),
            (SecretSource::Literal, _) => "…".to_owned(),
        }
    }
}

/// Why a secret isn't released for a request.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Withheld {
    #[error("environment variable {0} is not set")]
    EnvUnset(String),
    #[error(
        "the active plugin sends traffic to {host}, which this account was not added for; add the account again to confirm"
    )]
    NewHost { host: String },
    /// A sign-in account whose tokens don't cover a host the active plugin sends to.
    #[error(
        "the active plugin sends traffic to {host}, which this account's sign-in did not cover; run nullrouter accounts signin {provider} {name}"
    )]
    NewTokenHost { host: String, provider: String, name: String },
    /// No tokens, or a permanent refresh failure (research R10).
    #[error("needs sign-in: run nullrouter accounts signin {provider} {name}")]
    NeedsSignIn { provider: String, name: String },
    /// The provider refused a fresh token's request (FR-004b).
    #[error("refused by provider: {reason}; run nullrouter accounts signin {provider} {name}")]
    Refused { provider: String, name: String, reason: String },
    /// The access token is past its expiry and the refresh is being retried.
    #[error("token expired, refresh retrying")]
    Refreshing,
}

impl Withheld {
    /// The record class of an out-of-service sign-in account; `None` for the other
    /// reasons.
    pub fn class(&self) -> Option<ErrorClass> {
        match self {
            Self::NeedsSignIn { .. } => Some(ErrorClass::NeedsSignIn),
            Self::Refused { .. } => Some(ErrorClass::Refused),
            Self::Refreshing => Some(ErrorClass::TokenRefreshing),
            Self::EnvUnset(_) | Self::NewHost { .. } | Self::NewTokenHost { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AccountError {
    #[error("account name {0:?} must be 1–32 characters of a-z, 0-9, _ and -")]
    BadName(String),
    #[error("provider {provider} already has an account named {name}")]
    Duplicate { provider: String, name: String },
    #[error("provider {provider} has no account named {name}")]
    NotFound { provider: String, name: String },
    #[error("priority must be a number of 0 or more")]
    BadPriority,
}

#[derive(Debug, Default)]
pub struct Accounts {
    pub path: PathBuf,
    list: Vec<Account>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: u32,
    #[serde(default, rename = "account")]
    accounts: Vec<RawAccount>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAccount {
    provider: String,
    name: String,
    #[serde(default)]
    kind: AccountKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret: Option<RawSecret>,
    #[serde(default)]
    order: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    disabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    poll_interval: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    routing: Option<RawRouting>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hosts: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proxy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exclusive_use: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRouting {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_lifetime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    price: Option<RawPrice>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    window: BTreeMap<String, RawWindow>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPrice {
    input: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_read: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cache_write: Option<f64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWindow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capacity: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    length: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reserve: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_weights: Option<PartialTokenWeights>,
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    model_multiplier: IndexMap<String, f64>,
}

impl RawRouting {
    fn read(self) -> Result<RoutingOverrides, String> {
        let duration = |at: &str, v: &str| parse_duration(v).map_err(|e| format!("{at} {e}"));
        let percent = |at: &str, v: &str| Percent::parse(v).map_err(|e| format!("{at} {e}"));
        let mut window = BTreeMap::new();
        for (name, w) in self.window {
            let at = |k: &str| format!("window.{name}.{k}");
            window.insert(
                name.clone(),
                WindowOverride {
                    capacity: w.capacity,
                    length: w.length.as_deref().map(|v| duration(&at("length"), v)).transpose()?,
                    reserve: w.reserve.as_deref().map(|v| percent(&at("reserve"), v)).transpose()?,
                    token_weights: w.token_weights,
                    model_multiplier: w.model_multiplier,
                },
            );
        }
        Ok(RoutingOverrides {
            cache_lifetime: self.cache_lifetime.as_deref().map(|v| duration("cache_lifetime", v)).transpose()?,
            reserve: self.reserve.as_deref().map(|v| percent("reserve", v)).transpose()?,
            price: self.price.map(|p| PriceOverride {
                input: p.input,
                output: p.output,
                cache_read: p.cache_read,
                cache_write: p.cache_write,
            }),
            window,
        })
    }

    fn write(o: &RoutingOverrides) -> Option<Self> {
        if o.is_empty() {
            return None;
        }
        Some(Self {
            cache_lifetime: o.cache_lifetime.map(format_duration),
            reserve: o.reserve.map(|p| p.to_string()),
            price: o.price.map(|p| RawPrice {
                input: p.input,
                output: p.output,
                cache_read: p.cache_read,
                cache_write: p.cache_write,
            }),
            window: o
                .window
                .iter()
                .map(|(n, w)| {
                    let raw = RawWindow {
                        capacity: w.capacity,
                        length: w.length.map(format_duration),
                        reserve: w.reserve.map(|p| p.to_string()),
                        token_weights: w.token_weights,
                        model_multiplier: w.model_multiplier.clone(),
                    };
                    (n.clone(), raw)
                })
                .collect(),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum RawSecret {
    Literal(String),
    Env { env: String },
}

/// The last four characters, for listings.
pub fn last4(s: &str) -> String {
    let skip = s.chars().count().saturating_sub(4);
    s.chars().skip(skip).collect()
}

pub fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Every host a provider sends requests to: its endpoints of every type, their token-count
/// URLs, and any schema-1 transports.
pub fn provider_hosts(p: &ProviderEntity) -> BTreeSet<String> {
    let urls = p
        .endpoints
        .values()
        .flat_map(|e| e.0.iter())
        .flat_map(|e| std::iter::once(e.url.as_str()).chain(e.token_count.iter().map(|t| t.url.as_str())));
    let transports = p.all_transports();
    let bases =
        transports.iter().flat_map(|t| t.base_url.iter().chain(t.base_urls.iter().flatten())).map(String::as_str);
    urls.chain(bases).filter_map(|u| Url::parse(u).ok()?.host_str().map(str::to_owned)).collect()
}

impl Accounts {
    /// Reads `path`; a missing file is no accounts.
    pub fn load(path: &Path) -> Result<Self, FileError> {
        match files::read_private(path)? {
            None => Ok(Self { path: path.to_owned(), list: Vec::new() }),
            Some(text) => Self::parse(&text, path, |v| std::env::var(v).ok()),
        }
    }

    /// Parses schema 1 (every account a `key` account) or schema 2.
    pub fn parse(text: &str, path: &Path, env: impl Fn(&str) -> Option<String>) -> Result<Self, FileError> {
        let raw: RawFile = toml::from_str(text).map_err(|e| FileError::toml(path, text, &e))?;
        if !matches!(raw.schema, 1 | SCHEMA) {
            return Err(FileError::invalid(
                path,
                format!("schema {} is not supported (expected 1 or {SCHEMA})", raw.schema),
            ));
        }
        let mut list: Vec<Account> = Vec::new();
        for a in raw.accounts {
            let who = format!("account {}/{}", a.provider, a.name);
            if !valid_name(&a.name) {
                return Err(FileError::invalid(path, AccountError::BadName(a.name).to_string()));
            }
            if list.iter().any(|b| b.provider == a.provider && b.name == a.name) {
                return Err(FileError::invalid(
                    path,
                    AccountError::Duplicate { provider: a.provider, name: a.name }.to_string(),
                ));
            }
            if raw.schema == 1
                && (a.kind != AccountKind::Key
                    || a.poll_interval.is_some()
                    || a.priority.is_some()
                    || a.routing.is_some()
                    || a.proxy.is_some()
                    || a.exclusive_use.is_some())
            {
                return Err(FileError::invalid(
                    path,
                    format!(
                        "{who}: `kind`, `poll_interval`, `priority`, `routing`, `proxy` and `exclusive_use` need schema 2"
                    ),
                ));
            }
            let exclusive_use = a
                .exclusive_use
                .as_deref()
                .map(|v| {
                    crate::clock::parse_rfc3339(v)
                        .ok_or_else(|| format!("{who}: exclusive_use must be an RFC 3339 time, not {v:?}"))
                })
                .transpose()
                .map_err(|e| FileError::invalid(path, e))?;
            let proxy = a.proxy;
            if let Some(p) = &proxy
                && p != crate::connection::proxy::NONE
                && !crate::connection::proxy::valid_name(p)
            {
                return Err(FileError::invalid(path, format!("{who}: proxy {p:?} is not a proxy name or \"none\"")));
            }
            let priority = a.priority.unwrap_or(DEFAULT_PRIORITY);
            if !priority.is_finite() || priority < 0.0 {
                return Err(FileError::invalid(path, format!("{who}: priority must be 0 or more")));
            }
            let routing = a
                .routing
                .map(RawRouting::read)
                .transpose()
                .map_err(|e| FileError::invalid(path, format!("{who}: routing.{e}")))?
                .unwrap_or_default();
            if let Some(problem) = routing.problem() {
                return Err(FileError::invalid(path, format!("{who}: routing.{problem}")));
            }
            let poll_interval = a
                .poll_interval
                .as_deref()
                .map(parse_duration)
                .transpose()
                .map_err(|e| FileError::invalid(path, format!("{who}: poll_interval {e}")))?;
            let (source, secret) = match (a.kind, a.secret) {
                (AccountKind::Key, None) => {
                    return Err(FileError::invalid(path, format!("{who}: a key account needs a `secret`")));
                }
                (AccountKind::Key, Some(RawSecret::Literal(s))) => (SecretSource::Literal, Some(SecretString::new(s))),
                (AccountKind::Key, Some(RawSecret::Env { env: var })) => {
                    (SecretSource::Env(var.clone()), env(&var).filter(|s| !s.is_empty()).map(SecretString::new))
                }
                (AccountKind::Signin, Some(_)) => {
                    return Err(FileError::invalid(
                        path,
                        format!("{who}: a sign-in account has no `secret`; its tokens live in tokens.toml"),
                    ));
                }
                (AccountKind::Signin, None) if !a.hosts.is_empty() => {
                    return Err(FileError::invalid(
                        path,
                        format!("{who}: a sign-in account's hosts are kept with its tokens, not in `hosts`"),
                    ));
                }
                (AccountKind::Signin, None) => (SecretSource::Literal, None),
            };
            list.push(Account {
                provider: a.provider,
                name: a.name,
                kind: a.kind,
                source,
                secret,
                order: a.order,
                disabled: a.disabled,
                poll_interval,
                priority,
                routing,
                hosts: a.hosts.into_iter().collect(),
                proxy,
                exclusive_use,
            });
        }
        Ok(Self { path: path.to_owned(), list })
    }

    pub fn to_toml(&self) -> String {
        let accounts = self
            .list
            .iter()
            .map(|a| RawAccount {
                provider: a.provider.clone(),
                name: a.name.clone(),
                kind: a.kind,
                secret: match (&a.kind, &a.source) {
                    (AccountKind::Signin, _) => None,
                    (AccountKind::Key, SecretSource::Env(v)) => Some(RawSecret::Env { env: v.clone() }),
                    (AccountKind::Key, SecretSource::Literal) => Some(RawSecret::Literal(
                        a.secret.as_ref().map(|s| s.with_exposed(str::to_owned)).unwrap_or_default(),
                    )),
                },
                order: a.order,
                disabled: a.disabled,
                poll_interval: a.poll_interval.map(format_duration),
                priority: (a.priority != DEFAULT_PRIORITY).then_some(a.priority),
                routing: RawRouting::write(&a.routing),
                hosts: if a.is_signin() { Vec::new() } else { a.hosts.iter().cloned().collect() },
                proxy: a.proxy.clone(),
                exclusive_use: a.exclusive_use.map(crate::clock::rfc3339),
            })
            .collect();
        toml::to_string(&RawFile { schema: SCHEMA, accounts }).expect("accounts serialise")
    }

    pub fn save(&self) -> Result<(), FileError> {
        files::write_private(&self.path, &self.to_toml())
    }

    pub fn iter(&self) -> impl Iterator<Item = &Account> {
        self.list.iter()
    }

    /// A provider's enabled accounts in try order (then file order).
    pub fn for_provider<'a>(&'a self, provider: &'a str) -> impl Iterator<Item = &'a Account> {
        let mut v: Vec<&Account> = self.list.iter().filter(|a| a.provider == provider && !a.disabled).collect();
        v.sort_by_key(|a| a.order);
        v.into_iter()
    }

    pub fn get(&self, provider: &str, name: &str) -> Option<&Account> {
        self.list.iter().find(|a| a.provider == provider && a.name == name)
    }

    /// Accounts for providers that aren't loaded: reported, not an error.
    pub fn unused<'a>(&'a self, registry: &'a Registry) -> impl Iterator<Item = &'a Account> {
        self.list.iter().filter(|a| registry.provider(&a.provider).is_err())
    }

    /// Adds an account, or replaces one with the same name (which re-confirms its hosts).
    pub fn add(&mut self, mut account: Account) -> Result<(), AccountError> {
        if !valid_name(&account.name) {
            return Err(AccountError::BadName(account.name));
        }
        match self.list.iter_mut().find(|a| a.provider == account.provider && a.name == account.name) {
            Some(a) => {
                account.disabled = a.disabled;
                *a = account;
            }
            None => self.list.push(account),
        }
        Ok(())
    }

    pub fn remove(&mut self, provider: &str, name: &str) -> Result<Account, AccountError> {
        let at = self.position(provider, name)?;
        Ok(self.list.remove(at))
    }

    pub fn set_disabled(&mut self, provider: &str, name: &str, disabled: bool) -> Result<(), AccountError> {
        let at = self.position(provider, name)?;
        self.list[at].disabled = disabled;
        Ok(())
    }

    /// Sets the account's quota polling interval; `None` returns it to the default. Returns
    /// the interval the account now polls at (a shorter one is raised to the floor).
    pub fn set_poll_interval(
        &mut self,
        provider: &str,
        name: &str,
        every: Option<Duration>,
    ) -> Result<Duration, AccountError> {
        let at = self.position(provider, name)?;
        self.list[at].poll_interval = every;
        Ok(self.list[at].poll_interval())
    }

    /// Sets the account's priority (0 = never cold work).
    pub fn set_priority(&mut self, provider: &str, name: &str, priority: f64) -> Result<(), AccountError> {
        if !priority.is_finite() || priority < 0.0 {
            return Err(AccountError::BadPriority);
        }
        let at = self.position(provider, name)?;
        self.list[at].priority = priority;
        Ok(())
    }

    /// Sets (`Some`) or clears (`None`) the account's proxy assignment.
    pub fn set_proxy(&mut self, provider: &str, name: &str, proxy: Option<String>) -> Result<(), AccountError> {
        let at = self.position(provider, name)?;
        self.list[at].proxy = proxy;
        Ok(())
    }

    /// Replaces the account's routing overrides.
    pub fn set_routing(&mut self, provider: &str, name: &str, routing: RoutingOverrides) -> Result<(), AccountError> {
        let at = self.position(provider, name)?;
        self.list[at].routing = routing;
        Ok(())
    }

    /// Refuses an override naming a window its provider doesn't declare. Providers that aren't
    /// loaded are skipped (they are reported as unused accounts).
    pub fn check_routing(&self, registry: &Registry) -> Result<(), FileError> {
        for a in &self.list {
            let Ok(p) = registry.provider(&a.provider) else { continue };
            let declared = p.routing();
            if let Some(name) = a.routing.unknown_windows(declared.windows).first() {
                return Err(FileError::invalid(
                    &self.path,
                    format!(
                        "account {}/{}: routing.window.{name}: {} declares no window with that name",
                        a.provider, a.name, a.provider
                    ),
                ));
            }
            if let Some(problem) = a.routing.meter_problem(&a.provider, declared.windows) {
                let who = format!("account {}/{}", a.provider, a.name);
                return Err(FileError::invalid(&self.path, format!("{who}: {problem}")));
            }
            if a.exclusive_use.is_some()
                && let Some(problem) = a.exclusive_use_problem(p)
            {
                return Err(FileError::invalid(&self.path, problem));
            }
        }
        Ok(())
    }

    fn position(&self, provider: &str, name: &str) -> Result<usize, AccountError> {
        self.list
            .iter()
            .position(|a| a.provider == provider && a.name == name)
            .ok_or_else(|| AccountError::NotFound { provider: provider.to_owned(), name: name.to_owned() })
    }

    /// Fills in the hosts of hand-written accounts from the loaded providers. Returns
    /// whether any account was bound, so the caller saves the binding: a later plugin that
    /// moves the provider to other hosts must find it.
    pub fn bind_unbound(&mut self, registry: &Registry) -> bool {
        let mut bound = false;
        for a in self.list.iter_mut().filter(|a| a.hosts.is_empty() && !a.is_signin()) {
            if let Ok(p) = registry.provider(&a.provider) {
                a.hosts = provider_hosts(p);
                bound |= !a.hosts.is_empty();
            }
        }
        bound
    }
}

/// `d` the way `parse_duration` reads it back: the largest whole unit.
fn format_duration(d: Duration) -> String {
    let (secs, ms) = (d.as_secs(), d.subsec_millis());
    match () {
        () if ms != 0 => format!("{}ms", d.as_millis()),
        () if secs != 0 && secs % 86_400 == 0 => format!("{}d", secs / 86_400),
        () if secs != 0 && secs % 3600 == 0 => format!("{}h", secs / 3600),
        () if secs != 0 && secs % 60 == 0 => format!("{}m", secs / 60),
        () => format!("{secs}s"),
    }
}

/// What [`release`] hands out: a key account's secret, or a sign-in account's current
/// token view (one atomic load; the view stays valid while held, even across a refresh).
#[derive(Debug)]
pub enum Released<'a> {
    Key(&'a SecretString),
    Token(Arc<TokenView>),
}

impl Released<'_> {
    /// The value for the auth header.
    pub fn secret(&self) -> &SecretString {
        match self {
            Self::Key(s) => s,
            Self::Token(v) => &v.entry.access_token,
        }
    }

    /// The token view, for a sign-in account (claims for `[identity]`).
    pub fn token(&self) -> Option<&TokenView> {
        match self {
            Self::Key(_) => None,
            Self::Token(v) => Some(v),
        }
    }
}

/// Why a sign-in account can't serve now, or `None` when it can (and for `key`
/// accounts). Disabled accounts aren't asked: the plan filters them silently.
pub fn out_of_service(account: &Account, tokens: &TokenCells) -> Option<Withheld> {
    if !account.is_signin() {
        return None;
    }
    view_out_of_service(account, tokens.get(&account.provider, &account.name).as_deref())
}

/// [`out_of_service`] for a sign-in account's view, already loaded.
fn view_out_of_service(account: &Account, view: Option<&TokenView>) -> Option<Withheld> {
    let needs = || Withheld::NeedsSignIn { provider: account.provider.clone(), name: account.name.clone() };
    let Some(view) = view else { return Some(needs()) };
    match &view.state {
        AccountState::Active | AccountState::Disabled => None,
        AccountState::NeedsSignIn { .. } => Some(needs()),
        AccountState::Refused { reason, .. } => Some(Withheld::Refused {
            provider: account.provider.clone(),
            name: account.name.clone(),
            reason: reason.clone(),
        }),
        // Research R10 (amended): only a token past its expiry stops serving.
        AccountState::Refreshing { .. } => view.expired(SystemTime::now()).then_some(Withheld::Refreshing),
    }
}

/// The secret for a request to `active`, if every host it sends to is one the account was
/// added for. A sign-in account's current access token is read from `tokens`, and is
/// released only while the account is in service and every host the active plugin sends
/// the token to (`token_hosts`) is one its sign-in was bound to (research R5, FR-031).
///
/// The token cell is read once (one atomic load), so the state checked and the token
/// released come from the same generation even while a refresh swaps the cell.
pub fn release<'a>(
    account: &'a Account,
    active: &ProviderEntity,
    tokens: &TokenCells,
) -> Result<Released<'a>, Withheld> {
    if account.is_signin() {
        let view = tokens.get(&account.provider, &account.name);
        if let Some(w) = view_out_of_service(account, view.as_deref()) {
            return Err(w);
        }
        let view = view
            .ok_or_else(|| Withheld::NeedsSignIn { provider: account.provider.clone(), name: account.name.clone() })?;
        return match active.token_hosts().into_iter().find(|h| !view.entry.hosts.contains(h)) {
            Some(host) => {
                Err(Withheld::NewTokenHost { host, provider: account.provider.clone(), name: account.name.clone() })
            }
            None => Ok(Released::Token(view)),
        };
    }
    let secret = account.secret.as_ref().ok_or_else(|| match &account.source {
        SecretSource::Env(v) => Withheld::EnvUnset(v.clone()),
        SecretSource::Literal => Withheld::EnvUnset(String::new()),
    })?;
    match provider_hosts(active).into_iter().find(|h| !account.hosts.contains(h)) {
        Some(host) => Err(Withheld::NewHost { host }),
        None => Ok(Released::Key(secret)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE_TEXT: &str = r#"
schema = 1
[[account]]
provider = "anthropic"
name = "backup"
secret = "sk-ant-backup-000000"
order = 1
[[account]]
provider = "anthropic"
name = "main"
secret = { env = "NR_TEST_MAIN" }
hosts = ["api.anthropic.com"]
[[account]]
provider = "ghost"
name = "x"
secret = "sk-ghost-00000000"
disabled = true
"#;

    fn parse() -> Accounts {
        Accounts::parse(FILE_TEXT, Path::new("accounts.toml"), |v| {
            (v == "NR_TEST_MAIN").then(|| "sk-from-env-1234".to_owned())
        })
        .unwrap()
    }

    #[test]
    fn parses_orders_and_hides_secrets() {
        let a = parse();
        let names: Vec<&str> = a.for_provider("anthropic").map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["main", "backup"]);
        let main = a.get("anthropic", "main").unwrap();
        assert!(main.secret.as_ref().unwrap().matches("sk-from-env-1234"));
        assert_eq!(main.shown_secret(), "env:NR_TEST_MAIN");
        assert_eq!(a.get("anthropic", "backup").unwrap().shown_secret(), "…0000");
        assert!(!format!("{a:?}").contains("sk-ant-backup"));
        assert_eq!(a.for_provider("ghost").count(), 0, "disabled accounts are skipped");
    }

    #[test]
    fn round_trips_without_resolving_env() {
        let a = parse();
        let text = a.to_toml();
        assert!(text.contains("env = \"NR_TEST_MAIN\"") && !text.contains("sk-from-env"));
        let b = Accounts::parse(&text, Path::new("accounts.toml"), |_| None).unwrap();
        assert_eq!(b.iter().count(), 3);
        assert!(b.get("anthropic", "main").unwrap().secret.is_none());
        assert!(b.get("anthropic", "backup").unwrap().secret.as_ref().unwrap().matches("sk-ant-backup-000000"));
    }

    #[test]
    fn rejects_bad_names_duplicates_and_schemas() {
        let p = Path::new("accounts.toml");
        let dup = "schema = 1\n[[account]]\nprovider=\"a\"\nname=\"m\"\nsecret=\"s\"\n[[account]]\nprovider=\"a\"\nname=\"m\"\nsecret=\"t\"\n";
        assert!(Accounts::parse(dup, p, |_| None).unwrap_err().to_string().contains("already has"));
        let bad = "schema = 1\n[[account]]\nprovider=\"a\"\nname=\"Main!\"\nsecret=\"s\"\n";
        assert!(Accounts::parse(bad, p, |_| None).is_err());
        assert!(Accounts::parse("schema = 3\n", p, |_| None).is_err());
        assert!(valid_name("main_2-b") && !valid_name("") && !valid_name(&"a".repeat(33)));
    }

    #[test]
    fn add_replace_disable_remove() {
        let mut a = parse();
        let acct = |name: &str| {
            Account::key(
                "anthropic",
                name,
                SecretSource::Literal,
                Some(SecretString::new("sk-new-000000000")),
                5,
                BTreeSet::new(),
            )
        };
        assert_eq!(a.add(acct("Bad Name")), Err(AccountError::BadName("Bad Name".into())));
        a.set_disabled("anthropic", "backup", true).unwrap();
        a.add(acct("backup")).unwrap();
        let b = a.get("anthropic", "backup").unwrap();
        assert!(b.disabled && b.secret.as_ref().unwrap().matches("sk-new-000000000"));
        a.remove("anthropic", "backup").unwrap();
        assert!(matches!(a.remove("anthropic", "backup"), Err(AccountError::NotFound { .. })));
    }

    const SCHEMA2: &str = r#"
schema = 2
[[account]]
provider = "grok-cli"
name = "work"
kind = "signin"
order = 0
poll_interval = "15m"
[[account]]
provider = "opencode-go"
name = "main"
kind = "key"
secret = { env = "OPENCODE_GO_KEY" }
[[account]]
provider = "xai"
name = "fast"
kind = "signin"
poll_interval = "30s"
"#;

    #[test]
    fn schema_2_carries_kinds_and_poll_intervals() {
        let a = Accounts::parse(SCHEMA2, Path::new("accounts.toml"), |_| None).unwrap();
        let work = a.get("grok-cli", "work").unwrap();
        assert!(work.is_signin() && work.secret.is_none() && work.hosts.is_empty());
        assert_eq!(work.poll_interval(), Duration::from_secs(900));
        assert_eq!(a.get("xai", "fast").unwrap().poll_interval(), MIN_POLL_INTERVAL, "raised to the floor");
        let main = a.get("opencode-go", "main").unwrap();
        assert_eq!((main.kind, main.poll_interval()), (AccountKind::Key, DEFAULT_POLL_INTERVAL));

        let text = a.to_toml();
        assert!(text.starts_with("schema = 2\n"), "{text}");
        assert!(text.contains("poll_interval = \"15m\"") && text.contains("kind = \"signin\""), "{text}");
        let b = Accounts::parse(&text, Path::new("accounts.toml"), |_| None).unwrap();
        assert_eq!(b.get("grok-cli", "work").unwrap().poll_interval, Some(Duration::from_secs(900)));
        assert!(b.get("xai", "fast").unwrap().is_signin());
    }

    #[test]
    fn schema_1_loads_as_key_accounts_and_saves_as_2() {
        let a = parse();
        assert!(a.iter().all(|a| a.kind == AccountKind::Key));
        let text = a.to_toml();
        assert!(text.starts_with("schema = 2\n") && text.contains("kind = \"key\""), "{text}");
    }

    #[test]
    fn kind_rules() {
        let p = Path::new("accounts.toml");
        let cases = [
            (
                "schema = 2\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"signin\"\nsecret=\"sk-0000000000\"\n",
                "has no `secret`",
            ),
            ("schema = 2\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"key\"\n", "needs a `secret`"),
            (
                "schema = 2\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"signin\"\nhosts=[\"h\"]\n",
                "kept with its tokens",
            ),
            ("schema = 1\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"signin\"\n", "need schema 2"),
            (
                "schema = 2\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"signin\"\npoll_interval=\"soon\"\n",
                "poll_interval",
            ),
            ("schema = 2\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"oauth\"\n", "unknown variant"),
        ];
        for (text, want) in cases {
            let err = Accounts::parse(text, p, |_| None).unwrap_err().to_string();
            assert!(err.contains(want), "{want}: {err}");
        }
        assert_eq!(format_duration(Duration::from_secs(7200)), "2h");
        assert_eq!(format_duration(Duration::from_secs(90)), "90s");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1500ms");
    }

    const ROUTING: &str = r#"
schema = 2
[[account]]
provider = "anthropic"
name = "max"
kind = "signin"
priority = 2.5
[account.routing]
cache_lifetime = "1h"
reserve = "10%"
price = { input = 3.0, output = 15.0 }
window."5-hour" = { capacity = 12000000.0, length = "5h", reserve = "8%" }
[[account]]
provider = "anthropic"
name = "pro"
kind = "signin"
"#;

    #[test]
    fn priority_and_routing_overrides_default_round_trip_and_stay_out_of_the_file() {
        let p = Path::new("accounts.toml");
        let a = Accounts::parse(ROUTING, p, |_| None).unwrap();
        let max = a.get("anthropic", "max").unwrap();
        assert_eq!(max.priority, 2.5);
        assert_eq!(max.routing.cache_lifetime, Some(Duration::from_secs(3600)));
        assert_eq!(max.routing.reserve, Some(Percent(10.0)));
        assert_eq!(max.routing.price.unwrap().output, Some(15.0));
        let w = &max.routing.window["5-hour"];
        assert_eq!(
            (w.capacity, w.length, w.reserve),
            (Some(12_000_000.0), Some(Duration::from_secs(5 * 3600)), Some(Percent(8.0)))
        );
        let pro = a.get("anthropic", "pro").unwrap();
        assert_eq!(pro.priority, DEFAULT_PRIORITY);
        assert!(pro.routing.is_empty());

        let text = a.to_toml();
        let b = Accounts::parse(&text, p, |_| None).unwrap();
        assert_eq!(b.get("anthropic", "max").unwrap().routing, max.routing);
        assert_eq!(b.get("anthropic", "max").unwrap().priority, 2.5);
        // Keys are written only when set.
        let pro_part = text.split("[[account]]").last().unwrap();
        assert!(!pro_part.contains("priority") && !pro_part.contains("routing"), "{text}");
    }

    #[test]
    fn priority_and_routing_refusals() {
        let p = Path::new("accounts.toml");
        let head = "schema = 2\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"signin\"\n";
        let cases = [
            ("priority = -1\n", "priority must be 0 or more"),
            ("[account.routing]\ncache_lifetime = \"25h\"\n", "at most 24h"),
            ("[account.routing]\nreserve = \"60%\"\n", "between 0% and 50%"),
            ("[account.routing]\nreserve = \"5\"\n", "not a percentage"),
            ("[account.routing]\nprice = { input = -1.0 }\n", "price.input"),
            ("[account.routing]\nwindow.w = { capacity = 0.0 }\n", "window.w.capacity"),
            ("[account.routing]\nunknown = 1\n", "unknown field"),
        ];
        for (tail, want) in cases {
            let err = Accounts::parse(&format!("{head}{tail}"), p, |_| None).unwrap_err().to_string();
            assert!(err.contains(want), "{want}: {err}");
        }
        let one = "schema = 1\n[[account]]\nprovider=\"x\"\nname=\"m\"\nsecret=\"sk-0000000000\"\npriority = 2\n";
        assert!(Accounts::parse(one, p, |_| None).unwrap_err().to_string().contains("need schema 2"));
    }

    const WEIGHTS: &str = r#"
schema = 2
[[account]]
provider = "anthropic"
name = "max"
kind = "signin"
[account.routing]
window."5-hour" = { token_weights = { output = 15.0 }, model_multiplier = { "claude-opus-*" = 1.5 } }
"#;

    #[test]
    fn window_weights_and_multipliers_parse_and_round_trip() {
        let p = Path::new("accounts.toml");
        let a = Accounts::parse(WEIGHTS, p, |_| None).unwrap();
        let routing = &a.get("anthropic", "max").unwrap().routing;
        let w = &routing.window["5-hour"];
        assert_eq!(w.token_weights, Some(PartialTokenWeights { output: Some(15.0), ..Default::default() }));
        assert_eq!(w.model_multiplier.get("claude-opus-*"), Some(&1.5));
        let b = Accounts::parse(&a.to_toml(), p, |_| None).unwrap();
        assert_eq!(&b.get("anthropic", "max").unwrap().routing, routing);
    }

    #[test]
    fn window_weight_value_refusals_use_the_gate_wording() {
        let p = Path::new("accounts.toml");
        let head = "schema = 2\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"signin\"\n[account.routing]\n";
        let cases = [
            ("window.w = { token_weights = { output = -1.0 } }\n", "window.w.token_weights: output must be 0 or more"),
            (
                "window.w = { model_multiplier = { \"big-*\" = 0.0 } }\n",
                "window.w.model_multiplier: \"big-*\": a factor must be more than 0",
            ),
            ("window.w = { token_weights = { thinking = 2.0 } }\n", "unknown field"),
        ];
        for (tail, want) in cases {
            let err = Accounts::parse(&format!("{head}{tail}"), p, |_| None).unwrap_err().to_string();
            assert!(err.contains(want), "{want}: {err}");
        }
    }

    #[test]
    fn window_weights_refused_on_requests_and_undeclared_globs() {
        use nullrouter_registry::schema::RoutingDecl;
        let p = Path::new("accounts.toml");
        let declared = toml::from_str::<RoutingDecl>(
            "[[window]]\nname = \"5-hour\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\n\
             model_multiplier = { \"claude-*\" = 2.0 }\n\n[[window]]\nname = \"per-minute\"\nlength = \"1m\"\n\
             unit = \"requests\"\n",
        )
        .unwrap()
        .window;

        // The glob `claude-opus-*` is not one the window declares (`claude-*` is).
        let a = Accounts::parse(WEIGHTS, p, |_| None).unwrap();
        let r = &a.get("anthropic", "max").unwrap().routing;
        assert_eq!(
            r.meter_problem("anthropic", &declared).as_deref(),
            Some("routing.window.5-hour.model_multiplier: anthropic declares no glob \"claude-opus-*\" for window 5-hour"),
        );

        // Token weights on a requests window.
        let text = "schema = 2\n[[account]]\nprovider=\"anthropic\"\nname=\"max\"\nkind=\"signin\"\n\
                    [account.routing]\nwindow.\"per-minute\" = { token_weights = { output = 15.0 } }\n";
        let b = Accounts::parse(text, p, |_| None).unwrap();
        let r = &b.get("anthropic", "max").unwrap().routing;
        assert_eq!(
            r.meter_problem("anthropic", &declared).as_deref(),
            Some("routing.window.per-minute.token_weights: only applies to unit \"weighted_tokens\""),
        );

        // A declared glob on a weighted window is accepted.
        let text = "schema = 2\n[[account]]\nprovider=\"anthropic\"\nname=\"max\"\nkind=\"signin\"\n\
                    [account.routing]\nwindow.\"5-hour\" = { token_weights = { output = 15.0 }, \
                    model_multiplier = { \"claude-*\" = 1.5 } }\n";
        let c = Accounts::parse(text, p, |_| None).unwrap();
        assert_eq!(c.get("anthropic", "max").unwrap().routing.meter_problem("anthropic", &declared), None);
    }

    #[test]
    fn setters_validate() {
        let mut a = Accounts::parse(ROUTING, Path::new("accounts.toml"), |_| None).unwrap();
        a.set_priority("anthropic", "pro", 0.0).unwrap();
        assert_eq!(a.get("anthropic", "pro").unwrap().priority, 0.0);
        assert_eq!(a.set_priority("anthropic", "pro", -1.0), Err(AccountError::BadPriority));
        assert_eq!(a.set_priority("anthropic", "pro", f64::NAN), Err(AccountError::BadPriority));
        assert!(matches!(a.set_priority("anthropic", "nope", 1.0), Err(AccountError::NotFound { .. })));
    }

    #[test]
    fn saved_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = parse();
        a.path = dir.path().join(FILE);
        a.save().unwrap();
        let b = Accounts::load(&a.path).unwrap();
        assert_eq!(b.iter().count(), 3);
    }

    const EXCLUSIVE: &str = r#"
schema = 2
[[account]]
provider = "anthropic"
name = "max"
kind = "signin"
exclusive_use = "2026-10-07T12:00:00Z"
[[account]]
provider = "anthropic"
name = "pro"
kind = "signin"
"#;

    #[test]
    fn exclusive_use_round_trips_as_rfc_3339_and_stays_out_when_unset() {
        let p = Path::new("accounts.toml");
        let a = Accounts::parse(EXCLUSIVE, p, |_| None).unwrap();
        let since = crate::clock::parse_rfc3339("2026-10-07T12:00:00Z");
        assert!(since.is_some());
        assert_eq!(a.get("anthropic", "max").unwrap().exclusive_use, since);
        assert_eq!(a.get("anthropic", "pro").unwrap().exclusive_use, None);

        let text = a.to_toml();
        assert!(text.contains("exclusive_use = \"2026-10-07T12:00:00Z\""), "{text}");
        let pro_part = text.split("[[account]]").last().unwrap();
        assert!(!pro_part.contains("exclusive_use"), "{text}");
        let b = Accounts::parse(&text, p, |_| None).unwrap();
        assert_eq!(b.get("anthropic", "max").unwrap().exclusive_use, since);

        let bad = "schema = 2\n[[account]]\nprovider=\"x\"\nname=\"m\"\nkind=\"signin\"\nexclusive_use=\"soon\"\n";
        assert!(Accounts::parse(bad, p, |_| None).unwrap_err().to_string().contains("RFC 3339"));
        let one = "schema = 1\n[[account]]\nprovider=\"x\"\nname=\"m\"\nsecret=\"sk-0000000000\"\nexclusive_use=\"2026-10-07T12:00:00Z\"\n";
        assert!(Accounts::parse(one, p, |_| None).unwrap_err().to_string().contains("need schema 2"));
    }

    const ACME: &str = r#"
schema = 2
id = "acme"
category = "apikey"
[auth]
header = "x-api-key"
scheme = "raw"
[[endpoints.text]]
url = "https://api.acme.example/v1/messages"
wire = "anthropic-messages"
[[models]]
id = "slow"
kind = "llm"
"#;

    #[test]
    fn exclusive_use_refused_when_the_provider_reports_no_quota() {
        let acme = nullrouter_registry::validate_user_plugin(ACME, Path::new("acme.toml"))
            .unwrap_or_else(|e| panic!("{e:#?}"));
        let want = "account acme/main: exclusive use needs quota polls; acme reports no quota for this account";
        let key = Account::key("acme", "main", SecretSource::Literal, None, 0, BTreeSet::new());
        assert_eq!(key.exclusive_use_problem(&acme).as_deref(), Some(want));
        let signin = Account::signin("acme", "main", 0);
        assert_eq!(signin.exclusive_use_problem(&acme).as_deref(), Some(want));
    }
}
