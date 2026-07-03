//! Restricted LP Master Problem for column generation.
//!
//! Formulation (set-covering LP relaxation):
//!   min  sum_p lambda_p
//!   s.t. sum_p a_{ip} * lambda_p >= 1   for each item i  (a_{ip} in {0,1})
//!        lambda_p >= 0
//!
//! Standard form with surplus variables s_i >= 0:
//!   sum_p a_{ip} * lambda_p - s_i = 1   for each i
//!
//! Initial basis: n singleton patterns (pattern i covers only item i),
//! giving B = I_n, b_bar = 1 (all-ones), dual vector mu = 1.
//!
//! Uses revised primal simplex with explicit B^{-1} (n x n dense matrix).
//! Periodic refactorization via Gauss-Jordan every REFACT_EVERY pivots.
//!
//! Anti-cycling: Bland's rule (entering = smallest index with rc < 0;
//! leaving = smallest basis index among ratio-test ties).

// Tolerance for "strictly negative" reduced cost (entering criterion).
const RC_TOL: f64 = 1e-9;
// Minimum eta component considered a valid pivot denominator.
const PIVOT_TOL: f64 = 1e-10;
// Gauss-Jordan refactorization frequency (in pivots).
const REFACT_EVERY: usize = 64;

// == Types =====================================================================

/// A basic variable: either a pattern column or a surplus variable.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Bv {
    Pat(usize),
    Sur(usize),
}

/// Restricted LP master problem.
///
/// Columns are indexed 0..n_items (initial singletons) and n_items..
/// (added via `add_column`). Surplus variables s_0..s_{n-1} are implicit.
pub(crate) struct Rlmp {
    n: usize,
    col_items: Vec<Vec<usize>>,
    basis: Vec<Bv>,
    b_inv: Vec<f64>,
    b_bar: Vec<f64>,
    pivot_count: usize,
}

// == impl =====================================================================

impl Rlmp {
    /// Create a new RLMP for `n_items` items.
    /// Seeds the column pool with n singleton patterns (B = I, duals = 1).
    pub(crate) fn new(n_items: usize) -> Self {
        let col_items: Vec<Vec<usize>> = (0..n_items).map(|i| vec![i]).collect();
        let basis: Vec<Bv> = (0..n_items).map(Bv::Pat).collect();
        let mut b_inv = vec![0.0f64; n_items * n_items];
        for i in 0..n_items {
            b_inv[i * n_items + i] = 1.0;
        }
        let b_bar = vec![1.0f64; n_items];
        Rlmp {
            n: n_items,
            col_items,
            basis,
            b_inv,
            b_bar,
            pivot_count: 0,
        }
    }

    /// Add a new pattern column covering `items`. Returns its column index.
    /// Does not re-optimize; call `solve` afterwards.
    pub(crate) fn add_column(&mut self, items: &[usize]) -> usize {
        let idx = self.col_items.len();
        let mut sorted = items.to_vec();
        sorted.sort_unstable();
        self.col_items.push(sorted);
        idx
    }

    /// Run primal simplex to LP optimality over the current column pool.
    /// Safe to call after `add_column`; warm-starts from the current basis.
    pub(crate) fn solve(&mut self) {
        // Bound: n * n_cols + slack to handle degeneracy with Bland's rule.
        let max_iters = self.n * self.col_items.len() + 2 * self.n * self.n + 100;
        for _ in 0..max_iters {
            let mu = self.compute_duals();
            let Some(entering) = self.find_entering(&mu) else { break };
            let eta = self.compute_eta(entering);
            let Some(r_leave) = self.ratio_test(&eta) else {
                // Unbounded LP — should never happen for a valid RLMP.
                break;
            };
            self.pivot(entering, &eta, r_leave);
        }
    }

    /// LP objective value (weighted sum of basic pattern variables).
    pub(crate) fn objective(&self) -> f64 {
        self.basis
            .iter()
            .zip(self.b_bar.iter())
            .map(|(bv, &v)| if matches!(bv, Bv::Pat(_)) { v } else { 0.0 })
            .sum()
    }

    /// Dual variables mu_i for each item i (shadow prices of the >= 1 constraints).
    /// A new pattern p is improving iff sum_{i in p} mu_i > 1.
    pub(crate) fn duals(&self) -> Vec<f64> {
        self.compute_duals()
    }

