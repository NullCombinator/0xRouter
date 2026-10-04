//! Quota window state: reported windows matched to declared meters (research R5, R6).
//!
//! A poll says how much of each window is left; a plugin's meter says how long the window is, what
//! it holds, what its floor is and how a request is charged. [`quota_for`] joins them, by window
//! name, into the state a placement reads. Pure: the poll, the meters and the account's own recent
//! traffic come in as arguments.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use nullrouter_registry::schema::{MeterDecl, MeterUnit, Percent, QuotaUnit, ResetKind, glob_match, parse_anchor};

use super::QuotaSource;
use crate::accounts::{RoutingOverrides, WindowOverride};
use crate::quota::extract::QuotaWindow;

/// The reserve floor of a window that declares none (contracts/routing-schema.md).
const DEFAULT_RESERVE: f64 = 0.05;

/// What a window does in the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Shapes pace and rate (length of an hour or more, with a reset).
    Pacing,
    /// Shorter than an hour: admits or refuses, never paces (FR-017).
    Admission,
    /// No reset (a credit balance): admits while above its floor, never paces.
    Balance,
}

/// One quota window of one account, as a decision sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowState {
    pub name: String,
    pub unit: MeterUnit,
    /// `None` when no meter declares it: the window is paced from its reset alone.
    pub length: Option<Duration>,
    /// In `unit`. A percent window with no declared capacity counts out of 100 (its own unit).
    pub capacity: f64,
    /// The capacity was not declared or reported (shown as `capacity assumed`).
    pub capacity_assumed: bool,
    /// Left at the last poll, in `unit`. `None` for a window no poll covered.
    pub remaining_at_poll: Option<f64>,
    pub polled_at: Option<SystemTime>,
    /// Left now, in `unit`, never below 0.
    pub remaining_now: f64,
    pub resets_at: Option<SystemTime>,
    /// The floor, as a fraction of capacity.
    pub reserve: f64,
    pub role: Role,
    /// An admission window that is full right now.
    pub refuses: bool,
    /// 0router's own counted traffic since the last poll (or since the window's reset when one
    /// has passed since, or since the window began for an estimated one), in `unit`.
    pub cost_since_poll: f64,
    /// A reset passed since the last poll: `remaining_now` counts from the reset.
    pub rolled_over: bool,
}

impl WindowState {
    /// The fraction of the window left, 0 to 1.
    pub fn fraction_left(&self) -> f64 {
        if self.capacity > 0.0 { (self.remaining_now / self.capacity).clamp(0.0, 1.0) } else { 0.0 }
    }

    /// At or below its reserve floor. An admission window has no floor: it refuses instead.
    pub fn at_floor(&self) -> bool {
        self.role != Role::Admission && self.fraction_left() <= self.reserve
    }
}

/// How the account's quota is known, with the display states of the routing view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotaState {
    pub source: QuotaSource,
    /// A report is declared but nothing has been polled yet: on pace (π = 1).
    pub pending_first_poll: bool,
    /// The latest poll failed after a good one: the estimate keeps running.
    pub stale: bool,
}

/// Everything known about one account's quota at a decision.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountQuota {
    pub state: QuotaState,
    pub polled_at: Option<SystemTime>,
    pub windows: Vec<WindowState>,
}

impl AccountQuota {
    /// A pay-as-you-go account: no windows, nothing to pace.
    pub fn payg() -> Self {
        Self {
            state: QuotaState { source: QuotaSource::PayAsYouGo, pending_first_poll: false, stale: false },
            polled_at: None,
            windows: Vec::new(),
        }
    }

    /// Any window at or below its floor.
    pub fn at_floor(&self) -> bool {
        self.windows.iter().any(WindowState::at_floor)
    }

    /// Any admission window full right now.
    pub fn refused(&self) -> bool {
        self.windows.iter().any(|w| w.refuses)
    }
}

