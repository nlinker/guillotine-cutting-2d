use crate::model::{FreeRect, Placement, Problem, Solution};

struct Shelf {
    y: u32,
    h: u32,
    x_cursor: u32,
}


/// Standalone multi-sheet BFDH solver.
///
/// Sorts all pieces by height descending (rotating if it makes the piece taller),
/// then places them using Best-Fit Decreasing Height shelf packing.
/// Opens a new sheet whenever the current one overflows.
pub fn bfdh_solve(problem: &Problem) -> Solution {
    let sw = problem.sheet.width;
    let sh = problem.sheet.height;

    let mut items = (0..problem.pieces.len())
        .map(|i| {
            let p = &problem.pieces[i];
            if p.can_rotate && p.width > p.height { (i, p.height, p.width) }
            else { (i, p.width, p.height) }
        })
        .collect::<Vec<(usize, u32, u32)>>();
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
                fits_in(fr, piece, false)
                    .map(|(pw, ph, rotated)| (fr.w * fr.h - pw * ph, i, pw, ph, rotated))
            })
            .min_by_key(|&(waste, _, _, _, _)| waste);

        let (fi, pw, ph, rotated) = match best {
            Some((_, fi, pw, ph, rotated)) => (fi, pw, ph, rotated),
            None => {
                let new_fr = sheet_rect(problem, sheets_open);
                let (pw, ph, rotated) = fits_in(&new_fr, piece, false)
                    .expect("piece must fit on empty sheet");
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

        let max_rect_area = |splits: &[FreeRect]| {
            splits.iter().map(|r| r.w * r.h).max().unwrap_or(0)
        };
        let chosen = if max_rect_area(&splits_llas) > max_rect_area(&splits_slas) {
            splits_llas
        } else {
            splits_slas
        };

        free.extend(chosen);
    }

    Solution { placements, leftovers: free }
}

/// Standalone multi-sheet Simple solver (NFDH with in-row gap-fill).
///
/// Mirrors the VBA algorithm from `tmp/Module1.bas` (`dim_ras` sub):
/// sort pieces by height descending, place left-to-right in a single active
/// row; when the current piece does not fit, gap-fill the remaining row width
/// with any smaller unplaced piece that does fit, then advance to the next
/// row. Opens a new sheet when the piece would exceed the sheet height.
pub fn simple_solve(problem: &Problem) -> Solution {
    let sw = problem.sheet.width;
    let sh = problem.sheet.height;

    let mut items = (0..problem.pieces.len())
        .map(|i| {
            let p = &problem.pieces[i];
            if p.can_rotate && p.width > p.height { (i, p.height, p.width) }
            else { (i, p.width, p.height) }
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));

    let n = items.len();
    let mut placements: Vec<Placement> = Vec::with_capacity(n);
    let mut placed = vec![false; n];
    let mut sheet_idx = 0usize;
    let mut x = 0u32;
    let mut y = 0u32;
    let mut row_h = 0u32;

    let mut i = 0;
    while i < n {
        if placed[i] { i += 1; continue; }
        let (orig_idx, pw, ph) = items[i];

        if x + pw <= sw && y + ph <= sh {
            let rotated = problem.pieces[orig_idx].can_rotate
                && problem.pieces[orig_idx].width != pw;
            placements.push(Placement { sheet_idx, piece_idx: orig_idx, x, y, rotated });
            placed[i] = true;
            x += pw;
            row_h = row_h.max(ph);
            i += 1;
        } else {
            // Gap-fill remaining row width with any smaller unplaced piece that fits
            for j in (i + 1)..n {
                if placed[j] { continue; }
                let (idx_j, pw_j, ph_j) = items[j];
                if x + pw_j <= sw && y + ph_j <= sh {
                    let rotated = problem.pieces[idx_j].can_rotate
                        && problem.pieces[idx_j].width != pw_j;
                    placements.push(Placement {
                        sheet_idx, piece_idx: idx_j, x, y, rotated,
                    });
                    placed[j] = true;
                    x += pw_j;
                    row_h = row_h.max(ph_j);
                }
            }
            // Advance to next row if piece will fit there; otherwise open new sheet
            let next_y = y + row_h;
            if row_h > 0 && next_y + ph <= sh {
                y = next_y;
                x = 0;
                row_h = 0;
            } else {
                sheet_idx += 1;
                y = 0;
                x = 0;
                row_h = 0;
            }
        }
    }

    Solution { placements, leftovers: vec![] }
}

