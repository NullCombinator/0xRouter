//! Alerts: what an operator should look at (`adapters/alerts.toml`, data-model `Alert`).
//!
//! An alert says which adapter and version, which request (by id) and why, in fixed wording plus
//! codes. It never holds content: `detail` is built from two `&'static str`s, so a request's text
//! can't reach it (FR-025). Every alert is also logged at `warn`.

use std::fmt;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::HarnessName;
use crate::store::{Store, StoreError, VersionId, write_private};

const FILE: &str = "alerts.toml";
/// Repeats of an `adapter_failed` alert closer together than this fold into one.
pub const FOLD_WINDOW: SignedDuration = SignedDuration::from_secs(60);
/// The most alerts kept. Past it the oldest acknowledged ones go first, then the oldest.
pub const MAX_ALERTS: usize = 500;
const ID_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    Guardrail,
    AdapterFailed,
    SourceMismatch,
    ModuleRefused,
    RebuildFailed,
    Quarantined,
    Refused,
}

impl fmt::Display for AlertKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AlertKind::Guardrail => "guardrail",
            AlertKind::AdapterFailed => "adapter_failed",
            AlertKind::SourceMismatch => "source_mismatch",
            AlertKind::ModuleRefused => "module_refused",
            AlertKind::RebuildFailed => "rebuild_failed",
            AlertKind::Quarantined => "quarantined",
            AlertKind::Refused => "refused",
        })
    }
}

/// `al_` and 10 lowercase letters or digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AlertId(String);

impl AlertId {
    fn random() -> Self {
        let mut bytes = [0u8; 10];
        getrandom::fill(&mut bytes).expect("the OS random source is available");
        let tail: String = bytes.iter().map(|b| char::from(ID_CHARS[usize::from(*b) % ID_CHARS.len()])).collect();
        Self(format!("al_{tail}"))
    }

    pub fn parse(s: &str) -> Option<Self> {
        let tail = s.strip_prefix("al_")?;
        let ok = tail.len() == 10 && tail.bytes().all(|b| ID_CHARS.contains(&b));
        ok.then(|| Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AlertId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    pub id: AlertId,
    pub kind: AlertKind,
    pub harness: HarnessName,
    pub version: VersionId,
    /// The request this came from, by id. Absent for alerts raised at load or build time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<String>,
    /// A fixed message and codes, never content.
    pub detail: String,
    /// RFC 3339. For a folded alert, the latest occurrence.
    pub at: String,
    /// How many occurrences it stands for.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub count: u32,
    /// RFC 3339, once acknowledged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acked: Option<String>,
}

fn one() -> u32 {
    1
}

fn is_one(n: &u32) -> bool {
    *n == 1
}

/// What the caller says happened. `message` and `codes` are fixed strings by type.
#[derive(Debug, Clone)]
pub struct NewAlert {
    pub kind: AlertKind,
    pub harness: HarnessName,
    pub version: VersionId,
    pub record: Option<String>,
    pub message: &'static str,
    pub codes: &'static [&'static str],
}

impl NewAlert {
    fn detail(&self) -> String {
        if self.codes.is_empty() {
            self.message.to_owned()
        } else {
            format!("{}: {}", self.message, self.codes.join(","))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AlertError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("{path}: {reason}")]
    Corrupt { path: PathBuf, reason: String },
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default, rename = "alert")]
    alerts: Vec<Alert>,
}

/// The alert file. One writer at a time within a process; a second process (the CLI acking while
/// `serve` raises) replaces the file whole, so the last write wins.
#[derive(Debug)]
pub struct AlertLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AlertLog {
    pub fn open(store: &Store) -> Self {
        Self { path: store.root().join(FILE), lock: Mutex::new(()) }
    }

    fn guard(&self) -> MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn read(&self) -> Result<File, AlertError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(File::default()),
            Err(source) => return Err(StoreError::Io { path: self.path.clone(), source }.into()),
        };
        toml::from_str(&text)
            .map_err(|e| AlertError::Corrupt { path: self.path.clone(), reason: e.message().to_owned() })
    }

    fn write(&self, file: &File) -> Result<(), AlertError> {
        let text = toml::to_string(file)
            .map_err(|e| AlertError::Corrupt { path: self.path.clone(), reason: e.to_string() })?;
        Ok(write_private(&self.path, text.as_bytes())?)
    }

    /// Records an alert, or folds it into a recent identical `adapter_failed` one. Returns the
    /// alert as stored.
    pub fn raise(&self, new: NewAlert, now: Timestamp) -> Result<Alert, AlertError> {
        let detail = new.detail();
        tracing::warn!(
            kind = %new.kind,
            harness = %new.harness,
            version = %new.version,
            record = new.record.as_deref().unwrap_or(""),
            "adapter alert: {detail}"
        );
        let _held = self.guard();
        let mut file = self.read()?;

        if new.kind == AlertKind::AdapterFailed {
            let same = |a: &Alert| {
                a.kind == new.kind
                    && a.harness == new.harness
                    && a.version == new.version
                    && a.detail == detail
                    && a.acked.is_none()
                    && a.at.parse::<Timestamp>().is_ok_and(|then| now.duration_since(then) <= FOLD_WINDOW)
            };
            if let Some(found) = file.alerts.iter_mut().rev().find(|a| same(a)) {
                found.count = found.count.saturating_add(1);
                found.at = now.to_string();
                let stored = found.clone();
                self.write(&file)?;
                return Ok(stored);
            }
        }

        let id = loop {
            let id = AlertId::random();
            if file.alerts.iter().all(|a| a.id != id) {
                break id;
            }
        };
        let alert = Alert {
            id,
            kind: new.kind,
            harness: new.harness,
            version: new.version,
            record: new.record,
            detail,
            at: now.to_string(),
            count: 1,
            acked: None,
        };
        file.alerts.push(alert.clone());
        trim(&mut file.alerts);
        self.write(&file)?;
        Ok(alert)
    }

    /// Every alert, oldest first.
    pub fn list(&self) -> Result<Vec<Alert>, AlertError> {
        let _held = self.guard();
        Ok(self.read()?.alerts)
    }

    /// How many alerts are not yet acknowledged.
    pub fn unacked(&self) -> Result<usize, AlertError> {
        Ok(self.list()?.iter().filter(|a| a.acked.is_none()).count())
    }

    /// Acknowledges one alert. `false`: there is none with that id, or it was acknowledged.
    pub fn ack(&self, id: &AlertId, now: Timestamp) -> Result<bool, AlertError> {
        let _held = self.guard();
        let mut file = self.read()?;
        let Some(alert) = file.alerts.iter_mut().find(|a| &a.id == id && a.acked.is_none()) else { return Ok(false) };
        alert.acked = Some(now.to_string());
        self.write(&file)?;
        Ok(true)
    }

    /// Acknowledges every open alert and returns how many.
    pub fn ack_all(&self, now: Timestamp) -> Result<usize, AlertError> {
        let _held = self.guard();
        let mut file = self.read()?;
        let stamp = now.to_string();
        let mut n = 0;
        for alert in file.alerts.iter_mut().filter(|a| a.acked.is_none()) {
            alert.acked = Some(stamp.clone());
            n += 1;
        }
        if n > 0 {
            self.write(&file)?;
        }
        Ok(n)
    }
}

/// Keeps at most [`MAX_ALERTS`]: acknowledged alerts go first, oldest first, then the oldest.
fn trim(alerts: &mut Vec<Alert>) {
    while alerts.len() > MAX_ALERTS {
        let oldest = alerts.iter().position(|a| a.acked.is_some()).unwrap_or(0);
        alerts.remove(oldest);
    }
}
