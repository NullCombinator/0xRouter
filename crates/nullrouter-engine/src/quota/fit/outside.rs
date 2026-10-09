//! The outside-use list, usage alerts and acknowledgements (research R10, R12).
//!
//! `$NULLROUTER_HOME/quota/<provider>/<account>.outside.jsonl` holds one [`Line`] per
//! outside-use entry or steady rate, per alert raised, per acknowledgement and per
//! reclassification. It sits beside the account's history, is written through the history
//! file's lock so it interleaves safely with `quota prune`, and is 0600 in 0700 directories.
//!
//! This module logs nothing: no `tracing` calls. Alerts and log lines are raised by the
//! detector (T059), and only for accounts with an exclusive-use declaration (FR-021, FR-022).

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::clock;
use crate::files::FileError;
use crate::quota::history;

/// The line format version.
pub const VERSION: u32 = 1;

/// The kind of an outside-use entry (data-model § Outside-use entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutsideType {
    /// Outside use while 0router sent nothing (R6 rule 1).
    Idle,
    /// The excess of a busy interval (R6 rule 2, 3).
    Busy,
    /// A steady rate per hour, with its part of the day, still ongoing until it ends.
    Steady,
}

/// One outside-use entry as kept. Times are RFC 3339 with milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutsideEntry {
    pub v: u32,
    /// A ULID.
    pub id: String,
    pub window: String,
    #[serde(rename = "type")]
    pub ty: OutsideType,
    pub start: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_per_hour: Option<f64>,
    /// The part of the day, like `08-12`, for a steady rate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub part: Option<String>,
    pub unit: String,
    pub found_at: String,
}

impl OutsideEntry {
    /// `start` as a time; `None` when it doesn't parse.
    pub fn start_time(&self) -> Option<SystemTime> {
        clock::parse_rfc3339(&self.start)
    }
}

/// One line of the outside-use file (contracts/state-files.md).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Line {
    Entry(OutsideEntry),
    Alert {
        v: u32,
        id: String,
        /// The outside-use entry's id.
        entry: String,
        raised_at: String,
    },
    Ack {
        v: u32,
        /// The alert's id.
        alert: String,
        at: String,
    },
    /// A provisional busy entry that turned out to be a rule change (R6). Readers drop the entry.
    Reclassified {
        v: u32,
        entry: String,
        at: String,
        reason: String,
    },
}

/// `quota/<provider>/<account>.outside.jsonl`, validated like the history file.
pub fn outside_file(home: &Path, provider: &str, account: &str) -> Result<PathBuf, FileError> {
    let hist = history::history_file(home, provider, account)?;
    Ok(hist.with_file_name(format!("{account}.outside.jsonl")))
}

/// Appends `lines` to the account's outside-use file. Holds the history file's lock while it
/// writes, so it serializes with appends to the history and with `quota prune`. Writes nothing
/// when `lines` is empty.
pub fn append(home: &Path, provider: &str, account: &str, lines: &[Line]) -> Result<(), FileError> {
    if lines.is_empty() {
        return Ok(());
    }
    let path = outside_file(home, provider, account)?;
    let hist = history::history_file(home, provider, account)?;
    let mut text = String::new();
    for line in lines {
        let json = serde_json::to_string(line).map_err(|e| FileError::invalid(&path, e.to_string()))?;
        text.push_str(&json);
        text.push('\n');
    }
    // Held until the end of this function: the history file's advisory lock.
    let _lock = history::open_append(&hist)?;
    history::private_dirs(&path)?;
    let mut f = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
        .map_err(history::io_err(&path))?;
    f.write_all(text.as_bytes()).and_then(|()| f.sync_data()).map_err(history::io_err(&path))
}

