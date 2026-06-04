use crate::model::{Placement, Problem, Solution};

/// Standalone multi-sheet BFDH solver.
///
/// Sorts all pieces by height descending (rotating if it makes the piece taller),
/// then places them using Best-Fit Decreasing Height shelf packing.
/// Opens a new sheet whenever the current one overflows.
pub fn bfdh_solve(problem: &Problem) -> Solution {
    let sw = problem.sheet.width;
    let sh = problem.sheet.height;

    let mut items: Vec<(usize, u32, u32)> = (0..problem.pieces.len())
        .map(|i| {
            let p = &problem.pieces[i];
            if p.can_rotate && p.width > p.height { (i, p.height, p.width) }
            else { (i, p.width, p.height) }
        })
        .collect();
    items.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));

    let mut placements: Vec<Placement> = Vec::with_capacity(items.len());
    let mut sheet_idx = 0usize;
    let mut shelves: Vec<Shelf> = Vec::new();
    let mut y_cursor: u32 = 0;

    for (orig_idx, pw, ph) in &items {
        let best = shelves
            .iter_mut()
            .filter(|s| *ph <= s.h && sw - s.x_cursor >= *pw)
            .min_by_key(|s| sw - s.x_cursor - pw);

        if let Some(shelf) = best {
            let rotated = problem.pieces[*orig_idx].can_rotate
                && problem.pieces[*orig_idx].width != *pw;
            placements.push(Placement {
                sheet_idx, piece_idx: *orig_idx, x: shelf.x_cursor, y: shelf.y, rotated,
            });
            shelf.x_cursor += pw;
        } else if y_cursor + ph <= sh {
            let rotated = problem.pieces[*orig_idx].can_rotate
                && problem.pieces[*orig_idx].width != *pw;
            placements.push(Placement {
                sheet_idx, piece_idx: *orig_idx, x: 0, y: y_cursor, rotated,
            });
            shelves.push(Shelf { y: y_cursor, h: *ph, x_cursor: *pw });
            y_cursor += ph;
        } else {
            sheet_idx += 1;
            shelves.clear();
            y_cursor = *ph;
            let rotated = problem.pieces[*orig_idx].can_rotate
                && problem.pieces[*orig_idx].width != *pw;
            placements.push(Placement {
                sheet_idx, piece_idx: *orig_idx, x: 0, y: 0, rotated,
            });
            shelves.push(Shelf { y: 0, h: *ph, x_cursor: *pw });
        }
    }
    Solution { placements, leftovers: vec![] }
}

/// Post-GA per-sheet optimizer using BFDH (Best-Fit Decreasing Height).
///
/// For each sheet in `sol`, tries to repack all pieces from that sheet using
/// the BFDH shelf heuristic: pieces are sorted by height descending and placed
/// into the best-fitting existing shelf (minimum leftover width) or a new one.
///
/// If pieces from a sheet do not fit on a single sheet under BFDH, the original
/// GA placements for that sheet are preserved unchanged.
///
/// The result has the same number of sheets as the input solution.
pub fn greedy_improve_solution(problem: &Problem, sol: &Solution) -> Solution {
    let n_sheets = sol.sheets_used();
    let mut improved: Vec<Placement> = Vec::with_capacity(sol.placements.len());
    for sheet_idx in 0..n_sheets {
        let original: Vec<Placement> = sol
            .placements
            .iter()
            .filter(|pl| pl.sheet_idx == sheet_idx)
            .cloned()
            .collect();
        let piece_indices: Vec<usize> = original.iter().map(|pl| pl.piece_idx).collect();
        match bfdh_pack(problem, &piece_indices, sheet_idx) {
            Some(pls) => improved.extend(pls),
            None => improved.extend(original),
        }
    }
    Solution {
        placements: improved,
        leftovers: vec![],
    }
}

struct Shelf {
    y: u32,
    h: u32,
    x_cursor: u32,
}

