//! The outside-use list, usage alerts and acknowledgements (research R10, R12).
//!
//! `$NULLROUTER_HOME/quota/<provider>/<account>.outside.jsonl` holds one [`Line`] per
//! outside-use entry or steady rate, per alert raised, per acknowledgement and per
//! reclassification. It sits beside the account's history, is written through the history
//! file's lock so it interleaves safely with `quota prune`, and is 0600 in 0700 directories.
//!
//! This module builds the alert lines and their text, and the `quota.alert` log line. The
//! learner decides when to raise them, and only for accounts with an exclusive-use declaration
//! at the entry's start (FR-022, FR-024).

use std::collections::{BTreeMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::clock;
use crate::files::FileError;
use crate::quota::extract::rfc3339_millis;
use crate::quota::history;

use super::model::PARTS;
use super::test::boundary;

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
        /// What the alert says. Lines written before the detector existed have none.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
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

/// A usage alert as the operator sees it (contracts/state-files.md, plus `text`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Alert {
    pub v: u32,
    /// A ULID.
    pub id: String,
    /// The outside-use entry that raised it.
    pub entry: String,
    pub raised_at: String,
    /// What the alert says. `None` for a line written without text.
    pub text: Option<String>,
}

/// The account's alerts no `ack` line names, newest first. An unreadable or missing file: none
/// (the view fails open).
pub fn alerts(home: &Path, provider: &str, account: &str) -> Vec<Alert> {
    let Ok(path) = outside_file(home, provider, account) else { return Vec::new() };
    unacknowledged(&path).unwrap_or_default()
}

/// The alerts of the file at `path` that no `ack` line names, newest first. No file: none.
fn unacknowledged(path: &Path) -> Result<Vec<Alert>, FileError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(history::io_err(path)(e)),
    };
    let mut raised: Vec<Alert> = Vec::new();
    let mut acked: HashSet<String> = HashSet::new();
    for line in text.lines() {
        match serde_json::from_str::<Line>(line) {
            Ok(Line::Alert { v, id, entry, raised_at, text }) => raised.push(Alert { v, id, entry, raised_at, text }),
            Ok(Line::Ack { alert, .. }) => {
                acked.insert(alert);
            }
            _ => {}
        }
    }
    Ok(raised.into_iter().rev().filter(|a| !acked.contains(&a.id)).collect())
}

/// What [`ack`] acknowledges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckTarget<'a> {
    /// The alert whose id is this, or starts with it: the short form the CLI shows.
    Id(&'a str),
    /// Every unacknowledged alert.
    All,
}

/// Acknowledges the account's unacknowledged alerts that `target` names (FR-027). Appends one
/// `ack` line per alert in a single write, and returns their ids. An `Id` that matches no
/// unacknowledged alert (or is empty) acknowledges nothing. An `Id` prefix that matches more than
/// one alert is an error, and writes nothing. An alert already acknowledged is not written again.
pub fn ack(
    home: &Path,
    provider: &str,
    account: &str,
    target: AckTarget<'_>,
    now: SystemTime,
) -> Result<Vec<String>, FileError> {
    let path = outside_file(home, provider, account)?;
    let open = unacknowledged(&path)?;
    let ids: Vec<String> = match target {
        AckTarget::All => open.into_iter().map(|a| a.id).collect(),
        AckTarget::Id("") => Vec::new(),
        AckTarget::Id(prefix) => {
            let mut hits = open.into_iter().filter(|a| a.id.starts_with(prefix)).map(|a| a.id);
            match (hits.next(), hits.next()) {
                (None, _) => Vec::new(),
                (Some(id), None) => vec![id],
                (Some(_), Some(_)) => {
                    return Err(FileError::invalid(&path, format!("alert id {prefix} is ambiguous; give more of it")));
                }
            }
        }
    };
    let at = rfc3339_millis(now);
    let lines: Vec<Line> = ids.iter().map(|id| Line::Ack { v: VERSION, alert: id.clone(), at: at.clone() }).collect();
    append(home, provider, account, &lines)?;
    Ok(ids)
}

/// The part of the day `q` as the entry names it: `08-12`.
pub fn part_name(q: usize) -> String {
    format!("{:02}-{:02}", q * 4, q * 4 + 4)
}

