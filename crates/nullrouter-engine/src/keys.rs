//! Agent keys (`keys.toml`, data-model § AgentKey, research R12).
//!
//! A key is shown once when issued; the file keeps its SHA-256 digest, never the key.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
pub use nullrouter_adapters::HarnessName;
pub use nullrouter_registry::schema::BreakBehaviour;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::accounts::last4;
use crate::clock;
use crate::files::{self, FileError};

pub const FILE: &str = "keys.toml";
pub const PREFIX: &str = "0r-";
/// 9router `normalizeSessionId`.
pub const MAX_SESSION_CHARS: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentKey {
    pub id: String,
    pub name: String,
    pub digest: String,
    pub last4: String,
    pub created: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_behaviour: Option<BreakBehaviour>,
    /// The client harness this key's requests come from; its adapter runs on them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<HarnessName>,
}

/// The agent a request belongs to: its key, and the client's session when it sends one.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct AgentId {
    pub key: String,
    pub session: Option<String>,
}

impl AgentId {
    pub fn new(key: impl Into<String>, session: Option<&str>) -> Self {
        let session =
            session.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.chars().take(MAX_SESSION_CHARS).collect());
        Self { key: key.into(), session }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeyError {
    #[error("a key named {0} already exists")]
    Duplicate(String),
    #[error("no key named or with id {0}")]
    NotFound(String),
    #[error("key names can't be empty")]
    EmptyName,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    schema: u32,
    #[serde(default, rename = "key")]
    keys: Vec<AgentKey>,
}

#[derive(Debug, Default)]
pub struct Keys {
    pub path: PathBuf,
    list: Vec<AgentKey>,
    by_digest: HashMap<String, usize>,
}

pub fn digest(key: &str) -> String {
    let mut out = String::with_capacity(71);
    out.push_str("sha256:");
    for b in Sha256::digest(key.as_bytes()) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn random<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("the OS random source is available");
    buf
}

/// A fresh key: `0r-` + base64url of 32 random bytes (43 characters).
pub fn generate() -> String {
    format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(random::<32>()))
}

fn new_id() -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let id: String = random::<8>().iter().map(|b| ALPHABET[usize::from(*b) % ALPHABET.len()] as char).collect();
    format!("ak_{id}")
}

impl Keys {
    pub fn load(path: &Path) -> Result<Self, FileError> {
        match files::read_private(path)? {
            None => Ok(Self { path: path.to_owned(), ..Self::default() }),
            Some(text) => Self::parse(&text, path),
        }
    }

    pub fn parse(text: &str, path: &Path) -> Result<Self, FileError> {
        let raw: RawFile = toml::from_str(text).map_err(|e| FileError::toml(path, text, &e))?;
        if raw.schema != 1 {
            return Err(FileError::invalid(path, format!("schema {} is not supported (expected 1)", raw.schema)));
        }
        let mut keys = Self { path: path.to_owned(), ..Self::default() };
        for k in raw.keys {
            if keys.list.iter().any(|o| o.name == k.name || o.id == k.id) {
                return Err(FileError::invalid(path, format!("key {} / {} appears twice", k.id, k.name)));
            }
            keys.push(k);
        }
        Ok(keys)
    }

    fn push(&mut self, k: AgentKey) {
        self.by_digest.insert(k.digest.clone(), self.list.len());
        self.list.push(k);
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(&RawFile { schema: 1, keys: self.list.clone() }).expect("keys serialise")
    }

    pub fn save(&self) -> Result<(), FileError> {
        files::write_private(&self.path, &self.to_toml())
    }

    pub fn iter(&self) -> impl Iterator<Item = &AgentKey> {
        self.list.iter()
    }

    /// The key record a presented key belongs to. A revoked key never matches.
    pub fn lookup(&self, presented: &str) -> Option<&AgentKey> {
        let k = &self.list[*self.by_digest.get(&digest(presented))?];
        k.revoked.is_none().then_some(k)
    }

