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

    /// A TOML parse error of `text`, by line and column only (security review L4): the
    /// parser's own message quotes the failing line, which in `tokens.toml` or
    /// `accounts.toml` may be a secret.
    pub(crate) fn toml(path: &Path, text: &str, e: &toml::de::Error) -> Self {
        let at = e
            .span()
            .and_then(|span| text.get(..span.start))
            .map(|before| {
                let line = before.matches('\n').count() + 1;
                let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
                format!("line {line}, column {column}: ")
            })
            .unwrap_or_default();
        let message = e.message().split_whitespace().collect::<Vec<_>>().join(" ");
        Self::invalid(path, format!("{at}{message}"))
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

/// [`read_private`], refusing a symbolic link (security review L5): a link would be read
/// through, and the next write would replace it, leaving the old content at its target.
pub fn read_private_file(path: &Path) -> Result<Option<String>, FileError> {
    refuse_symlink(path)?;
    read_private(path)
}

/// An error when `path` is a symbolic link; `Ok` when it isn't or doesn't exist.
pub fn refuse_symlink(path: &Path) -> Result<(), FileError> {
    match fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(FileError::invalid(
            path,
            "is a symbolic link; 0router keeps this file itself: replace the link with the file",
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(FileError::Io { path: path.to_owned(), source }),
    }
}

/// Writes `text` to a temporary sibling created 0600, syncs it, renames it over `path`,
/// then syncs the directory (security review L9), so a power loss can't bring the old
/// file back after the rename. A crash leaves either the old file or the new one.
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
        fs::rename(&tmp, path)?;
        // Best effort: some file systems can't sync a directory; the rename stands anyway.
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
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

    #[test]
    fn parse_errors_name_the_place_not_the_line() {
        let text = "schema = 1\n[[token]]\naccess_token = tok-SENTINEL-L4\n";
        let e = toml::from_str::<toml::Table>(text).unwrap_err();
        assert!(e.to_string().contains("tok-SENTINEL-L4"), "the parser quotes the line");
        let err = FileError::toml(Path::new("tokens.toml"), text, &e).to_string();
        assert!(!err.contains("SENTINEL") && !err.contains('\n'), "{err}");
        assert!(err.starts_with("tokens.toml: line 3, column 16: "), "{err}");
    }

    #[test]
    fn a_symlinked_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("elsewhere.toml");
        write_private(&target, "schema = 1\n").unwrap();
        let link = dir.path().join("tokens.toml");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(read_private(&link).unwrap().as_deref(), Some("schema = 1\n"), "followed by the plain read");
        let err = read_private_file(&link).unwrap_err().to_string();
        assert!(err.contains("symbolic link"), "{err}");
        assert!(read_private_file(&dir.path().join("missing.toml")).unwrap().is_none());
    }
}
