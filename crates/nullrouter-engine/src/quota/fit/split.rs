//! Pooling and split-off of an account that disagrees with the pooled weights (research R8).
//!
//! Weights and multipliers are pooled per plugin. Each account is also fitted alone, and its own
//! `ρ`/`w`/`μ` are tested against the pooled values with the always-valid test. An account that
//! disagrees is split off: its rows leave the pool and it keeps its own numbers.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use super::TokenClass;
use super::model::{self, Fit, Kind, MRow, P, Spec, Theta};
use super::test::rejects;

/// An account that left the pool, when and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub since: SystemTime,
    /// Names the number and ratio, e.g. `weight.output 2.1× the pooled value`.
    pub reason: String,
}

/// The pooled numbers the kind has: `ρ` on percent windows, `w` on counted ones, `μ` per glob
/// on either. None for request kinds.
pub fn pooled_numbers(spec: &Spec) -> Vec<P> {
    let mut out = Vec::new();
    match spec.kind {
        Kind::Percent => out.extend((0..3).map(P::Rho)),
        Kind::Counted => out.extend((0..4).map(P::W)),
        Kind::RequestsPercent | Kind::RequestsCounted => return out,
    }
    out.extend((0..spec.globs.len()).map(P::Mu));
    out
}

/// The meter-number text of a pooled parameter.
fn number_name(spec: &Spec, p: P) -> String {
    match p {
        P::Rho(0) => "weight.output".to_owned(),
        P::Rho(1) => "weight.cache_read".to_owned(),
        P::Rho(2) => "weight.cache_write".to_owned(),
        P::Rho(c) => format!("weight.rho{c}"),
        P::W(c) => format!("weight.{}", TokenClass::ALL.get(c).map_or("unknown", |t| t.as_str())),
        P::Mu(i) => format!("multiplier.{}", spec.globs.get(i).map_or("?", String::as_str)),
        P::K(a) => format!("capacity.{a}"),
        P::B(a, q) => format!("rate.{a}.{q}"),
    }
}

/// Tests each non-split account's own numbers against the pooled fit. Returns the accounts to
/// split off, each with its reason. Empty when fewer than two non-split accounts have rows
/// (research R8: a lone account has nothing to disagree with).
pub fn test_splits(
    spec: &Spec,
    rows: &[MRow],
    pooled: &Fit,
    already_split: &BTreeSet<usize>,
    now: SystemTime,
) -> BTreeMap<usize, Split> {
    let mut out = BTreeMap::new();
    let numbers = pooled_numbers(spec);
    let with_rows: BTreeSet<usize> =
        rows.iter().map(|r| r.acct).filter(|a| !already_split.contains(a) && *a < spec.accounts.len()).collect();
    if numbers.is_empty() || with_rows.len() < 2 {
        return out;
    }
    let m = with_rows.len() * numbers.len();
    for &a in &with_rows {
        let own_spec = Spec {
            kind: spec.kind,
            accounts: vec![spec.accounts[a].clone()],
            globs: spec.globs.clone(),
            utc_offset_secs: spec.utc_offset_secs,
        };
        let own_rows: Vec<MRow> =
            rows.iter().filter(|r| r.acct == a).map(|r| MRow { acct: 0, ..r.clone() }).collect();
        let mut start = Theta::neutral(1, spec.globs.len());
        if let (Some(k), Some(b)) = (pooled.theta.k.get(a), pooled.theta.b.get(a)) {
            start.k[0] = *k;
            start.b[0] = *b;
        }
        start.rho = pooled.theta.rho;
        start.w = pooled.theta.w;
        start.mu = pooled.theta.mu.clone();
        let Some(own) = model::fit(&own_spec, &own_rows, &start) else { continue };
        // (|z|, number, own/pooled ratio) of the strongest rejection.
        let mut best: Option<(f64, P, f64)> = None;
        for &p in &numbers {
            if !own.active.contains(&p) || !pooled.active.contains(&p) {
                continue;
            }
            let (Some(se_own), Some(se_pool)) = (own.se(p), pooled.se(p)) else { continue };
            let se = (se_own * se_own + se_pool * se_pool).sqrt();
            let (e, n) = (own.theta.get(p).ln(), pooled.theta.get(p).ln());
            if rejects(e, n, se, m) {
                let z = (e - n).abs() / se;
                if best.is_none_or(|(bz, ..)| z > bz) {
                    best = Some((z, p, (e - n).exp()));
                }
            }
        }
        if let Some((_, p, ratio)) = best {
            let ratio = if ratio >= 1.0 { ratio } else { 1.0 / ratio };
            out.insert(a, Split { since: now, reason: format!("{} {ratio:.1}× the pooled value", number_name(spec, p)) });
        }
    }
    out
}