/// One attempt's traffic through an account, as the meters charge it.
#[derive(Debug, Clone, PartialEq)]
pub struct Traffic {
    pub at: SystemTime,
    pub model: String,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// Traffic through one account under one upstream model, summed: what a window's meter charges.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Spent {
    pub model: String,
    pub requests: u64,
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

/// The traffic of one clock hour.
#[derive(Debug, Clone, PartialEq)]
pub struct Bucket {
    pub start: SystemTime,
    pub spent: Vec<Spent>,
}

const HOUR: Duration = Duration::from_secs(3600);

/// What [`quota_for`] reads.
#[derive(Debug, Clone, Copy)]
pub struct MeterInput<'a> {
    /// The plugin's `[[routing.window]]` meters.
    pub declared: &'a [MeterDecl],
    pub overrides: &'a RoutingOverrides,
    /// The provider reports this account's quota (a `[quota]` section applies).
    pub report_declared: bool,
    /// The latest good poll's windows and time.
    pub polled: Option<(&'a [QuotaWindow], SystemTime)>,
    /// A failed poll is newer than the last good one.
    pub last_poll_failed: bool,
    /// 0router's own recent traffic through the account (at least the longest admission window).
    pub recent: &'a [Traffic],
    /// Traffic counted since the last poll, by model.
    pub since_poll: &'a [Spent],
    /// Hourly traffic, oldest first, from the longest window's start: what estimated windows and
    /// windows whose reset passed since the poll count from.
    pub history: &'a [Bucket],
}

/// What one request costs a window, in the window's unit: request windows count 1, token windows
/// weigh each token class and apply the first model multiplier whose glob matches.
pub fn cost(meter: &MeterDecl, t: &Traffic) -> f64 {
    cost_spent(
        meter,
        &Spent { model: t.model.clone(), requests: 1, input: t.input, output: t.output, cache_read: t.cache_read, cache_write: t.cache_write },
    )
}

/// What the summed traffic `s` costs a window, in the window's unit.
pub fn cost_spent(meter: &MeterDecl, s: &Spent) -> f64 {
    match meter.unit {
        MeterUnit::Requests => s.requests as f64,
        MeterUnit::WeightedTokens => {
            let w = meter.token_weights.unwrap_or_default();
            let tokens = s.input as f64 * w.input
                + s.output as f64 * w.output
                + s.cache_read as f64 * w.cache_read
                + s.cache_write as f64 * w.cache_write;
            let factor = meter.model_multiplier.iter().find(|(g, _)| glob_match(g, &s.model)).map_or(1.0, |(_, f)| *f);
            tokens * factor
        }
    }
}

/// The cost of the hours that end after `from`. An hour straddling `from` counts whole.
fn cost_since(m: &MeterDecl, history: &[Bucket], from: SystemTime) -> f64 {
    history.iter().filter(|b| b.start + HOUR > from).flat_map(|b| &b.spent).map(|s| cost_spent(m, s)).sum()
}

/// Joins the poll, the meters and the account's overrides into window state (research R5).
pub fn quota_for(input: &MeterInput<'_>, now: SystemTime) -> AccountQuota {
    let has_capacity = input.declared.iter().any(|m| capacity_of(m, input.overrides).is_some());
    let (source, pending_first_poll) = match (input.report_declared, input.polled) {
        (true, Some(_)) => (QuotaSource::Polled, false),
        (true, None) => (QuotaSource::Polled, true),
        (false, _) if has_capacity => (QuotaSource::Estimated, false),
        (false, _) => return AccountQuota::payg(),
    };
    let polled_at = input.polled.map(|(_, at)| at);
    let stale = input.last_poll_failed && input.polled.is_some();
    let state = QuotaState { source, pending_first_poll, stale };

    let mut windows: Vec<WindowState> = Vec::new();
    if let Some((reported, at)) = input.polled {
        for r in reported {
            let meter = input.declared.iter().find(|m| m.name == r.name || glob_match(&m.name, &r.name));
            windows.push(reported_window(r, meter, input.overrides, (at, now), input.since_poll, input.history));
        }
    }
    // Declared meters no report covers: admission limits and, for an estimated or not yet polled
    // account, every meter with a capacity.
    for m in input.declared {
        if windows.iter().any(|w| meter_names(m, &w.name)) {
            continue;
        }
        let admission = m.is_admission();
        if !admission && source == QuotaSource::Polled && !pending_first_poll {
            continue;
        }
        let w = if !admission && source == QuotaSource::Estimated {
            estimated_window(m, input.overrides, input.history, now)
        } else {
            declared_window(m, input.overrides, input.recent, now)
        };
        windows.extend(w);
    }
    assume_capacities(&mut windows);
    AccountQuota { state, polled_at, windows }
}

fn meter_names(m: &MeterDecl, window: &str) -> bool {
    m.name == window || glob_match(&m.name, window)
}

fn window_override<'a>(o: &'a RoutingOverrides, name: &str) -> Option<&'a WindowOverride> {
    o.window.get(name)
}

/// The window's capacity in its meter's unit: the account's override, else the meter's.
fn capacity_of(m: &MeterDecl, o: &RoutingOverrides) -> Option<f64> {
    window_override(o, &m.name).and_then(|w| w.capacity).or(m.capacity)
}

fn reserve_of(m: Option<&MeterDecl>, name: &str, o: &RoutingOverrides) -> f64 {
    let pct = window_override(o, name)
        .and_then(|w| w.reserve)
        .or(o.reserve)
        .or_else(|| m.and_then(|m| m.reserve))
        .map_or(DEFAULT_RESERVE * 100.0, |p: Percent| p.0);
    pct / 100.0
}

fn length_of(m: Option<&MeterDecl>, name: &str, o: &RoutingOverrides) -> Option<Duration> {
    window_override(o, name).and_then(|w| w.length).or_else(|| m.map(|m| m.length))
}