/// The key of a part's rate in `alerted_rates`: `outside use 08-12@<account>`.
pub fn rate_key(account: &str, q: usize) -> String {
    format!("outside use {}@{account}", part_name(q))
}

/// Whether `delta` (a rate, or a rate minus another, per hour, linear scale) with standard error
/// `se` is clear of 0 on the positive or negative side under the always-valid test (R5),
/// Bonferroni-split over the `PARTS` rates of the account: `|delta / se²| >= boundary(1/se², PARTS)`.
fn clear_of_zero(delta: f64, se: f64) -> bool {
    if !(se.is_finite() && se > 0.0 && delta.is_finite()) {
        return false;
    }
    let v = 1.0 / (se * se);
    (delta * v).abs() >= boundary(v, PARTS)
}

/// Whether the rate `est` is above 0 under the test. A rate at or below 0 never passes (true
/// rates are not negative).
pub fn rate_established(est: f64, se: f64) -> bool {
    est > 0.0 && clear_of_zero(est, se)
}

/// The `steady` entry to append, if any, for account `account`'s part-of-day rate `est` (with
/// standard error `se`) in `window`. `alerted` is `alerted_rates`: nothing is made while the
/// rate stays, and a new entry follows when the test on `est - last` rejects. Returns the key and the new rate to record in
/// `alerted_rates` once the line is written.
#[allow(clippy::too_many_arguments)]
pub fn steady_line(
    alerted: &BTreeMap<String, f64>,
    window: &str,
    unit: &str,
    account: &str,
    q: usize,
    (est, se): (f64, f64),
    since: SystemTime,
    now: SystemTime,
) -> Option<(String, f64, Line)> {
    if !rate_established(est, se) {
        return None;
    }
    let key = rate_key(account, q);
    if let Some(last) = alerted.get(&key)
        && !clear_of_zero(est - last, se)
    {
        return None;
    }
    let entry = OutsideEntry {
        v: VERSION,
        id: ulid::Ulid::new().to_string(),
        window: window.to_owned(),
        ty: OutsideType::Steady,
        start: rfc3339_millis(since),
        end: None,
        amount: None,
        rate_per_hour: Some(est),
        part: Some(part_name(q)),
        unit: unit.to_owned(),
        found_at: rfc3339_millis(now),
    };
    Some((key, est, Line::Entry(entry)))
}

/// The `idle` or `busy` entry for an interval of `window` from `start` to `end` that moved
/// `amount` beyond what 0router's traffic explains, found at `now`. Returns its id and the line.
pub fn interval_line(
    window: &str,
    unit: &str,
    ty: OutsideType,
    (start, end): (SystemTime, SystemTime),
    amount: f64,
    now: SystemTime,
) -> (String, Line) {
    let id = ulid::Ulid::new().to_string();
    let entry = OutsideEntry {
        v: VERSION,
        id: id.clone(),
        window: window.to_owned(),
        ty,
        start: rfc3339_millis(start),
        end: Some(rfc3339_millis(end)),
        amount: Some(amount),
        rate_per_hour: None,
        part: None,
        unit: unit.to_owned(),
        found_at: rfc3339_millis(now),
    };
    (id, Line::Entry(entry))
}

/// Whether the account was exclusive-use at `start`: declared at or before it.
pub fn exclusive_at(since: Option<SystemTime>, start: SystemTime) -> bool {
    since.is_some_and(|s| s <= start)
}

/// `n` with no decimals from 100 up, one from 10, else two, and no trailing zeros: `4`, `0.2`,
/// `12.5`, `150`.
fn num(n: f64) -> String {
    let s = if n.abs() >= 100.0 {
        format!("{n:.0}")
    } else if n.abs() >= 10.0 {
        format!("{n:.1}")
    } else {
        format!("{n:.2}")
    };
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s }
}

/// `4%` for percent, else `4 credits`.
fn amount_text(n: f64, unit: &str) -> String {
    if unit == "percent" { format!("{}%", num(n)) } else { format!("{} {unit}", num(n)) }
}

/// `HH:MM` in UTC.
fn hhmm(t: SystemTime) -> String {
    let secs = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{:02}:{:02}", secs % 86_400 / 3600, secs % 3600 / 60)
}

