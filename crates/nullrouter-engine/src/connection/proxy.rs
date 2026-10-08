//! Proxies: definitions, selection, health and pausing (spec 013, research R7, R8).
//!
//! `proxies.toml` (mode 0600, data-model § Proxy) holds `[[proxy]] { name, url, username?,
//! password? }`. The password is a string or `{ env = "VAR" }`; only its source is ever shown.

use std::collections::BTreeSet;
use std::path::Path;

use nullrouter_registry::SecretString;
use reqwest::Url;
use serde::{Deserialize, Serialize};

use crate::files::{self, FileError};

pub const FILE: &str = "proxies.toml";
pub const SCHEMA: u32 = 1;
/// The value that means "no proxy" in an assignment; it can't name a proxy.
pub const NONE: &str = "none";

/// Where a proxy's password comes from. Only the source is ever shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasswordSource {
    Literal,
    Env(String),
}

#[derive(Clone)]
pub struct Proxy {
    pub name: String,
    /// `scheme://host:port`, without credentials.
    pub url: String,
    pub username: Option<String>,
    pub password: Option<PasswordSource>,
    /// The password as read: the literal, or the variable's value (`None` when it isn't set).
    pub secret: Option<SecretString>,
}

impl std::fmt::Debug for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Proxy")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("username", &self.username.as_ref().map(|_| "…"))
            .field("password", &self.password)
            .finish()
    }
}

impl Proxy {
    /// `scheme://host:port` for listings; never a credential.
    pub fn shown(&self) -> String {
        self.url.clone()
    }