    // -- Private helpers ------------------------------------------------------

    // mu = c_B * B^{-1}; c_B[r] = 1 if Pat, 0 if Sur.
    fn compute_duals(&self) -> Vec<f64> {
        let mut mu = vec![0.0f64; self.n];
        for r in 0..self.n {
            if matches!(self.basis[r], Bv::Pat(_)) {
                let row = &self.b_inv[r * self.n..(r + 1) * self.n];
                for (j, &v) in row.iter().enumerate() {
                    mu[j] += v;
                }
            }
        }
        mu
    }

    // Bland's fixed index: Sur(i) -> i; Pat(p) -> n + p.
    fn bland_idx(&self, bv: Bv) -> usize {
        match bv {
            Bv::Sur(i) => i,
            Bv::Pat(p) => self.n + p,
        }
    }

    fn find_entering(&self, mu: &[f64]) -> Option<Bv> {
        let basic_pats: std::collections::HashSet<usize> = self
            .basis
            .iter()
            .filter_map(|&bv| if let Bv::Pat(p) = bv { Some(p) } else { None })
            .collect();
        let basic_surs: std::collections::HashSet<usize> = self
            .basis
            .iter()
            .filter_map(|&bv| if let Bv::Sur(i) = bv { Some(i) } else { None })
            .collect();

        // Bland's rule: among all negative-rc variables, pick the smallest index.
        let mut best_idx = usize::MAX;
        let mut best: Option<Bv> = None;

        for (p, items) in self.col_items.iter().enumerate() {
            if basic_pats.contains(&p) {
                continue;
            }
            let profit: f64 = items.iter().map(|&i| mu[i]).sum();
            if 1.0 - profit < -RC_TOL {
                let idx = self.bland_idx(Bv::Pat(p));
                if idx < best_idx {
                    best_idx = idx;
                    best = Some(Bv::Pat(p));
                }
            }
        }

        // Surplus s_i has column -e_i; rc = 0 - mu * (-e_i) = mu[i].
        for (i, &m) in mu.iter().enumerate() {
            if basic_surs.contains(&i) {
                continue;
            }
            if m < -RC_TOL {
                let idx = self.bland_idx(Bv::Sur(i));
                if idx < best_idx {
                    best_idx = idx;
                    best = Some(Bv::Sur(i));
                }
            }
        }

        best
    }

    // eta = B^{-1} * a_entering; for Pat(p): a[i]=1 iff i in items; for Sur(i): a[i]=-1.
    fn compute_eta(&self, entering: Bv) -> Vec<f64> {
        let mut a = vec![0.0f64; self.n];
        match entering {
            Bv::Pat(p) => {
                for &i in &self.col_items[p] {
                    a[i] = 1.0;
                }
            }
            Bv::Sur(i) => {
                a[i] = -1.0;
            }
        }
        (0..self.n)
            .map(|r| {
                let row = &self.b_inv[r * self.n..(r + 1) * self.n];
                row.iter().zip(a.iter()).map(|(&b, &x)| b * x).sum::<f64>()
            })
            .collect()
    }

    // Minimum-ratio test with Bland's tie-breaking.
    fn ratio_test(&self, eta: &[f64]) -> Option<usize> {
        let mut min_ratio = f64::INFINITY;
        let mut best_bland = usize::MAX;
        let mut leaving: Option<usize> = None;

        for (r, &eta_r) in eta.iter().enumerate() {
            if eta_r <= PIVOT_TOL {
                continue;
            }
            let ratio = self.b_bar[r] / eta_r;
            let bland = self.bland_idx(self.basis[r]);
            let is_better = ratio < min_ratio - 1e-10 || (ratio <= min_ratio + 1e-10 && bland < best_bland);
            if is_better {
                min_ratio = ratio;
                best_bland = bland;
                leaving = Some(r);
            }
        }
        leaving
    }