/// A reported window. Percent and credit windows convert through the capacity when there is one.
fn reported_window(
    r: &QuotaWindow,
    meter: Option<&MeterDecl>,
    o: &RoutingOverrides,
    (polled_at, now): (SystemTime, SystemTime),
    since_poll: &[Spent],
    history: &[Bucket],
) -> WindowState {
    let unit = match (meter, r.unit) {
        (Some(m), _) => m.unit,
        (None, QuotaUnit::Requests) => MeterUnit::Requests,
        (None, _) => MeterUnit::WeightedTokens,
    };
    // How much is left of the window as a fraction, and the window's own size when it reports one.
    let reported_limit = r.limit.filter(|l| *l > 0.0);
    let (fraction, own_capacity) = match r.unit {
        QuotaUnit::Percent => {
            let left = r.remaining.or_else(|| r.used.map(|u| 100.0 - u)).unwrap_or(100.0);
            ((left / 100.0).clamp(0.0, 1.0), None)
        }
        _ => {
            let left = r.remaining.or_else(|| Some(reported_limit? - r.used?));
            match (left, reported_limit) {
                (Some(l), Some(lim)) => ((l / lim).clamp(0.0, 1.0), Some(lim)),
                (Some(l), None) => (if l > 0.0 { 1.0 } else { 0.0 }, None),
                (None, _) => (1.0, reported_limit),
            }
        }
    };
    let declared = meter.and_then(|m| capacity_of(m, o)).or_else(|| window_override(o, &r.name).and_then(|w| w.capacity));
    // A declared capacity is in the meter's unit; a reported one only when the units agree.
    let comparable = matches!(
        (r.unit, unit),
        (QuotaUnit::Requests, MeterUnit::Requests) | (QuotaUnit::Tokens, MeterUnit::WeightedTokens)
    );
    let (capacity, assumed) = match (declared, own_capacity) {
        (Some(c), _) => (c, false),
        (None, Some(c)) if comparable => (c, false),
        _ => (100.0, true),
    };
    let length = length_of(meter, &r.name, o);
    let mut remaining = fraction * capacity;
    let mut resets_at = r.resets_at;
    // Our own traffic since the poll, in the window's unit. Nothing is charged against a window
    // whose capacity is assumed: it is in the report's unit, not the meter's.
    let mut used = meter.map_or(0.0, |m| since_poll.iter().map(|s| cost_spent(m, s)).sum());
    let mut rolled_over = false;
    // A reset that has passed counts as happened: a full window less what was sent since, the
    // next reset a length later (FR-022). The next poll confirms or corrects it.
    if let (Some(at), Some(len)) = (resets_at, length)
        && at <= now
        && !len.is_zero()
    {
        let behind = now.duration_since(at).unwrap_or_default().as_secs_f64();
        let steps = (behind / len.as_secs_f64()).floor() + 1.0;
        let next = at + len.mul_f64(steps);
        used = meter.map_or(0.0, |m| cost_since(m, history, next - len));
        remaining = capacity;
        resets_at = Some(next);
        rolled_over = true;
    }
    if !assumed {
        remaining = (remaining - used).max(0.0);
    }
    let role = match (length, resets_at) {
        (Some(l), _) if l < Duration::from_secs(3600) => Role::Admission,
        (_, None) => Role::Balance,
        _ => Role::Pacing,
    };
    WindowState {
        name: r.name.clone(),
        unit,
        length,
        capacity,
        capacity_assumed: assumed,
        remaining_at_poll: Some(fraction * capacity),
        polled_at: Some(polled_at),
        remaining_now: remaining.max(0.0),
        resets_at,
        reserve: reserve_of(meter, &r.name, o),
        role,
        refuses: false,
        cost_since_poll: used,
        rolled_over,
    }
}

/// A declared meter with no report: an admission limit counted from 0router's own traffic, or the
/// window of an account that isn't polled yet (full).
fn declared_window(
    m: &MeterDecl,
    o: &RoutingOverrides,
    recent: &[Traffic],
    now: SystemTime,
) -> Option<WindowState> {
    let capacity = capacity_of(m, o)?;
    let length = length_of(Some(m), &m.name, o).unwrap_or(m.length);
    let admission = length < Duration::from_secs(3600);
    let used: f64 = if admission {
        let from = now.checked_sub(length).unwrap_or(SystemTime::UNIX_EPOCH);
        recent.iter().filter(|t| t.at > from && t.at <= now).map(|t| cost(m, t)).sum()
    } else {
        0.0
    };
    Some(WindowState {
        name: m.name.clone(),
        unit: m.unit,
        length: Some(length),
        capacity,
        capacity_assumed: false,
        remaining_at_poll: None,
        polled_at: None,
        remaining_now: (capacity - used).max(0.0),
        resets_at: None,
        reserve: reserve_of(Some(m), &m.name, o),
        role: if admission { Role::Admission } else { Role::Balance },
        refuses: admission && used >= capacity,
        cost_since_poll: used,
        rolled_over: false,
    })
}

