use crate::model::{
    FreeRect, Piece, PieceType, Placement, PlacementSpec, Problem, ProblemSpec, Sheet, Solution, SolutionSpec,
};

/// Expands a `ProblemSpec` into a flat `Problem` (one `Piece` per physical copy), enlarging
/// sheet and pieces by `kerf` so the decoder can place flush, and shrinking the sheet by
/// `margin`. Panics if the margin leaves no room.
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
        .piece_types
        .iter()
        .flat_map(|ps| {
            (0..ps.count).map(|_| Piece {
                name: ps.name.clone(),
                width: ps.width + k,
                height: ps.height + k,
                can_rotate: ps.can_rotate,
            })
        })
        .collect::<Vec<Piece>>();
    Problem { sheet: Sheet { width: spec.sheet.width - 2 * m + k, height: spec.sheet.height - 2 * m + k }, pieces }
}

/// Converts a `SolutionSpec` (type-indexed) into a flat `Solution`, mapping each
/// `ptype_idx` to a flat piece index (copies of the same type get indices in spec order).
pub fn expand_solution(sol: &SolutionSpec, spec: &ProblemSpec) -> Solution {
    let type_to_flat_start = spec
        .piece_types
        .iter()
        .scan(0usize, |acc, ps| {
            let start = *acc;
            *acc += ps.count as usize;
            Some(start)
        })
        .collect::<Vec<usize>>();
    let mut type_used: Vec<usize> = vec![0; spec.piece_types.len()];
    let placements = sol
        .placements
        .iter()
        .map(|pl| {
            let ti = pl.ptype_idx;
            let flat_idx = type_to_flat_start[ti] + type_used[ti];
            type_used[ti] += 1;
            Placement { sheet_idx: pl.sheet_idx, piece_idx: flat_idx, x: pl.x, y: pl.y, rotated: pl.rotated }
        })
        .collect::<Vec<Placement>>();
    Solution { placements, leftovers: sol.leftovers.clone() }
}

/// Collapses a flat `Problem` into a `ProblemSpec` by merging runs of consecutive pieces
/// with matching `(name, width, height, can_rotate)`; non-consecutive duplicates stay separate.
pub fn shrink_problem(problem: &Problem) -> ProblemSpec {
    let mut pieces: Vec<PieceType> = Vec::new();
    for p in &problem.pieces {
        if let Some(last) = pieces.last_mut()
            && last.name == p.name
            && last.width == p.width
            && last.height == p.height
            && last.can_rotate == p.can_rotate
        {
            last.count += 1;
        } else {
            pieces.push(PieceType {
                name: p.name.clone(),
                width: p.width,
                height: p.height,
                count: 1,
                can_rotate: p.can_rotate,
            });
        }
    }
    ProblemSpec { sheet: problem.sheet, kerf: 0, margin: 0, piece_types: pieces }
}

/// Converts a flat `Solution` back into a `SolutionSpec`: maps each flat `piece_idx` back
/// to its type index, and shifts coordinates by `+spec.margin` to restore sheet coordinates.
pub fn shrink_solution(sol: &Solution, spec: &ProblemSpec) -> SolutionSpec {
    let m = spec.margin;
    let flat_to_type = spec
        .piece_types
        .iter()
        .enumerate()
        .flat_map(|(ti, ps)| (0..ps.count).map(move |_| ti))
        .collect::<Vec<usize>>();
    let mut placements = sol
        .placements
        .iter()
        .map(|pl| PlacementSpec {
            sheet_idx: pl.sheet_idx,
            ptype_idx: flat_to_type[pl.piece_idx],
            x: pl.x + m,
            y: pl.y + m,
            rotated: pl.rotated,
        })
        .collect::<Vec<PlacementSpec>>();
    placements.sort_by_key(|pl| (pl.sheet_idx, pl.x, pl.y));
    let leftovers = sol
        .leftovers
        .iter()
        .map(|fr| FreeRect { x: fr.x + m, y: fr.y + m, ..*fr })
        .collect();
    SolutionSpec { placements, leftovers }
}

/// Build a `flat_to_type` mapping: `flat_to_type[flat_idx] = type_idx`.
pub fn flat_to_type_map(spec: &ProblemSpec) -> Vec<usize> {
    spec.piece_types
        .iter()
        .enumerate()
        .flat_map(|(ti, ps)| (0..ps.count).map(move |_| ti))
        .collect()
}
