//! The break detector: confidence sequences started each hour over the last 48 h (research R9).
//!
//! For each fitted number, a sequence starts at every whole hour of the last 48 hours. Each
//! refits the window from the rows after its start alone and tests the number it gets against
//! the value now in effect, with the always-valid test of R5 (`m` = 48 starts × numbers). The
//! start with the strongest rejection is the break's `at`.
//!
//! Rows scored are the evidence rows and the busy rows classified as outside use, provisional or
//! already final: a proportional excess (a rule change) must show as a break, not a leak, and a
//! burst that doesn't track traffic is one large residual among many rows, which moves the
//! traffic-direction score little. Idle outside rows are never scored: no rule change makes an
//! idle interval drain.
//!
//! This module only detects. The learner applies a break (epoch, number states, reclassifying
//! rows) because it owns the window.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::MeterNumber;
use super::classify::is_evidence;
use super::model::{self, Fit, MRow, P, Spec};
use super::rows::{Class, Row};
use super::split;
use super::test::{boundary, rejects};

/// How far back the sequences start.
pub const LOOKBACK_HOURS: u64 = 48;
/// The fewest rows a start may fit on; it also needs twice as many rows as parameters, so one
/// large residual can't be absorbed by the parameters and read as a change.
const MIN_ROWS: usize = 8;

/// A detected break (data-model § Break).
#[derive(Debug, Clone, PartialEq)]
pub struct Break {
    /// The start hour the detector chose.
    pub at: SystemTime,
    pub detected_at: SystemTime,
    pub window: String,
    pub number: MeterNumber,
    /// Set for a capacity.
    pub account: Option<String>,
    /// The fitted value that stopped being used, in natural units.
    pub replaced: f64,
}

impl Break {
    /// The number's key in the store: `<number>` or `capacity@<account>`.
    pub fn key(&self) -> String {
        match &self.account {
            Some(a) => format!("{}@{a}", self.number),
            None => self.number.to_string(),
        }
    }
}

/// A fitted number to watch.
#[derive(Debug, Clone)]
pub struct Target {
    pub number: MeterNumber,
    pub p: P,
    pub account: Option<String>,
    /// `Some` for a capacity: `C = scale / k`.
    pub scale: Option<f64>,
}

impl Target {
    fn natural(&self, fit: &Fit) -> f64 {
        let v = fit.theta.get(self.p);
        self.scale.map_or(v, |s| s / v)
    }
}

/// Whether the detector scores `r`.
fn scored(r: &Row) -> bool {
    is_evidence(r.class) || (r.has_traffic() && matches!(r.class, Class::OutsideProvisional { .. } | Class::Outside))
}

fn ceil_hour(t: SystemTime) -> SystemTime {
    let s = t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    UNIX_EPOCH + Duration::from_secs(s.div_ceil(3600) * 3600)
}

/// Whether the rejection of `t` still stands with the one row the start's fit explains worst
/// left out. A rule change shows in many rows; a burst that doesn't track traffic is one row, and
/// a few-row fit can bend to it (research R9: it must not move the traffic-direction score).
fn survives_without_worst_row(spec: &Spec, sub: &[MRow], f: &Fit, t: &Target, null: f64, m: usize, current: &Fit) -> bool {
    let miss = |r: &MRow| (r.y - f.predict(spec, r).0).abs();
    let Some(worst) = (0..sub.len()).max_by(|a, b| miss(&sub[*a]).partial_cmp(&miss(&sub[*b])).unwrap_or(std::cmp::Ordering::Equal)) else {
        return false;
    };
    let rest: Vec<MRow> = sub.iter().enumerate().filter(|(i, _)| *i != worst).map(|(_, r)| r.clone()).collect();
    if rest.len() < MIN_ROWS {
        return false;
    }
    let Some(g) = model::fit(spec, &rest, &current.theta) else { return false };
    if g.rows < 2 * g.active.len() || !g.active.contains(&t.p) {
        return false;
    }
    g.se(t.p).is_some_and(|se| rejects(t.natural(&g).ln(), null, se, m))
}