/// A declared window nobody reports, counted from 0router's own traffic (research R6): full at
/// its start, less what was sent since. Where the window starts follows the meter's `reset`.
fn estimated_window(m: &MeterDecl, o: &RoutingOverrides, history: &[Bucket], now: SystemTime) -> Option<WindowState> {
    let capacity = capacity_of(m, o)?;
    let length = length_of(Some(m), &m.name, o).unwrap_or(m.length);
    let (start, end) = match m.reset.unwrap_or(ResetKind::Rolling) {
        ResetKind::Fixed => m.anchor.as_deref().and_then(|a| fixed_span(a, length, now)),
        ResetKind::FirstUse => Some(first_use_span(m, history, length, now)),
        ResetKind::Rolling => None,
    }
    // A rolling window frees quota continuously: it always has a whole length ahead.
    .unwrap_or((now.checked_sub(length).unwrap_or(UNIX_EPOCH), now + length));
    let used = cost_since(m, history, start);
    Some(WindowState {
        name: m.name.clone(),
        unit: m.unit,
        length: Some(length),
        capacity,
        capacity_assumed: false,
        remaining_at_poll: None,
        polled_at: None,
        remaining_now: (capacity - used).max(0.0),
        resets_at: Some(end),
        reserve: reserve_of(Some(m), &m.name, o),
        role: Role::Pacing,
        refuses: false,
        cost_since_poll: used,
        rolled_over: false,
    })
}

/// The window a `first_use` meter is in: it began with the first request after the previous one
/// ended and lasts a length. With none running, the next request starts a whole one.
fn first_use_span(m: &MeterDecl, history: &[Bucket], length: Duration, now: SystemTime) -> (SystemTime, SystemTime) {
    let mut start: Option<SystemTime> = None;
    for b in history {
        if b.spent.iter().all(|s| cost_spent(m, s) == 0.0 && s.requests == 0) {
            continue;
        }
        if start.is_none_or(|s| b.start >= s + length) {
            start = Some(b.start);
        }
    }
    match start {
        Some(s) if s + length > now => (s, s + length),
        _ => (now, now + length),
    }
}

fn unix_secs(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_secs()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_secs()).unwrap_or(i64::MAX),
    }
}

fn from_unix_secs(s: i64) -> SystemTime {
    if s >= 0 { UNIX_EPOCH + Duration::from_secs(s.unsigned_abs()) } else { UNIX_EPOCH - Duration::from_secs(s.unsigned_abs()) }
}

/// The fixed window containing `now`: `anchor + n × length`. A weekly anchor names a weekday
/// (0 is Monday); another day number is a day of the month and the windows are calendar months.
fn fixed_span(anchor: &str, length: Duration, now: SystemTime) -> Option<(SystemTime, SystemTime)> {
    const DAY: i64 = 86_400;
    let (day, minutes) = parse_anchor(anchor).ok()?;
    let offset = i64::from(minutes) * 60;
    let now_s = unix_secs(now);
    let len_s = i64::try_from(length.as_secs()).ok().filter(|l| *l > 0)?;
    if let Some(d) = day.filter(|_| len_s % (7 * DAY) != 0) {
        // The month's day, kept within 1..=28 so every month has it.
        let d = d.clamp(1, 28);
        let (y, m, _) = crate::clock::civil_from_days(now_s.div_euclid(DAY));
        let at = |y: i64, m: i64| {
            let (y, m) = (y + (m - 1).div_euclid(12), (m - 1).rem_euclid(12) + 1);
            crate::clock::days_from_civil(y, u32::try_from(m).unwrap_or(1), d) * DAY + offset
        };
        let this = at(y, i64::from(m));
        let (start, next) = if this <= now_s { (this, at(y, i64::from(m) + 1)) } else { (at(y, i64::from(m) - 1), this) };
        return Some((from_unix_secs(start), from_unix_secs(next)));
    }
    // 1970-01-01 was a Thursday: weekday 3 counting from Monday.
    let first = day.map_or(0, |d| (i64::from(d) - 3).rem_euclid(7) * DAY) + offset;
    let start = first + (now_s - first).div_euclid(len_s) * len_s;
    Some((from_unix_secs(start), from_unix_secs(start + len_s)))
}

/// A percent window with no capacity takes the median capacity of the account's other windows of
/// the same length (research R5); it stays flagged `assumed`.
fn assume_capacities(windows: &mut [WindowState]) {
    let known: Vec<(Option<Duration>, MeterUnit, f64)> =
        windows.iter().filter(|w| !w.capacity_assumed).map(|w| (w.length, w.unit, w.capacity)).collect();
    for w in windows.iter_mut().filter(|w| w.capacity_assumed) {
        let mut peers: Vec<f64> =
            known.iter().filter(|(l, u, _)| *l == w.length && *u == w.unit && l.is_some()).map(|(_, _, c)| *c).collect();
        if peers.is_empty() {
            // Paced in the report's own unit, where the meter's cost means nothing.
            w.cost_since_poll = 0.0;
            continue;
        }
        peers.sort_by(f64::total_cmp);
        let median = peers[peers.len() / 2];
        // The report's own fraction, or a whole window after a reset, in the median's unit; our
        // traffic is charged against it now that the unit is the meter's.
        let at_poll = if w.rolled_over { 1.0 } else { w.remaining_at_poll.map_or_else(|| w.fraction_left(), |r| r / w.capacity) };
        w.capacity = median;
        w.remaining_at_poll = w.remaining_at_poll.map(|_| at_poll * median);
        w.remaining_now = (at_poll * median - w.cost_since_poll).max(0.0);
    }
}