/// Try to pack `piece_indices` onto a single sheet using BFDH.
/// Returns `Some(placements)` with all pieces on `sheet_idx`, or `None` if overflow.
fn bfdh_pack(problem: &Problem, piece_indices: &[usize], sheet_idx: usize) -> Option<Vec<Placement>> {
    if piece_indices.is_empty() {
        return Some(vec![]);
    }
    let sw = problem.sheet.width;
    let sh = problem.sheet.height;

    // For can_rotate pieces orient so that the longest side is the height
    // (taller pieces define deeper shelves → same-height pieces share a shelf).
    let mut items: Vec<(usize, u32, u32)> = piece_indices
        .iter()
        .map(|&i| {
            let p = &problem.pieces[i];
            if p.can_rotate && p.width > p.height {
                (i, p.height, p.width) // placed_w=p.height, placed_h=p.width
            } else {
                (i, p.width, p.height)
            }
        })
        .collect();
    // Sort by placed_h desc, tiebreak placed_w desc
    items.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));

    let mut shelves: Vec<Shelf> = Vec::new();
    let mut placements: Vec<Placement> = Vec::with_capacity(items.len());
    let mut y_cursor: u32 = 0;

    for (orig_idx, pw, ph) in &items {
        // Best-fit: find shelf with smallest remaining width where the piece fits
        let best = shelves
            .iter_mut()
            .filter(|s| *ph <= s.h && sw - s.x_cursor >= *pw)
            .min_by_key(|s| sw - s.x_cursor - pw);

        if let Some(shelf) = best {
            let x = shelf.x_cursor;
            let rotated = {
                let p = &problem.pieces[*orig_idx];
                p.can_rotate && p.width != *pw
            };
            placements.push(Placement {
                sheet_idx,
                piece_idx: *orig_idx,
                x,
                y: shelf.y,
                rotated,
            });
            shelf.x_cursor += pw;
        } else {
            // Open a new shelf
            if y_cursor + ph > sh {
                return None;
            }
            let rotated = {
                let p = &problem.pieces[*orig_idx];
                p.can_rotate && p.width != *pw
            };
            placements.push(Placement {
                sheet_idx,
                piece_idx: *orig_idx,
                x: 0,
                y: y_cursor,
                rotated,
            });
            shelves.push(Shelf {
                y: y_cursor,
                h: *ph,
                x_cursor: *pw,
            });
            y_cursor += ph;
        }
    }
    Some(placements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, model::validate_solution, parse::parse_problem};

    #[test]
    fn bfdh_fits_single_sheet() {
        // Sheet 200x100, kerf=0. Four 50x50 pieces: should pack as 2x2 grid on 1 sheet.
        let spec = parse_problem("200x100F:0:50x50/4").unwrap();
        let problem = expand_problem(&spec);
        let indices: Vec<usize> = (0..4).collect();
        let pls = bfdh_pack(&problem, &indices, 0).expect("should fit");
        assert_eq!(pls.len(), 4);
        assert!(pls.iter().all(|pl| pl.sheet_idx == 0));
    }

    #[test]
    fn bfdh_overflow_returns_none() {
        // Sheet 100x100, kerf=0. Three 100x60 pieces: cannot all fit on one sheet.
        let spec = parse_problem("100x100F:0:100x60/3").unwrap();
        let problem = expand_problem(&spec);
        let indices: Vec<usize> = (0..3).collect();
        assert!(bfdh_pack(&problem, &indices, 0).is_none());
    }

    #[test]
    fn greedy_improve_preserves_sheet_count() {
        let spec = parse_problem("100x100F:0:60x60/4").unwrap();
        let problem = expand_problem(&spec);
        let spec_arc = std::sync::Arc::new(spec.clone());
        // Build a trivial solution: put all pieces on separate sheets (worst case)
        let flat_placements: Vec<Placement> = (0..4)
            .map(|i| Placement {
                sheet_idx: i,
                piece_idx: i,
                x: 0,
                y: 0,
                rotated: false,
            })
            .collect();
        let sol = Solution {
            placements: flat_placements,
            leftovers: vec![],
        };
        let improved = greedy_improve_solution(&problem, &sol);
        // Each sheet has 1 piece, BFDH trivially keeps them (1 piece always fits)
        assert_eq!(improved.placements.len(), 4);
        let errors = validate_solution(&problem, &improved);
        assert!(errors.is_empty(), "validation errors: {:?}", errors);
        let _ = spec_arc; // silence unused warning
    }
}
