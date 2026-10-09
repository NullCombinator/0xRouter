//! `quota/fit/<provider>.json`: the stored fit state, its load and save, and the meter hash
//! (research R11, R12; contracts/state-files.md).
//!
//! This is the file layer only. The stored types hold what survives a restart: each window's
//! epoch, the hash of its declared meter, number states with their times, splits, breaks, a
//! folded prior and alerted rates. Estimates are not stored. The replay of history rows from
//! each window's epoch, which rebuilds them, is `Fits::observe` (T027, in `quota/fit/mod.rs`),
//! written separately; this file neither reads nor interprets rows.

use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nullrouter_registry::schema::MeterDecl;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::clock;
use crate::files::{self, FileError};
use crate::quota::history::DIR as QUOTA_DIR;

/// The directory under `quota/`.
const DIR: &str = "fit";
/// The file format version.
pub const VERSION: u32 = 1;

/// `quota/fit/<provider>.json` as stored.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StoredFit {
    pub v: u32,
    #[serde(default)]
    pub windows: BTreeMap<String, StoredWindow>,
}

/// One window of one plugin. Times are RFC 3339 strings.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StoredWindow {
    pub epoch: String,
    pub meter_hash: String,
    #[serde(default)]
    pub restarted: Option<Restart>,
    /// Keyed `<number>@<account>` for per-account numbers, else `<number>`.
    #[serde(default)]
    pub numbers: BTreeMap<String, StoredNumber>,
    /// Keyed by account.
    #[serde(default)]
    pub splits: BTreeMap<String, Split>,
    /// Keyed by account.
    #[serde(default)]
    pub account_epochs: BTreeMap<String, String>,
    #[serde(default)]
    pub breaks: Vec<StoredBreak>,
    #[serde(default)]
    pub prior: Option<Prior>,
    /// Keyed by model glob or part-of-day rate name, as the replay names them.
    #[serde(default)]
    pub alerted_rates: BTreeMap<String, f64>,
}

/// Why a window's epoch was reset at `at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Restart {
    pub at: String,
    pub reason: String,
}

/// A number's state and when it entered it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredNumber {
    pub state: String,
    #[serde(default)]
    pub since: Option<String>,
}

/// An account split off from the pooled numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub since: String,
    pub reason: String,
}

/// A detected break: the number that stopped fitting and the value it replaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredBreak {
    pub at: String,
    pub detected_at: String,
    pub number: String,
    pub replaced: f64,
}

/// The estimate and information of rows pruned from inside the epoch (research R11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prior {
    pub through: String,
    pub params: Vec<f64>,
    pub mean: Vec<f64>,
    pub information: Vec<Vec<f64>>,
}

/// `quota/fit/<provider>.json` under `home`. The provider name must be one path component,
/// as in `history.rs`.
pub fn fit_file(home: &Path, provider: &str) -> Result<PathBuf, FileError> {
    let dir = home.join(QUOTA_DIR).join(DIR);
    let ok = !provider.is_empty() && !provider.starts_with('.') && !provider.contains(['/', '\\', '\0']);
    if !ok {
        return Err(FileError::invalid(&dir, format!("{provider:?} is not a provider name")));
    }
    Ok(dir.join(format!("{provider}.json")))
}

fn put(text: &mut String, key: &str, value: impl fmt::Display) {
    let _ = writeln!(text, "{key}={value}");
}

/// `sha256:<hex>` of the declared meter's canonical text: one `key=value` line per field, in a
/// fixed order, globs in declared order. Equal meters hash equal; a changed field changes it.
pub fn meter_hash(m: &MeterDecl) -> String {
    let mut text = String::new();
    put(&mut text, "name", format!("{:?}", m.name));
    put(&mut text, "length_secs", m.length.as_secs_f64());
    put(&mut text, "unit", m.unit.as_str());
    put(&mut text, "capacity", format!("{:?}", m.capacity));
    let w = m.token_weights.as_ref();
    put(&mut text, "weight.input", format!("{:?}", w.map(|w| w.input)));
    put(&mut text, "weight.output", format!("{:?}", w.map(|w| w.output)));
    put(&mut text, "weight.cache_read", format!("{:?}", w.map(|w| w.cache_read)));
    put(&mut text, "weight.cache_write", format!("{:?}", w.map(|w| w.cache_write)));
    for (glob, factor) in &m.model_multiplier {
        put(&mut text, "model_multiplier", format!("{glob:?}={factor}"));
    }
    put(&mut text, "reserve", format!("{:?}", m.reserve.map(|p| p.0)));
    put(&mut text, "reset", format!("{:?}", m.reset));
    put(&mut text, "anchor", format!("{:?}", m.anchor));
    format!("sha256:{:x}", Sha256::digest(text.as_bytes()))
}

