//! The dashboard change log (spec 014 R7): one JSON line per browser change, in
//! `<home>/dashboard-log.jsonl`. It lives here, not in the dashboard crate, so the CLI's
//! `dashboard log` can read it without depending on the dashboard.
//!
//! The file is mode 0600 and keeps the newest 1,000 lines; it is rewritten once it reaches 1,200.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use nullrouter_registry::OperatorHome;
use serde::{Deserialize, Serialize};

/// The file's name in the operator home.
pub const FILE: &str = "dashboard-log.jsonl";
/// The lines kept after a rewrite.
const KEEP: usize = 1000;
/// The line count at which the file is rewritten down to [`KEEP`].
const REWRITE_AT: usize = 1200;

/// How a browser change ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Done,
    SavedNotApplied,
    Started,
    Refused,
}

/// One change, as one line of the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// RFC 3339 with offset, e.g. `2026-10-22T14:03:11+02:00`.
    pub at: String,
    /// The stable op id, `<area>.<op>`, e.g. `accounts.remove`.
    pub op: String,
    /// The typed subject, e.g. `anthropic/work`.
    pub subject: String,
    pub result: Outcome,
    /// `Done.text` or `OpError.text`.
    pub text: String,
}

/// The change log's path in the operator home.
pub fn path(home: &OperatorHome) -> PathBuf {
    home.path().join(FILE)
}

/// Appends `entry` as one line, creating the file 0600 when it is missing. Once the file holds
/// 1,200 lines or more it is rewritten to its newest 1,000.
pub fn append(home: &OperatorHome, entry: &Entry) -> io::Result<()> {
    let path = path(home);
    let mut line = serde_json::to_string(entry).map_err(io::Error::other)?;
    line.push('\n');
    let mut file = OpenOptions::new().create(true).append(true).mode(0o600).open(&path)?;
    file.write_all(line.as_bytes())?;
    drop(file);
    let all = lines(&path)?;
    if all.len() >= REWRITE_AT {
        rewrite(&path, &all[all.len() - KEEP..])?;
    }
    Ok(())
}

/// The newest `limit` entries, newest first. A missing file is empty; a line that doesn't parse
/// is skipped.
pub fn read(home: &OperatorHome, limit: usize) -> io::Result<Vec<Entry>> {
    Ok(lines(&path(home))?.iter().rev().filter_map(|l| serde_json::from_str::<Entry>(l).ok()).take(limit).collect())
}

/// Every line of `path`, oldest first; a missing file has none.
fn lines(path: &Path) -> io::Result<Vec<String>> {
    match File::open(path) {
        Ok(file) => Ok(BufReader::new(file).lines().map_while(Result::ok).collect()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Writes `keep` to a 0600 temp file beside `path`, then renames it over `path`.
fn rewrite(path: &Path, keep: &[String]) -> io::Result<()> {
    let tmp = path.with_file_name(format!("{FILE}.tmp"));
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut out = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    for line in keep {
        out.write_all(line.as_bytes())?;
        out.write_all(b"\n")?;
    }
    out.sync_all()?;
    drop(out);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn entry(i: usize) -> Entry {
        Entry {
            at: format!("2026-10-22T14:03:{:02}+02:00", i % 60),
            op: "accounts.remove".into(),
            subject: format!("anthropic/work{i}"),
            result: Outcome::Done,
            text: format!("anthropic/work{i}: removed"),
        }
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn the_file_is_bounded_and_keeps_the_newest_lines() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        for i in 0..1250 {
            append(&home, &entry(i)).unwrap();
        }
        let text = std::fs::read_to_string(path(&home)).unwrap();
        let count = text.lines().count();
        assert!((1000..=1199).contains(&count), "{count} lines");
        let last = text.lines().last().unwrap();
        assert_eq!(serde_json::from_str::<Entry>(last).unwrap(), entry(1249));
        assert_eq!(read(&home, 1).unwrap(), vec![entry(1249)]);
    }

    #[test]
    fn the_file_is_mode_0600_and_stays_so_after_a_rewrite() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        append(&home, &entry(0)).unwrap();
        assert_eq!(mode(&path(&home)), 0o600);
        for i in 1..1250 {
            append(&home, &entry(i)).unwrap();
        }
        assert_eq!(mode(&path(&home)), 0o600);
    }

    #[test]
    fn read_is_newest_first_and_honours_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        assert_eq!(read(&home, 10).unwrap(), Vec::new(), "a missing file is empty");
        for i in 0..5 {
            append(&home, &entry(i)).unwrap();
        }
        let got = read(&home, 3).unwrap();
        assert_eq!(got, vec![entry(4), entry(3), entry(2)]);
        assert_eq!(read(&home, 100).unwrap().len(), 5);
    }

    #[test]
    fn a_line_that_does_not_parse_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let home = OperatorHome::new(dir.path());
        append(&home, &entry(0)).unwrap();
        let mut file = OpenOptions::new().append(true).open(path(&home)).unwrap();
        file.write_all(b"not json\n").unwrap();
        drop(file);
        append(&home, &entry(1)).unwrap();
        assert_eq!(read(&home, 10).unwrap(), vec![entry(1), entry(0)]);
    }
}
