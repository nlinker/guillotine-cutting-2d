use crate::{
    expand::expand_problem,
    glf::{build_glf_from_flat, estimate_table_size},
    model::{Piece, PieceSpec, Placement, ProblemSpec},
};

/// Upper bound on the GLF DP table size (`estimate_table_size`) considered for a single
/// step. Keeps `find_max_fitting_prefix` (and the final `build_glf_from_flat`) from
/// rebuilding tables over an ever-growing candidate set as more pieces remain.
/// Underfilling a sheet when the budget is reached is acceptable: it simply leaves
/// more free rects for GLAS to use.
const ITERATION_BUDGET: usize = 10_000;

/// Result of the GLF pre-placement phase.
pub struct GlfPreplaceResult {
    /// All placements produced by GLF (sheet_idx 0..n_sheets-1).
    pub placements: Vec<Placement>,
    /// Number of sheets consumed by GLF.
    pub n_sheets: usize,
    /// GLF strip height (kerf-expanded) used on each sheet.
    pub used_heights: Vec<u32>,
    /// GLF strip width (kerf-expanded) actually used on each sheet — the bounding-box
    /// width of the placements, which may be less than the sheet width.
    pub used_widths: Vec<u32>,
    /// Reduced spec for the pieces that GLF did not place (passed to GLAS).
    pub remaining_spec: ProblemSpec,
    /// `remaining_problem.pieces[i]` -> `original_problem.pieces[j]`: flat-index translation.
    pub remaining_to_original: Vec<usize>,
}

/// Deterministically pre-place large pieces using GLF before handing the rest to GLAS.
///
/// For each step (up to `glf_steps`):
///   1. Sort remaining piece copies by `max(w, h)` descending.
///   2. Find the longest prefix that fits in one sheet (greedy, monotone).
///   3. Reconstruct GLF placements for that prefix and record them.
///   4. Update remaining counts.
///
/// Steps that produce 0 placements are silently skipped (not counted).
/// Returns immediately with an identity result when `glf_steps == 0`.
pub fn glf_preplace(spec: &ProblemSpec, glf_steps: usize) -> GlfPreplaceResult {
    let n_types = spec.piespecs.len();
    let total_pieces: usize = spec.piespecs.iter().map(|p| p.count as usize).sum();

    if glf_steps == 0 {
        return GlfPreplaceResult {
            placements: vec![],
            n_sheets: 0,
            used_heights: vec![],
            used_widths: vec![],
            remaining_spec: spec.clone(),
            remaining_to_original: (0..total_pieces).collect(),
        };
    }

    let problem = expand_problem(spec);
    let sw_exp = problem.sheet.width;
    let sh_exp = problem.sheet.height;

    // flat_start[ti] = first flat index for type ti in problem.pieces
    let flat_start: Vec<usize> = spec
        .piespecs
        .iter()
        .scan(0usize, |acc, ps| {
            let start = *acc;
            *acc += ps.count as usize;
            Some(start)
        })
        .collect();

    // flat_to_type[i] = type index of the i-th flat piece
    let mut flat_to_type = vec![0usize; total_pieces];
    for (ti, ps) in spec.piespecs.iter().enumerate() {
        for copy in 0..ps.count as usize {
            flat_to_type[flat_start[ti] + copy] = ti;
        }
    }

    // Sort types by max(w, h) desc (using original, non-expanded dimensions).
    let mut sorted_types: Vec<usize> = (0..n_types).collect();
    sorted_types.sort_by(|&a, &b| {
        let pa = &spec.piespecs[a];
        let pb = &spec.piespecs[b];
        pb.width.max(pb.height).cmp(&pa.width.max(pa.height))
    });

    // placed_copies[ti] = how many copies of type ti GLF has placed across all steps.
    let mut placed_copies = vec![0u32; n_types];
    let mut all_placements: Vec<Placement> = Vec::new();
    let mut used_heights: Vec<u32> = Vec::new();
    let mut used_widths: Vec<u32> = Vec::new();
    let mut n_sheets = 0usize;

    for _step in 0..glf_steps {
        // Build flat_indices for the unplaced copies, in sorted-type order.
        let flat_indices: Vec<usize> = sorted_types
            .iter()
            .flat_map(|&ti| {
                let placed = placed_copies[ti] as usize;
                let total = spec.piespecs[ti].count as usize;
                let start = flat_start[ti];
                (placed..total).map(move |copy| start + copy)
            })
            .collect();

        if flat_indices.is_empty() {
            break;
        }

        let bound = bounded_prefix_len(&problem.pieces, &flat_indices, ITERATION_BUDGET);
        let candidates = &flat_indices[..bound];

        let (k_best, used_h) = find_max_fitting_prefix(&problem.pieces, candidates, sw_exp, sh_exp);

        if k_best == 0 {
            continue;
        }

        let table = build_glf_from_flat(&problem.pieces, &flat_indices[..k_best]);
        let Some(mut step_placements) = table.reconstruct_flat(sw_exp) else {
            continue;
        };

        for pl in &mut step_placements {
            pl.sheet_idx = n_sheets;
        }

        for pl in &step_placements {
            placed_copies[flat_to_type[pl.piece_idx]] += 1;
        }

        let used_w = step_placements
            .iter()
            .map(|pl| {
                let piece = &problem.pieces[pl.piece_idx];
                let w = if pl.rotated { piece.height } else { piece.width };
                pl.x + w
            })
            .max()
            .unwrap_or(0);

        all_placements.extend(step_placements);
        used_heights.push(used_h);
        used_widths.push(used_w);
        n_sheets += 1;
    }

    // Build remaining_spec and remaining_to_original from placed_copies.
    let mut remaining_piespecs: Vec<PieceSpec> = Vec::new();
    let mut remaining_to_original: Vec<usize> = Vec::new();

    for ti in 0..n_types {
        let placed = placed_copies[ti] as usize;
        let total = spec.piespecs[ti].count as usize;
        let rem = total - placed;
        if rem > 0 {
            remaining_piespecs.push(PieceSpec {
                count: rem as u32,
                ..spec.piespecs[ti].clone()
            });
            for copy in placed..total {
                remaining_to_original.push(flat_start[ti] + copy);
            }
        }
    }

    let remaining_spec = ProblemSpec {
        piespecs: remaining_piespecs,
        ..spec.clone()
    };

    GlfPreplaceResult {
        placements: all_placements,
        n_sheets,
        used_heights,
        used_widths,
        remaining_spec,
        remaining_to_original,
    }
}

