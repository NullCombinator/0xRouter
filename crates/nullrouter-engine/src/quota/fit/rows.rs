//! History → rows of evidence: bridging failed polls, the set-aside rules (research R2).
//!
//! A row is the interval between two consecutive good poll entries of one account, for one
//! reported window. Rows are derived from `quota/<provider>/<account>.jsonl` and never stored.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use nullrouter_registry::schema::{MeterDecl, QuotaUnit, glob_match};

use super::TokenClass;
use crate::quota::QuotaWindow;
use crate::quota::history::Entry;
use crate::quota::tally::AccountTally;

/// The multiplier group a model belongs to: the first glob of the declared meter that matches
/// it, or `Plain` (factor 1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Plain,
    Glob(String),
}

/// Why a row is neither evidence nor outside use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetAside {
    /// The window reset inside the interval: `resets_at` changed, or `used` fell.
    Reset,
    /// An attempt in the interval has usage the provider didn't report.
    UsageUnreported,
    /// The window was exhausted at either end: a full window hides use.
    Exhausted,
}

/// How a row counts. Rows leave `rows_from` as `Evidence` or `SetAside`; classification
/// (`classify.rs`, research R6) refines `Evidence` into the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Evidence,
    /// Idle, within one step of no change: evidence of zero use.
    Idle,
    /// Busy excess beyond the predictive range; final at `until`, unless a break explains it.
    OutsideProvisional {
        until: SystemTime,
    },
    Outside,
    SetAside(SetAside),
}

/// One interval of one reported window.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub account: String,
    /// The reported window's name.
    pub window: String,
    pub start: SystemTime,
    pub end: SystemTime,
    /// The reported use change, in the window's report unit.
    pub y: f64,
    /// 0router's traffic in the interval, by multiplier group and token class.
    pub x: BTreeMap<(Group, TokenClass), u64>,
    /// Attempts in the interval.
    pub requests: u64,
    /// The interval's length in hours.
    pub hours: f64,
    pub class: Class,
}

impl Row {
    /// Whether 0router sent anything through the account in the interval.
    pub fn has_traffic(&self) -> bool {
        self.requests > 0 || self.x.values().any(|n| *n > 0)
    }
}

/// The use a window reports: `used`, else what `remaining` leaves of 100 (percent) or of the
/// limit.
pub fn reading(w: &QuotaWindow) -> Option<f64> {
    w.used.or_else(|| {
        let remaining = w.remaining?;
        match (w.unit, w.limit) {
            (QuotaUnit::Percent, _) => Some(100.0 - remaining),
            (_, Some(limit)) => Some(limit - remaining),
            _ => None,
        }
    })
}

/// Whether a window is full.
fn exhausted(w: &QuotaWindow) -> bool {
    if w.remaining.is_some_and(|r| r <= 0.0) {
        return true;
    }
    match (w.unit, reading(w)) {
        (QuotaUnit::Percent, Some(u)) => u >= 100.0,
        (_, Some(u)) => w.limit.is_some_and(|l| u >= l),
        _ => false,
    }
}

fn group_of(meter: &MeterDecl, model: &str) -> Group {
    meter.model_multiplier.keys().find(|g| glob_match(g, model)).map_or(Group::Plain, |g| Group::Glob(g.clone()))
}

/// The windows of `entry` the meter covers, by reported name.
fn windows_of<'a>(entry: &'a Entry, meter: &MeterDecl) -> impl Iterator<Item = &'a QuotaWindow> {
    let name = meter.name.clone();
    entry.windows.iter().filter(move |w| w.name == name || glob_match(&name, &w.name))
}

/// The rows `meter` sees in `history` (one account's entries, oldest first), from `since` on.
///
/// A failed poll is bridged: the interval runs from the last good entry to the next, and the
/// tally is the sum of every entry after the last good one up to and including the next. An
/// interval that starts before `since` is left out, so an epoch never contains a row that
/// began before it.
pub fn rows_from(account: &str, history: &[Entry], meter: &MeterDecl, since: SystemTime) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut prev: Option<(SystemTime, &Entry)> = None;
    let mut pending = AccountTally::new();
    for entry in history {
        let Some(at) = entry.time() else { continue };
        crate::quota::tally::merge(&mut pending, &entry.tally);
        if !entry.ok {
            continue;
        }
        if let Some((start, before)) = prev.filter(|(start, _)| *start >= since) {
            for w in windows_of(entry, meter) {
                let Some(b) = before.windows.iter().find(|b| b.name == w.name) else { continue };
                if let Some(row) = row_of(account, meter, &pending, start, at, b, w) {
                    rows.push(row);
                }
            }
        }
        prev = Some((at, entry));
        pending.clear();
    }
    rows
}

fn row_of(
    account: &str,
    meter: &MeterDecl,
    tally: &AccountTally,
    start: SystemTime,
    end: SystemTime,
    before: &QuotaWindow,
    after: &QuotaWindow,
) -> Option<Row> {
    let (u0, u1) = (reading(before)?, reading(after)?);
    let mut x = BTreeMap::new();
    let (mut requests, mut unreported) = (0, 0);
    for (model, t) in tally {
        requests += t.requests;
        unreported += t.requests_usage_unreported;
        let group = group_of(meter, model);
        for (class, n) in [
            (TokenClass::Input, t.input),
            (TokenClass::Output, t.output),
            (TokenClass::CacheRead, t.cache_read),
            (TokenClass::CacheWrite, t.cache_write),
        ] {
            if n > 0 {
                *x.entry((group.clone(), class)).or_insert(0) += n;
            }
        }
    }
    let reset = u1 < u0 || (before.resets_at.is_some() && after.resets_at.is_some() && before.resets_at != after.resets_at);
    let class = if reset {
        Class::SetAside(SetAside::Reset)
    } else if unreported > 0 {
        Class::SetAside(SetAside::UsageUnreported)
    } else if exhausted(before) || exhausted(after) {
        Class::SetAside(SetAside::Exhausted)
    } else {
        Class::Evidence
    };
    let hours = end.duration_since(start).unwrap_or(Duration::ZERO).as_secs_f64() / 3600.0;
    Some(Row { account: account.to_owned(), window: after.name.clone(), start, end, y: u1 - u0, x, requests, hours, class })
}
