use crate::model::{
    FreeRect, Piece, PieceSpec, Placement, PlacementSpec, Problem, ProblemSpec, Sheet, Solution, SolutionSpec,
};

// == expand_* : spec (type-indexed) -> flat ===================================

/// Expand a `ProblemSpec` into a flat `Problem` (one `Piece` entry per physical copy).
///
/// Sheet and every piece are enlarged by `kerf` so that the decoder can place pieces
/// flush (kerf = 0) while preserving correct spatial relationships.
/// The sheet is also reduced by `margin` on every edge. Panics if the margin leaves no room.
pub fn expand_problem(spec: &ProblemSpec) -> Problem {
    let m = spec.margin;
    let k = spec.kerf;
    assert!(
        m * 2 < spec.sheet.width && m * 2 < spec.sheet.height,
        "margin ({m}) must be less than half the sheet dimensions ({}×{})",
        spec.sheet.width,
        spec.sheet.height,
    );
    let pieces = spec
        .piespecs
        .iter()
        .flat_map(|ps| {
            (0..ps.count).map(|_| Piece {
                name: ps.name.clone(),
                width: ps.width + k,
                height: ps.height + k,
                can_rotate: ps.can_rotate,
            })
        })
        .collect();
    Problem {
        sheet: Sheet {
            width: spec.sheet.width - 2 * m + k,
            height: spec.sheet.height - 2 * m + k,
        },
        pieces,
    }
}

/// Convert a `SolutionSpec` (type-indexed) into a flat `Solution`.
///
/// Each `PlacementSpec.piespec_idx` (type) is mapped to a flat piece index using the
/// spec. Copies of the same type are assigned flat indices in spec order.
pub fn expand_solution(sol: &SolutionSpec, spec: &ProblemSpec) -> Solution {
    let type_to_flat_start: Vec<usize> = spec
        .piespecs
        .iter()
        .scan(0usize, |acc, ps| {
            let start = *acc;
            *acc += ps.count as usize;
            Some(start)
        })
        .collect();
    let mut type_used: Vec<usize> = vec![0; spec.piespecs.len()];
    let placements = sol
        .placements
        .iter()
        .map(|pl| {
            let ti = pl.piespec_idx;
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
        kerf: 0,
        margin: 0,
        piespecs: pieces,
    }
}

/// Convert a flat `Solution` into a `SolutionSpec` using the originating `ProblemSpec`.
///
/// Each `Placement.piece_idx` (flat) is mapped back to the type index via the
/// `flat_to_type` table built from `spec`. Coordinates are shifted by `+spec.margin`
/// to restore physical sheet coordinates.
pub fn shrink_solution(sol: &Solution, spec: &ProblemSpec) -> SolutionSpec {
    let m = spec.margin;
    let flat_to_type: Vec<usize> = spec
        .piespecs
        .iter()
        .enumerate()
        .flat_map(|(ti, ps)| (0..ps.count).map(move |_| ti))
        .collect();
    let placements = sol
        .placements
        .iter()
        .map(|pl| PlacementSpec {
            sheet_idx: pl.sheet_idx,
            piespec_idx: flat_to_type[pl.piece_idx],
            x: pl.x + m,
            y: pl.y + m,
            rotated: pl.rotated,
        })
        .collect();
    let leftovers = sol
        .leftovers
        .iter()
        .map(|fr| FreeRect {
            x: fr.x + m,
            y: fr.y + m,
            ..*fr
        })
        .collect();
    SolutionSpec { placements, leftovers }
}

// == helpers ===================================================================

/// Build a `flat_to_type` mapping: `flat_to_type[flat_idx] = type_idx`.
pub fn flat_to_type_map(spec: &ProblemSpec) -> Vec<usize> {
    spec.piespecs
        .iter()
        .enumerate()
        .flat_map(|(ti, ps)| (0..ps.count).map(move |_| ti))
        .collect()
}