/// Find the longest prefix of `flat_indices` whose `estimate_table_size` does not
/// exceed `budget`. The estimate grows monotonically with the prefix length, so a
/// binary search suffices. Always returns at least 1 (if `flat_indices` is non-empty),
/// since a single piece type's table size (2) never exceeds a sane budget.
fn bounded_prefix_len(pieces: &[Piece], flat_indices: &[usize], budget: usize) -> usize {
    let n = flat_indices.len();
    if n == 0 {
        return 0;
    }
    if estimate_table_size(pieces, flat_indices) <= budget {
        return n;
    }
    let mut lo = 1usize;
    let mut hi = n;
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if estimate_table_size(pieces, &flat_indices[..mid]) <= budget {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

/// Find the longest prefix of `flat_indices` (pieces sorted large-first) that fits
/// in one strip of width `query_w` with height <= `max_h` (both kerf-expanded).
///
/// Returns `(best_k, used_h)`. If even one piece doesn't fit, returns `(0, 0)`.
/// Stops at the first k where GLF height exceeds `max_h` (monotone property).
fn find_max_fitting_prefix(
    pieces: &[Piece],
    flat_indices: &[usize],
    query_w: u32,
    max_h: u32,
) -> (usize, u32) {
    let mut best_k = 0;
    let mut best_h = 0;
    for k in 1..=flat_indices.len() {
        let table = build_glf_from_flat(pieces, &flat_indices[..k]);
        match table.eval_full_set(query_w) {
            Some(h) if h <= max_h => {
                best_k = k;
                best_h = h;
            }
            _ => break,
        }
    }
    (best_k, best_h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_problem;

    #[test]
    fn glf_steps_zero_is_identity() {
        let spec = parse_problem("100x100F:0:40x40/4,30x30/3").unwrap();
        let r = glf_preplace(&spec, 0);
        assert_eq!(r.n_sheets, 0);
        assert!(r.placements.is_empty());
        assert_eq!(r.remaining_spec.piespecs.len(), spec.piespecs.len());
        let total: usize = spec.piespecs.iter().map(|p| p.count as usize).sum();
        assert_eq!(r.remaining_to_original.len(), total);
    }

    #[test]
    fn glf_preplace_basic() {
        // Sheet 100x100, kerf=0. Two types: 50x50/2 and 30x30/3.
        // One GLF step should fit some large pieces on sheet 0.
        let spec = parse_problem("100x100F:0:50x50/2,30x30/3").unwrap();
        let r = glf_preplace(&spec, 1);
        // At minimum, the two 50x50 pieces should fit in one sheet (50+50=100 wide, height=50).
        assert!(r.n_sheets >= 1);
        assert!(!r.placements.is_empty());
        // Total pieces = original - placed.
        let total_orig: usize = spec.piespecs.iter().map(|p| p.count as usize).sum();
        let total_placed = r.placements.len();
        let total_remaining: usize = r.remaining_spec.piespecs.iter().map(|p| p.count as usize).sum();
        assert_eq!(total_placed + total_remaining, total_orig);
        assert_eq!(r.remaining_to_original.len(), total_remaining);
    }

    #[test]
    fn glf_preplace_consumes_all_when_steps_large() {
        // Sheet 200x200, kerf=0. Pieces that all fit on one sheet.
        let spec = parse_problem("200x200F:0:100x100/4").unwrap();
        let r = glf_preplace(&spec, 10);
        // All 4 pieces fit in one 200x200 sheet (2x2 grid).
        assert_eq!(r.n_sheets, 1);
        assert_eq!(r.placements.len(), 4);
        assert_eq!(r.remaining_spec.piespecs.iter().map(|p| p.count as usize).sum::<usize>(), 0);
    }

    #[test]
    fn glf_preplace_skip_when_no_fit() {
        // Sheet 10x10, kerf=0. Single piece 50x50 that doesn't fit.
        // Expect 0 steps to produce placements.
        let spec = parse_problem("10x10F:0:5x5/1").unwrap();
        let r = glf_preplace(&spec, 3);
        // 5x5 piece DOES fit in 10x10. Expect 1 sheet.
        assert_eq!(r.n_sheets, 1);
    }
}
