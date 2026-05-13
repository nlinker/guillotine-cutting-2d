use crate::model::{Piece, PieceSpec, Placement, PlacementSpec, Problem, ProblemSpec, Solution, SolutionSpec};

// == expand_* : spec (type-indexed) -> flat ===================================

/// Expand a `ProblemSpec` into a flat `Problem` (one `Piece` entry per physical copy).
pub fn expand_problem(spec: &ProblemSpec) -> Problem {
    let pieces = spec
        .pieces
        .iter()
        .flat_map(|ps| {
            (0..ps.count).map(|_| Piece {
                name: ps.name.clone(),
                width: ps.width,
                height: ps.height,
                can_rotate: ps.can_rotate,
            })
        })
        .collect();
    Problem {
        sheet: spec.sheet,
        kerf: spec.kerf,
        pieces,
    }
}

/// Convert a `SolutionSpec` (type-indexed) into a flat `Solution`.
///
/// Each `PlacementSpec.piece_idx` (type) is mapped to a flat piece index using the
/// spec. Copies of the same type are assigned flat indices in spec order.
pub fn expand_solution(sol: &SolutionSpec, spec: &ProblemSpec) -> Solution {
    let type_to_flat_start: Vec<usize> = spec
        .pieces
        .iter()
        .scan(0usize, |acc, ps| {
            let start = *acc;
            *acc += ps.count as usize;
            Some(start)
        })
        .collect();
    let mut type_used: Vec<usize> = vec![0; spec.pieces.len()];
    let placements = sol
        .placements
        .iter()
        .map(|pl| {
            let ti = pl.piece_idx;
            let flat_idx = type_to_flat_start[ti] + type_used[ti];
            type_used[ti] += 1;
            Placement {
                sheet_idx: pl.sheet_idx,
                piece_idx: flat_idx,
                x: pl.x,
                y: pl.y,
                rotated: pl.rotated,
            }
        })
        .collect();
    Solution {
        placements,
        leftovers: sol.leftovers.clone(),
    }
}

// == shrink_* : flat -> spec (type-indexed) ===================================

/// Collapse a flat `Problem` into a `ProblemSpec` by grouping consecutive identical pieces.
///
/// Pieces are grouped by run: consecutive pieces with matching `(name, width, height,
/// can_rotate)` are merged into one `PieceSpec` with their combined count. Non-consecutive
/// identical pieces become separate entries.
pub fn shrink_problem(problem: &Problem) -> ProblemSpec {
    let mut pieces: Vec<PieceSpec> = Vec::new();
    for p in &problem.pieces {
        if let Some(last) = pieces.last_mut()
            && last.name == p.name
            && last.width == p.width
            && last.height == p.height
            && last.can_rotate == p.can_rotate
        {
            last.count += 1;
        } else {
            pieces.push(PieceSpec {
                name: p.name.clone(),
                width: p.width,
                height: p.height,
                count: 1,
                can_rotate: p.can_rotate,
            });
        }
    }
    ProblemSpec {
        sheet: problem.sheet,
        kerf: problem.kerf,
        pieces,
    }
}

/// Convert a flat `Solution` into a `SolutionSpec` using the originating `ProblemSpec`.
///
/// Each `Placement.piece_idx` (flat) is mapped back to the type index via the
/// `flat_to_type` table built from `spec`.
pub fn shrink_solution(sol: &Solution, spec: &ProblemSpec) -> SolutionSpec {
    let flat_to_type: Vec<usize> = spec
        .pieces
        .iter()
        .enumerate()
        .flat_map(|(ti, ps)| (0..ps.count).map(move |_| ti))
        .collect();
    let placements = sol
        .placements
        .iter()
        .map(|pl| PlacementSpec {
            sheet_idx: pl.sheet_idx,
            piece_idx: flat_to_type[pl.piece_idx],
            x: pl.x,
            y: pl.y,
            rotated: pl.rotated,
        })
        .collect();
    SolutionSpec {
        placements,
        leftovers: sol.leftovers.clone(),
    }
}

// == helpers ===================================================================

/// Build a `flat_to_type` mapping: `flat_to_type[flat_idx] = type_idx`.
pub fn flat_to_type_map(spec: &ProblemSpec) -> Vec<usize> {
    spec.pieces
        .iter()
        .enumerate()
        .flat_map(|(ti, ps)| (0..ps.count).map(move |_| ti))
        .collect()
}
