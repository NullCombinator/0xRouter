//! The per-window model: parameters, prediction, Gauss–Newton, generalised least squares (research R3, R4).
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
/// The most rounds of re-estimating `σ²_e` and refitting.
const EXTRA_ROUNDS: usize = 4;
/// Rows of serial correlation the ranges allow for: an hour at 10-minute polls.
const HAC_LAGS: usize = 6;

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

/// The name a stored prior gives `p`: `k@<account>`, `b@<account>.<part>`, `rho.<class>`,
/// `w.<class>`, `mult.<glob>`.
pub fn param_name(spec: &Spec, p: P) -> String {
    let acct = |a: usize| spec.accounts.get(a).map_or("?", String::as_str);
    let class = |c: usize| TokenClass::ALL.get(c + 1).map_or("?", |t| t.as_str());
    match p {
        P::K(a) => format!("k@{}", acct(a)),
        P::B(a, q) => format!("b@{}.{q}", acct(a)),
        P::Rho(c) => format!("rho.{}", class(c)),
        P::W(c) => format!("w.{}", TokenClass::ALL.get(c).map_or("?", |t| t.as_str())),
        P::Mu(i) => format!("mult.{}", spec.globs.get(i).map_or("?", String::as_str)),
    }
}

/// A parameter's value in the space the fit moves in: the log for log parameters.
pub fn fit_space(p: P, v: f64) -> f64 {
    if p.is_log() { v.ln() } else { v }
}

/// Rows pruned from inside the epoch, folded into one Gaussian term on the parameters (research
/// R11): the objective gains `(u − mean)ᵀ · information · (u − mean)`, `u` in fit space.
#[derive(Debug, Clone, PartialEq)]
pub struct PriorTerm {
    /// Parameter names, as [`param_name`] gives them.
    pub params: Vec<String>,
    pub mean: Vec<f64>,
    pub information: Vec<Vec<f64>>,
}

impl PriorTerm {
    /// The term laid over `active`: its information matrix and mean, zero where `active` has no
    /// matching parameter. `None` when a name isn't a parameter of this spec (the prior belongs
    /// to another meter) or the shapes disagree.
    fn lay_over(&self, spec: &Spec, probe: &[P], active: &[P]) -> Option<(Mat, Vec<f64>)> {
        let n = self.params.len();
        if self.mean.len() != n || self.information.len() != n || self.information.iter().any(|r| r.len() != n) {
            return None;
        }
        let slots: Vec<Option<usize>> = self
            .params
            .iter()
            .map(|name| {
                let q = probe.iter().find(|q| param_name(spec, **q) == *name)?;
                Some(active.iter().position(|a| a == q))
            })
            .collect::<Option<Vec<_>>>()?;
        let mut pi = Mat::zeros(active.len(), active.len());
        let mut pm = vec![0.0; active.len()];
        for (j, sj) in slots.iter().enumerate() {
            let Some(a) = sj else { continue };
            pm[*a] = self.mean[j];
            for (k, sk) in slots.iter().enumerate() {
                if let Some(b) = sk {
                    pi[(*a, *b)] += self.information[j][k];
                }
            }
        }
        (pi.data.iter().chain(&pm).all(|v| v.is_finite())).then_some((pi, pm))
    }
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

/// The Cholesky factor of the rows' noise covariance (research R4). Within a run of touching rows
/// of one account the covariance is tridiagonal: `1/6 + σ²_e` on the diagonal, `−1/12` between
/// neighbours (they share a reading). The factor is lower bidiagonal: `diag[i]` on the diagonal,
/// `sub[i]` below it (0 where a run starts).
struct Whitener {
    sub: Vec<f64>,
    diag: Vec<f64>,
}

impl Whitener {
    fn new(rows: &[MRow], extra: f64) -> Self {
        let d = ROW_VAR + extra;
        let (mut sub, mut diag) = (Vec::with_capacity(rows.len()), Vec::<f64>::with_capacity(rows.len()));
        for (i, r) in rows.iter().enumerate() {
            let joined = i > 0 && rows[i - 1].acct == r.acct && rows[i - 1].end == r.start;
            let s = if joined { -READING_VAR / diag[i - 1] } else { 0.0 };
            sub.push(s);
            diag.push((d - s * s).sqrt());
        }
        Self { sub, diag }
    }

    /// `L⁻¹ v`, one row at a time.
    fn residuals(&self, v: &mut [f64]) {
        for i in 0..v.len() {
            let prev = if i > 0 { v[i - 1] } else { 0.0 };
            v[i] = (v[i] - self.sub[i] * prev) / self.diag[i];
        }
    }

