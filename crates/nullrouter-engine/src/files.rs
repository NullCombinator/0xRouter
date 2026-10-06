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

/// `dashboard.toml` (spec 009, data-model § `dashboard.toml`): the digest of the one dashboard
/// token, never the token. Absent until the first `nullrouter dashboard token`.
pub const DASHBOARD_FILE: &str = "dashboard.toml";

/// The dashboard token's digest and when it was issued. Both `None`: no token yet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardToken {
    /// SHA-256 of the full token text (`nrd_…`), 64 lowercase hex characters.
    pub digest: Option<String>,
    /// RFC 3339 UTC.
    pub issued: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDashboardFile {
    schema: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    issued: Option<String>,
}

/// A dashboard token starts with this, then 43 base64url characters (32 random bytes).
pub const DASHBOARD_TOKEN_PREFIX: &str = "nrd_";

impl DashboardToken {
    /// A fresh token and the record to store for it. The token text is the only copy; the record
    /// holds its digest and the time.
    pub fn issue() -> (String, Self) {
        use base64::Engine as _;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("the OS random source is available");
        let token = format!("{DASHBOARD_TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes));
        let record = Self { digest: Some(Self::digest_of(&token)), issued: Some(crate::clock::now_rfc3339()) };
        (token, record)
    }

    /// The digest of `token` as `dashboard.toml` keeps it.
    pub fn digest_of(token: &str) -> String {
        use sha2::{Digest, Sha256};
        use std::fmt::Write as _;
        Sha256::digest(token.as_bytes()).iter().fold(String::with_capacity(64), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
    }

    /// The file under `home`; a missing file is no token. Refuses a shared file.
    pub fn load(home: &Path) -> Result<Self, FileError> {
        let path = home.join(DASHBOARD_FILE);
        match read_private_file(&path)? {
            None => Ok(Self::default()),
            Some(text) => Self::parse(&text, &path),
        }
    }

    fn parse(text: &str, path: &Path) -> Result<Self, FileError> {
        let raw: RawDashboardFile = toml::from_str(text).map_err(|e| FileError::toml(path, text, &e))?;
        if raw.schema != 1 {
            return Err(FileError::invalid(path, format!("schema {} is not supported (expected 1)", raw.schema)));
        }
        if let Some(d) = &raw.token_digest
            && !(d.len() == 64 && d.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return Err(FileError::invalid(path, "token_digest is not 64 lowercase hex characters"));
        }
        if let Some(t) = &raw.issued
            && crate::clock::parse_rfc3339(t).is_none()
        {
            return Err(FileError::invalid(path, "issued is not an RFC 3339 time"));
        }
        if raw.token_digest.is_some() != raw.issued.is_some() {
            return Err(FileError::invalid(path, "token_digest and issued go together"));
        }
        Ok(Self { digest: raw.token_digest, issued: raw.issued })
    }

    /// Writes the file under `home`: mode 0600, atomically.
    pub fn save(&self, home: &Path) -> Result<(), FileError> {
        let path = home.join(DASHBOARD_FILE);
        let raw = RawDashboardFile { schema: 1, token_digest: self.digest.clone(), issued: self.issued.clone() };
        let text = toml::to_string(&raw).map_err(|e| FileError::invalid(&path, e.to_string()))?;
        write_private(&path, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_issued_token_is_nrd_and_43_chars_and_only_its_digest_is_kept() {
        let (token, record) = DashboardToken::issue();
        assert!(token.starts_with("nrd_") && token.len() == 4 + 43, "{token}");
        assert!(token[4..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
        assert_eq!(record.digest, Some(DashboardToken::digest_of(&token)));
        assert!(crate::clock::parse_rfc3339(record.issued.as_deref().unwrap()).is_some());
        let (other, _) = DashboardToken::issue();
        assert_ne!(token, other);
        let dir = tempfile::tempdir().unwrap();
        record.save(dir.path()).unwrap();
        let text = fs::read_to_string(dir.path().join(DASHBOARD_FILE)).unwrap();
        assert!(!text.contains(&token), "the token itself is never stored");
    }

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
    #[test]
    fn dashboard_token_file_round_trips_private() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(DashboardToken::load(dir.path()).unwrap(), DashboardToken::default(), "missing file: no token");

        let token = DashboardToken {
            digest: Some(DashboardToken::digest_of("nrd_example")),
            issued: Some("2026-10-05T11:50:00Z".into()),
        };
        assert_eq!(token.digest.as_ref().unwrap().len(), 64);
        token.save(dir.path()).unwrap();
        let path = dir.path().join(DASHBOARD_FILE);
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains("nrd_example"), "only the digest is stored: {text}");
        assert_eq!(DashboardToken::load(dir.path()).unwrap(), token);

        // Replacing the token replaces both fields.
        let next = DashboardToken {
            digest: Some(DashboardToken::digest_of("nrd_other")),
            issued: Some("2026-10-06T08:00:00Z".into()),
        };
        next.save(dir.path()).unwrap();
        assert_eq!(DashboardToken::load(dir.path()).unwrap(), next);

        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(DashboardToken::load(dir.path()).unwrap_err().to_string().contains("chmod 600"));
    }

    #[test]
    fn dashboard_token_file_rejects_bad_content() {
        let dir = tempfile::tempdir().unwrap();
        let digest = "a".repeat(64);
        for bad in [
            "schema = 2\n".to_owned(),
            format!("schema = 1\ntoken_digest = \"{}\"\nissued = \"2026-10-05T11:50:00Z\"\n", "A".repeat(64)),
            format!("schema = 1\ntoken_digest = \"{}\"\nissued = \"2026-10-05T11:50:00Z\"\n", "a".repeat(63)),
            format!("schema = 1\ntoken_digest = \"{digest}\"\n"),
            format!("schema = 1\ntoken_digest = \"{digest}\"\nissued = \"yesterday\"\n"),
            "schema = 1\nextra = 1\n".to_owned(),
        ] {
            write_private(&dir.path().join(DASHBOARD_FILE), &bad).unwrap();
            assert!(DashboardToken::load(dir.path()).is_err(), "{bad}");
        }
        write_private(&dir.path().join(DASHBOARD_FILE), "schema = 1\n").unwrap();
        assert_eq!(DashboardToken::load(dir.path()).unwrap(), DashboardToken::default());
    }
}