/// The breaks among `targets` as of `now`. `rows` are the epoch's rows; `current` is the fit in
/// effect (over evidence only), `skip` the split-off account indices left out of the pool, and
/// `account_epochs` the time a capacity's account last restarted (no start precedes it).
#[allow(clippy::too_many_arguments)]
pub fn detect(
    window: &str,
    spec: &Spec,
    rows: &[Row],
    skip: &BTreeSet<usize>,
    current: &Fit,
    targets: &[Target],
    epoch: SystemTime,
    account_epochs: &BTreeMap<String, SystemTime>,
    now: SystemTime,
) -> Vec<Break> {
    if targets.is_empty() {
        return Vec::new();
    }
    let lookback = now.checked_sub(Duration::from_secs(LOOKBACK_HOURS * 3600)).unwrap_or(UNIX_EPOCH);
    let counted: Vec<Row> = rows.iter().filter(|r| scored(r)).cloned().collect();
    let all: Vec<MRow> = split::without(&model::prepare(spec, &counted), skip);
    let m = LOOKBACK_HOURS as usize * targets.len();
    // Per target, the strongest rejection: (score / boundary, start).
    let mut best: Vec<Option<(f64, SystemTime)>> = vec![None; targets.len()];
    let mut start = ceil_hour(lookback.max(epoch));
    while start < now {
        let from = start.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
        let at = start;
        start += Duration::from_secs(3600);
        let sub: Vec<MRow> = all.iter().filter(|r| r.start >= from).cloned().collect();
        if sub.len() < MIN_ROWS {
            continue;
        }
        let Some(f) = model::fit(spec, &sub, &current.theta) else { continue };
        if f.rows < 2 * f.active.len() {
            continue;
        }
        for (i, t) in targets.iter().enumerate() {
            if t.account.as_ref().and_then(|a| account_epochs.get(a)).is_some_and(|e| at < *e) {
                continue;
            }
            if !f.active.contains(&t.p) || !current.active.contains(&t.p) {
                continue;
            }
            let Some(se) = f.se(t.p) else { continue };
            let (est, null) = (t.natural(&f).ln(), t.natural(current).ln());
            if rejects(est, null, se, m) && survives_without_worst_row(spec, &sub, &f, t, null, m, current) {
                let v = 1.0 / (se * se);
                let score = ((est - null) * v).abs() / boundary(v, m);
                if best[i].is_none_or(|(b, _)| score > b) {
                    best[i] = Some((score, at));
                }
            }
        }
    }
    targets
        .iter()
        .zip(best)
        .filter_map(|(t, b)| {
            let (_, at) = b?;
            Some(Break {
                at,
                detected_at: now,
                window: window.to_owned(),
                number: t.number.clone(),
                account: t.account.clone(),
                replaced: t.natural(current),
            })
        })
        .collect()
}

/// A value as the log prints it: `13.8M`.
fn human(v: f64) -> String {
    let a = v.abs();
    if a >= 1e9 {
        format!("{:.1}G", v / 1e9)
    } else if a >= 1e6 {
        format!("{:.1}M", v / 1e6)
    } else if a >= 1e3 {
        format!("{:.1}K", v / 1e3)
    } else {
        format!("{}", (v * 1000.0).round() / 1000.0)
    }
}

/// The one `warn` line per break (FR-016, contracts/cli.md § serve log). A pooled number names
/// no account: `account=*`.
pub fn log(provider: &str, b: &Break) {
    tracing::warn!(
        target: "quota.fit",
        provider,
        account = b.account.as_deref().unwrap_or("*"),
        window = b.window.as_str(),
        number = %b.number,
        around = %crate::clock::rfc3339(b.at),
        replaced = %human(b.replaced),
        "provider rules changed"
    );
}

#[cfg(test)]
mod tests {
    use super::super::model::{Kind, Theta};
    use super::super::rows::Group;
    use super::super::TokenClass;
    use super::*;

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn spec() -> Spec {
        Spec { kind: Kind::Percent, accounts: vec!["a".into()], globs: vec![], utc_offset_secs: 0 }
    }