/// The weekday in UTC, like `Wed`.
fn weekday(t: SystemTime) -> &'static str {
    let days = t.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs() / 86_400);
    // 1970-01-01 was a Thursday.
    ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7) as usize]
}

/// What an alert for `entry` says (FR-025): facts only, UTC times, no cause named. `None` when
/// the entry's times don't parse.
pub fn alert_text(provider: &str, account: &str, entry: &OutsideEntry) -> Option<String> {
    let start = entry.start_time()?;
    let who = format!("{provider}/{account}");
    let window = &entry.window;
    if entry.ty == OutsideType::Steady {
        let rate = entry.rate_per_hour?;
        let per_hour = if entry.unit == "percent" { format!("{}%/h", num(rate)) } else { format!("{} {}/h", num(rate), entry.unit) };
        let part = entry.part.as_deref().unwrap_or("all day");
        return Some(format!(
            "{who}: about {per_hour} of {window} used {part} since {} {} with no traffic from 0router",
            weekday(start),
            hhmm(start)
        ));
    }
    let end = clock::parse_rfc3339(entry.end.as_deref()?)?;
    let amount = amount_text(entry.amount?, &entry.unit);
    let range = format!("{}\u{2013}{}", hhmm(start), hhmm(end));
    let day = weekday(start);
    Some(match entry.ty {
        OutsideType::Idle => format!("{who}: {amount} of {window} used {range} {day} with no traffic from 0router"),
        _ => format!("{who}: {amount} of {window} used {range} {day} beyond what 0router's traffic explains"),
    })
}

/// The `alert` line for `entry`, with a new id and its text. `None` when the text can't be built.
pub fn alert_line(provider: &str, account: &str, entry: &OutsideEntry, now: SystemTime) -> Option<Line> {
    let text = alert_text(provider, account, entry)?;
    Some(Line::Alert {
        v: VERSION,
        id: ulid::Ulid::new().to_string(),
        entry: entry.id.clone(),
        raised_at: rfc3339_millis(now),
        text: Some(text),
    })
}

/// Logs `WARN quota.alert: unexplained use …` for `entry`, once its alert is written (FR-024).
pub fn log_alert(provider: &str, account: &str, entry: &OutsideEntry) {
    let unit = entry.unit.as_str();
    let amount = match (entry.amount, entry.rate_per_hour) {
        (Some(a), _) => amount_text(a, unit),
        (None, Some(r)) => format!("{}/h", amount_text(r, unit)),
        (None, None) => String::new(),
    };
    tracing::warn!(
        target: "quota.alert",
        provider,
        account,
        window = entry.window.as_str(),
        start = entry.start.as_str(),
        end = entry.end.as_deref().unwrap_or(""),
        amount = amount.as_str(),
        idle = entry.ty == OutsideType::Idle,
        "unexplained use"
    );
}

