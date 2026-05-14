use std::collections::HashMap;

use crate::model::{PlacementSpec, ProblemSpec, SolutionSpec};

/// A piece type used in the GPF DP table.
#[derive(Debug, Clone)]
pub struct GpfType {
    pub width: u32,
    pub height: u32,
    pub count: u32,
    pub can_rotate: bool,
}
/// Step function f(x; D): sorted by x ascending, heights strictly descending.
/// Semantics: f(x) = h_i for x ∈ [x_i, x_{i+1}), ∞ for x < x_1.
/// Empty vec = identically ∞ (infeasible).
/// Invariants: x_i < x_{i+1} and h_i > h_{i+1} for all i
/// For example, for `[(1,4),(2,2),(4,1)]` the f graph is
/// ```none
///   h
/// ∞ ↑
///   │
/// 4 │    ●────○
/// 3 │    │
/// 2 │    │    ●─────────○
/// 1 │    │    │         ●───────→
///   └────┴────┴─────────────────→ x
///        1    2    3    4
/// ```
pub type StepFn = Vec<(u32, u32)>;

/// Evaluate f(x): return height at given width, None if infeasible.
fn eval_f(f: &StepFn, x: u32) -> Option<u32> {
    // Find last breakpoint with x_i <= x (f is non-increasing step function).
    // Breakpoints sorted ascending, so find the last one where x_i <= x.
    let pos = f.partition_point(|&(xi, _)| xi <= x);
    if pos == 0 { None } else { Some(f[pos - 1].1) }
}

/// Evaluate f⁻¹(h): return minimum x such that f(x) <= h, None if no such x.
fn eval_f_inv(f: &StepFn, h: u32) -> Option<u32> {
    // Heights decrease: f[0].1 > f[1].1 > ...
    // Want first i, where f[i].1 <= h, that gives minimum x = f[i].0
    for &(xi, hi) in f {
        if hi <= h {
            return Some(xi);
        }
    }
    None
}

/// Remove points that do not strictly decrease in height (dominated or flat segments).
/// Input must be sorted by x ascending.
fn simplify(pts: &mut StepFn) {
    if pts.len() <= 1 {
        return;
    }
    let mut write = 1;
    for read in 1..pts.len() {
        if pts[read].1 < pts[write - 1].1 {
            pts[write] = pts[read];
            write += 1;
        }
        // If height == previous or x is duplicate: drop
    }
    pts.truncate(write);
}

/// H-cut: stack D1 on top of D2 (same width, add heights).
pub fn h_cut(f1: &StepFn, f2: &StepFn) -> StepFn {
    if f1.is_empty() || f2.is_empty() {
        return vec![];
    }
    // Merge x-breakpoints
    let mut xs = f1
        .iter()
        .map(|&(x, _)| x)
        .chain(f2.iter().map(|&(x, _)| x))
        .collect::<Vec<_>>();
    xs.sort_unstable();
    xs.dedup();

    let mut result = Vec::with_capacity(xs.len());
    for x in xs {
        if let (Some(h1), Some(h2)) = (eval_f(f1, x), eval_f(f2, x)) {
            result.push((x, h1 + h2));
        }
    }
    simplify(&mut result);
    result
}

/// V-cut: place D1 and D2 side by side (same height, add widths).
pub fn v_cut(f1: &StepFn, f2: &StepFn) -> StepFn {
    if f1.is_empty() || f2.is_empty() {
        return vec![];
    }
    // Collect candidate heights from both functions, sorted descending
    let mut hs = f1
        .iter()
        .map(|&(_, h)| h)
        .chain(f2.iter().map(|&(_, h)| h))
        .collect::<Vec<u32>>();
    hs.sort_unstable_by(|a, b| b.cmp(a));
    hs.dedup();

    let mut result = Vec::with_capacity(hs.len());
    for h in hs {
        if let (Some(w1), Some(w2)) = (eval_f_inv(f1, h), eval_f_inv(f2, h)) {
            result.push((w1 + w2, h));
        }
    }
    // result is in order of decreasing h → need to re-sort by x ascending
    // (w values may not be monotone before simplification)
    result.sort_unstable_by_key(|&(x, _)| x);
    simplify(&mut result);
    result
}