    fn t(i: usize) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(i as u64 * 600)
    }

    /// `n` ten-minute rows; row `i` uses capacity scale `k(i)` (percent per weighted token).
    fn make(n: usize, k: impl Fn(usize) -> f64) -> Vec<Row> {
        let mut g = Lcg(7);
        (0..n)
            .map(|i| {
                let tokens = [(g.next() * 4000.0).round(), (g.next() * 2000.0).round(), (g.next() * 8000.0).round(), (g.next() * 1000.0).round()];
                let y = (k(i) * (tokens[0] + 15.0 * tokens[1] + 0.1 * tokens[2] + 1.25 * tokens[3])).round();
                let x = TokenClass::ALL.iter().zip(tokens).map(|(c, n)| ((Group::Plain, *c), n as u64)).filter(|(_, n)| *n > 0).collect();
                Row {
                    account: "a".into(),
                    window: "weekly".into(),
                    start: t(i),
                    end: t(i + 1),
                    y,
                    x,
                    requests: 1,
                    hours: 1.0 / 6.0,
                    class: Class::Evidence,
                }
            })
            .collect()
    }

    fn current(rows: &[Row]) -> Fit {
        let s = spec();
        let evidence: Vec<Row> = rows.iter().filter(|r| is_evidence(r.class)).cloned().collect();
        let mut start = Theta::neutral(1, 0);
        start.k[0] = 1e-3;
        model::fit(&s, &model::prepare(&s, &evidence), &start).expect("current fit")
    }

    fn targets() -> Vec<Target> {
        vec![
            Target { number: MeterNumber::Capacity, p: P::K(0), account: Some("a".into()), scale: Some(100.0) },
            Target { number: MeterNumber::Weight(TokenClass::Output), p: P::Rho(0), account: None, scale: None },
        ]
    }

    fn run(rows: &[Row], targets: &[Target]) -> Vec<Break> {
        let fit = current(rows);
        detect("weekly", &spec(), rows, &BTreeSet::new(), &fit, targets, UNIX_EPOCH, &BTreeMap::new(), t(rows.len()))
    }

    #[test]
    fn a_halved_capacity_is_found_near_the_hour_it_halved() {
        // 72 hours at 1e-3, then 3 hours at 2e-3 (capacity halved): the first 12 rows are final
        // outside use, the last 6 still provisional.
        let h = 72 * 6;
        let mut rows = make(h + 18, |i| if i < h { 1e-3 } else { 2e-3 });
        for (j, r) in rows.iter_mut().enumerate().skip(h) {
            r.class = if j < h + 12 { Class::Outside } else { Class::OutsideProvisional { until: t(h + 30) } };
        }
        let cap = vec![targets().remove(0)];
        let found = run(&rows, &cap);
        assert_eq!(found.len(), 1, "{found:?}");
        let b = &found[0];
        let hour = Duration::from_secs(3600);
        assert!(b.at >= t(h) - 2 * hour && b.at <= t(h) + 2 * hour, "at {:?}", b.at);
        assert_eq!(b.number, MeterNumber::Capacity);
        assert_eq!(b.key(), "capacity@a");
        assert!((b.replaced / 1e5 - 1.0).abs() < 0.1, "replaced {}", b.replaced);
    }

    #[test]
    fn stationary_noisy_data_raises_no_break() {
        let rows = make(120 * 6, |_| 1e-3);
        assert_eq!(run(&rows, &targets()), vec![]);
    }

    #[test]
    fn one_large_outside_use_row_raises_no_break() {
        let mut rows = make(120 * 6, |_| 1e-3);
        let last = rows.len() - 4;
        rows[last].y += 40.0;
        rows[last].class = Class::OutsideProvisional { until: t(rows.len() + 3) };
        assert_eq!(run(&rows, &targets()), vec![]);
    }

    #[test]
    fn the_log_value_reads_like_the_contract() {
        assert_eq!(human(13_800_000.0), "13.8M");
        assert_eq!(human(0.1), "0.1");
    }
}
