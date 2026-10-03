//! Provider accounts (`accounts.toml`, data-model § ProviderAccount).
//!
//! A secret is bound to the hosts its provider sent traffic to when the account was added.
//! If a replacing plugin points the provider elsewhere, the secret is withheld until the
//! operator adds the account again (slice 002 FR-012a rule, applied to operator secrets).
//!
//! Schema 2 (spec 005, research R5) adds sign-in accounts: `kind = "signin"`, no secret in
//! this file; the tokens and the hosts they are bound to live in `tokens.toml`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nullrouter_registry::schema::parse_duration;
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
    /// Hosts a `key` account's secret may be sent to. Empty in a hand-written file: bound
    /// to the provider's hosts at load. Always empty for a sign-in account: its tokens
    /// carry their own hosts.
    pub hosts: BTreeSet<String>,
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
            hosts,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    hosts: Vec<String>,
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
        let raw: RawFile = toml::from_str(text).map_err(|e| FileError::invalid(path, e.to_string()))?;
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
            if raw.schema == 1 && (a.kind != AccountKind::Key || a.poll_interval.is_some()) {
                return Err(FileError::invalid(path, format!("{who}: `kind` and `poll_interval` need schema 2")));
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
                hosts: a.hosts.into_iter().collect(),
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
                hosts: if a.is_signin() { Vec::new() } else { a.hosts.iter().cloned().collect() },
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

    #[test]
    fn saved_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = parse();
        a.path = dir.path().join(FILE);
        a.save().unwrap();
        let b = Accounts::load(&a.path).unwrap();
        assert_eq!(b.iter().count(), 3);
    }
}