    /// Issues a key; the returned string is the only time the key exists outside a client.
    pub fn issue(
        &mut self,
        name: &str,
        break_behaviour: Option<BreakBehaviour>,
    ) -> Result<(String, &AgentKey), KeyError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(KeyError::EmptyName);
        }
        if self.list.iter().any(|k| k.name == name) {
            return Err(KeyError::Duplicate(name.to_owned()));
        }
        let key = generate();
        let id = std::iter::repeat_with(new_id).find(|id| self.list.iter().all(|k| &k.id != id)).expect("an unused id");
        self.push(AgentKey {
            id,
            name: name.to_owned(),
            digest: digest(&key),
            last4: last4(&key),
            created: clock::now_rfc3339(),
            revoked: None,
            break_behaviour,
            harness: None,
        });
        Ok((key, self.list.last().expect("just pushed")))
    }

    fn find_mut(&mut self, name_or_id: &str) -> Result<&mut AgentKey, KeyError> {
        self.list
            .iter_mut()
            .find(|k| k.id == name_or_id || k.name == name_or_id)
            .ok_or_else(|| KeyError::NotFound(name_or_id.to_owned()))
    }

    pub fn revoke(&mut self, name_or_id: &str) -> Result<(), KeyError> {
        let k = self.find_mut(name_or_id)?;
        k.revoked.get_or_insert_with(clock::now_rfc3339);
        Ok(())
    }

    /// Binds the key to a harness; `None` makes it a plain client again.
    pub fn set_harness(&mut self, name_or_id: &str, h: Option<HarnessName>) -> Result<(), KeyError> {
        self.find_mut(name_or_id)?.harness = h;
        Ok(())
    }

    /// Sets the key's override; `None` returns it to the operator default.
    pub fn set_break(&mut self, name_or_id: &str, b: Option<BreakBehaviour>) -> Result<(), KeyError> {
        self.find_mut(name_or_id)?.break_behaviour = b;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_have_the_documented_shape() {
        let k = generate();
        assert_eq!(k.len(), 3 + 43);
        assert!(k.starts_with(PREFIX));
        assert!(k[3..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_ne!(k, generate());
        let id = new_id();
        assert!(id.starts_with("ak_") && id.len() == 11);
        assert_eq!(digest("abc"), "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn issue_lookup_revoke_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let mut keys = Keys { path: dir.path().join(FILE), ..Keys::default() };
        let (key, rec) = keys.issue("laptop", Some(BreakBehaviour::ErrorEvent)).unwrap();
        let id = rec.id.clone();
        assert_eq!(rec.last4, key[key.len() - 4..]);
        assert!(matches!(keys.issue("laptop", None), Err(KeyError::Duplicate(_))));
        assert_eq!(keys.lookup(&key).map(|k| k.id.as_str()), Some(id.as_str()));
        assert!(keys.lookup("0r-wrong").is_none());

        keys.save().unwrap();
        let text = std::fs::read_to_string(&keys.path).unwrap();
        assert!(!text.contains(&key), "the key itself is never stored");
        assert!(text.contains("break_behaviour = \"error_event\""));

        let mut again = Keys::load(&keys.path).unwrap();
        assert!(again.lookup(&key).is_some());
        again.set_break(&id, None).unwrap();
        again.revoke("laptop").unwrap();
        assert!(again.lookup(&key).is_none(), "a revoked key never matches");
        assert!(matches!(again.revoke("nope"), Err(KeyError::NotFound(_))));
    }

    #[test]
    fn sessions_are_trimmed_and_capped() {
        assert_eq!(AgentId::new("ak_1", Some("  ")).session, None);
        let long = "é".repeat(300);
        assert_eq!(AgentId::new("ak_1", Some(&long)).session.unwrap().chars().count(), MAX_SESSION_CHARS);
    }
}
