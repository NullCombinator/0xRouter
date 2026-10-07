//! Small dense linear algebra for the fit: Cholesky, solve, inverse (plan, Complexity Tracking).
//!
//! The model has at most about 30 parameters, so a row-major `Vec<f64>` and a few loops replace a
//! linear-algebra crate. Every routine that can meet a singular matrix returns `None` for it.

/// A dense row-major matrix.
#[derive(Debug, Clone, PartialEq)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
}

impl Mat {
    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self { rows, cols, data: vec![0.0; rows * cols] }
    }

    pub fn identity(n: usize) -> Self {
        let mut m = Self::zeros(n, n);
        for i in 0..n {
            m[(i, i)] = 1.0;
        }
        m
    }

    /// From nested rows; every row must have the same length.
    pub fn from_rows(rows: &[&[f64]]) -> Self {
        let cols = rows.first().map_or(0, |r| r.len());
        let mut m = Self::zeros(rows.len(), cols);
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r.len(), cols, "ragged matrix");
            m.data[i * cols..(i + 1) * cols].copy_from_slice(r);
        }
        m
    }

    pub fn transpose(&self) -> Mat {
        let mut t = Mat::zeros(self.cols, self.rows);
        for i in 0..self.rows {
            for j in 0..self.cols {
                t[(j, i)] = self[(i, j)];
            }
        }
        t
    }

    pub fn mul(&self, other: &Mat) -> Mat {
        assert_eq!(self.cols, other.rows, "shape mismatch");
        let mut out = Mat::zeros(self.rows, other.cols);
        for i in 0..self.rows {
            for k in 0..self.cols {
                let a = self[(i, k)];
                if a == 0.0 {
                    continue;
                }
                for j in 0..other.cols {
                    out[(i, j)] += a * other[(k, j)];
                }
            }
        }
        out
    }

    /// Cholesky factor `L` (lower triangular, `L·Lᵀ = self`) of a symmetric positive-definite
    /// matrix; `None` when a pivot is not positive.
    pub fn cholesky(&self) -> Option<Mat> {
        assert_eq!(self.rows, self.cols, "cholesky needs a square matrix");
        let n = self.rows;
        let mut l = Mat::zeros(n, n);
        for i in 0..n {
            for j in 0..=i {
                let mut s = self[(i, j)];
                for k in 0..j {
                    s -= l[(i, k)] * l[(j, k)];
                }
                if i == j {
                    if s <= 0.0 || !s.is_finite() {
                        return None;
                    }
                    l[(i, i)] = s.sqrt();
                } else {
                    l[(i, j)] = s / l[(j, j)];
                }
            }
        }
        Some(l)
    }

    /// Solves `self · x = b` for a symmetric positive-definite `self`.
    pub fn solve(&self, b: &[f64]) -> Option<Vec<f64>> {
        assert_eq!(self.rows, b.len(), "shape mismatch");
        let l = self.cholesky()?;
        Some(l.back_substitute(&l.forward_substitute(b)))
    }

    /// The inverse of a symmetric positive-definite matrix.
    pub fn inverse(&self) -> Option<Mat> {
        let n = self.rows;
        let l = self.cholesky()?;
        let mut inv = Mat::zeros(n, n);
        let mut e = vec![0.0; n];
        for j in 0..n {
            e.fill(0.0);
            e[j] = 1.0;
            let col = l.back_substitute(&l.forward_substitute(&e));
            for (i, v) in col.into_iter().enumerate() {
                inv[(i, j)] = v;
            }
        }
        Some(inv)
    }

    /// `self · x` for a column vector.
    pub fn mul_vec(&self, x: &[f64]) -> Vec<f64> {
        assert_eq!(self.cols, x.len(), "shape mismatch");
        (0..self.rows).map(|i| (0..self.cols).map(|j| self[(i, j)] * x[j]).sum()).collect()
    }

    /// Solves `L·y = b` for a lower-triangular `self`.
    fn forward_substitute(&self, b: &[f64]) -> Vec<f64> {
        let mut y = vec![0.0; b.len()];
        for i in 0..b.len() {
            let s: f64 = (0..i).map(|k| self[(i, k)] * y[k]).sum();
            y[i] = (b[i] - s) / self[(i, i)];
        }
        y
    }

    /// Solves `Lᵀ·x = y` for a lower-triangular `self`.
    fn back_substitute(&self, y: &[f64]) -> Vec<f64> {
        let n = y.len();
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            let s: f64 = (i + 1..n).map(|k| self[(k, i)] * x[k]).sum();
            x[i] = (y[i] - s) / self[(i, i)];
        }
        x
    }
}

impl std::ops::Index<(usize, usize)> for Mat {
    type Output = f64;

    fn index(&self, (i, j): (usize, usize)) -> &f64 {
        &self.data[i * self.cols + j]
    }
}

impl std::ops::IndexMut<(usize, usize)> for Mat {
    fn index_mut(&mut self, (i, j): (usize, usize)) -> &mut f64 {
        &mut self.data[i * self.cols + j]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spd4() -> Mat {
        // AᵀA + I for a fixed A: symmetric positive definite.
        let a = Mat::from_rows(&[
            &[2.0, -1.0, 0.5, 0.0],
            &[0.3, 1.5, -0.7, 1.0],
            &[1.0, 0.0, 2.0, -0.4],
            &[-0.2, 0.8, 0.1, 1.2],
            &[0.9, 0.4, -1.1, 0.6],
        ]);
        let mut m = a.transpose().mul(&a);
        for i in 0..4 {
            m[(i, i)] += 1.0;
        }
        m
    }

    fn close(a: &Mat, b: &Mat, tol: f64) -> bool {
        a.rows == b.rows && a.cols == b.cols && a.data.iter().zip(&b.data).all(|(x, y)| (x - y).abs() <= tol)
    }

    #[test]
    fn cholesky_factor_reproduces_the_matrix() {
        let m = spd4();
        let l = m.cholesky().expect("spd");
        for i in 0..4 {
            for j in i + 1..4 {
                assert_eq!(l[(i, j)], 0.0, "upper triangle is zero");
            }
        }
        assert!(close(&l.mul(&l.transpose()), &m, 1e-12));
    }

    #[test]
    fn solve_returns_the_known_vector() {
        let m = spd4();
        let x = [1.0, -2.0, 0.5, 3.0];
        let b = m.mul_vec(&x);
        let got = m.solve(&b).expect("spd");
        for (g, w) in got.iter().zip(x) {
            assert!((g - w).abs() < 1e-9, "{g} vs {w}");
        }
    }

    #[test]
    fn inverse_times_matrix_is_identity() {
        let m = spd4();
        let inv = m.inverse().expect("spd");
        assert!(close(&inv.mul(&m), &Mat::identity(4), 1e-9));
        assert!(close(&m.mul(&inv), &Mat::identity(4), 1e-9));
    }

    #[test]
    fn a_singular_matrix_is_refused() {
        // Rank 1: the second row repeats the first.
        let m = Mat::from_rows(&[&[1.0, 2.0], &[2.0, 4.0]]);
        assert!(m.cholesky().is_none());
        assert!(m.solve(&[1.0, 1.0]).is_none());
        assert!(m.inverse().is_none());
        // Negative definite and non-finite are refused too.
        assert!(Mat::from_rows(&[&[-1.0]]).cholesky().is_none());
        assert!(Mat::from_rows(&[&[f64::NAN]]).cholesky().is_none());
    }
}
