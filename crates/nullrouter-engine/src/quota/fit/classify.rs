//! Row classification (idle, busy, provisional, outside) and separability (research R6, R7).
//!
//! Each new row is classified against the fit before it joins it. Idle rows (0router sent
//! nothing) are judged by the reading step alone; busy rows by the fit's predictive range.

use std::time::Duration;
use std::time::SystemTime;

use super::linalg::Mat;
use super::model::{self, Fit, MRow, P, Spec};
use super::rows::{Class, Row};

/// Rows a busy excess stays provisional: about an hour at 10-minute polls (R6).
pub const SETTLE_ROWS: u32 = 6;
/// A number is not separable above this variance inflation factor, or this partner correlation.
pub const VIF_LIMIT: f64 = 50.0;
pub const CORR_LIMIT: f64 = 0.98;
/// The two-sided 95% normal quantile of the predictive range.
const Z95: f64 = 1.96;

/// The class of one row against `fit` (`None`: no fit yet, so busy rows are evidence).
///
/// `step` is the provider's reading resolution (1 percent point, or 1 unit). An idle row that
/// moved by less than two steps is evidence (rounding can show one step of nothing); two steps
/// or more is outside use at once, since no rule change can make an idle interval drain
/// (SC-007). A busy row above the predictive upper range plus one step is provisionally outside.
pub fn classify(row: &Row, spec: &Spec, fit: Option<&Fit>, step: f64) -> Class {
    if matches!(row.class, Class::SetAside(_)) {
        return row.class;
    }
    if !row.has_traffic() {
        return if row.y >= 2.0 * step - 1e-9 { Class::Outside } else { Class::Idle };
    }
    let Some(fit) = fit else { return Class::Evidence };
    let Some(m) = model::prepare(spec, std::slice::from_ref(row)).into_iter().next() else { return Class::Evidence };
    if row.y <= upper(fit, spec, &m, step) {
        Class::Evidence
    } else {
        let span = row.end.duration_since(row.start).unwrap_or(Duration::ZERO);
        Class::OutsideProvisional { until: row.end + span * SETTLE_ROWS }
    }
}

/// The upper end of the 95% predictive range of `m`'s reading, plus one step.
pub fn upper(fit: &Fit, spec: &Spec, m: &MRow, step: f64) -> f64 {
    let (mean, var) = fit.predict(spec, m);
    mean + Z95 * (var + fit.row_noise()).sqrt() + step
}

/// Re-classifies a whole epoch against the current fit, in time order (a refit may narrow the
/// range, and an early busy excess then moves to outside use with its original times, R6).
///
/// A provisional row whose settle span is over is final. The break detector (R9) reclassifies
/// rows it explains separately; this function never turns a final `Outside` row back.
pub fn reclassify_epoch(rows: &mut [Row], spec: &Spec, fit: Option<&Fit>, step: f64, now: SystemTime) {
    for row in rows.iter_mut() {
        let was = row.class;
        if matches!(was, Class::SetAside(_)) {
            continue;
        }
        // Classify against the row's own data stripped of its earlier verdict.
        let mut probe = row.clone();
        probe.class = Class::Evidence;
        let fresh = classify(&probe, spec, fit, step);
        row.class = match (was, fresh) {
            // Keep the original settle deadline.
            (Class::OutsideProvisional { until }, Class::OutsideProvisional { .. }) => {
                if until <= now { Class::Outside } else { Class::OutsideProvisional { until } }
            }
            (Class::Outside, Class::OutsideProvisional { .. }) => Class::Outside,
            (_, Class::OutsideProvisional { until }) if until <= now => Class::Outside,
            (_, fresh) => fresh,
        };
    }
}

/// What the evidence rows count in the fit: everything but outside use and set-aside rows.
pub fn is_evidence(class: Class) -> bool {
    matches!(class, Class::Evidence | Class::Idle)
}

/// One number the traffic can't tell apart from another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inseparable {
    pub number: P,
    /// The parameter it is most collinear with, which may be a part-of-day outside rate.
    pub partner: Option<P>,
    pub vif: f64,
}

/// After a refit: the meter numbers (not the outside rates) that are not separable (R7). A
/// number is not separable when its variance inflation factor exceeds [`VIF_LIMIT`], or its
/// partial correlation with another parameter exceeds [`CORR_LIMIT`] in absolute value. A fit
/// whose information can't be inverted calls every number not separable.
pub fn separability(fit: &Fit) -> Vec<Inseparable> {
    let n = fit.active.len();
    let numbers = || fit.active.iter().enumerate().filter(|(_, p)| !matches!(p, P::B(..)));
    // Correlation matrix of the information.
    let mut r = Mat::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            let d = (fit.info[(i, i)] * fit.info[(j, j)]).sqrt();
            r[(i, j)] = if d > 0.0 { fit.info[(i, j)] / d } else { 0.0 };
        }
    }
    let Some(inv) = r.inverse() else {
        return numbers().map(|(_, p)| Inseparable { number: *p, partner: None, vif: f64::INFINITY }).collect();
    };
    let mut out = Vec::new();
    for (i, p) in numbers() {
        let vif = inv[(i, i)];
        let (mut best, mut partner) = (0.0f64, None);
        for j in (0..n).filter(|j| *j != i) {
            let partial = (-inv[(i, j)] / (inv[(i, i)] * inv[(j, j)]).sqrt()).abs();
            if partial > best {
                best = partial;
                partner = Some(fit.active[j]);
            }
        }
        if vif > VIF_LIMIT || best > CORR_LIMIT {
            out.push(Inseparable { number: *p, partner, vif });
        }
    }
    out
}