/// What [`load`] found.
#[derive(Debug, Clone, PartialEq)]
pub enum Loaded {
    /// No file: every window starts at its first poll.
    Missing,
    Ok(StoredFit),
    /// The file didn't parse (or has another version) and was renamed to `renamed_to`. The
    /// caller restarts the fit and warns.
    Bad { renamed_to: PathBuf },
}

/// Loads `quota/fit/<provider>.json`. A file that doesn't parse is renamed to
/// `<provider>.json.bad-<unix millis>` and reported as [`Loaded::Bad`]; it is never overwritten.
/// An `Err` also means the bad file could not be renamed, so the caller must not save over it.
pub fn load(home: &Path, provider: &str) -> Result<Loaded, FileError> {
    let path = fit_file(home, provider)?;
    let text = match files::read_private_file(&path) {
        Ok(None) => return Ok(Loaded::Missing),
        Ok(Some(text)) => text,
        // Not UTF-8: garbage, not an I/O failure.
        Err(FileError::Io { source, .. }) if source.kind() == io::ErrorKind::InvalidData => {
            return Ok(Loaded::Bad { renamed_to: set_aside(&path)? });
        }
        Err(e) => return Err(e),
    };
    match serde_json::from_str::<StoredFit>(&text) {
        Ok(fit) if fit.v == VERSION => Ok(Loaded::Ok(fit)),
        _ => Ok(Loaded::Bad { renamed_to: set_aside(&path)? }),
    }
}

/// Renames `path` to `<name>.bad-<unix millis>` beside it.
fn set_aside(path: &Path) -> Result<PathBuf, FileError> {
    let millis = clock::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    let to = path.with_file_name(format!("{name}.bad-{millis}"));
    fs::rename(path, &to).map_err(|source| FileError::Io { path: path.to_owned(), source })?;
    Ok(to)
}

/// Writes `quota/fit/<provider>.json` atomically, mode 0600, in directories of mode 0700. The
/// stored version is always [`VERSION`].
pub fn save(home: &Path, provider: &str, fit: &StoredFit) -> Result<(), FileError> {
    let path = fit_file(home, provider)?;
    let dir = home.join(QUOTA_DIR).join(DIR);
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|source| FileError::Io { path: dir.clone(), source })?;
    files::refuse_symlink(&path)?;
    let mut out = fit.clone();
    out.v = VERSION;
    let text = serde_json::to_string_pretty(&out).map_err(|e| FileError::invalid(&path, e.to_string()))?;
    files::write_private(&path, &text)
}

/// Deletes `quota/fit/<provider>.json` (a removed plugin's fits go with it). Returns whether a
/// file was there. A missing file is not an error; set-aside `.bad-*` copies are left alone.
pub fn remove(home: &Path, provider: &str) -> Result<bool, FileError> {
    let path = fit_file(home, provider)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(FileError::Io { path, source }),
    }
}

/// Whether the last save reached the disk. The caller keeps the fit in memory while saves
/// fail, calls [`record`](Self::record) after each save, and shows [`warning`](Self::warning).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveState {
    /// When a save last succeeded.
    pub last_ok: Option<SystemTime>,
    /// When the current run of failed saves began; `None` while saves succeed.
    pub failing_since: Option<SystemTime>,
    /// Whether the latest failure was out of space.
    pub disk_full: bool,
}

impl SaveState {
    pub fn record(&mut self, result: Result<(), FileError>, now: SystemTime) {
        match result {
            Ok(()) => {
                self.last_ok = Some(now);
                self.failing_since = None;
                self.disk_full = false;
            }
            Err(e) => {
                self.disk_full = matches!(&e, FileError::Io { source, .. } if is_disk_full(source));
                if self.failing_since.is_none() {
                    self.failing_since = Some(now);
                }
            }
        }
    }

    /// `fit state not saved since HH:MM (disk full)` while saves fail, in UTC.
    pub fn warning(&self) -> Option<String> {
        let since = self.failing_since?;
        let when = clock::rfc3339(since);
        let at = when.get(11..16).unwrap_or("??:??");
        let reason = if self.disk_full { " (disk full)" } else { "" };
        Some(format!("fit state not saved since {at}{reason}"))
    }
}

