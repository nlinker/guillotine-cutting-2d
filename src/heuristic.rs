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
        free.extend(guillotine_split(&fr, pw, ph, false));
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

