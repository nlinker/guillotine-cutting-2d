use crate::model::{Placement, Problem, Solution};

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