/// Pointwise minimum of two step-functions.
pub fn min_fn(f1: &StepFn, f2: &StepFn) -> StepFn {
    if f1.is_empty() {
        return f2.clone();
    }
    if f2.is_empty() {
        return f1.clone();
    }
    let mut xs = f1
        .iter()
        .map(|&(x, _)| x)
        .chain(f2.iter().map(|&(x, _)| x))
        .collect::<Vec<u32>>();
    xs.sort_unstable();
    xs.dedup();

    let mut result = Vec::with_capacity(xs.len());
    for x in xs {
        match (eval_f(f1, x), eval_f(f2, x)) {
            (Some(h1), Some(h2)) => result.push((x, h1.min(h2))),
            (Some(h), None) | (None, Some(h)) => result.push((x, h)),
            (None, None) => {}
        }
    }
    simplify(&mut result);
    result
}

/// The complete GPF DP table.
pub struct GpfTable {
    pub types: Vec<GpfType>,
    strides: Vec<usize>,
    cells: Vec<StepFn>,
    /// For each GPF type index, the list of `ProblemSpec.pieces` indices that
    /// contributed to it (one entry per physical copy, in spec order).
    type_to_spec: Vec<Vec<usize>>,
}

impl GpfTable {
    fn flat_index(&self, counts: &[u32]) -> usize {
        counts.iter().zip(&self.strides).map(|(&k, &s)| k as usize * s).sum()
    }

    fn total_subsets(&self) -> usize {
        self.types.iter().map(|t| (t.count + 1) as usize).product::<usize>()
    }

    /// Decode flat index back to per-type counts.
    fn decode_index(&self, mut idx: usize) -> Vec<u32> {
        let t = self.types.len();
        let mut counts = vec![0u32; t];
        // Type 0 is least significant digit (stride=1), so extract left-to-right.
        for (count, pt) in counts.iter_mut().zip(&self.types) {
            let modulus = (pt.count + 1) as usize;
            *count = (idx % modulus) as u32;
            idx /= modulus;
        }
        counts
    }

