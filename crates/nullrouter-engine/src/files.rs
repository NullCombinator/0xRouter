//! Operator files that hold secrets or digests (`accounts.toml`, `keys.toml`): read only
//! when private to the owner, written atomically with mode 0600.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("{path} is readable by other users (mode {mode:o}); run `chmod 600 {path}`")]
    NotPrivate { path: PathBuf, mode: u32 },
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{path}: {reason}")]
    Invalid { path: PathBuf, reason: String },
}

impl FileError {
    pub(crate) fn invalid(path: &Path, reason: impl Into<String>) -> Self {
        Self::Invalid { path: path.to_owned(), reason: reason.into() }
    }
}

/// The file's text, or `None` when it doesn't exist. Refuses a group- or world-readable
/// file.
pub fn read_private(path: &Path) -> Result<Option<String>, FileError> {
    let io = |source| FileError::Io { path: path.to_owned(), source };
    let meta = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(e)),
    };
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(FileError::NotPrivate { path: path.to_owned(), mode });
    }
    fs::read_to_string(path).map(Some).map_err(io)
}

/// Writes `text` to a temporary sibling created 0600, syncs it, then renames it over
/// `path`. A crash leaves either the old file or the new one.
pub fn write_private(path: &Path, text: &str) -> Result<(), FileError> {
    let io = |source| FileError::Io { path: path.to_owned(), source };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(io)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_files_are_private_and_shared_ones_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/accounts.toml");
        assert!(read_private(&path).unwrap().is_none());
        write_private(&path, "schema = 1\n").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(read_private(&path).unwrap().as_deref(), Some("schema = 1\n"));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let err = read_private(&path).unwrap_err().to_string();
        assert!(err.contains("accounts.toml") && err.contains("chmod 600"), "{err}");
    }
}
