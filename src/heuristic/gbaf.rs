use crate::model::{FreeRect, Placement, Problem, Solution};

/// Standalone multi-sheet Guillotine Best-Area-Fit (GBAF) solver.
///
/// Pieces are sorted by area descending. For each piece the free rect with
/// minimum waste `area(fr) - area(piece)` is chosen (Best-Area-Fit criterion).
/// The chosen rect is split with the SLAS heuristic, preserving the guillotine
/// constraint throughout. Opens a new sheet when nothing fits on the current ones.
pub fn gbaf_solve(problem: &Problem) -> Solution {
    use crate::slas::decoder::{fits_in, guillotine_split, sheet_rect};

    let mut order = (0..problem.pieces.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        let pa = &problem.pieces[a];
        let pb = &problem.pieces[b];
        (pb.width * pb.height).cmp(&(pa.width * pa.height))
    });

    let mut placements: Vec<Placement> = Vec::with_capacity(order.len());
    let mut free: Vec<FreeRect> = vec![sheet_rect(problem, 0)];
    let mut sheets_open = 1usize;

    for orig_idx in order {
        let piece = &problem.pieces[orig_idx];

        let best = free
            .iter()
            .enumerate()
            .filter_map(|(i, fr)| {
                fits_in(fr, piece, false).map(|(pw, ph, rotated)| (fr.w * fr.h - pw * ph, i, pw, ph, rotated))
            })
            .min_by_key(|&(waste, _, _, _, _)| waste);

        let (fi, pw, ph, rotated) = match best {
            Some((_, fi, pw, ph, rotated)) => (fi, pw, ph, rotated),
            None => {
                let new_fr = sheet_rect(problem, sheets_open);
                let (pw, ph, rotated) = fits_in(&new_fr, piece, false).expect("piece must fit on empty sheet");
                let fi = free.len();
                free.push(new_fr);
                sheets_open += 1;
                (fi, pw, ph, rotated)
            }
        };

        let fr = free.remove(fi);
        placements.push(Placement {
            sheet_idx: fr.sheet_idx,
            piece_idx: orig_idx,
            x: fr.x,
            y: fr.y,
            rotated,
        });

        // Lookahead-1: try both SLAS (inverse=false) and LLAS (inverse=true) splits,
        // pick whichever preserves the larger of the two resulting free rects.
        // This favours splits that keep a wide/tall rect available for future large pieces,
        // avoiding the failure mode where a narrow split blocks subsequent wide pieces.
        let splits_slas = guillotine_split(&fr, pw, ph, false);
        let splits_llas = guillotine_split(&fr, pw, ph, true);

        let max_rect_area = |splits: &[FreeRect]| splits.iter().map(|r| r.w * r.h).max().unwrap_or(0);
        let chosen = if max_rect_area(&splits_llas) > max_rect_area(&splits_slas) {
            splits_llas
        } else {
            splits_slas
        };

        free.extend(chosen);
    }

    Solution {
        placements,
        leftovers: free,
    }
}
