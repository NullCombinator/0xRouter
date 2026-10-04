//! The router's live state: the fingerprint salt, and (with the warm store and ledgers, added by
//! their own tasks) everything the placement reads. Unlike its siblings this file does I/O: it
//! reads or creates `routing/salt`. The decision itself stays in the pure files.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use std::collections::BTreeMap;

use super::Tier;
use super::ledger::Ledger;
use super::warm::WarmStore;
use crate::files::{self, FileError};

/// `routing/salt` under the operator home.
pub const SALT_FILE: &str = "routing/salt";
pub const SALT_LEN: usize = 32;

/// Seeds the fingerprints: 32 random bytes, created once, 0600. Without it a stolen journal
/// could be checked against guessed prompts. Never printed, logged or put in a record.
#[derive(Clone)]
pub struct Salt([u8; SALT_LEN]);

impl Salt {
    pub fn bytes(&self) -> &[u8; SALT_LEN] {
        &self.0
    }

    /// A fixed salt, for tests that compare fingerprints.
    pub fn from_bytes(bytes: [u8; SALT_LEN]) -> Self {
        Self(bytes)
    }

    /// Reads `<home>/routing/salt`, or creates it when it doesn't exist. A file of the wrong
    /// length is refused rather than replaced: replacing it would make every stored
    /// fingerprint unmatchable without the operator knowing.
    pub fn load_or_create(home: &Path) -> Result<Self, FileError> {
        let path = home.join(SALT_FILE);
        let io = |source| FileError::Io { path: path.clone(), source };
        files::refuse_symlink(&path)?;
        match fs::read(&path) {
            Ok(b) => {
                let bytes = <[u8; SALT_LEN]>::try_from(b.as_slice())
                    .map_err(|_| FileError::invalid(&path, "is not 32 bytes; remove it to start with new fingerprints"))?;
                return Ok(Self(bytes));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e)),
        }
        let mut bytes = [0u8; SALT_LEN];
        getrandom::fill(&mut bytes).expect("the OS random source is available");
        if let Some(dir) = path.parent() {
            fs::DirBuilder::new().mode(0o700).recursive(true).create(dir).map_err(io)?;
        }
        match OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path) {
            Ok(mut f) => {
                f.write_all(&bytes).and_then(|()| f.sync_all()).map_err(io)?;
                if let Some(dir) = path.parent() {
                    crate::journal::writer::sync_dir(dir).map_err(io)?;
                }
                Ok(Self(bytes))
            }
            // Another start won the race: use its salt.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Self::load_or_create(home),
            Err(e) => Err(io(e)),
        }
    }
}

impl std::fmt::Debug for Salt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Salt(..)")
    }
}

/// What the router remembers between requests. One lock covers it: a placement reads it and a
/// success writes it, and the journal line for a change is queued under the same lock (R12).
#[derive(Debug, Default)]
pub struct RouterState {
    pub warm: WarmStore,
    /// Deficits by `(target, tier)`.
    pub ledgers: BTreeMap<(String, Tier), Ledger>,
}

/// The router's state, held by the engine.
#[derive(Debug)]
pub struct Router {
    pub salt: Salt,
    state: Mutex<RouterState>,
}

impl Router {
    pub fn open(home: &Path) -> Result<Self, FileError> {
        Ok(Self { salt: Salt::load_or_create(home)?, state: Mutex::default() })
    }

    /// The router's state, locked. Never held across an `.await`.
    pub fn lock(&self) -> MutexGuard<'_, RouterState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn the_salt_is_created_once_private_and_reused() {
        let home = tempfile::tempdir().unwrap();
        let a = Salt::load_or_create(home.path()).unwrap();
        let b = Salt::load_or_create(home.path()).unwrap();
        assert_eq!(a.bytes(), b.bytes());
        let meta = fs::metadata(home.path().join(SALT_FILE)).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(meta.len(), 32);
        assert_ne!(a.bytes(), &[0u8; 32]);
    }

    #[test]
    fn a_damaged_salt_is_refused_not_replaced() {
        let home = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join("routing")).unwrap();
        fs::write(home.path().join(SALT_FILE), b"short").unwrap();
        assert!(Salt::load_or_create(home.path()).is_err());
        assert_eq!(fs::read(home.path().join(SALT_FILE)).unwrap(), b"short");
    }

    #[test]
    fn the_salt_never_prints() {
        let s = Salt::from_bytes([7; 32]);
        assert!(!format!("{s:?}").contains('7'));
    }
}