#[cfg(test)]
mod tests {
    use std::time::UNIX_EPOCH;

    use nullrouter_registry::schema::RoutingDecl;

    use super::*;

    fn t(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn meters(toml: &str) -> Vec<MeterDecl> {
        toml::from_str::<RoutingDecl>(toml).unwrap().window
    }

    fn pct(name: &str, used: f64, resets: Option<u64>) -> QuotaWindow {
        QuotaWindow {
            name: name.into(),
            unit: QuotaUnit::Percent,
            used: Some(used),
            limit: None,
            remaining: None,
            resets_at: resets.map(t),
        }
    }

    const ANTHROPIC: &str = r#"
        [[window]]
        name = "5-hour"
        length = "5h"
        unit = "weighted_tokens"
        capacity = 9_000_000
        reserve = "10%"
        [[window]]
        name = "weekly"
        length = "7d"
        unit = "weighted_tokens"
        capacity = 90_000_000
        [[window]]
        name = "per-minute"
        length = "1m"
        unit = "requests"
        capacity = 3
        [[window]]
        name = "credits"
        length = "30d"
        unit = "weighted_tokens"
        capacity = 1000
    "#;

    fn input<'a>(
        declared: &'a [MeterDecl],
        o: &'a RoutingOverrides,
        polled: Option<(&'a [QuotaWindow], SystemTime)>,
        recent: &'a [Traffic],
    ) -> MeterInput<'a> {
        MeterInput { declared, overrides: o, report_declared: true, polled, last_poll_failed: false, recent, since_poll: &[], history: &[] }
    }

    #[test]
    fn a_polled_percent_window_converts_through_the_meters_capacity() {
        let d = meters(ANTHROPIC);
        let o = RoutingOverrides::default();
        let rep = [pct("5-hour", 40.0, Some(36_000)), pct("weekly", 10.0, Some(500_000))];
        let q = quota_for(&input(&d, &o, Some((&rep, t(1000))), &[]), t(2000));
        assert_eq!(q.state, QuotaState { source: QuotaSource::Polled, pending_first_poll: false, stale: false });
        let w = &q.windows[0];
        assert_eq!((w.name.as_str(), w.role, w.capacity, w.remaining_now), ("5-hour", Role::Pacing, 9_000_000.0, 5_400_000.0));
        assert_eq!(w.reserve, 0.10);
        assert_eq!(q.windows[1].reserve, DEFAULT_RESERVE, "no reserve declared: 5%");
        assert!(!w.capacity_assumed && !q.at_floor());
    }

    #[test]
    fn roles_pacing_admission_and_balance() {
        let d = meters(ANTHROPIC);
        let o = RoutingOverrides::default();
        let rep = [pct("5-hour", 0.0, Some(36_000)), pct("credits", 50.0, None)];
        let q = quota_for(&input(&d, &o, Some((&rep, t(1000))), &[]), t(2000));
        let role = |n: &str| q.windows.iter().find(|w| w.name == n).unwrap().role;
        assert_eq!(role("5-hour"), Role::Pacing);
        assert_eq!(role("credits"), Role::Balance, "a window with no reset only admits");
        assert_eq!(role("per-minute"), Role::Admission, "declared, not reported: counted from own traffic");
    }

    #[test]
    fn an_admission_window_refuses_from_own_traffic_in_the_trailing_length() {
        let d = meters(ANTHROPIC);
        let o = RoutingOverrides::default();
        let traffic = |at| Traffic { at: t(at), model: "m".into(), input: 1, output: 1, cache_read: 0, cache_write: 0 };
        let rep = [pct("5-hour", 0.0, Some(36_000))];
        let recent = [traffic(1950), traffic(1960), traffic(1990), traffic(1800)];
        let q = quota_for(&input(&d, &o, Some((&rep, t(1000))), &recent), t(2000));
        let pm = q.windows.iter().find(|w| w.name == "per-minute").unwrap();
        assert!(pm.refuses && q.refused(), "3 requests in the last minute against a limit of 3");
        let q = quota_for(&input(&d, &o, Some((&rep, t(1000))), &recent[..2]), t(2000));
        assert!(!q.refused());
    }

    #[test]
    fn the_floor_is_the_overrides_then_the_meters_then_five_percent() {
        let d = meters(ANTHROPIC);
        let mut o = RoutingOverrides::default();
        let rep = [pct("5-hour", 91.0, Some(36_000)), pct("weekly", 96.0, Some(500_000))];
        let q = quota_for(&input(&d, &o, Some((&rep, t(1000))), &[]), t(2000));
        assert!(q.windows[0].at_floor(), "9% left is under the declared 10%");
        assert!(q.windows[1].at_floor(), "4% left is under the default 5%");
        o.reserve = Some(Percent(1.0));
        let q = quota_for(&input(&d, &o, Some((&rep, t(1000))), &[]), t(2000));
        assert!(!q.at_floor(), "the account's reserve beats the meters'");
        o.window.insert("5-hour".into(), WindowOverride { reserve: Some(Percent(20.0)), ..Default::default() });
        assert!(quota_for(&input(&d, &o, Some((&rep, t(1000))), &[]), t(2000)).windows[0].at_floor());
    }

