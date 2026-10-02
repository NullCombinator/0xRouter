//! Provider accounts (`accounts.toml`, data-model § ProviderAccount).
//!
//! A secret is bound to the hosts its provider sent traffic to when the account was added.
//! If a replacing plugin points the provider elsewhere, the secret is withheld until the
//! operator adds the account again (slice 002 FR-012a rule, applied to operator secrets).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use zerorouter_registry::{ProviderEntity, Registry, SecretString};

use crate::files::{self, FileError};

pub const FILE: &str = "accounts.toml";

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
    pub source: SecretSource,
    /// `None` when the source is an unset environment variable.
    pub secret: Option<SecretString>,
    pub order: i64,
    pub disabled: bool,
    /// Hosts the secret may be sent to. Empty in a hand-written file: bound to the
    /// provider's hosts at load.
    pub hosts: BTreeSet<String>,
}

impl Account {
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
    secret: RawSecret,
    #[serde(default)]
    order: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    disabled: bool,
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

    pub fn parse(text: &str, path: &Path, env: impl Fn(&str) -> Option<String>) -> Result<Self, FileError> {
        let raw: RawFile = toml::from_str(text).map_err(|e| FileError::invalid(path, e.to_string()))?;
        if raw.schema != 1 {
            return Err(FileError::invalid(path, format!("schema {} is not supported (expected 1)", raw.schema)));
        }
        let mut list: Vec<Account> = Vec::new();
        for a in raw.accounts {
            if !valid_name(&a.name) {
                return Err(FileError::invalid(path, AccountError::BadName(a.name).to_string()));
            }
            if list.iter().any(|b| b.provider == a.provider && b.name == a.name) {
                return Err(FileError::invalid(
                    path,
                    AccountError::Duplicate { provider: a.provider, name: a.name }.to_string(),
                ));
            }
            let (source, secret) = match a.secret {
                RawSecret::Literal(s) => (SecretSource::Literal, Some(SecretString::new(s))),
                RawSecret::Env { env: var } => {
                    (SecretSource::Env(var.clone()), env(&var).filter(|s| !s.is_empty()).map(SecretString::new))
                }
            };
            list.push(Account {
                provider: a.provider,
                name: a.name,
                source,
                secret,
                order: a.order,
                disabled: a.disabled,
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
                secret: match &a.source {
                    SecretSource::Env(v) => RawSecret::Env { env: v.clone() },
                    SecretSource::Literal => {
                        RawSecret::Literal(a.secret.as_ref().map(|s| s.with_exposed(str::to_owned)).unwrap_or_default())
                    }
                },
                order: a.order,
                disabled: a.disabled,
                hosts: a.hosts.iter().cloned().collect(),
            })
            .collect();
        toml::to_string(&RawFile { schema: 1, accounts }).expect("accounts serialise")
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
        for a in self.list.iter_mut().filter(|a| a.hosts.is_empty()) {
            if let Ok(p) = registry.provider(&a.provider) {
                a.hosts = provider_hosts(p);
                bound |= !a.hosts.is_empty();
            }
        }
        bound
    }
}

/// The secret for a request to `active`, if every host it sends to is one the account was
/// added for.
pub fn release<'a>(account: &'a Account, active: &ProviderEntity) -> Result<&'a SecretString, Withheld> {
    let secret = account.secret.as_ref().ok_or_else(|| match &account.source {
        SecretSource::Env(v) => Withheld::EnvUnset(v.clone()),
        SecretSource::Literal => Withheld::EnvUnset(String::new()),
    })?;
    match provider_hosts(active).into_iter().find(|h| !account.hosts.contains(h)) {
        Some(host) => Err(Withheld::NewHost { host }),
        None => Ok(secret),
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
secret = { env = "ZR_TEST_MAIN" }
hosts = ["api.anthropic.com"]
[[account]]
provider = "ghost"
name = "x"
secret = "sk-ghost-00000000"
disabled = true
"#;

    fn parse() -> Accounts {
        Accounts::parse(FILE_TEXT, Path::new("accounts.toml"), |v| {
            (v == "ZR_TEST_MAIN").then(|| "sk-from-env-1234".to_owned())
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
        assert_eq!(main.shown_secret(), "env:ZR_TEST_MAIN");
        assert_eq!(a.get("anthropic", "backup").unwrap().shown_secret(), "…0000");
        assert!(!format!("{a:?}").contains("sk-ant-backup"));
        assert_eq!(a.for_provider("ghost").count(), 0, "disabled accounts are skipped");
    }

    #[test]
    fn round_trips_without_resolving_env() {
        let a = parse();
        let text = a.to_toml();
        assert!(text.contains("env = \"ZR_TEST_MAIN\"") && !text.contains("sk-from-env"));
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
        assert!(Accounts::parse("schema = 2\n", p, |_| None).is_err());
        assert!(valid_name("main_2-b") && !valid_name("") && !valid_name(&"a".repeat(33)));
    }

    #[test]
    fn add_replace_disable_remove() {
        let mut a = parse();
        let acct = |name: &str| Account {
            provider: "anthropic".into(),
            name: name.into(),
            source: SecretSource::Literal,
            secret: Some(SecretString::new("sk-new-000000000")),
            order: 5,
            disabled: false,
            hosts: BTreeSet::new(),
        };
        assert_eq!(a.add(acct("Bad Name")), Err(AccountError::BadName("Bad Name".into())));
        a.set_disabled("anthropic", "backup", true).unwrap();
        a.add(acct("backup")).unwrap();
        let b = a.get("anthropic", "backup").unwrap();
        assert!(b.disabled && b.secret.as_ref().unwrap().matches("sk-new-000000000"));
        a.remove("anthropic", "backup").unwrap();
        assert!(matches!(a.remove("anthropic", "backup"), Err(AccountError::NotFound { .. })));
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