/// ENOSPC and EDQUOT, as the journal writer counts them.
fn is_disk_full(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(28 | 122)) || e.kind() == io::ErrorKind::StorageFull
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use super::*;

    fn meter(extra: &str) -> MeterDecl {
        let text = format!(
            "name = \"5-hour\"\nlength = \"5h\"\nunit = \"weighted_tokens\"\ncapacity = 1000000\n{extra}\n"
        );
        toml::from_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    fn sample() -> StoredFit {
        let window = StoredWindow {
            epoch: "2026-10-07T10:00:00.000Z".into(),
            meter_hash: meter_hash(&meter("")),
            restarted: Some(Restart {
                at: "2026-10-07T10:00:00.000Z".into(),
                reason: "plugin_meter_changed".into(),
            }),
            numbers: BTreeMap::from([
                (
                    "weight.output".to_owned(),
                    StoredNumber { state: "fitted".into(), since: Some("2026-10-08T09:10:00.000Z".into()) },
                ),
                ("capacity@max".to_owned(), StoredNumber { state: "learning".into(), since: None }),
            ]),
            splits: BTreeMap::from([(
                "team".to_owned(),
                Split { since: "2026-10-09T11:00:00.000Z".into(), reason: "weight.output 2.1x the pooled value".into() },
            )]),
            account_epochs: BTreeMap::from([("max".to_owned(), "2026-10-07T10:00:00.000Z".to_owned())]),
            breaks: vec![StoredBreak {
                at: "2026-10-13T14:00:00.000Z".into(),
                detected_at: "2026-10-13T17:20:00.000Z".into(),
                number: "capacity@max".into(),
                replaced: 13_800_000.0,
            }],
            prior: Some(Prior {
                through: "2026-10-06T00:00:00.000Z".into(),
                params: vec![0.25, 1.5],
                mean: vec![0.25, 1.5],
                information: vec![vec![2.0, 0.5], vec![0.5, 3.0]],
            }),
            alerted_rates: BTreeMap::from([("claude-opus-*".to_owned(), 0.2)]),
        };
        StoredFit { v: VERSION, windows: BTreeMap::from([("5-hour".to_owned(), window)]) }
    }

    #[test]
    fn a_saved_fit_loads_back_equal_and_private() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path(), "anthropic").unwrap(), Loaded::Missing);
        let fit = sample();
        save(dir.path(), "anthropic", &fit).unwrap();
        assert_eq!(load(dir.path(), "anthropic").unwrap(), Loaded::Ok(fit));
        let path = fit_file(dir.path(), "anthropic").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn a_garbage_file_is_renamed_aside_and_never_overwritten_silently() {
        let dir = tempfile::tempdir().unwrap();
        let path = fit_file(dir.path(), "anthropic").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        files::write_private(&path, "{ not json").unwrap();
        let Loaded::Bad { renamed_to } = load(dir.path(), "anthropic").unwrap() else { panic!("expected Bad") };
        assert!(renamed_to.exists(), "{renamed_to:?}");
        assert!(!path.exists(), "the original is gone");
        let name = renamed_to.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("anthropic.json.bad-"), "{name}");
        assert_eq!(load(dir.path(), "anthropic").unwrap(), Loaded::Missing);
    }

    #[test]
    fn a_file_that_is_not_utf8_is_set_aside_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = fit_file(dir.path(), "anthropic").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, [0xff, 0xfe, 0x00]).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(load(dir.path(), "anthropic").unwrap(), Loaded::Bad { .. }));
        assert!(!path.exists());
    }

    #[test]
    fn the_meter_hash_changes_with_one_weight_and_not_otherwise() {
        let base = meter("token_weights = { output = 15.0 }\nmodel_multiplier = { \"claude-opus-*\" = 1.5 }");
        let same = meter("token_weights = { output = 15.0 }\nmodel_multiplier = { \"claude-opus-*\" = 1.5 }");
        let other = meter("token_weights = { output = 14.0 }\nmodel_multiplier = { \"claude-opus-*\" = 1.5 }");
        assert_eq!(meter_hash(&base), meter_hash(&same));
        assert_ne!(meter_hash(&base), meter_hash(&other));
        let hash = meter_hash(&base);
        assert!(hash.starts_with("sha256:") && hash.len() == "sha256:".len() + 64, "{hash}");
    }

    #[test]
    fn a_failed_save_warns_until_one_succeeds() {
        let mut s = SaveState::default();
        let t0 = UNIX_EPOCH + Duration::from_secs(1_790_521_200); // 2026-09-27T15:00:00Z
        s.record(Ok(()), t0);
        assert_eq!(s.warning(), None);
        let full = || FileError::Io { path: "fit.json".into(), source: io::Error::from_raw_os_error(28) };
        s.record(Err(full()), t0 + Duration::from_secs(60));
        assert_eq!(s.warning().as_deref(), Some("fit state not saved since 15:01 (disk full)"));
        s.record(Err(full()), t0 + Duration::from_secs(120));
        assert_eq!(s.warning().as_deref(), Some("fit state not saved since 15:01 (disk full)"));
        s.record(Ok(()), t0 + Duration::from_secs(180));
        assert_eq!(s.warning(), None);
    }

    #[test]
    fn remove_deletes_the_fit_file_and_a_missing_one_is_fine() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        save(dir, "p", &sample()).unwrap();
        assert!(remove(dir, "p").unwrap());
        assert!(!remove(dir, "p").unwrap());
        assert!(matches!(load(dir, "p"), Ok(Loaded::Missing)));
    }
}
