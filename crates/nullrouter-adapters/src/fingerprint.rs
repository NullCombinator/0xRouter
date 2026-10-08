//! The source fingerprint: one hash over a whole source tree (data-model `SourceFp`).
//!
//! The canonical form is every regular file, sorted by relative path, each written as
//! `path\0len\0bytes` with `len` in decimal. Directories, file modes and times are not part of
//! it, so the same files hash the same wherever and whenever they were unpacked. A rename, an
//! added file or any changed byte changes it.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const PREFIX: &str = "sha256:";

/// `sha256:` and 64 lowercase hex characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SourceFp(String);

#[derive(Debug, thiserror::Error)]
pub enum FingerprintError {
    #[error("not a source fingerprint: {0:?}")]
    Malformed(String),
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    /// A symlink, device or other non-regular entry. A source tree holds only files.
    #[error("{0}: not a regular file or directory")]
    NotRegular(PathBuf),
    #[error("{0}: the name is not UTF-8")]
    NotUtf8(PathBuf),
}

impl SourceFp {
    pub fn parse(s: &str) -> Result<Self, FingerprintError> {
        let hex = s.strip_prefix(PREFIX).unwrap_or("");
        let ok = hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if ok { Ok(Self(s.to_owned())) } else { Err(FingerprintError::Malformed(s.to_owned())) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first 8 hex characters, the part of a `VersionId`.
    pub fn short(&self) -> &str {
        &self.0[PREFIX.len()..PREFIX.len() + 8]
    }
}

impl fmt::Display for SourceFp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SourceFp {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

/// The fingerprint of `files`, given as `(relative path, bytes)` in any order. Paths use `/`.
pub fn of_files(files: &[(String, Vec<u8>)]) -> SourceFp {
    let mut order: Vec<&(String, Vec<u8>)> = files.iter().collect();
    order.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    let mut hash = Sha256::new();
    for (path, bytes) in order {
        hash.update(path.as_bytes());
        hash.update([0]);
        hash.update(bytes.len().to_string().as_bytes());
        hash.update([0]);
        hash.update(bytes);
    }
    SourceFp(format!("{PREFIX}{:x}", hash.finalize()))
}

/// The fingerprint of the tree under `root`.
pub fn of_dir(root: &Path) -> Result<SourceFp, FingerprintError> {
    let mut files = Vec::new();
    collect(root, "", &mut files)?;
    Ok(of_files(&files))
}

fn collect(dir: &Path, prefix: &str, out: &mut Vec<(String, Vec<u8>)>) -> Result<(), FingerprintError> {
    let io = |path: &Path, source| FingerprintError::Io { path: path.to_owned(), source };
    for entry in fs::read_dir(dir).map_err(|e| io(dir, e))? {
        let entry = entry.map_err(|e| io(dir, e))?;
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|_| FingerprintError::NotUtf8(path.clone()))?;
        let relative = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        // `symlink_metadata`: a link is refused, never followed out of the tree.
        let kind = fs::symlink_metadata(&path).map_err(|e| io(&path, e))?.file_type();
        if kind.is_dir() {
            collect(&path, &relative, out)?;
        } else if kind.is_file() {
            out.push((relative, fs::read(&path).map_err(|e| io(&path, e))?));
        } else {
            return Err(FingerprintError::NotRegular(path));
        }
    }
    Ok(())
}