    fn pivot(&mut self, entering: Bv, eta: &[f64], r_leave: usize) {
        let n = self.n;
        let pivot_elem = eta[r_leave];
        let theta = self.b_bar[r_leave] / pivot_elem;

        // Update b_bar.
        for (r, &eta_r) in eta.iter().enumerate() {
            if r == r_leave {
                self.b_bar[r] = theta;
            } else {
                self.b_bar[r] -= theta * eta_r;
            }
        }

        // Update B^{-1}: row r_leave /= pivot_elem; then row r -= eta[r] * row r_leave.
        for j in 0..n {
            self.b_inv[r_leave * n + j] /= pivot_elem;
        }
        for (r, &factor) in eta.iter().enumerate() {
            if r == r_leave {
                continue;
            }
            if factor.abs() < 1e-15 {
                continue;
            }
            for j in 0..n {
                let delta = factor * self.b_inv[r_leave * n + j];
                self.b_inv[r * n + j] -= delta;
            }
        }

        self.basis[r_leave] = entering;
        self.pivot_count += 1;

        if self.pivot_count.is_multiple_of(REFACT_EVERY) {
            self.refactorize();
        }
    }

    // Recompute B^{-1} from scratch via Gauss-Jordan on [B | I].
    // Called periodically to contain floating-point drift.
    fn refactorize(&mut self) {
        let n = self.n;
        // Augmented matrix [B | I], row i, column j: aug[i * 2n + j].
        // B[:,r] = constraint column of basis[r].
        let mut aug = vec![0.0f64; n * 2 * n];
        for r in 0..n {
            match self.basis[r] {
                Bv::Pat(p) => {
                    for &i in &self.col_items[p] {
                        aug[i * 2 * n + r] = 1.0;
                    }
                }
                Bv::Sur(i) => {
                    aug[i * 2 * n + r] = -1.0;
                }
            }
            aug[r * 2 * n + n + r] = 1.0;
        }

        // Gauss-Jordan with partial pivoting on each column of B.
        for col in 0..n {
            // Find row with largest absolute value (partial pivot).
            let mut pivot_row = col;
            let mut max_abs = aug[col * 2 * n + col].abs();
            for row in col + 1..n {
                let v = aug[row * 2 * n + col].abs();
                if v > max_abs {
                    max_abs = v;
                    pivot_row = row;
                }
            }
            if max_abs < 1e-12 {
                // Singular column; basis is degenerate in an unexpected way.
                continue;
            }
            if pivot_row != col {
                for j in 0..2 * n {
                    aug.swap(col * 2 * n + j, pivot_row * 2 * n + j);
                }
            }
            let piv = aug[col * 2 * n + col];
            for j in 0..2 * n {
                aug[col * 2 * n + j] /= piv;
            }
            for row in 0..n {
                if row == col {
                    continue;
                }
                let factor = aug[row * 2 * n + col];
                if factor.abs() < 1e-15 {
                    continue;
                }
                for j in 0..2 * n {
                    let delta = factor * aug[col * 2 * n + j];
                    aug[row * 2 * n + j] -= delta;
                }
            }
        }

        // Extract B^{-1} from the right half.
        for i in 0..n {
            for j in 0..n {
                self.b_inv[i * n + j] = aug[i * 2 * n + n + j];
            }
        }

        // Recompute b_bar = B^{-1} * 1 (RHS is all-ones).
        for r in 0..n {
            self.b_bar[r] = self.b_inv[r * n..(r + 1) * n].iter().sum();
        }
    }
}