    /// Every secret value of this proxy, for the redactor: the password, and the username.
    pub fn secrets(&self) -> Vec<SecretString> {
        let user = self.username.as_ref().map(|u| SecretString::new(u.clone()));
        self.secret.iter().map(|s| s.with_exposed(|v| SecretString::new(v.to_owned()))).chain(user).collect()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Proxies {
    list: Vec<Proxy>,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum RawPassword {
    Literal(String),
    Env { env: String },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProxy {
    name: String,
    url: String,
    username: Option<String>,
    password: Option<RawPassword>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: u32,
    #[serde(default, rename = "proxy")]
    proxies: Vec<RawProxy>,
}

/// `[a-z0-9][a-z0-9_-]{0,31}`.
pub fn valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=32).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_' || *c == b'-')
}

/// A proxy URL's rule: `http`, `https` or `socks5`, a host and a port, and no credentials.
pub fn check_url(url: &str) -> Result<(), String> {
    let u = Url::parse(url).map_err(|e| format!("{e}"))?;
    if !matches!(u.scheme(), "http" | "https" | "socks5") {
        return Err(format!("scheme {:?} is not http, https or socks5", u.scheme()));
    }
    if u.host_str().is_none() {
        return Err("has no host".into());
    }
    if u.port().is_none() {
        return Err("has no port".into());
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err("must not carry credentials; give them as username and password".into());
    }
    Ok(())
}

impl Proxies {
    pub fn iter(&self) -> impl Iterator<Item = &Proxy> {
        self.list.iter()
    }

    pub fn get(&self, name: &str) -> Option<&Proxy> {
        self.list.iter().find(|p| p.name == name)
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Reads `path`; a missing file is no proxies. A file other users can read is refused.
    pub fn load(path: &Path) -> Result<Self, FileError> {
        match files::read_private_file(path)? {
            None => Ok(Self::default()),
            Some(text) => Self::parse(&text, path, |v| std::env::var(v).ok()),
        }
    }

    pub fn parse(text: &str, path: &Path, env: impl Fn(&str) -> Option<String>) -> Result<Self, FileError> {
        let raw: RawFile = toml::from_str(text).map_err(|e| FileError::toml(path, text, &e))?;
        if raw.schema != SCHEMA {
            return Err(FileError::invalid(path, format!("schema {} is not supported (expected {SCHEMA})", raw.schema)));
        }
        let mut seen = BTreeSet::new();
        let mut list = Vec::new();
        for (i, p) in raw.proxies.into_iter().enumerate() {
            let at = |field: &str, rule: String| FileError::invalid(path, format!("proxy[{i}].{field}: {rule}"));
            if p.name == NONE {
                return Err(at("name", format!("{NONE:?} is reserved")));
            }
            if !valid_name(&p.name) {
                return Err(at("name", format!("{:?} is not a name; use 1-32 of a-z, 0-9, _ and -", p.name)));
            }
            if !seen.insert(p.name.clone()) {
                return Err(at("name", format!("{:?} is used twice", p.name)));
            }
            check_url(&p.url).map_err(|e| at("url", e))?;
            let (password, secret) = match p.password {
                None => (None, None),
                Some(RawPassword::Literal(v)) => (Some(PasswordSource::Literal), Some(SecretString::new(v))),
                Some(RawPassword::Env { env: var }) => {
                    let value = env(&var).filter(|v| !v.is_empty()).map(SecretString::new);
                    (Some(PasswordSource::Env(var)), value)
                }
            };
            list.push(Proxy { name: p.name, url: p.url, username: p.username, password, secret });
        }
        Ok(Self { list })
    }

    /// The file's text. An `env` password is written as its reference, never its value.
    pub fn to_toml(&self) -> String {
        let raw = RawFile {
            schema: SCHEMA,
            proxies: self
                .list
                .iter()
                .map(|p| RawProxy {
                    name: p.name.clone(),
                    url: p.url.clone(),
                    username: p.username.clone(),
                    password: p.password.as_ref().and_then(|s| match s {
                        PasswordSource::Env(v) => Some(RawPassword::Env { env: v.clone() }),
                        PasswordSource::Literal => p
                            .secret
                            .as_ref()
                            .map(|s| RawPassword::Literal(s.with_exposed(str::to_owned))),
                    }),
                })
                .collect(),
        };
        toml::to_string(&raw).expect("proxies serialise")
    }

    /// Writes `path` atomically with mode 0600.
    pub fn save(&self, path: &Path) -> Result<(), FileError> {
        files::write_private(path, &self.to_toml())
    }

    /// Adds `proxy`, refusing a taken or invalid name or URL.
    pub fn add(&mut self, proxy: Proxy) -> Result<(), String> {
        if proxy.name == NONE {
            return Err(format!("{NONE:?} is reserved"));
        }
        if !valid_name(&proxy.name) {
            return Err(format!("{:?} is not a name; use 1-32 of a-z, 0-9, _ and -", proxy.name));
        }
        if self.get(&proxy.name).is_some() {
            return Err(format!("a proxy named {:?} exists; remove it first", proxy.name));
        }
        check_url(&proxy.url).map_err(|e| format!("url: {e}"))?;
        self.list.push(proxy);
        Ok(())
    }

    /// Removes `name`; `false` when there is none.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|p| p.name != name);
        self.list.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Proxies, String> {
        Proxies::parse(text, Path::new("proxies.toml"), |v| (v == "EU_PW").then(|| "pw-from-env".to_owned()))
            .map_err(|e| e.to_string())
    }

    const GOOD: &str = r#"
schema = 1

[[proxy]]
name = "eu-exit"
url = "socks5://10.0.0.5:1080"
username = "router"
password = { env = "EU_PW" }

[[proxy]]
name = "plain"
url = "http://proxy.example:3128"
password = "hunter2-literal"
"#;

    #[test]
    fn a_file_loads_with_literal_and_env_passwords() {
        let p = parse(GOOD).unwrap();
        let eu = p.get("eu-exit").unwrap();
        assert_eq!(eu.password, Some(PasswordSource::Env("EU_PW".into())));
        assert!(eu.secret.as_ref().unwrap().matches("pw-from-env"));
        assert!(p.get("plain").unwrap().secret.as_ref().unwrap().matches("hunter2-literal"));
        assert_eq!(eu.shown(), "socks5://10.0.0.5:1080");
    }

    #[test]
    fn an_unset_env_password_loads_without_a_value() {
        let p = Proxies::parse(GOOD, Path::new("p.toml"), |_| None).unwrap();
        assert!(p.get("eu-exit").unwrap().secret.is_none());
    }

    #[test]
    fn names_urls_and_uniqueness_are_validated_with_the_field_named() {
        let bad = |name: &str, url: &str| format!("schema = 1\n[[proxy]]\nname = \"{name}\"\nurl = \"{url}\"\n");
        for (name, url, want) in [
            ("none", "http://h:1", "proxy[0].name: \"none\" is reserved"),
            ("Bad Name", "http://h:1", "proxy[0].name"),
            ("-lead", "http://h:1", "proxy[0].name"),
            ("a", "ftp://h:1", "proxy[0].url: scheme \"ftp\""),
            ("a", "http://h", "proxy[0].url: has no port"),
            ("a", "http://u:p@h:1", "proxy[0].url: must not carry credentials"),
            ("a", "socks5://:1080", "proxy[0].url"),
        ] {
            let e = parse(&bad(name, url)).unwrap_err();
            assert!(e.contains(want), "{name} {url}: {e}");
        }
        let twice = format!("{}\n[[proxy]]\nname = \"a\"\nurl = \"http://h:2\"\n", bad("a", "http://h:1"));
        assert!(parse(&twice).unwrap_err().contains("proxy[1].name: \"a\" is used twice"));
        assert!(parse("schema = 2\n").unwrap_err().contains("schema 2 is not supported"));
        assert!(parse("schema = 1\n[[proxy]]\nname = \"a\"\nurl = \"http://h:1\"\nextra = 1\n").is_err());
    }

    #[test]
    fn saving_writes_an_env_password_as_a_reference_and_round_trips() {
        let p = parse(GOOD).unwrap();
        let text = p.to_toml();
        assert!(text.contains("env = \"EU_PW\"") && !text.contains("pw-from-env"), "{text}");
        let again = parse(&text).unwrap();
        assert_eq!(again.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["eu-exit", "plain"]);
        assert!(again.get("plain").unwrap().secret.as_ref().unwrap().matches("hunter2-literal"));
    }

    #[test]
    fn the_file_is_written_private_and_a_readable_one_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE);
        parse(GOOD).unwrap().save(&path).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(Proxies::load(&path).is_ok());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(Proxies::load(&path), Err(FileError::NotPrivate { .. })));
        assert!(Proxies::load(&dir.path().join("missing.toml")).unwrap().is_empty());
    }

    #[test]
    fn debug_output_and_secrets_never_carry_the_password() {
        let p = parse(GOOD).unwrap();
        let shown = format!("{p:?}");
        assert!(!shown.contains("pw-from-env") && !shown.contains("hunter2") && !shown.contains("router"), "{shown}");
        let secrets = p.get("eu-exit").unwrap().secrets();
        assert_eq!(secrets.len(), 2, "the password and the username go to the redactor");
    }

    #[test]
    fn add_and_remove_keep_names_unique() {
        let mut p = Proxies::default();
        let one = |name: &str, url: &str| Proxy {
            name: name.into(),
            url: url.into(),
            username: None,
            password: None,
            secret: None,
        };
        p.add(one("a", "http://h:1")).unwrap();
        assert!(p.add(one("a", "http://h:2")).unwrap_err().contains("exists"));
        assert!(p.add(one("none", "http://h:2")).unwrap_err().contains("reserved"));
        assert!(p.add(one("b", "http://h")).unwrap_err().contains("url"));
        assert!(p.remove("a") && !p.remove("a"));
    }
}
