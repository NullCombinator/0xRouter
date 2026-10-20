//! The always-valid significance test: a normal-mixture confidence sequence (research R5).
//!
//! One test decides every question the fit asks: a number's significance, a split-off, a break,
//! a busy-time alert and a steady-rate alert. It holds at every poll for a number's whole life,
//! so re-checking at each poll costs nothing.

/// The lifetime false-correction bound per number (clarify Q2): 0.1%.
pub const ALPHA: f64 = 0.001;

/// The mixture's tuning constant, derived in closed form (research R5), never fitted on a run.
///
/// Howard et al. (2021) tune a normal mixture for a target information `V*` and level `a` with
/// `ρ = V* / (2·ln(1/a) + ln(2·ln(1/a) + 1))`. Call the denominator `D(a)`. A log error of
/// `ln 2` (a factor-2 error) in a number with information `V` gives a score `|Z| ≈ ln 2 · V`,
/// and the boundary is about `sqrt(V · D(a))`, so the error is detected when
/// `V ≈ D(a) / (ln 2)²`. That is the information a day of typical traffic must supply, and the
/// point where the boundary should be tightest: `V* = D(a) / (ln 2)²`. Then
/// `ρ = V* / D(a) = 1 / (ln 2)²`, whatever the level `a` (so whatever `m`). About 2.08.
pub const RHO: f64 = 1.0 / (std::f64::consts::LN_2 * std::f64::consts::LN_2);

/// Whether the estimate `est_log` of a log-scale number, with standard error `se`, excludes
/// the value `null_log` under the confidence sequence at level `ALPHA`, Bonferroni-split over
/// `m` numbers tested together.
///
/// With information `V = 1/se²` and score `Z = (est − null)·V`, it rejects when
/// `|Z| ≥ sqrt((V + ρ)·(ln((V + ρ)/ρ) + 2·ln(m/α)))`.
pub fn rejects(est_log: f64, null_log: f64, se: f64, m: usize) -> bool {
    if !(se.is_finite() && se > 0.0 && est_log.is_finite() && null_log.is_finite()) {
        return false;
    }
    let v = 1.0 / (se * se);
    let z = (est_log - null_log) * v;
    z.abs() >= boundary(v, m)
}

/// The boundary on `|Z|` at information `v`.
pub fn boundary(v: f64, m: usize) -> f64 {
    let m = m.max(1) as f64;
    ((v + RHO) * (((v + RHO) / RHO).ln() + 2.0 * (m / ALPHA).ln())).sqrt()
}

/// A small seeded generator of standard normals, for the null-rate measurement.
struct Normals(u64);

impl Normals {
    fn uniform(&mut self) -> f64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        ((self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    fn normal(&mut self) -> f64 {
        let (a, b) = (self.uniform(), self.uniform());
        (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()
    }
}

/// The fraction of `runs` null sequences that cross the boundary at any of `looks` looks, each
/// look adding `info_per_look` information. Under the null the score `Z` is a random walk with
/// variance `info_per_look` per look. `sim_suite` prints it beside the bound `ALPHA`.
pub fn simulated_null_rate(runs: u32, looks: u32, info_per_look: f64, m: usize, seed: u64) -> f64 {
    let mut rng = Normals(seed | 1);
    let mut crossed = 0;
    for _ in 0..runs {
        let (mut z, mut v) = (0.0, 0.0);
        for _ in 0..looks {
            v += info_per_look;
            z += rng.normal() * info_per_look.sqrt();
            if z.abs() >= boundary(v, m) {
                crossed += 1;
                break;
            }
        }
    }
    f64::from(crossed) / f64::from(runs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_null_is_crossed_in_at_most_a_tenth_of_a_percent_of_runs() {
        // 2,000 runs of 1,000 looks, with several numbers tested together.
        let mut worst = 0.0f64;
        for m in [1, 4, 12] {
            let rate = simulated_null_rate(2_000, 1_000, 0.3, m, 0x012_5EED + m as u64);
            println!("null rejection rate (m = {m}): {rate} (bound {ALPHA})");
            worst = worst.max(rate);
        }
        assert!(worst <= ALPHA, "{worst}");
    }

    #[test]
    fn a_factor_two_error_is_found_within_the_day_and_a_half_of_traffic() {
        // The estimate sits at ln 2 from the declared value; information grows 0.3 per 10 minutes.
        let diff = std::f64::consts::LN_2;
        let found = (1..=216).find(|k| {
            let v = 0.3 * f64::from(*k);
            rejects(diff, 0.0, (1.0 / v).sqrt(), 8)
        });
        assert!(found.is_some(), "not rejected within 36 hours");
        // And a right number (no difference) is never rejected at the same information.
        assert!((1..=1_000).all(|k| !rejects(0.0, 0.0, (1.0 / (0.3 * f64::from(k))).sqrt(), 8)));
    }

    #[test]
    fn degenerate_inputs_do_not_reject() {
        assert!(!rejects(1.0, 0.0, 0.0, 4));
        assert!(!rejects(f64::NAN, 0.0, 0.1, 4));
        assert!(!rejects(1.0, 0.0, f64::INFINITY, 4));
    }
}
