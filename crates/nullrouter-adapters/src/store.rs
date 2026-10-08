//! The adapter store: `adapters/index.toml` and one directory per version, under the operator's
//! home (data-model `AdapterIndex`, `AdapterVersion` state machine).
//!
//! Files are written atomically with mode 0600 inside directories of mode 0700. The store refuses
//! to open an `adapters/` that anyone but its owner can enter.

use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::HarnessName;
use crate::fingerprint::{self, SourceFp};

const INDEX: &str = "index.toml";
const SCHEMA: u32 = 1;
const DEFAULT_RESERVE_OUTPUT: u32 = 4096;

/// `v` + the package version + `-` + the first 8 hex of the source fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VersionId(String);

impl VersionId {
    pub fn new(version: &semver::Version, fp: &SourceFp) -> Self {
        Self(format!("v{version}-{}", fp.short()))
    }

    /// The id a run record names, which is the index's id for a third-party adapter.
    pub fn from_run(version: &str) -> Self {
        Self(version.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VersionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionState {
    Queued,
    Building,
    InReview,
    Quarantined,
    Reported,
    Approved,
    Suspect,
    Refused,
    Rejected,
    Superseded,
}

impl VersionState {
    pub const ALL: [VersionState; 10] = [
        VersionState::Queued,
        VersionState::Building,
        VersionState::InReview,
        VersionState::Quarantined,
        VersionState::Reported,
        VersionState::Approved,
        VersionState::Suspect,
        VersionState::Refused,
        VersionState::Rejected,
        VersionState::Superseded,
    ];

    /// No way out: the version stays as it ended.
    pub fn is_terminal(self) -> bool {
        matches!(self, VersionState::Refused | VersionState::Rejected | VersionState::Superseded)
    }

    /// Whether the state machine has an edge from `self` to `to`.
    pub fn can_become(self, to: VersionState) -> bool {
        use VersionState::*;
        matches!(
            (self, to),
            (Queued, Building | Refused)
                | (Building, InReview | Refused)
                | (InReview, Reported | Quarantined)
                | (Quarantined, InReview)
                | (Reported, Approved | Rejected)
                | (Approved, Suspect | Superseded)
                | (Suspect, Approved | Superseded)
        )
    }
}

impl fmt::Display for VersionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = serde_json::to_value(self).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default();
        f.write_str(&s)
    }
}

/// Where a version's source came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    Catalogue(String),
    Local(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewConfig {
    pub model: String,
    pub budget_tokens: u64,
    #[serde(default = "default_reserve")]
    pub reserve_output: u32,
}

fn default_reserve() -> u32 {
    DEFAULT_RESERVE_OUTPUT
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionEntry {
    pub id: VersionId,
    pub semver: String,
    pub state: VersionState,
    pub source_fp: SourceFp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wasm_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kit_abi: Option<u32>,
    pub submitted: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub state_reason: String,
    /// A rebuild for a newer kit ABI is under way (FR-032). The state does not change.
    #[serde(default, skip_serializing_if = "is_false")]
    pub rebuilding: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub rebuild_failed: bool,
    pub origin: Origin,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessEntry {
    pub name: HarnessName,
    /// The approved version that serves. Absent: none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<VersionId>,
    /// `catalogue` or `local`: where versions came from last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, rename = "version")]
    pub versions: Vec<VersionEntry>,
}

/// `adapters/index.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewConfig>,
    #[serde(default, rename = "harness")]
    pub harnesses: Vec<HarnessEntry>,
}

impl Default for Index {
    fn default() -> Self {
        Self { schema: SCHEMA, review: None, harnesses: Vec::new() }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{path} can be entered by other users (mode {mode:o}); run `chmod 700 {path}`")]
    NotPrivate { path: PathBuf, mode: u32 },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {reason}")]
    BadIndex { path: PathBuf, reason: String },
    #[error("{0} is not a path inside a version directory")]
    BadPath(String),
    #[error("no harness {0} in the index")]
    UnknownHarness(String),
    #[error("no version {version} of {harness} in the index")]
    UnknownVersion { harness: String, version: String },
    #[error("{harness} already has version {version}")]
    DuplicateVersion { harness: String, version: String },
    #[error("a version cannot go from {from} to {to}")]
    Transition { from: VersionState, to: VersionState },
    /// What is on disk is not what was gated and built (`source_mismatch`).
    #[error("{what} of {harness} {version} is not what was gated and built")]
    SourceMismatch { harness: String, version: String, what: &'static str },
    #[error(transparent)]
    Fingerprint(#[from] fingerprint::FingerprintError),
}

impl Index {
    pub fn harness(&self, name: &HarnessName) -> Option<&HarnessEntry> {
        self.harnesses.iter().find(|h| &h.name == name)
    }

    fn harness_mut(&mut self, name: &HarnessName) -> Result<&mut HarnessEntry, StoreError> {
        self.harnesses.iter_mut().find(|h| &h.name == name).ok_or_else(|| StoreError::UnknownHarness(name.to_string()))
    }

    pub fn version(&self, name: &HarnessName, id: &VersionId) -> Option<&VersionEntry> {
        self.harness(name)?.versions.iter().find(|v| &v.id == id)
    }

    fn version_mut(&mut self, name: &HarnessName, id: &VersionId) -> Result<&mut VersionEntry, StoreError> {
        let unknown = || StoreError::UnknownVersion { harness: name.to_string(), version: id.to_string() };
        self.harness_mut(name)?.versions.iter_mut().find(|v| &v.id == id).ok_or_else(unknown)
    }

    /// Adds a freshly submitted version in `queued`, creating the harness entry if needed.
    pub fn submit(&mut self, name: &HarnessName, mut entry: VersionEntry) -> Result<(), StoreError> {
        if self.version(name, &entry.id).is_some() {
            return Err(StoreError::DuplicateVersion { harness: name.to_string(), version: entry.id.to_string() });
        }
        entry.state = VersionState::Queued;
        if self.harness(name).is_none() {
            self.harnesses.push(HarnessEntry { name: name.clone(), active: None, source: None, versions: Vec::new() });
        }
        self.harness_mut(name)?.versions.push(entry);
        Ok(())
    }

    /// Moves a version along one edge of the state machine, and records why.
    pub fn transition(
        &mut self,
        name: &HarnessName,
        id: &VersionId,
        to: VersionState,
        reason: &str,
    ) -> Result<(), StoreError> {
        let v = self.version_mut(name, id)?;
        if !v.state.can_become(to) {
            return Err(StoreError::Transition { from: v.state, to });
        }
        v.state = to;
        v.state_reason = reason.to_owned();
        Ok(())
    }

    /// The operator's approval of a `reported` version: it becomes `active`, and the version that
    /// was active (if any) becomes `superseded`. Requests already running on that one finish on it.
    pub fn approve(&mut self, name: &HarnessName, id: &VersionId) -> Result<(), StoreError> {
        let previous = self.harness(name).and_then(|h| h.active.clone());
        self.transition(name, id, VersionState::Approved, "")?;
        if let Some(old) = previous.filter(|old| old != id) {
            self.transition(name, &old, VersionState::Superseded, "a newer version was approved")?;
        }
        self.harness_mut(name)?.active = Some(id.clone());
        Ok(())
    }

    /// The version that serves `name`: the active one, if it is `approved`. A `suspect` one
    /// stays active but does not serve (FR-017).
    pub fn serving(&self, name: &HarnessName) -> Option<&VersionEntry> {
        let h = self.harness(name)?;
        let active = h.active.as_ref()?;
        h.versions.iter().find(|v| &v.id == active && v.state == VersionState::Approved)
    }
}

/// `$NULLROUTER_HOME/adapters`.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Opens the store under `home`, creating `adapters/` with mode 0700 if it is missing. An
    /// existing one that is group- or world-accessible is refused.
    pub fn open(home: &Path) -> Result<Self, StoreError> {
        let root = home.join("adapters");
        match fs::metadata(&root) {
            Ok(meta) => {
                let mode = meta.permissions().mode() & 0o777;
                if mode & 0o077 != 0 {
                    return Err(StoreError::NotPrivate { path: root, mode });
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => private_dirs(&root)?,
            Err(source) => return Err(StoreError::Io { path: root, source }),
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn version_dir(&self, name: &HarnessName, id: &VersionId) -> PathBuf {
        self.root.join(name.as_str()).join(id.as_str())
    }

    /// The index, or an empty one when there is none yet.
    pub fn load_index(&self) -> Result<Index, StoreError> {
        let path = self.root.join(INDEX);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Index::default()),
            Err(source) => return Err(StoreError::Io { path, source }),
        };
        let index: Index = toml::from_str(&text)
            .map_err(|e| StoreError::BadIndex { path: path.clone(), reason: e.message().to_owned() })?;
        if index.schema != SCHEMA {
            return Err(StoreError::BadIndex { path, reason: format!("schema {} is not supported", index.schema) });
        }
        Ok(index)
    }

    pub fn save_index(&self, index: &Index) -> Result<(), StoreError> {
        let path = self.root.join(INDEX);
        let text =
            toml::to_string(index).map_err(|e| StoreError::BadIndex { path: path.clone(), reason: e.to_string() })?;
        write_private(&path, text.as_bytes())
    }

    /// Writes `bytes` to `relative` (such as `source/src/lib.rs` or `module.wasm`) inside a
    /// version's directory.
    pub fn write_version_file(
        &self,
        name: &HarnessName,
        id: &VersionId,
        relative: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let inside = Path::new(relative).components().all(|c| matches!(c, Component::Normal(_)));
        if relative.is_empty() || !inside {
            return Err(StoreError::BadPath(relative.to_owned()));
        }
        write_private(&self.version_dir(name, id).join(relative), bytes)
    }

    /// Checks the version on disk against the index: its `source/` must hash to `source_fp`, and
    /// `module.wasm` (when the entry has a `wasm_hash`) to that. Any difference is
    /// `SourceMismatch`: something changed the files after they were gated and built.
    pub fn verify(&self, name: &HarnessName, entry: &VersionEntry) -> Result<(), StoreError> {
        let dir = self.version_dir(name, &entry.id);
        let mismatch =
            |what| StoreError::SourceMismatch { harness: name.to_string(), version: entry.id.to_string(), what };
        match fingerprint::of_dir(&dir.join("source")) {
            Ok(fp) if fp == entry.source_fp => {}
            Ok(_) | Err(fingerprint::FingerprintError::NotRegular(_)) => return Err(mismatch("the source")),
            Err(fingerprint::FingerprintError::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                return Err(mismatch("the source"));
            }
            Err(e) => return Err(e.into()),
        }
        if let Some(expected) = &entry.wasm_hash {
            let wasm = fs::read(dir.join("module.wasm")).map_err(|_| mismatch("the module"))?;
            if &nullrouter_sandbox::wasm_hash(&wasm) != expected {
                return Err(mismatch("the module"));
            }
        }
        Ok(())
    }
}

/// Creates `path` and any missing parents with mode 0700.
fn private_dirs(path: &Path) -> Result<(), StoreError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|source| StoreError::Io { path: path.to_owned(), source })
}

/// Writes `bytes` to `path` whole or not at all: to a sibling file with mode 0600, synced, then
/// renamed over `path`.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let io = |source| StoreError::Io { path: path.to_owned(), source };
    let dir = path.parent().ok_or_else(|| StoreError::BadPath(path.display().to_string()))?;
    private_dirs(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let temp = dir.join(format!(".{name}.tmp"));
    // A leftover from a crash is replaced, not trusted.
    let _ = fs::remove_file(&temp);
    let mut file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temp).map_err(io)?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(e) = written.and_then(|()| fs::rename(&temp, path)) {
        let _ = fs::remove_file(&temp);
        return Err(io(e));
    }
    Ok(())
}
