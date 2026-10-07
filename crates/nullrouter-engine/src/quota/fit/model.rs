//! The per-window model: parameters, prediction, Gauss–Newton, sandwich covariance (research R3, R4).
//!
//! ```text
//! percent window:   E[y] = k_a · Σ_g μ_g · (I_g + ρ_o·O_g + ρ_r·R_g + ρ_w·W_g) + b_{a,q}·t
//! counted window:   E[y] =       Σ_g μ_g · (w_i·I_g + w_o·O_g + w_r·R_g + w_w·W_g) + b_{a,q}·t
//! requests:         E[y] = k_a · N + b_{a,q}·t   (percent)   or   N + b_{a,q}·t   (counted)
//! ```
//!
//! `k_a`, `ρ`, `w` and `μ` are positive and fitted in log space; the steady outside rates
//! `b_{a,q}` are linear and fitted unconstrained (read through `Theta::rate`, which clamps at 0). Group 0 is the models no glob matches (factor 1).
//! The fit is a pure function of rows and a starting point.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use super::TokenClass;
use super::linalg::Mat;
use super::rows::{Group, Row};

/// Parts of the day an account's steady outside rate is kept for (four hours each).
pub const PARTS: usize = 6;
/// Seconds in a part of the day.
const PART_SECS: f64 = 4.0 * 3600.0;
/// The variance of one reading rounded to a whole step, and of a row (a difference of two).
const READING_VAR: f64 = 1.0 / 12.0;
const ROW_VAR: f64 = 2.0 * READING_VAR;
/// The longest Gauss–Newton run, and the deepest step halving.
const MAX_ITER: usize = 20;
const MAX_HALVINGS: u32 = 10;

/// What the window counts and how it reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Weighted tokens, reported as percent: `k_a` carries the capacity.
    Percent,
    /// Weighted tokens, reported in absolute units: the weights are the numbers.
    Counted,
    /// Requests, reported as percent.
    RequestsPercent,
    /// Requests, reported in absolute units.
    RequestsCounted,
}

impl Kind {
    fn has_k(self) -> bool {
        matches!(self, Self::Percent | Self::RequestsPercent)
    }

    fn tokens(self) -> bool {
        matches!(self, Self::Percent | Self::Counted)
    }
}

/// The shape of one window's problem.
#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub kind: Kind,
    /// Accounts, in the order of `Theta::k` and `Theta::b`.
    pub accounts: Vec<String>,
    /// The meter's multiplier globs, in declared order; `Theta::mu` follows them.
    pub globs: Vec<String>,
    /// Seconds east of UTC, for the part of the day (operator local time).
    pub utc_offset_secs: i64,
}

/// One parameter of the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum P {
    /// `k_a` of account `a`.
    K(usize),
    /// `b_{a,q}`.
    B(usize, usize),
    /// `ρ` of output (0), cache read (1), cache write (2), relative to input.
    Rho(usize),
    /// `w` of input (0), output (1), cache read (2), cache write (3), on a counted window.
    W(usize),
    /// `μ` of glob `i`.
    Mu(usize),
}

impl P {
    /// Whether the parameter is fitted in log space (all but the steady rates).
    pub fn is_log(self) -> bool {
        !matches!(self, Self::B(..))
    }
}

/// The parameter values.
#[derive(Debug, Clone, PartialEq)]
pub struct Theta {
    pub k: Vec<f64>,
    pub b: Vec<[f64; PARTS]>,
    pub rho: [f64; 3],
    pub w: [f64; 4],
    pub mu: Vec<f64>,
}

impl Theta {
    /// Every parameter at the neutral value: `k = 1`, weights and multipliers 1, no outside use.
    pub fn neutral(accounts: usize, globs: usize) -> Self {
        Self { k: vec![1.0; accounts], b: vec![[0.0; PARTS]; accounts], rho: [1.0; 3], w: [1.0; 4], mu: vec![1.0; globs] }
    }

    /// The steady outside rate of account `a` in part `q`, never below 0.
    pub fn rate(&self, a: usize, q: usize) -> f64 {
        self.b[a][q].max(0.0)
    }