/// The line that withdraws entry `entry` because its row counts as evidence again.
pub fn reclassified_line(entry: &str, reason: &str, now: SystemTime) -> Line {
    Line::Reclassified { v: VERSION, entry: entry.to_owned(), at: rfc3339_millis(now), reason: reason.to_owned() }
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
        let alert = Line::Alert { v: VERSION, id: "01JBD".to_owned(), entry: "01JBE".to_owned(), raised_at: at(1500), text: None };
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
    fn a_steady_rate_is_listed_once_and_again_when_it_changes() {
        let t0 = UNIX_EPOCH + Duration::from_secs(1000);
        let now = UNIX_EPOCH + Duration::from_secs(9000);
        let mut alerted = BTreeMap::new();
        // 0.2 per hour with se 0.02 is far from 0.
        let (k, r, line) = steady_line(&alerted, "weekly", "percent", "max", 2, (0.2, 0.02), t0, now).unwrap();
        assert_eq!(k, "outside use 08-12@max");
        let Line::Entry(e) = line else { panic!("entry") };
        assert_eq!((e.ty, e.part.as_deref(), e.rate_per_hour), (OutsideType::Steady, Some("08-12"), Some(0.2)));
        assert_eq!(e.start, at(1000));
        alerted.insert(k, r);
        // Same rate, a little noise: nothing.
        assert!(steady_line(&alerted, "weekly", "percent", "max", 2, (0.21, 0.02), t0, now).is_none());
        // Doubled: a new entry.
        assert!(steady_line(&alerted, "weekly", "percent", "max", 2, (0.4, 0.02), t0, now).is_some());
        // Another part is its own rate.
        assert!(steady_line(&alerted, "weekly", "percent", "max", 3, (0.2, 0.02), t0, now).is_some());
    }

    #[test]
    fn a_rate_that_is_not_clear_of_zero_is_not_listed() {
        let t = UNIX_EPOCH;
        let none = BTreeMap::new();
        assert!(steady_line(&none, "w", "percent", "a", 0, (0.2, 0.2), t, t).is_none());
        assert!(steady_line(&none, "w", "percent", "a", 0, (-0.5, 0.01), t, t).is_none());
        assert!(steady_line(&none, "w", "percent", "a", 0, (0.0, 0.0), t, t).is_none());
    }

    #[test]
    fn appending_nothing_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "anthropic", "max", &[]).unwrap();
        assert!(!outside_file(dir.path(), "anthropic", "max").unwrap().exists());
    }

    fn when(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn alert_line(id: &str, entry: &str, secs: u64) -> Line {
        Line::Alert { v: VERSION, id: id.to_owned(), entry: entry.to_owned(), raised_at: at(secs), text: None }
    }

    fn alert_ids(home: &Path) -> Vec<String> {
        alerts(home, "anthropic", "max").into_iter().map(|a| a.id).collect()
    }

    #[test]
    fn ack_by_full_id_leaves_the_other_alert_listed() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        append(home, "anthropic", "max", &[alert_line("01JKA1", "01JKE1", 1500), alert_line("01JKB2", "01JKE2", 1600)])
            .unwrap();

        let got = ack(home, "anthropic", "max", AckTarget::Id("01JKA1"), when(1700)).unwrap();
        assert_eq!(got, ["01JKA1"]);
        assert_eq!(alert_ids(home), ["01JKB2"]);
        // An id that names nothing acknowledges nothing and is not an error.
        assert!(ack(home, "anthropic", "max", AckTarget::Id("01JZZ"), when(1700)).unwrap().is_empty());
        assert!(ack(home, "anthropic", "max", AckTarget::Id(""), when(1700)).unwrap().is_empty());
        assert_eq!(alert_ids(home), ["01JKB2"]);
    }

    #[test]
    fn ack_by_prefix_acknowledges_the_one_alert_it_names() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        append(home, "anthropic", "max", &[alert_line("01JLA1", "01JLE1", 1500), alert_line("01JLB2", "01JLE2", 1600)])
            .unwrap();

        let got = ack(home, "anthropic", "max", AckTarget::Id("01JLB"), when(1700)).unwrap();
        assert_eq!(got, ["01JLB2"]);
        assert_eq!(alert_ids(home), ["01JLA1"]);
    }

    #[test]
    fn ack_all_acknowledges_only_the_unacknowledged_alerts() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let lines = [
            alert_line("01JMA1", "01JME1", 1500),
            alert_line("01JMB2", "01JME2", 1600),
            alert_line("01JMC3", "01JME3", 1700),
        ];
        append(home, "anthropic", "max", &lines).unwrap();
        ack(home, "anthropic", "max", AckTarget::Id("01JMA1"), when(1800)).unwrap();

        let got = ack(home, "anthropic", "max", AckTarget::All, when(1900)).unwrap();
        assert_eq!(got, ["01JMC3", "01JMB2"]);
        assert!(alert_ids(home).is_empty());
    }

    #[test]
    fn an_ambiguous_prefix_is_an_error_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        append(home, "anthropic", "max", &[alert_line("01JNA1", "01JNE1", 1500), alert_line("01JNA2", "01JNE2", 1600)])
            .unwrap();
        let path = outside_file(home, "anthropic", "max").unwrap();
        let before = fs::read_to_string(&path).unwrap();

        let err = ack(home, "anthropic", "max", AckTarget::Id("01JNA"), when(1700)).unwrap_err();
        assert!(err.to_string().contains("ambiguous"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
        assert_eq!(alert_ids(home), ["01JNA2", "01JNA1"]);
    }

    #[test]
    fn a_second_ack_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        append(home, "anthropic", "max", &[alert_line("01JPA1", "01JPE1", 1500)]).unwrap();
        assert_eq!(ack(home, "anthropic", "max", AckTarget::Id("01JPA1"), when(1700)).unwrap(), ["01JPA1"]);
        let path = outside_file(home, "anthropic", "max").unwrap();
        let before = fs::read_to_string(&path).unwrap();

        assert!(ack(home, "anthropic", "max", AckTarget::Id("01JPA1"), when(1800)).unwrap().is_empty());
        assert!(ack(home, "anthropic", "max", AckTarget::All, when(1800)).unwrap().is_empty());
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn an_acknowledged_alert_leaves_the_list_and_its_entry_stays() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        append(home, "anthropic", "max", &[Line::Entry(entry("01JQE1", OutsideType::Busy, 1000))]).unwrap();
        append(home, "anthropic", "max", &[alert_line("01JQA1", "01JQE1", 1500)]).unwrap();
        assert_eq!(alert_ids(home), ["01JQA1"]);

        ack(home, "anthropic", "max", AckTarget::Id("01JQA1"), when(1700)).unwrap();
        assert!(alert_ids(home).is_empty());
        assert_eq!(ids(&read(home, "anthropic", "max", None, None).unwrap()), ["01JQE1"]);
    }

    fn text_entry(ty: OutsideType) -> OutsideEntry {
        // 2026-10-07 is a Wednesday.
        let mut e = entry("01JTX", ty, 0);
        e.start = "2026-10-07T02:10:00.000Z".to_owned();
        e.end = Some("2026-10-07T02:30:00.000Z".to_owned());
        e
    }

    #[test]
    fn alert_texts_state_facts_in_utc_and_never_name_a_cause() {
        let idle = alert_text("anthropic", "max", &text_entry(OutsideType::Idle)).unwrap();
        assert_eq!(idle, "anthropic/max: 4% of weekly used 02:10\u{2013}02:30 Wed with no traffic from 0router");
        let busy = alert_text("anthropic", "max", &text_entry(OutsideType::Busy)).unwrap();
        assert_eq!(busy, "anthropic/max: 4% of weekly used 02:10\u{2013}02:30 Wed beyond what 0router's traffic explains");
        let mut steady = text_entry(OutsideType::Steady);
        steady.amount = None;
        steady.end = None;
        steady.rate_per_hour = Some(0.2);
        steady.part = Some("08-12".to_owned());
        let text = alert_text("anthropic", "max", &steady).unwrap();
        assert_eq!(text, "anthropic/max: about 0.2%/h of weekly used 08-12 since Wed 02:10 with no traffic from 0router");
        for t in [idle, busy, text] {
            assert!(!t.to_lowercase().contains("leak") && !t.to_lowercase().contains("key"), "{t}");
        }
        let mut credits = text_entry(OutsideType::Idle);
        credits.unit = "credits".to_owned();
        credits.amount = Some(12.5);
        assert!(alert_text("p", "a", &credits).unwrap().starts_with("p/a: 12.5 credits of weekly"));
    }

    #[test]
    fn exclusive_use_counts_from_its_declaration() {
        let t = |s| UNIX_EPOCH + Duration::from_secs(s);
        assert!(exclusive_at(Some(t(100)), t(100)));
        assert!(exclusive_at(Some(t(100)), t(200)));
        assert!(!exclusive_at(Some(t(100)), t(99)));
        assert!(!exclusive_at(None, t(200)));
    }

    #[test]
    fn an_alert_line_keeps_its_text_and_an_old_line_without_text_still_reads() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let e = text_entry(OutsideType::Idle);
        let line = super::alert_line("anthropic", "max", &e, UNIX_EPOCH).unwrap();
        append(home, "anthropic", "max", &[Line::Entry(e), line, alert_line_plain("01JZOLD", "01JTX")]).unwrap();
        let got = alerts(home, "anthropic", "max");
        assert_eq!(got.len(), 2);
        assert!(got.iter().any(|a| a.text.as_deref().is_some_and(|t| t.contains("with no traffic from 0router"))));
        assert!(got.iter().any(|a| a.id == "01JZOLD" && a.text.is_none()));
    }

    fn alert_line_plain(id: &str, entry: &str) -> Line {
        Line::Alert { v: VERSION, id: id.to_owned(), entry: entry.to_owned(), raised_at: at(1500), text: None }
    }
}