/// Beam search over guillotine cut trees with Best-Area-Fit piece placement.
///
/// Maintains up to `beam_width` candidate states (free-rect lists + placements)
/// in parallel. Pieces are placed in decreasing-area order. At each step, for
/// every state the free rect with minimum waste is chosen (BAF); then both SLAS
/// and LLAS split directions are tried, yielding at most 2·beam_width next-states.
/// Next-states are scored by `(sheets_open, baf_waste_for_next_piece)` (lower =
/// better) and the top `beam_width` are kept. When `beam_width == 1` the
/// behaviour is equivalent to GBAF with lookahead-1 split selection.
pub fn beam_solve(problem: &Problem, beam_width: usize) -> Solution {
    use crate::slas::decoder::{fits_in, guillotine_split, sheet_rect};

    #[derive(Clone)]
    struct State {
        free: Vec<FreeRect>,
        placements: Vec<Placement>,
        sheets_open: usize,
    }

    let mut order = (0..problem.pieces.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        let pa = &problem.pieces[a];
        let pb = &problem.pieces[b];
        (pb.width * pb.height).cmp(&(pa.width * pa.height))
    });

    let mut beam = vec![State {
        free: vec![sheet_rect(problem, 0)],
        placements: Vec::with_capacity(order.len()),
        sheets_open: 1,
    }];

    for orig_idx in order {
        let piece = &problem.pieces[orig_idx];
        let mut candidates: Vec<State> = Vec::with_capacity(beam.len() * 2);

        for state in beam {
            let State { mut free, placements, sheets_open } = state;

            let best = free
                .iter()
                .enumerate()
                .filter_map(|(i, fr)| {
                    fits_in(fr, piece, false)
                        .map(|(pw, ph, rotated)| (fr.w * fr.h - pw * ph, i, pw, ph, rotated))
                })
                .min_by_key(|&(waste, _, _, _, _)| waste);

            let (fi, pw, ph, rotated, new_sheets) = match best {
                Some((_, fi, pw, ph, rotated)) => (fi, pw, ph, rotated, sheets_open),
                None => {
                    let new_fr = sheet_rect(problem, sheets_open);
                    let (pw, ph, rotated) = fits_in(&new_fr, piece, false)
                        .expect("piece must fit on empty sheet");
                    let fi = free.len();
                    free.push(new_fr);
                    (fi, pw, ph, rotated, sheets_open + 1)
                }
            };

            let fr = free.remove(fi);
            let placement = Placement {
                sheet_idx: fr.sheet_idx, piece_idx: orig_idx, x: fr.x, y: fr.y, rotated,
            };
            // Both splits are identical when lw==0 or lh==0; only one candidate needed.
            let inversions: &[bool] =
                if fr.w - pw > 0 && fr.h - ph > 0 { &[false, true] } else { &[false] };

            for &inverse in inversions {
                let mut nf = free.clone();
                nf.extend(guillotine_split(&fr, pw, ph, inverse));
                let mut np = placements.clone();
                np.push(placement);
                candidates.push(State { free: nf, placements: np, sheets_open: new_sheets });
            }
        }

        // Score: (sheets_open, stranded_free_area, -max_rect_on_last_sheet). Lower is better.
        // stranded = total area of free rects on every sheet except the newest one;
        // minimising it forces the beam to fill existing sheets before opening new ones.
        // As a tie-breaker we maximise the largest single free rect on the current sheet,
        // which preserves room for future wide/tall pieces and avoids bad-split fragmentation.
        candidates.sort_by_key(|s| {
            let last = s.sheets_open.saturating_sub(1);
            let stranded: u32 = s.free
                .iter()
                .filter(|r| r.sheet_idx < last)
                .map(|r| r.w * r.h)
                .sum();
            let max_last: u32 = s.free
                .iter()
                .filter(|r| r.sheet_idx == last)
                .map(|r| r.w * r.h)
                .max()
                .unwrap_or(0);
            (s.sheets_open, stranded, u32::MAX - max_last)
        });
        candidates.truncate(beam_width);
        beam = candidates;
    }

    let best = beam.remove(0);
    Solution { placements: best.placements, leftovers: best.free }
}