    /// `L⁻¹ G`, column by column, for row-major gradients.
    fn gradients(&self, g: &mut [Vec<f64>]) {
        for i in 0..g.len() {
            let (done, rest) = g.split_at_mut(i);
            let prev = done.last();
            for (j, v) in rest[0].iter_mut().enumerate() {
                *v = (*v - self.sub[i] * prev.map_or(0.0, |p| p[j])) / self.diag[i];
            }
        }
    }
}

fn raw_residuals(spec: &Spec, th: &Theta, rows: &[MRow]) -> Vec<f64> {
    rows.iter().map(|r| r.y - eval(spec, th, r, &[], None)).collect()
}

/// The generalised squared error: the whitened residuals' sum of squares.
fn rss_of(spec: &Spec, th: &Theta, rows: &[MRow], wh: &Whitener) -> f64 {
    let mut r = raw_residuals(spec, th, rows);
    wh.residuals(&mut r);
    r.iter().map(|v| v * v).sum()
}

/// The extra variance `σ²_e` that makes the whitened residuals' mean square 1 over `dof`
/// degrees of freedom, floored at 0.
fn extra_variance(rows: &[MRow], raw: &[f64], dof: f64) -> f64 {
    let q = |extra: f64| {
        let mut r = raw.to_vec();
        Whitener::new(rows, extra).residuals(&mut r);
        r.iter().map(|v| v * v).sum::<f64>()
    };
    if dof <= 0.0 || q(0.0) <= dof {
        return 0.0;
    }
    let (mut lo, mut hi) = (0.0, ROW_VAR);
    for _ in 0..60 {
        if q(hi) <= dof {
            break;
        }
        lo = hi;
        hi *= 4.0;
    }
    for _ in 0..50 {
        let mid = 0.5 * (lo + hi);
        if q(mid) > dof { lo = mid } else { hi = mid }
    }
    hi
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

/// [`fit_with_prior`] without a prior.
pub fn fit(spec: &Spec, rows: &[MRow], start: &Theta) -> Option<Fit> {
    fit_with_prior(spec, rows, start, None)
}

/// Fits the window from `start`, over `rows` (evidence only; the caller filters), with the
/// folded `prior` of pruned rows as one more Gaussian term. A prior that names a parameter this
/// spec lacks is ignored. `None` when no parameter is informed, there are no more rows than
/// parameters, or the information is singular.
pub fn fit_with_prior(spec: &Spec, rows: &[MRow], start: &Theta, prior: Option<&PriorTerm>) -> Option<Fit> {
    fit_inner(spec, rows, start, prior, None).map(|(f, _)| f)
}

/// Fits only `free` over `rows`, every other parameter held at its value in `start` (the break
/// detector's one-number refit, research R9). `None` as for [`fit`], or when no row informs a
/// parameter of `free`.
pub fn fit_only(spec: &Spec, rows: &[MRow], start: &Theta, free: &[P]) -> Option<Fit> {
    fit_inner(spec, rows, start, None, Some(free)).map(|(f, _)| f)
}

/// The standard error of `p` in `only`, a [`fit_only`] of `p` alone on `rows`, for testing it
/// against `held`, the fit it held every other number at (R9). Two corrections to `only.se`:
///
/// - The curvature is also taken at `held`'s value, and the smaller one counts. In log space
///   the gradient grows with the value, so a refit that a few rows drive 10× up (one rounding
///   step on rows with little traffic) would otherwise claim 10× the information it has.
/// - A held number off by `δ` moves the refit by `−δ·(g̃ₚ·g̃_q)/(g̃ₚ·g̃ₚ)` over the whitened
///   gradients, so their covariance in `held` adds `sᵀ·Σ·s`. The rows are also in `held`, which
///   makes the two agree more, not less: leaving that out keeps the test conservative.
pub fn held_se(spec: &Spec, rows: &[MRow], only: &Fit, p: P, held: &Fit) -> Option<f64> {
    if only.active != [p] {
        return None;
    }
    let mut se = only.se(p)?;
    let wh = Whitener::new(rows, only.sigma_e2);
    let mut at_null = gradients(spec, &held.theta, rows, &[p]);
    wh.gradients(&mut at_null);
    let info_null: f64 = at_null.iter().map(|g| g[0] * g[0]).sum();
    // `only` has one parameter: its covariance over its information is the serial factor.
    let factor = (only.cov[(0, 0)] * only.info[(0, 0)]).max(1.0);
    if info_null > 0.0 {
        se = se.max((factor / info_null).sqrt());
    } else {
        return None;
    }
    let others: Vec<(usize, P)> = held.active.iter().copied().enumerate().filter(|(_, q)| *q != p).collect();
    if others.is_empty() {
        return Some(se);
    }
    let cols: Vec<P> = std::iter::once(p).chain(others.iter().map(|(_, q)| *q)).collect();
    let mut g = gradients(spec, &only.theta, rows, &cols);
    wh.gradients(&mut g);
    let gpp: f64 = g.iter().map(|r| r[0] * r[0]).sum();
    if gpp <= 0.0 {
        return Some(se);
    }
    let s: Vec<f64> = (1..cols.len()).map(|j| -g.iter().map(|r| r[0] * r[j]).sum::<f64>() / gpp).collect();
    let mut var = 0.0;
    for (a, (ia, _)) in others.iter().enumerate() {
        for (b, (ib, _)) in others.iter().enumerate() {
            var += s[a] * s[b] * held.cov[(*ia, *ib)];
        }
    }
    let total = se * se + var.max(0.0);
    total.is_finite().then(|| total.sqrt())
}

/// The best of the fits from each of `starts`, by the objective (squared error plus the prior's
/// penalty). Gauss–Newton in log space cannot climb back from a parameter that earlier, thin
/// evidence drove to nearly 0 (its gradient vanishes with it), so a warm start is always
/// compared with a fresh one. `None` when no start gives a fit.
pub fn fit_best(spec: &Spec, rows: &[MRow], starts: &[Theta], prior: Option<&PriorTerm>) -> Option<Fit> {
    let mut best: Option<(Fit, f64)> = None;
    for start in starts {
        if let Some((f, obj)) = fit_inner(spec, rows, start, prior, None)
            && obj.is_finite()
            && best.as_ref().is_none_or(|(_, b)| obj < *b)
        {
            best = Some((f, obj));
        }
    }
    best.map(|(f, _)| f)
}

fn fit_inner(spec: &Spec, rows: &[MRow], start: &Theta, prior: Option<&PriorTerm>, free: Option<&[P]>) -> Option<(Fit, f64)> {
    let mut th = start.clone();
    // Keep the parameters some row informs (of `free`, when given).
    let probe = candidates(spec, &th);
    let probe_grads = gradients(spec, &th, rows, &probe);
    let active: Vec<P> = probe
        .iter()
        .enumerate()
        .filter(|(i, p)| free.is_none_or(|f| f.contains(p)) && probe_grads.iter().any(|g| g[*i] != 0.0))
        .map(|(_, p)| *p)
        .collect();
    let p = active.len();
    if p == 0 || rows.len() <= p {
        return None;
    }
    let (pi, pm) = prior
        .and_then(|t| t.lay_over(spec, &probe, &active))
        .unwrap_or_else(|| (Mat::zeros(p, p), vec![0.0; p]));
    let has_prior = pi.data.iter().any(|v| *v != 0.0);
    let penalty = |t: &Theta| -> f64 {
        if !has_prior {
            return 0.0;
        }
        let d: Vec<f64> = active.iter().zip(&pm).map(|(q, m)| fit_space(*q, t.get(*q)) - m).collect();
        d.iter().zip(pi.mul_vec(&d)).map(|(a, b)| a * b).sum()
    };
    // Generalised least squares (R4): the rows' whitened squared error plus the prior's penalty.
    // `σ²_e` is unknown, so the fit alternates: fit with the current `σ²_e`, re-estimate it from
    // the residuals, refit while it moves.
    let dof = (rows.len() - p) as f64;
    let mut extra = 0.0;
    let mut converged = false;
    for _ in 0..EXTRA_ROUNDS {
        let wh = Whitener::new(rows, extra);
        converged = gauss_newton(spec, rows, &active, &wh, &mut th, &penalty, &pi, &pm, has_prior)?;
        let next = extra_variance(rows, &raw_residuals(spec, &th, rows), dof);
        let settled = (next - extra).abs() <= 0.05 * next.max(0.01 * ROW_VAR);
        extra = next;
        if settled {
            break;
        }
    }
    // Covariance at the solution: the inverse of the whitened information plus the prior's.
    let wh = Whitener::new(rows, extra);
    let mut grads = gradients(spec, &th, rows, &active);
    wh.gradients(&mut grads);
    let rss = rss_of(spec, &th, rows, &wh);
    let mut info = information(&grads, p);
    for i in 0..p {
        for j in 0..p {
            info[(i, j)] += pi[(i, j)];
        }
    }
    let mut ridged = info.clone();
    let ridge = 1e-10 * (0..p).map(|i| info[(i, i)]).sum::<f64>() / p as f64 + 1e-300;
    for i in 0..p {
        ridged[(i, i)] += ridge;
    }
    let model_cov = ridged.inverse()?;
    let mut res = raw_residuals(spec, &th, rows);
    wh.residuals(&mut res);
    let factor = serial_factor(rows, &active, &grads, &res, &model_cov);
    let cov = model_cov.scale(factor);
    // Starts are compared under the rounding noise alone, which doesn't depend on the fit.
    let objective = rss_of(spec, &th, rows, &Whitener::new(rows, 0.0)) + penalty(&th);
    Some((Fit { theta: th, active, cov, info, rows: rows.len(), rss, sigma_e2: extra, converged }, objective))
}

/// How much serial correlation left in the whitened residuals widens the ranges (R4): the
/// largest ratio, over the meter numbers, of the Newey–West variance (Bartlett kernel over
/// [`HAC_LAGS`] rows, never across a run's start) to the model's, floored at 1. One factor for
/// every number keeps the covariance positive definite; the floor keeps a short, noisy early fit
/// from claiming more than rounding alone allows.
fn serial_factor(rows: &[MRow], active: &[P], grads: &[Vec<f64>], res: &[f64], cov: &Mat) -> f64 {
    let p = active.len();
    let scores: Vec<Vec<f64>> = grads.iter().zip(res).map(|(g, r)| g.iter().map(|v| v * r).collect()).collect();
    let mut meat = Mat::zeros(p, p);
    // How many rows back each row's run reaches.
    let mut reach = vec![0usize; rows.len()];
    for i in 1..rows.len() {
        if rows[i - 1].acct == rows[i].acct && rows[i - 1].end == rows[i].start {
            reach[i] = reach[i - 1] + 1;
        }
    }
    for (i, si) in scores.iter().enumerate() {
        outer_add(&mut meat, si, si, 1.0);
        for l in 1..=HAC_LAGS.min(reach[i]) {
            let w = 1.0 - l as f64 / (HAC_LAGS + 1) as f64;
            outer_add(&mut meat, si, &scores[i - l], w);
            outer_add(&mut meat, &scores[i - l], si, w);
        }
    }
    let hac = cov.mul(&meat).mul(cov);
    active
        .iter()
        .enumerate()
        .filter(|(_, q)| !matches!(q, P::B(..)))
        .map(|(i, _)| hac[(i, i)] / cov[(i, i)])
        .filter(|r| r.is_finite())
        .fold(1.0, f64::max)
}

/// Gauss–Newton under `wh` from `th`, with step halving. Returns whether it converged; `None`
/// when the normal equations are singular.
#[allow(clippy::too_many_arguments)]
fn gauss_newton(
    spec: &Spec,
    rows: &[MRow],
    active: &[P],
    wh: &Whitener,
    th: &mut Theta,
    penalty: &dyn Fn(&Theta) -> f64,
    pi: &Mat,
    pm: &[f64],
    has_prior: bool,
) -> Option<bool> {
    let p = active.len();
    let mut rss = rss_of(spec, th, rows, wh) + penalty(th);
    for _ in 0..MAX_ITER {
        let mut grads = gradients(spec, th, rows, active);
        wh.gradients(&mut grads);
        let mut r = raw_residuals(spec, th, rows);
        wh.residuals(&mut r);
        let mut a = information(&grads, p);
        let mut g = vec![0.0; p];
        for (ri, gr) in r.iter().zip(&grads) {
            for (gi, v) in g.iter_mut().zip(gr) {
                *gi += v * ri;
            }
        }
        if has_prior {
            let toward: Vec<f64> = active.iter().zip(pm).map(|(q, m)| m - fit_space(*q, th.get(*q))).collect();
            for (gi, v) in g.iter_mut().zip(pi.mul_vec(&toward)) {
                *gi += v;
            }
            for i in 0..p {
                for j in 0..p {
                    a[(i, j)] += pi[(i, j)];
                }
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
            let rn = rss_of(spec, &next, rows, wh) + penalty(&next);
            if rn.is_finite() && rn <= rss {
                let rel = (rss - rn) / rss.max(1e-300);
                *th = next;
                rss = rn;
                moved = true;
                if rel < 1e-12 || delta.iter().map(|d| d.abs()).fold(0.0, f64::max) * s < 1e-10 {
                    return Some(true);
                }
                break;
            }
            s *= 0.5;
        }
        if !moved {
            return Some(true);
        }
    }
    Some(false)
}

/// The natural-unit values of every fitted parameter, keyed by parameter (for tests and views).
pub fn values(fit: &Fit) -> BTreeMap<P, f64> {
    fit.active.iter().map(|p| (*p, fit.theta.get(*p))).collect()
}

impl Fit {
    /// The expected `y` of `row` under the fit, and the variance of that expected value (the
    /// parameters' uncertainty only: the row's own noise is added by the caller).
    pub fn predict(&self, spec: &Spec, row: &MRow) -> (f64, f64) {
        let mut g = vec![0.0; self.active.len()];
        let mean = eval(spec, &self.theta, row, &self.active, Some(&mut g));
        let cg = self.cov.mul_vec(&g);
        let var: f64 = g.iter().zip(&cg).map(|(a, b)| a * b).sum();
        (mean, var.max(0.0))
    }

    /// The variance of one new row's reading beyond the mean: rounding plus the extra variance.
    pub fn row_noise(&self) -> f64 {
        ROW_VAR + self.sigma_e2
    }
}