    #[test]
    fn capacity_override_and_missing_capacity() {
        let d = meters(ANTHROPIC);
        let mut o = RoutingOverrides::default();
        o.window.insert("5-hour".into(), WindowOverride { capacity: Some(12_000_000.0), ..Default::default() });
        let rep = [pct("5-hour", 50.0, Some(36_000)), pct("mystery", 25.0, Some(36_000))];
        let q = quota_for(&input(&d, &o, Some((&rep, t(1000))), &[]), t(2000));
        assert_eq!(q.windows[0].capacity, 12_000_000.0);
        let m = &q.windows[1];
        assert!(m.capacity_assumed && m.length.is_none() && m.capacity == 100.0, "paced in its own unit: {m:?}");
        assert_eq!(m.remaining_now, 75.0);
    }

    #[test]
    fn a_percent_window_with_no_capacity_takes_the_median_of_same_length_peers() {
        let d = meters(
            r#"
            [[window]]
            name = "a"
            length = "5h"
            unit = "weighted_tokens"
            capacity = 100
            [[window]]
            name = "b"
            length = "5h"
            unit = "weighted_tokens"
            capacity = 300
            [[window]]
            name = "c"
            length = "5h"
            unit = "weighted_tokens"
            [[window]]
            name = "d"
            length = "5h"
            unit = "weighted_tokens"
            capacity = 200
        "#,
        );
        let o = RoutingOverrides::default();
        let rep = [pct("a", 0.0, Some(36_000)), pct("b", 0.0, Some(36_000)), pct("c", 50.0, Some(36_000)), pct("d", 0.0, Some(36_000))];
        let q = quota_for(&input(&d, &o, Some((&rep, t(1000))), &[]), t(2000));
        let c = &q.windows[2];
        assert!(c.capacity_assumed);
        assert_eq!((c.capacity, c.remaining_now), (200.0, 100.0), "median of 100, 200, 300; half left");
    }

    #[test]
    fn a_passed_reset_counts_as_reset_and_rolls_forward() {
        let d = meters(ANTHROPIC);
        let o = RoutingOverrides::default();
        let rep = [pct("5-hour", 90.0, Some(10_000))];
        // 5h = 18,000 s. Reset at 10,000; now 30,000 is two windows later.
        let q = quota_for(&input(&d, &o, Some((&rep, t(9000))), &[]), t(30_000));
        let w = &q.windows[0];
        assert_eq!(w.remaining_now, 9_000_000.0);
        assert_eq!(w.remaining_at_poll, Some(900_000.0));
        assert_eq!(w.resets_at, Some(t(10_000 + 2 * 18_000)));
    }

    #[test]
    fn requests_windows_and_the_cost_of_a_request() {
        let meter = &meters(
            r#"
            [[window]]
            name = "w"
            length = "5h"
            unit = "weighted_tokens"
            token_weights = { input = 1.0, output = 5.0, cache_read = 0.1, cache_write = 1.25 }
            model_multiplier = { "claude-opus-*" = 5.0, "claude-*" = 2.0 }
            [[window]]
            name = "r"
            length = "1d"
            unit = "requests"
        "#,
        );
        let usage = |model: &str| Traffic { at: t(0), model: model.into(), input: 100, output: 10, cache_read: 1000, cache_write: 80 };
        assert_eq!(cost(&meter[1], &usage("x")), 1.0);
        let base = 100.0 + 50.0 + 100.0 + 100.0;
        assert_eq!(cost(&meter[0], &usage("gpt")), base);
        assert_eq!(cost(&meter[0], &usage("claude-opus-4-1")), base * 5.0, "the first matching glob wins");
        assert_eq!(cost(&meter[0], &usage("claude-haiku")), base * 2.0);
    }

    #[test]
    fn account_kinds() {
        let d = meters(ANTHROPIC);
        let o = RoutingOverrides::default();
        // Declared report, no poll yet.
        let q = quota_for(&input(&d, &o, None, &[]), t(0));
        assert_eq!(q.state, QuotaState { source: QuotaSource::Polled, pending_first_poll: true, stale: false });
        assert!(q.windows.iter().any(|w| w.name == "5-hour" && w.remaining_now == w.capacity), "full, on pace");
        // A failed poll after a good one is stale.
        let rep = [pct("5-hour", 0.0, Some(36_000))];
        let mut i = input(&d, &o, Some((&rep, t(100))), &[]);
        i.last_poll_failed = true;
        assert!(quota_for(&i, t(200)).state.stale);
        // No report, declared capacity: estimated.
        let mut i = input(&d, &o, None, &[]);
        i.report_declared = false;
        let q = quota_for(&i, t(0));
        assert_eq!(q.state.source, QuotaSource::Estimated);
        // No report and nothing declared: pay-as-you-go.
        let mut i = input(&[], &o, None, &[]);
        i.report_declared = false;
        assert_eq!(quota_for(&i, t(0)), AccountQuota::payg());
        // Meters without a capacity don't make an account estimated.
        let nocap = meters("[[window]]\nname = \"x\"\nlength = \"5h\"\nunit = \"requests\"");
        let mut i = input(&nocap, &o, None, &[]);
        i.report_declared = false;
        assert_eq!(quota_for(&i, t(0)).state.source, QuotaSource::PayAsYouGo);
    }