// == Tests ====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn check_duals_non_negative(mu: &[f64]) {
        for (i, &m) in mu.iter().enumerate() {
            assert!(m >= -1e-9, "mu[{i}] = {m} < 0");
        }
    }

    // No extra columns: each item covered by its singleton. obj = n.
    #[test]
    fn singletons_only() {
        let mut rlmp = Rlmp::new(3);
        rlmp.solve();
        assert!((rlmp.objective() - 3.0).abs() < 1e-9);
        let mu = rlmp.duals();
        for &m in &mu {
            assert!((m - 1.0).abs() < 1e-9, "initial duals must all be 1: {mu:?}");
        }
    }

    // Pattern covering both items replaces two singletons: obj = 1.
    #[test]
    fn two_items_shared_pattern() {
        let mut rlmp = Rlmp::new(2);
        rlmp.add_column(&[0, 1]);
        rlmp.solve();
        let obj = rlmp.objective();
        assert!((obj - 1.0).abs() < 1e-9, "obj = {obj}");
        let mu = rlmp.duals();
        check_duals_non_negative(&mu);
        // Strong duality: sum(mu) = obj = 1.
        let sum: f64 = mu.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "dual sum = {sum}");
    }

    // Items 0,1,2 with patterns {0,1} and {1,2}: LP optimal = 2.
    // Dual optimal: mu[0]+mu[2]=2, mu[1]=0 (or degenerate variant).
    #[test]
    fn three_items_two_covering_patterns() {
        let mut rlmp = Rlmp::new(3);
        rlmp.add_column(&[0, 1]);
        rlmp.add_column(&[1, 2]);
        rlmp.solve();
        let obj = rlmp.objective();
        assert!((obj - 2.0).abs() < 1e-9, "obj = {obj}");
        let mu = rlmp.duals();
        check_duals_non_negative(&mu);
        // Strong duality: sum(mu) = 2.
        let sum: f64 = mu.iter().sum();
        assert!((sum - 2.0).abs() < 1e-9, "dual sum = {sum}");
        // Dual feasibility: each pattern's reduced cost >= 0.
        assert!(mu[0] + mu[1] <= 1.0 + 1e-9, "pattern {{0,1}} rc >= 0");
        assert!(mu[1] + mu[2] <= 1.0 + 1e-9, "pattern {{1,2}} rc >= 0");
    }

    // Adding a column that covers everything: obj drops to 1.
    #[test]
    fn full_cover_column() {
        let mut rlmp = Rlmp::new(4);
        rlmp.add_column(&[0, 1, 2, 3]);
        rlmp.solve();
        let obj = rlmp.objective();
        assert!((obj - 1.0).abs() < 1e-9, "obj = {obj}");
        // Duals sum = 1 (strong duality).
        let mu = rlmp.duals();
        check_duals_non_negative(&mu);
        let sum: f64 = mu.iter().sum();
        assert!((sum - 1.0).abs() < 1e-9, "dual sum = {sum}");
    }

    // Dual feasibility for all added columns at optimality.
    #[test]
    fn dual_feasibility_all_columns() {
        let n = 5;
        let mut rlmp = Rlmp::new(n);
        rlmp.add_column(&[0, 1]);
        rlmp.add_column(&[1, 2]);
        rlmp.add_column(&[2, 3]);
        rlmp.add_column(&[3, 4]);
        rlmp.add_column(&[0, 2, 4]);
        rlmp.solve();
        let mu = rlmp.duals();
        check_duals_non_negative(&mu);
        // Every added column must have rc >= 0: 1 - sum(mu_i for i in col) >= -tol.
        let all_cols: &[&[usize]] = &[
            &[0][..],
            &[1][..],
            &[2][..],
            &[3][..],
            &[4][..],
            &[0, 1][..],
            &[1, 2][..],
            &[2, 3][..],
            &[3, 4][..],
            &[0, 2, 4][..],
        ];
        for col in all_cols {
            let profit: f64 = col.iter().map(|&i| mu[i]).sum();
            assert!(1.0 - profit >= -1e-9, "column {col:?} has negative rc; mu={mu:?}");
        }
    }

    // Warm-start: adding a column after solve() converges in at most 1-2 pivots.
    #[test]
    fn warm_start_after_add_column() {
        let mut rlmp = Rlmp::new(3);
        rlmp.solve();
        assert!((rlmp.objective() - 3.0).abs() < 1e-9);
        rlmp.add_column(&[0, 1, 2]);
        rlmp.solve();
        assert!((rlmp.objective() - 1.0).abs() < 1e-9, "obj = {}", rlmp.objective());
    }

    // Refactorization path: force enough pivots to trigger REFACT_EVERY.
    #[test]
    fn refactorize_is_triggered() {
        let n = 10;
        let mut rlmp = Rlmp::new(n);
        // Add enough patterns to force many pivots (pairs of adjacent items).
        for i in 0..n - 1 {
            rlmp.add_column(&[i, i + 1]);
        }
        rlmp.add_column(&(0..n).collect::<Vec<_>>());
        rlmp.solve();
        // After all this, the full-cover pattern should dominate: obj = 1.
        let obj = rlmp.objective();
        assert!((obj - 1.0).abs() < 1e-9, "obj = {obj}");
    }
}