    pub fn get(&self, p: P) -> f64 {
        match p {
            P::K(a) => self.k[a],
            P::B(a, q) => self.b[a][q],
            P::Rho(c) => self.rho[c],
            P::W(c) => self.w[c],
            P::Mu(i) => self.mu[i],
        }
    }

    pub fn set(&mut self, p: P, v: f64) {
        match p {
            P::K(a) => self.k[a] = v,
            P::B(a, q) => self.b[a][q] = v,
            P::Rho(c) => self.rho[c] = v,
            P::W(c) => self.w[c] = v,
            P::Mu(i) => self.mu[i] = v,
        }
    }

    /// Moves `p` by `delta`: multiplicatively (`exp`) for log parameters, additively for the
    /// steady rates. The rates are not clamped here: a rate near 0 is estimated below it about as
    /// often as above, and clamping inside the search stalls it and biases the other numbers.
    /// Readers clamp with [`Theta::rate`].
    fn step(&mut self, p: P, delta: f64) {
        let v = self.get(p);
        self.set(p, if p.is_log() { v * delta.exp() } else { v + delta });
    }
}

/// One row prepared for the model.
#[derive(Debug, Clone, PartialEq)]
pub struct MRow {
    pub acct: usize,
    pub y: f64,
    /// Tokens `[input, output, cache_read, cache_write]` by group; index 0 is "no multiplier".
    pub x: Vec<[f64; 4]>,
    /// Attempts, for request windows.
    pub n: f64,
    /// Hours in each part of the day.
    pub t: [f64; PARTS],
    /// Interval ends, in seconds since the epoch (rows that touch share a reading).
    pub start: f64,
    pub end: f64,
}

fn secs(t: SystemTime) -> f64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// Hours of `[start, end]` (seconds since the epoch) in each part of the local day.
pub fn hours_by_part(start: f64, end: f64, utc_offset_secs: i64) -> [f64; PARTS] {
    let mut out = [0.0; PARTS];
    let off = utc_offset_secs as f64;
    let mut at = start;
    while at < end {
        let local = at + off;
        let part_index = (local / PART_SECS).floor();
        let next = (part_index + 1.0) * PART_SECS - off;
        // Always advance, even when rounding puts the boundary at `at`.
        let to = next.min(end).max(at + 1e-6);
        let q = (part_index.rem_euclid(PARTS as f64)) as usize;
        out[q] += (to - at) / 3600.0;
        at = to;
    }
    out
}

