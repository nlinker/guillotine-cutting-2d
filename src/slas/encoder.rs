use smallvec::{SmallVec, smallvec};

use super::decoder::{CutCtx, Gene, Genome, find_placement, fits_in, guillotine_split, open_new_sheet, sheet_rect};
use crate::{
    expand, model,
    model::{FreeRect, Piece, Problem, Solution},
};

type FreeList = SmallVec<[(FreeRect, Option<CutCtx>); 16]>;

/// Encode a [`Solution`] as a [`Genome`] suitable as a GA seed.
///
/// Sorts placements by `(sheet_idx, x, y)` so pieces on the same sheet appear
/// together. Simulates the SLAS decoder step-by-step and chooses `point_selector`
/// values that steer it toward the original placement positions where possible.
///
/// The resulting genome may not reproduce `solution` exactly — SLAS splits can
/// differ from the original cut tree — but gives the GA a warm start with the
/// correct piece grouping and rotation preferences.
pub fn encode(solution: &Solution, problem: &Problem) -> Genome {
    let mut order: Vec<usize> = (0..solution.placements.len()).collect();
    order.sort_unstable_by_key(|&i| {
        let p = &solution.placements[i];
        (p.sheet_idx, p.x, p.y)
    });

    let mut free: FreeList = smallvec![(sheet_rect(problem, 0), None)];
    let mut sheets_open = 1usize;
    let mut genome = Vec::with_capacity(order.len());

    for (i, &pl_idx) in order.iter().enumerate() {
        let pl = &solution.placements[pl_idx];
        let piece = &problem.pieces[pl.piece_idx];

        let point_selector = preferred_free_rect(&free, pl.sheet_idx, pl.x, pl.y, piece, pl.rotated) as u32;

        let found = find_placement(&free, piece, pl.rotated, point_selector)
            .or_else(|| open_new_sheet(&mut free, &mut sheets_open, problem, piece, pl.rotated));

        if let Some((idx, pw, ph, _)) = found {
            let (fr, _) = free.remove(idx);

            let inverse = if let Some(&next_pl_idx) = order.get(i + 1) {
                let next_pl = &solution.placements[next_pl_idx];
                let next_piece = &problem.pieces[next_pl.piece_idx];
                let lw = fr.w - pw;
                let lh = fr.h - ph;
                if lw > 0 && lh > 0 {
                    let split_a = guillotine_split(&fr, pw, ph, false);
                    let split_b = guillotine_split(&fr, pw, ph, true);
                    let origin_in = |split: &SmallVec<[FreeRect; 2]>| {
                        free.iter().map(|(r, _)| r).chain(split.iter()).any(|r| {
                            r.sheet_idx == next_pl.sheet_idx
                                && r.x == next_pl.x
                                && r.y == next_pl.y
                                && fits_in(r, next_piece, next_pl.rotated).is_some()
                        })
                    };
                    !origin_in(&split_a) && origin_in(&split_b)
                } else {
                    false
                }
            } else {
                false
            };

            for fr_new in guillotine_split(&fr, pw, ph, inverse) {
                free.push((fr_new, None));
            }
            genome.push(Gene {
                piece_idx: pl.piece_idx,
                rotate: pl.rotated,
                point_selector,
                inverse,
            });
        }
    }

    genome
}

/// Encode a [`SolutionSpec`] as a [`Genome`] — spec-level counterpart of [`encode`].
pub fn encode_spec(spec: &model::ProblemSpec, sol: &model::SolutionSpec) -> Genome {
    let problem = expand::expand_problem(spec);
    let flat_sol = expand::expand_solution(sol, spec);
    encode(&flat_sol, &problem)
}

/// Returns the index of the preferred free rect for encoding a placement at `(sheet_idx, x, y)`.
/// Priority: exact origin match on the correct sheet -> any fitting rect on the same sheet ->
/// any fitting rect on any sheet. Falls back to 0 (decoder will open a new sheet).
fn preferred_free_rect(
    free: &[(FreeRect, Option<CutCtx>)],
    sheet_idx: usize,
    x: u32,
    y: u32,
    piece: &Piece,
    prefer_rotate: bool,
) -> usize {
    if let Some(i) = free.iter().position(|(fr, _)| {
        fr.sheet_idx == sheet_idx && fr.x == x && fr.y == y && fits_in(fr, piece, prefer_rotate).is_some()
    }) {
        return i;
    }
    if let Some(i) = free
        .iter()
        .position(|(fr, _)| fr.sheet_idx == sheet_idx && fits_in(fr, piece, prefer_rotate).is_some())
    {
        return i;
    }
    free.iter()
        .position(|(fr, _)| fits_in(fr, piece, prefer_rotate).is_some())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        expand::expand_problem,
        model::{Placement, Solution},
        parse::parse_problem,
        slas::decoder::decode,
    };

    fn g(piece_idx: usize, rotate: bool, ps: u32) -> Gene {
        Gene {
            piece_idx,
            rotate,
            point_selector: ps,
            inverse: false,
        }
    }

    #[test]
    fn encode_2x2_tiles_one_sheet() {
        // 4 identical 1×1 pieces tiling a 2×2 sheet exactly (kerf=0).
        // Encoder must produce a genome that decodes back to 1 sheet.
        let spec = parse_problem("2x2F:0:1x1/4").expect("parse");
        let problem = expand_problem(&spec);
        let optimal = Solution {
            placements: vec![
                Placement {
                    sheet_idx: 0,
                    piece_idx: 0,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
                Placement {
                    sheet_idx: 0,
                    piece_idx: 1,
                    x: 1,
                    y: 0,
                    rotated: false,
                },
                Placement {
                    sheet_idx: 0,
                    piece_idx: 2,
                    x: 0,
                    y: 1,
                    rotated: false,
                },
                Placement {
                    sheet_idx: 0,
                    piece_idx: 3,
                    x: 1,
                    y: 1,
                    rotated: false,
                },
            ],
            leftovers: vec![],
            mfg_cost: 0,
        };
        let genome = encode(&optimal, &problem);
        assert_eq!(genome.len(), 4);
        let sol = decode(&problem, &genome);
        assert_eq!(sol.sheets_used(), 1);
    }

    #[test]
    fn encode_preserves_sheet_count() {
        // encode(decode(genome)) must not increase sheet count
        let spec = parse_problem("200x150F:5:120x80,60x80,200x60,70x100r,60x70r").expect("parse");
        let problem = expand_problem(&spec);
        let genome_orig = vec![
            g(0, false, 0),
            g(1, false, 0),
            g(2, false, 0),
            g(3, true, 0),
            g(4, true, 2),
        ];
        let sol = decode(&problem, &genome_orig);
        let genome_enc = encode(&sol, &problem);
        let sol2 = decode(&problem, &genome_enc);
        assert!(sol2.sheets_used() <= sol.sheets_used());
    }

    #[test]
    fn encode_spec_roundtrip() {
        let spec = parse_problem("10x10F:0:3x4,4x3,5x5").expect("parse");
        let problem = expand_problem(&spec);
        let genome_orig: Genome = (0..3).map(|i| g(i, false, 0)).collect();
        let sol = decode(&problem, &genome_orig);
        let sol_spec = crate::expand::shrink_solution(&sol, &spec);
        let genome_enc = encode_spec(&spec, &sol_spec);
        let sol2 = decode(&problem, &genome_enc);
        assert!(sol2.sheets_used() <= sol.sheets_used());
    }
}