    /// Reconstruct an optimal 1-sheet placement for the full piece set at the given width.
    ///
    /// Returns `None` if the pieces don't fit at this width (GPF is infeasible).
    /// The returned `SolutionSpec` uses `piece_idx` = index into `ProblemSpec::pieces`.
    pub fn reconstruct(&self, width: u32) -> Option<SolutionSpec> {
        let height = self.eval_full_set(width)?;
        let full: Vec<u32> = self.types.iter().map(|t| t.count).collect();
        let mut placements = Vec::new();
        let mut used = vec![0usize; self.types.len()];
        if self.recon(&full, 0, 0, width, height, &mut placements, &mut used) {
            placements.sort_unstable_by_key(|p| (p.x, p.y));
            Some(SolutionSpec {
                placements,
                leftovers: vec![],
            })
        } else {
            None
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn recon(
        &self,
        counts: &[u32],
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        placements: &mut Vec<PlacementSpec>,
        used: &mut Vec<usize>,
    ) -> bool {
        let total: u32 = counts.iter().sum();
        if total == 0 {
            return true;
        }
        if total == 1 {
            let ti = counts
                .iter()
                .position(|&k| k == 1)
                .expect("total==1 implies a type with count==1");
            let spec_idx = self.type_to_spec[ti][used[ti]];
            used[ti] += 1;
            let rotated = self.types[ti].can_rotate
                && self.types[ti].width != self.types[ti].height
                && height == self.types[ti].width;
            placements.push(PlacementSpec {
                sheet_idx: 0,
                piece_idx: spec_idx,
                x,
                y,
                rotated,
            });
            return true;
        }

        let ranges: Vec<u32> = counts.to_vec();
        let n_splits: usize = ranges.iter().map(|&k| (k + 1) as usize).product();

        for sf in 0..n_splits {
            let d1 = decode_split(&ranges, sf);
            let t1: u32 = d1.iter().sum();
            if t1 == 0 || t1 == total {
                continue;
            }

            let d2: Vec<u32> = counts.iter().zip(&d1).map(|(&k, &a)| k - a).collect();
            let f1 = &self.cells[flat_index_with(&self.strides, &d1)];
            let f2 = &self.cells[flat_index_with(&self.strides, &d2)];
            if f1.is_empty() || f2.is_empty() {
                continue;
            }

            // H-cut: d1 on top, d2 on bottom
            if let (Some(h1), Some(h2)) = (eval_f(f1, width), eval_f(f2, width))
                && h1 + h2 == height
            {
                let snap = (placements.len(), used.clone());
                if self.recon(&d1, x, y, width, h1, placements, used)
                    && self.recon(&d2, x, y + h1, width, h2, placements, used)
                {
                    return true;
                }
                placements.truncate(snap.0);
                *used = snap.1;
            }

            // V-cut: d1 on left, d2 on right.
            // w1+w2 may be < width (unused space on the right is fine).
            if let (Some(w1), Some(w2)) = (eval_f_inv(f1, height), eval_f_inv(f2, height))
                && w1 + w2 <= width
            {
                let h1 = eval_f(f1, w1).unwrap_or(height);
                let h2 = eval_f(f2, w2).unwrap_or(height);
                let snap = (placements.len(), used.clone());
                if self.recon(&d1, x, y, w1, h1, placements, used)
                    && self.recon(&d2, x + w1, y, w2, h2, placements, used)
                {
                    return true;
                }
                placements.truncate(snap.0);
                *used = snap.1;
            }
        }
        false
    }

    /// Evaluate f(width; full set).
    pub fn eval_full_set(&self, width: u32) -> Option<u32> {
        let full = self.types.iter().map(|t| t.count).collect::<Vec<u32>>();
        let idx = self.flat_index(&full);
        eval_f(&self.cells[idx], width)
    }

    /// Evaluate f(width; subset specified by per-type counts).
    pub fn eval_subset(&self, counts: &[u32], width: u32) -> Option<u32> {
        let idx = self.flat_index(counts);
        eval_f(&self.cells[idx], width)
    }

    /// Print the full DP table: each non-empty subset, its step function, and f(width).
    pub fn display_table(&self, width: u32) -> String {
        let total = self.total_subsets();
        // Collect all non-empty subsets, sort by total piece count
        let mut rows = (1..total)
            .map(|idx| {
                let counts = self.decode_index(idx);
                let total_pieces: u32 = counts.iter().sum();
                (idx, counts, total_pieces)
            })
            .filter(|(idx, _, _)| !self.cells[*idx].is_empty())
            .map(|(_idx, counts, total_pieces)| (total_pieces as usize, counts))
            .collect::<Vec<_>>();
        rows.sort_by_key(|(tp, _)| *tp);

        // Type labels: A, B, C, ...
        let labels = (0..self.types.len())
            .map(|i| {
                if i < 26 {
                    format!("{}", (b'A' + i as u8) as char)
                } else {
                    format!("T{i}")
                }
            })
            .collect::<Vec<_>>();

        let mut lines = Vec::new();
        lines.push(format!("{:<20}  {:<40}  f({})", "Subset", "Step function", width));
        lines.push("-".repeat(70));

        for (_, counts) in &rows {
            let label = subset_label(&labels, counts);
            let idx = self.flat_index(counts);
            let f = &self.cells[idx];
            let step_str = format_step_fn(f);
            let f_width = eval_f(f, width)
                .map(|h| h.to_string())
                .unwrap_or_else(|| "∞".to_string());
            lines.push(format!("{:<20}  {:<40}  {}", label, step_str, f_width));
        }
        lines.join("\n")
    }
}

fn subset_label(labels: &[String], counts: &[u32]) -> String {
    let parts = counts
        .iter()
        .enumerate()
        .filter(|(_, k)| **k > 0)
        .map(|(i, &k)| {
            if k == 1 {
                labels[i].clone()
            } else {
                format!("{}·{}", k, labels[i])
            }
        })
        .collect::<Vec<_>>();
    format!("{{{}}}", parts.join(","))
}

fn format_step_fn(f: &StepFn) -> String {
    if f.is_empty() {
        return "∞".to_string();
    }
    let parts = f.iter().map(|&(x, h)| format!("({x},{h})")).collect::<Vec<_>>();
    format!("[{}]", parts.join(", "))
}

/// Build the GPF table for a problem spec.
///
/// Piece dimensions are expanded by `spec.kerf` (same transformation as `expand_problem`),
/// so the resulting step functions operate in the same kerf-free coordinate space as the decoder.
/// Query `eval_full_set` with `spec.sheet.width + spec.kerf` to check feasibility.
pub fn build_gpf(spec: &ProblemSpec) -> GpfTable {
    let k = spec.kerf;
    // Group piece specs by (width, height) preserving order of first appearance.
    // Multiple specs with the same dimensions are merged (summing counts).
    // ProblemSpec is normalized: (width, height, can_rotate) is unique per entry.
    let mut type_map: HashMap<(u32, u32, bool), usize> = HashMap::new();
    let mut types: Vec<GpfType> = Vec::new();
    for ps in &spec.pieces {
        let key = (ps.width + k, ps.height + k, ps.can_rotate);
        if let Some(&ti) = type_map.get(&key) {
            types[ti].count += ps.count;
        } else {
            let ti = types.len();
            type_map.insert(key, ti);
            types.push(GpfType {
                width: ps.width + k,
                height: ps.height + k,
                count: ps.count,
                can_rotate: ps.can_rotate,
            });
        }
    }
    let t = types.len();

    // 2. Compute strides: strides[i] = ∏_{j<i} (types[j].count + 1)
    let mut strides = vec![1usize; t];
    for i in 1..t {
        strides[i] = strides[i - 1] * (types[i - 1].count + 1) as usize;
    }
    let total: usize = if t == 0 {
        1
    } else {
        strides[t - 1] * (types[t - 1].count + 1) as usize
    };

    let mut cells: Vec<StepFn> = vec![vec![]; total];

    // 3. Collect all non-empty subsets sorted by total piece count (BFS order)
    let mut subsets: Vec<(u32, usize, Vec<u32>)> = Vec::with_capacity(total - 1);
    for flat in 1..total {
        let counts = decode_index_with(&types, flat);
        let total_pieces: u32 = counts.iter().sum();
        subsets.push((total_pieces, flat, counts));
    }
    subsets.sort_unstable_by_key(|&(tp, _, _)| tp);

    // 4. Fill DP table
    for (total_pieces, flat, counts) in subsets {
        if total_pieces == 1 {
            // Base case: find which type has exactly 1 piece
            for (i, &k) in counts.iter().enumerate() {
                if k == 1 {
                    let f_normal = vec![(types[i].width, types[i].height)];
                    cells[flat] = if types[i].can_rotate && types[i].width != types[i].height {
                        min_fn(&f_normal, &vec![(types[i].height, types[i].width)])
                    } else {
                        f_normal
                    };
                    break;
                }
            }
        } else {
            // Enumerate all splits D = D1 ∪ D2
            // D1 = a[i] pieces of type i, D2 = counts[i] - a[i]
            let mut best: StepFn = vec![];
            let ranges = counts.to_vec();
            let split_count: usize = ranges.iter().map(|&k| (k + 1) as usize).product();

            for split_flat in 0..split_count {
                // Decode split_flat into per-type split (a[i] for D1)
                let d1_counts = decode_split(&ranges, split_flat);
                let d1_total: u32 = d1_counts.iter().sum();
                if d1_total == 0 || d1_total == total_pieces {
                    continue; // skip empty splits
                }
                let d2_counts = counts.iter().zip(&d1_counts).map(|(&k, &a)| k - a).collect::<Vec<_>>();

                let d1_flat = flat_index_with(&strides, &d1_counts);
                let d2_flat = flat_index_with(&strides, &d2_counts);

                // Avoid processing (D1, D2) and (D2, D1) twice
                if d1_flat > d2_flat {
                    continue;
                }

                let f1 = &cells[d1_flat];
                let f2 = &cells[d2_flat];
                if f1.is_empty() || f2.is_empty() {
                    continue;
                }

                let fh = h_cut(f1, f2);
                let fv = v_cut(f1, f2);
                let candidate = min_fn(&fh, &fv);
                best = min_fn(&best, &candidate);
            }
            cells[flat] = best;
        }
    }

    // Build type_to_spec: for each GPF type, list of spec piece indices (one per copy).
    let mut type_to_spec: Vec<Vec<usize>> = vec![vec![]; t];
    for (spec_idx, ps) in spec.pieces.iter().enumerate() {
        let ti = type_map[&(ps.width + k, ps.height + k, ps.can_rotate)];
        for _ in 0..ps.count {
            type_to_spec[ti].push(spec_idx);
        }
    }

    GpfTable {
        types,
        strides,
        cells,
        type_to_spec,
    }
}

fn decode_index_with(types: &[GpfType], mut idx: usize) -> Vec<u32> {
    let mut counts = vec![0u32; types.len()];
    for (count, pt) in counts.iter_mut().zip(types) {
        let modulus = (pt.count + 1) as usize;
        *count = (idx % modulus) as u32;
        idx /= modulus;
    }
    counts
}

fn flat_index_with(strides: &[usize], counts: &[u32]) -> usize {
    counts.iter().zip(strides).map(|(&k, &s)| k as usize * s).sum()
}

fn decode_split(ranges: &[u32], mut idx: usize) -> Vec<u32> {
    let mut result = vec![0u32; ranges.len()];
    for (out, &range) in result.iter_mut().zip(ranges) {
        let modulus = (range + 1) as usize;
        *out = (idx % modulus) as u32;
        idx /= modulus;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_problem;

    #[test]
    fn eval_f_basic() {
        let f: StepFn = vec![(2, 3), (4, 2)];
        assert_eq!(eval_f(&f, 1), None);
        assert_eq!(eval_f(&f, 2), Some(3));
        assert_eq!(eval_f(&f, 3), Some(3));
        assert_eq!(eval_f(&f, 4), Some(2));
        assert_eq!(eval_f(&f, 10), Some(2));
    }

    #[test]
    fn eval_f_inv_basic() {
        // f = [(8,6),(10,3)]:
        //      x∈[0,8)  -> f(x) = ∞
        //      x∈[8,10) -> f(x) = 6
        //      x∈[10,∞) -> f(x) = 3
        // f⁻¹ = [(3,10),(6,8)]:
        //      x∈[0,3) -> f(x) = ∞
        //      x∈[3,6) -> f(x) = 10
        //      x∈[6,∞) -> f(x) = 8
        let f: StepFn = vec![(8, 6), (10, 3)];
        assert_eq!(eval_f_inv(&f, 2), None);
        assert_eq!(eval_f_inv(&f, 3), Some(10));
        assert_eq!(eval_f_inv(&f, 5), Some(10));
        assert_eq!(eval_f_inv(&f, 6), Some(8));
        assert_eq!(eval_f_inv(&f, 7), Some(8));
    }

    #[test]
    fn h_cut_basic() {
        // f({C}) = [(8,3)], f({A}) = [(2,3)]
        let fc: StepFn = vec![(8, 3)];
        let fa: StepFn = vec![(2, 3)];
        let fh = h_cut(&fc, &fa);
        assert_eq!(fh, vec![(8, 6)]);
    }

    #[test]
    fn v_cut_basic() {
        // f({C}) = [(8,3)], f({A}) = [(2,3)]
        let fc: StepFn = vec![(8, 3)];
        let fa: StepFn = vec![(2, 3)];
        let fv = v_cut(&fc, &fa);
        assert_eq!(fv, vec![(10, 3)]);
    }

    #[test]
    fn combined_ca() {
        let fc: StepFn = vec![(8, 3)];
        let fa: StepFn = vec![(2, 3)];
        let fh = h_cut(&fc, &fa);
        let fv = v_cut(&fc, &fa);
        let combined = min_fn(&fh, &fv);
        // x ∈ [8,10): H wins -> 6; x >= 10: V wins → 3
        assert_eq!(combined, vec![(8, 6), (10, 3)]);
    }

    #[test]
    fn tutorial_example() {
        // "10x8F:0:2x3/4,4x3,8x3,5x2/2" — all fixed, kerf=0
        let p = parse_problem("10x8F:0:2x3/4,4x3,8x3,5x2/2").unwrap();
        let table = build_gpf(&p);
        // Full set at width=10 must equal 8
        assert_eq!(table.eval_full_set(10), Some(8));
        // Base cases
        assert_eq!(table.eval_subset(&[1, 0, 0, 0], 2), Some(3)); // {A} at x=2
        assert_eq!(table.eval_subset(&[0, 1, 0, 0], 4), Some(3)); // {B} at x=4
        assert_eq!(table.eval_subset(&[0, 0, 1, 0], 8), Some(3)); // {C} at x=8
        assert_eq!(table.eval_subset(&[0, 0, 0, 1], 5), Some(2)); // {D} at x=5
    }
}