/// The rows as the model sees them, ordered by account, then time. `accounts` names the
/// account index of each row's `account`; a row of an unknown account is dropped.
pub fn prepare(spec: &Spec, rows: &[Row]) -> Vec<MRow> {
    let mut out: Vec<MRow> = rows
        .iter()
        .filter_map(|r| {
            let acct = spec.accounts.iter().position(|a| *a == r.account)?;
            let mut x = vec![[0.0; 4]; spec.globs.len() + 1];
            for ((g, class), n) in &r.x {
                let gi = match g {
                    Group::Plain => 0,
                    Group::Glob(name) => spec.globs.iter().position(|s| s == name).map_or(0, |i| i + 1),
                };
                let ci = TokenClass::ALL.iter().position(|c| c == class).unwrap_or(0);
                x[gi][ci] += *n as f64;
            }
            let (start, end) = (secs(r.start), secs(r.end));
            Some(MRow {
                acct,
                y: r.y,
                x,
                n: r.requests as f64,
                t: hours_by_part(start, end, spec.utc_offset_secs),
                start,
                end,
            })
        })
        .collect();
    out.sort_by(|a, b| (a.acct, a.start).partial_cmp(&(b.acct, b.start)).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// The expected `y` of `row`, and, when `grad` is given, `∂E[y]/∂(log p)` (`∂E[y]/∂p` for a
/// steady rate) for each parameter of `active`.
fn eval(spec: &Spec, th: &Theta, row: &MRow, active: &[P], grad: Option<&mut [f64]>) -> f64 {
    let kind = spec.kind;
    let scale = if kind.has_k() { th.k[row.acct] } else { 1.0 };
    // Token weights per class, by kind.
    let wt: [f64; 4] = match kind {
        Kind::Percent => [1.0, th.rho[0], th.rho[1], th.rho[2]],
        Kind::Counted => th.w,
        _ => [0.0; 4],
    };
    let mu = |g: usize| if g == 0 { 1.0 } else { th.mu[g - 1] };
    let group_sum = |g: usize| (0..4).map(|c| wt[c] * row.x[g][c]).sum::<f64>();
    let core = if kind.tokens() { (0..row.x.len()).map(|g| mu(g) * group_sum(g)).sum::<f64>() } else { row.n };
    let outside: f64 = (0..PARTS).map(|q| th.b[row.acct][q] * row.t[q]).sum();
    if let Some(g) = grad {
        for (slot, p) in g.iter_mut().zip(active) {
            *slot = match *p {
                P::K(a) if a == row.acct && kind.has_k() => scale * core,
                P::B(a, q) if a == row.acct => row.t[q],
                P::Rho(c) if kind == Kind::Percent => {
                    scale * (0..row.x.len()).map(|g| mu(g) * th.rho[c] * row.x[g][c + 1]).sum::<f64>()
                }
                P::W(c) if kind == Kind::Counted => (0..row.x.len()).map(|g| mu(g) * th.w[c] * row.x[g][c]).sum::<f64>(),
                P::Mu(i) if kind.tokens() => scale * th.mu[i] * group_sum(i + 1),
                _ => 0.0,
            };
        }
    }
    scale * core + outside
}

/// The parameters the kind has at all, before dropping those no row informs.
fn candidates(spec: &Spec, th: &Theta) -> Vec<P> {
    let mut out = Vec::new();
    if spec.kind.has_k() {
        out.extend((0..th.k.len()).map(P::K));
    }
    for a in 0..th.b.len() {
        out.extend((0..PARTS).map(|q| P::B(a, q)));
    }
    match spec.kind {
        Kind::Percent => out.extend((0..3).map(P::Rho)),
        Kind::Counted => out.extend((0..4).map(P::W)),
        _ => {}
    }
    if spec.kind.tokens() {
        out.extend((0..th.mu.len()).map(P::Mu));
    }
    out
}

/// A fitted window.
#[derive(Debug, Clone)]
pub struct Fit {
    pub theta: Theta,
    /// The parameters that were fitted: those some row informs.
    pub active: Vec<P>,
    /// Covariance of `active`, in log space for log parameters.
    pub cov: Mat,
    /// Information `JᵀJ` of `active` (separability reads it, research R7).
    pub info: Mat,
    pub rows: usize,
    pub rss: f64,
    /// The extra variance beyond rounding, floored at 0.
    pub sigma_e2: f64,
    pub converged: bool,
}

impl Fit {
    /// The standard error of `p` (log scale for log parameters), if it was fitted.
    pub fn se(&self, p: P) -> Option<f64> {
        let i = self.active.iter().position(|q| *q == p)?;
        let v = self.cov[(i, i)];
        (v.is_finite() && v >= 0.0).then(|| v.sqrt())
    }

    /// `(estimate, low, high)` of `p` in its natural units; the 95% range is
    /// `exp(log p ± 1.96·se)` for log parameters and `p ± 1.96·se` for rates (unclamped; readers clamp the rate).
    pub fn range(&self, p: P) -> Option<(f64, f64, f64)> {
        let se = self.se(p)?;
        let v = self.theta.get(p);
        Some(if p.is_log() {
            (v, v * (-1.96 * se).exp(), v * (1.96 * se).exp())
        } else {
            (v, v - 1.96 * se, v + 1.96 * se)
        })
    }
}

fn rss_of(spec: &Spec, th: &Theta, rows: &[MRow]) -> f64 {
    rows.iter().map(|r| (r.y - eval(spec, th, r, &[], None)).powi(2)).sum()
}

fn gradients(spec: &Spec, th: &Theta, rows: &[MRow], active: &[P]) -> Vec<Vec<f64>> {
    rows.iter()
        .map(|r| {
            let mut g = vec![0.0; active.len()];
            eval(spec, th, r, active, Some(&mut g));
            g
        })
        .collect()
}

fn outer_add(into: &mut Mat, a: &[f64], b: &[f64], w: f64) {
    for (i, ai) in a.iter().enumerate() {
        if *ai == 0.0 {
            continue;
        }
        for (j, bj) in b.iter().enumerate() {
            into[(i, j)] += w * ai * bj;
        }
    }
}

fn information(grads: &[Vec<f64>], p: usize) -> Mat {
    let mut m = Mat::zeros(p, p);
    for g in grads {
        outer_add(&mut m, g, g, 1.0);
    }
    m
}

/// Fits the window from `start`, over `rows` (evidence only; the caller filters). `None` when
/// no parameter is informed, there are no more rows than parameters, or the information is
/// singular.
pub fn fit(spec: &Spec, rows: &[MRow], start: &Theta) -> Option<Fit> {
    let mut th = start.clone();
    // Keep the parameters some row informs.
    let probe = candidates(spec, &th);
    let probe_grads = gradients(spec, &th, rows, &probe);
    let active: Vec<P> =
        probe.iter().enumerate().filter(|(i, _)| probe_grads.iter().any(|g| g[*i] != 0.0)).map(|(_, p)| *p).collect();
    let p = active.len();
    if p == 0 || rows.len() <= p {
        return None;
    }
    let mut rss = rss_of(spec, &th, rows);
    let mut converged = false;
    for _ in 0..MAX_ITER {
        let grads = gradients(spec, &th, rows, &active);
        let mut a = information(&grads, p);
        let mut g = vec![0.0; p];
        for (row, gr) in rows.iter().zip(&grads) {
            let r = row.y - eval(spec, &th, row, &[], None);
            for (gi, v) in g.iter_mut().zip(gr) {
                *gi += v * r;
            }
        }
        let ridge = 1e-10 * (0..p).map(|i| a[(i, i)]).sum::<f64>() / p as f64 + 1e-300;
        for i in 0..p {
            a[(i, i)] += ridge;
        }
        let delta = a.solve(&g)?;
        let mut s = 1.0;
        let mut moved = false;
        for _ in 0..=MAX_HALVINGS {
            let mut next = th.clone();
            for (q, d) in active.iter().zip(&delta) {
                next.step(*q, s * d);
            }
            let r = rss_of(spec, &next, rows);
            if r.is_finite() && r <= rss {
                let rel = (rss - r) / rss.max(1e-300);
                th = next;
                rss = r;
                moved = true;
                if rel < 1e-12 || delta.iter().map(|d| d.abs()).fold(0.0, f64::max) * s < 1e-10 {
                    converged = true;
                }
                break;
            }
            s *= 0.5;
        }
        if !moved || converged {
            converged = converged || !moved;
            break;
        }
    }
    // Covariance at the solution: J⁻¹ (J_r + σ²_e J) J⁻¹.
    let grads = gradients(spec, &th, rows, &active);
    let info = information(&grads, p);
    let mut ridged = info.clone();
    let ridge = 1e-10 * (0..p).map(|i| info[(i, i)]).sum::<f64>() / p as f64 + 1e-300;
    for i in 0..p {
        ridged[(i, i)] += ridge;
    }
    let inv = ridged.inverse()?;
    let mut jr = Mat::zeros(p, p);
    for (i, g) in grads.iter().enumerate() {
        outer_add(&mut jr, g, g, ROW_VAR);
        if let Some(next) = rows.get(i + 1)
            && next.acct == rows[i].acct
            && next.start == rows[i].end
        {
            // Adjacent rows share a reading: covariance −1/12.
            outer_add(&mut jr, g, &grads[i + 1], -READING_VAR);
            outer_add(&mut jr, &grads[i + 1], g, -READING_VAR);
        }
    }
    let sigma_e2 = (rss / (rows.len() - p) as f64 - ROW_VAR).max(0.0);
    for i in 0..p {
        for j in 0..p {
            jr[(i, j)] += sigma_e2 * info[(i, j)];
        }
    }
    let cov = inv.mul(&jr).mul(&inv);
    Some(Fit { theta: th, active, cov, info, rows: rows.len(), rss, sigma_e2, converged })
}

/// The natural-unit values of every fitted parameter, keyed by parameter (for tests and views).
pub fn values(fit: &Fit) -> BTreeMap<P, f64> {
    fit.active.iter().map(|p| (*p, fit.theta.get(*p))).collect()
}