/// The rows without those of the split accounts: the evidence the pool is fitted on.
pub fn without(rows: &[MRow], split: &BTreeSet<usize>) -> Vec<MRow> {
    rows.iter().filter(|r| !split.contains(&r.acct)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    fn spec(n: usize) -> Spec {
        Spec {
            kind: Kind::Percent,
            accounts: (0..n).map(|i| format!("a{i}")).collect(),
            globs: vec![],
            utc_offset_secs: 0,
        }
    }

    /// Noiseless rows for account `acct` with capacity scale `k` and output weight `rho_o`.
    fn rows_for(acct: usize, k: f64, rho_o: f64, seed: u64) -> Vec<MRow> {
        let mut g = Lcg(seed);
        (0..60)
            .map(|i| {
                let t = [(g.next() * 4000.0).round(), (g.next() * 2000.0).round(), (g.next() * 8000.0).round(), (g.next() * 1000.0).round()];
                let y = k * (t[0] + rho_o * t[1] + 0.1 * t[2] + 1.25 * t[3]);
                MRow {
                    acct,
                    y,
                    x: vec![t],
                    n: 1.0,
                    t: [0.0; model::PARTS],
                    start: f64::from(i) * 3600.0,
                    end: f64::from(i + 1) * 3600.0,
                }
            })
            .collect()
    }

    fn pooled_fit(spec: &Spec, rows: &[MRow]) -> Fit {
        let mut start = Theta::neutral(spec.accounts.len(), 0);
        start.k.iter_mut().for_each(|k| *k = 1e-3);
        model::fit(spec, rows, &start).expect("pooled fit")
    }

    #[test]
    fn agreeing_accounts_are_not_split() {
        let s = spec(3);
        let mut rows = rows_for(0, 1e-3, 15.0, 1);
        rows.extend(rows_for(1, 2e-3, 15.0, 2));
        rows.extend(rows_for(2, 5e-4, 15.0, 3));
        let pooled = pooled_fit(&s, &rows);
        assert!(test_splits(&s, &rows, &pooled, &BTreeSet::new(), SystemTime::now()).is_empty());
    }

    #[test]
    fn a_disagreeing_account_is_split_with_its_ratio() {
        let s = spec(4);
        let mut rows = rows_for(0, 1e-3, 15.0, 1);
        rows.extend(rows_for(1, 2e-3, 15.0, 2));
        rows.extend(rows_for(2, 5e-4, 15.0, 3));
        let pooled = pooled_fit(&s, &rows);
        rows.extend(rows_for(3, 1e-3, 30.0, 4));
        let splits = test_splits(&s, &rows, &pooled, &BTreeSet::new(), SystemTime::now());
        assert_eq!(splits.keys().copied().collect::<Vec<_>>(), vec![3]);
        assert!(splits[&3].reason.starts_with("weight.output 2.0×"), "{}", splits[&3].reason);
        let gone: BTreeSet<usize> = [3].into();
        assert!(without(&rows, &gone).iter().all(|r| r.acct != 3));
    }

    #[test]
    fn a_lone_account_is_never_split() {
        let s = spec(1);
        let rows = rows_for(0, 1e-3, 30.0, 5);
        let pooled = pooled_fit(&s, &rows);
        assert!(test_splits(&s, &rows, &pooled, &BTreeSet::new(), SystemTime::now()).is_empty());
    }
}