/// The account's outside-use entries, newest first: at most `limit`, with `start` at or after
/// `since`. An entry named by a later `reclassified` line is dropped. Unparseable lines are
/// skipped. No file: none.
pub fn read(
    home: &Path,
    provider: &str,
    account: &str,
    since: Option<SystemTime>,
    limit: Option<usize>,
) -> Result<Vec<OutsideEntry>, FileError> {
    let path = outside_file(home, provider, account)?;
    let mut f = match File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(history::io_err(&path)(e)),
    };
    let mut out = Vec::new();
    if limit == Some(0) {
        return Ok(out);
    }
    // A reclassification is appended after its entry, so walking backwards meets it first.
    let mut reclassified: HashSet<String> = HashSet::new();
    history::lines_backwards(&mut f, |line| {
        let Ok(parsed) = serde_json::from_slice::<Line>(line) else { return true };
        let entry = match parsed {
            Line::Entry(e) => e,
            Line::Reclassified { entry, .. } => {
                reclassified.insert(entry);
                return true;
            }
            Line::Alert { .. } | Line::Ack { .. } => return true,
        };
        if reclassified.contains(&entry.id) {
            return true;
        }
        if let Some(s) = since
            && entry.start_time().is_none_or(|t| t < s)
        {
            return true;
        }
        out.push(entry);
        limit.is_none_or(|l| out.len() < l)
    })
    .map_err(history::io_err(&path))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, UNIX_EPOCH};

    use super::*;
    use crate::quota::extract::rfc3339_millis;

    fn at(secs: u64) -> String {
        rfc3339_millis(UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn entry(id: &str, ty: OutsideType, secs: u64) -> OutsideEntry {
        OutsideEntry {
            v: VERSION,
            id: id.to_owned(),
            window: "weekly".to_owned(),
            ty,
            start: at(secs),
            end: None,
            amount: Some(4.0),
            rate_per_hour: None,
            part: None,
            unit: "percent".to_owned(),
            found_at: at(secs + 60),
        }
    }

    fn ids(list: &[OutsideEntry]) -> Vec<&str> {
        list.iter().map(|e| e.id.as_str()).collect()
    }

    #[test]
    fn a_reclassified_entry_is_dropped_and_the_rest_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let first = entry("01JBA", OutsideType::Busy, 1000);
        let second = entry("01JBB", OutsideType::Idle, 2000);
        let reclass =
            Line::Reclassified { v: VERSION, entry: "01JBA".to_owned(), at: at(3000), reason: "break".to_owned() };
        append(home, "anthropic", "max", &[Line::Entry(first), Line::Entry(second.clone())]).unwrap();
        append(home, "anthropic", "max", &[reclass]).unwrap();

        let got = read(home, "anthropic", "max", None, None).unwrap();
        assert_eq!(got, vec![second]);
    }

    #[test]
    fn since_and_limit_filter_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let lines = [
            Line::Entry(entry("01JB2", OutsideType::Idle, 2000)),
            Line::Entry(entry("01JB3", OutsideType::Busy, 3000)),
            Line::Entry(entry("01JB4", OutsideType::Steady, 4000)),
        ];
        append(home, "anthropic", "max", &lines).unwrap();

        let all = read(home, "anthropic", "max", None, None).unwrap();
        assert_eq!(ids(&all), ["01JB4", "01JB3", "01JB2"]);
        let since = UNIX_EPOCH + Duration::from_secs(2500);
        let later = read(home, "anthropic", "max", Some(since), None).unwrap();
        assert_eq!(ids(&later), ["01JB4", "01JB3"]);
        let two = read(home, "anthropic", "max", None, Some(2)).unwrap();
        assert_eq!(ids(&two), ["01JB4", "01JB3"]);
        let one = read(home, "anthropic", "max", Some(since), Some(1)).unwrap();
        assert_eq!(ids(&one), ["01JB4"]);
        assert!(read(home, "anthropic", "max", None, Some(0)).unwrap().is_empty());
    }

    #[test]
    fn a_missing_file_reads_empty_and_unparseable_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        assert!(read(home, "anthropic", "max", None, None).unwrap().is_empty());

        append(home, "anthropic", "max", &[Line::Entry(entry("01JBC", OutsideType::Idle, 1000))]).unwrap();
        let path = outside_file(home, "anthropic", "max").unwrap();
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"not json\n{\"v\":1,\"kind\":\"martian\"}\n").unwrap();
        assert_eq!(ids(&read(home, "anthropic", "max", None, None).unwrap()), ["01JBC"]);
    }

    #[test]
    fn alert_and_ack_lines_parse_and_are_skipped_by_read() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let alert = Line::Alert { v: VERSION, id: "01JBD".to_owned(), entry: "01JBE".to_owned(), raised_at: at(1500) };
        let ack = Line::Ack { v: VERSION, alert: "01JBD".to_owned(), at: at(1600) };
        let lines = [Line::Entry(entry("01JBE", OutsideType::Busy, 1000)), alert, ack];
        append(home, "anthropic", "max", &lines).unwrap();
        assert_eq!(ids(&read(home, "anthropic", "max", None, None).unwrap()), ["01JBE"]);
    }

    #[test]
    fn an_entry_line_has_the_contract_shape() {
        let mut e = entry("01JBF", OutsideType::Steady, 1000);
        e.amount = None;
        e.rate_per_hour = Some(0.2);
        e.part = Some("08-12".to_owned());
        let json = serde_json::to_string(&Line::Entry(e)).unwrap();
        assert!(json.contains("\"kind\":\"entry\"") && json.contains("\"type\":\"steady\""), "{json}");
        assert!(json.contains("\"part\":\"08-12\"") && !json.contains("amount") && !json.contains("end"), "{json}");
    }

    #[test]
    fn the_file_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        append(home, "anthropic", "max", &[Line::Entry(entry("01JBG", OutsideType::Idle, 1000))]).unwrap();
        let path = outside_file(home, "anthropic", "max").unwrap();
        assert_eq!(path.file_name().unwrap(), "max.outside.jsonl");
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn appending_nothing_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "anthropic", "max", &[]).unwrap();
        assert!(!outside_file(dir.path(), "anthropic", "max").unwrap().exists());
    }
}