    fn spent(model: &str, requests: u64, input: u64, output: u64) -> Spent {
        Spent { model: model.into(), requests, input, output, cache_read: 0, cache_write: 0 }
    }

    fn hour(h: u64, spent: Vec<Spent>) -> Bucket {
        Bucket { start: t(h * 3600), spent }
    }

    fn with<'a>(mut i: MeterInput<'a>, since: &'a [Spent], history: &'a [Bucket]) -> MeterInput<'a> {
        i.since_poll = since;
        i.history = history;
        i
    }

    const WEIGHTED: &str = r#"
        [[window]]
        name = "5-hour"
        length = "5h"
        unit = "weighted_tokens"
        capacity = 1000
        token_weights = { input = 1.0, output = 4.0 }
        model_multiplier = { "big-*" = 2.0 }
        [[window]]
        name = "daily"
        length = "1d"
        unit = "requests"
        capacity = 100
    "#;

    #[test]
    fn between_polls_the_estimate_is_the_poll_less_the_cost_of_what_was_sent() {
        let d = meters(WEIGHTED);
        let o = RoutingOverrides::default();
        let rep = [pct("5-hour", 20.0, Some(40_000)), pct("daily", 10.0, Some(80_000))];
        let since = [spent("small", 3, 100, 25), spent("big-1", 1, 50, 0)];
        let q = quota_for(&with(input(&d, &o, Some((&rep, t(1000))), &[]), &since, &[]), t(2000));
        let w = &q.windows[0];
        // 80% of 1000 left at the poll; 100 + 25*4 = 200, and 50 * 2 = 100, since.
        assert_eq!((w.remaining_at_poll, w.cost_since_poll, w.remaining_now), (Some(800.0), 300.0, 500.0));
        // A requests window counts one per request, whatever the models.
        let w = &q.windows[1];
        assert_eq!((w.remaining_at_poll, w.cost_since_poll, w.remaining_now), (Some(90.0), 4.0, 86.0));
    }

    #[test]
    fn the_estimate_never_goes_below_zero() {
        let d = meters(WEIGHTED);
        let o = RoutingOverrides::default();
        let rep = [pct("5-hour", 90.0, Some(40_000))];
        let since = [spent("small", 1, 5_000, 0)];
        let q = quota_for(&with(input(&d, &o, Some((&rep, t(1000))), &[]), &since, &[]), t(2000));
        assert_eq!(q.windows[0].remaining_now, 0.0);
        assert!(q.windows[0].at_floor());
    }

    #[test]
    fn a_passed_reset_counts_only_the_traffic_after_it() {
        let d = meters(WEIGHTED);
        let o = RoutingOverrides::default();
        // Reset at 10,000 s (hour 2, 2h46m in); now is 30,000 s, two lengths on: the window began at 28,000 s.
        let rep = [pct("5-hour", 90.0, Some(10_000))];
        let before = hour(2, vec![spent("small", 9, 900, 0)]);
        let after = hour(7, vec![spent("small", 1, 100, 25)]);
        let since = [spent("small", 10, 1000, 25)];
        let q = quota_for(&with(input(&d, &o, Some((&rep, t(9000))), &[]), &since, &[before, after]), t(30_000));
        let w = &q.windows[0];
        assert!(w.rolled_over);
        assert_eq!((w.cost_since_poll, w.remaining_now), (200.0, 800.0), "only the hour after the reset counts");
    }

    #[test]
    fn a_percent_window_with_an_assumed_capacity_is_charged_once_the_unit_is_known() {
        let d = meters(
            r#"
            [[window]]
            name = "a"
            length = "5h"
            unit = "weighted_tokens"
            capacity = 1000
            [[window]]
            name = "c"
            length = "5h"
            unit = "weighted_tokens"
        "#,
        );
        let o = RoutingOverrides::default();
        let rep = [pct("a", 0.0, Some(36_000)), pct("c", 50.0, Some(36_000))];
        let since = [spent("m", 1, 100, 0)];
        let q = quota_for(&with(input(&d, &o, Some((&rep, t(1000))), &[]), &since, &[]), t(2000));
        let c = &q.windows[1];
        assert!(c.capacity_assumed);
        assert_eq!((c.capacity, c.remaining_at_poll, c.remaining_now), (1000.0, Some(500.0), 400.0));
    }

    fn estimated(declared: &[MeterDecl], o: &RoutingOverrides, history: &[Bucket], now: u64) -> AccountQuota {
        let mut i = input(declared, o, None, &[]);
        i.report_declared = false;
        i.history = history;
        quota_for(&i, t(now))
    }

    #[test]
    fn a_rolling_window_counts_the_trailing_length() {
        let d = meters("[[window]]\nname = \"w\"\nlength = \"5h\"\nunit = \"requests\"\ncapacity = 10\nreset = \"rolling\"");
        let o = RoutingOverrides::default();
        // now = 10:30, so the trailing 5 hours begin at 5:30. Hour 4 ended at 5:00 and is out; hour 5
        // ends after 5:30 and counts whole.
        let h = [hour(4, vec![spent("m", 4, 0, 0)]), hour(5, vec![spent("m", 2, 0, 0)]), hour(9, vec![spent("m", 1, 0, 0)])];
        let q = estimated(&d, &o, &h, 10 * 3600 + 1800);
        assert_eq!(q.state.source, QuotaSource::Estimated);
        let w = &q.windows[0];
        assert_eq!((w.cost_since_poll, w.remaining_now, w.role), (3.0, 7.0, Role::Pacing));
        assert_eq!(w.resets_at, Some(t(10 * 3600 + 1800 + 5 * 3600)), "a rolling window always has a whole length ahead");
    }

    #[test]
    fn a_fixed_window_resets_at_the_anchor_plus_whole_lengths() {
        let d = meters(
            "[[window]]\nname = \"w\"\nlength = \"1d\"\nunit = \"requests\"\ncapacity = 10\nreset = \"fixed\"\nanchor = \"06:00+00:00\"",
        );
        let o = RoutingOverrides::default();
        // Day 3 at 09:00. The window began at 06:00 that day and ends at 06:00 the next.
        let now = 3 * 86_400 + 9 * 3600;
        let h = [hour(3 * 24 + 4, vec![spent("m", 5, 0, 0)]), hour(3 * 24 + 7, vec![spent("m", 2, 0, 0)])];
        let w = &estimated(&d, &o, &h, now).windows[0];
        assert_eq!((w.cost_since_poll, w.remaining_now), (2.0, 8.0));
        assert_eq!(w.resets_at, Some(t(4 * 86_400 + 6 * 3600)));
        // Before the anchor's time of day the window is the one begun the day before.
        let w = &estimated(&d, &o, &h[..1], 3 * 86_400 + 5 * 3600).windows[0];
        assert_eq!((w.cost_since_poll, w.resets_at), (5.0, Some(t(3 * 86_400 + 6 * 3600))));
    }

    #[test]
    fn a_weekly_anchor_names_its_weekday_and_a_monthly_one_its_day() {
        // 1970-01-05 was a Monday.
        let week = fixed_span("0 00:00+00:00", Duration::from_secs(7 * 86_400), t(10 * 86_400)).unwrap();
        assert_eq!(week, (t(4 * 86_400), t(11 * 86_400)), "day 10 is a Sunday; the week began Monday day 4");
        let month = fixed_span("15 12:00+00:00", Duration::from_secs(30 * 86_400), t(40 * 86_400)).unwrap();
        // Day 40 is 1970-02-10: the window began 1970-01-15 12:00 and ends 1970-02-15 12:00.
        assert_eq!(month, (t(14 * 86_400 + 12 * 3600), t(45 * 86_400 + 12 * 3600)));
    }

    #[test]
    fn a_first_use_window_starts_with_the_first_request_after_the_previous_one_ended() {
        let d = meters("[[window]]\nname = \"w\"\nlength = \"5h\"\nunit = \"requests\"\ncapacity = 10\nreset = \"first_use\"");
        let o = RoutingOverrides::default();
        // Used in hour 1 (window 1: hours 1-6), then hour 8 starts window 2 (8-13).
        let h = [hour(1, vec![spent("m", 3, 0, 0)]), hour(4, vec![spent("m", 2, 0, 0)]), hour(8, vec![spent("m", 1, 0, 0)])];
        let w = &estimated(&d, &o, &h, 10 * 3600).windows[0];
        assert_eq!((w.cost_since_poll, w.remaining_now), (1.0, 9.0));
        assert_eq!(w.resets_at, Some(t(13 * 3600)));
        // Inside window 1 the first two hours count and it ends five hours after the first request.
        let w = &estimated(&d, &o, &h[..2], 5 * 3600).windows[0];
        assert_eq!((w.remaining_now, w.resets_at), (5.0, Some(t(6 * 3600))));
        // With the last window over and nothing since, the next request opens a whole one.
        let w = &estimated(&d, &o, &h[..1], 20 * 3600).windows[0];
        assert_eq!((w.remaining_now, w.resets_at), (10.0, Some(t(25 * 3600))));
    }

    #[test]
    fn an_estimated_window_cost_uses_the_meters_weights() {
        let d = meters(WEIGHTED);
        let o = RoutingOverrides::default();
        let h = [hour(9, vec![spent("small", 2, 100, 25), spent("big-2", 1, 50, 0)])];
        let q = estimated(&d, &o, &h, 10 * 3600);
        let five = q.windows.iter().find(|w| w.name == "5-hour").unwrap();
        assert_eq!((five.cost_since_poll, five.remaining_now), (300.0, 700.0));
        let daily = q.windows.iter().find(|w| w.name == "daily").unwrap();
        assert_eq!((daily.cost_since_poll, daily.remaining_now), (3.0, 97.0));
    }
}
