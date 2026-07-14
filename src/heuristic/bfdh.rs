use crate::model::{Placement, Problem, Solution};

struct Shelf {
    sheet_idx: usize,
    y: u32,
    h: u32,
    x_cursor: u32,
}

/// Multi-sheet BFDH variant with shelf backfill.
///
/// Pieces are sorted by height descending (rotating to portrait if taller that way).
/// All open shelves across all sheets are retained. For each piece the algorithm
/// picks the fitting shelf with the minimum remaining width waste (best-fit);
/// if no shelf fits, a new shelf is opened on the earliest sheet that still has
/// vertical room, and only then is a new sheet allocated.
pub fn bfdh_solve(problem: &Problem) -> Solution {
    let sw = problem.sheet.width;
    let sh = problem.sheet.height;

    let mut items = (0..problem.pieces.len())
        .map(|i| {
            let p = &problem.pieces[i];
            if p.can_rotate && p.width > p.height {
                (i, p.height, p.width)
            } else {
                (i, p.width, p.height)
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)));

    let mut placements: Vec<Placement> = Vec::with_capacity(items.len());
    let mut shelves: Vec<Shelf> = Vec::new();
    let mut sheet_y: Vec<u32> = vec![0];

    for &(orig_idx, pw, ph) in &items {
        let rotated = problem.pieces[orig_idx].can_rotate && problem.pieces[orig_idx].width != pw;

        let best_idx = shelves
            .iter()
            .enumerate()
            .filter(|(_, s)| ph <= s.h && s.x_cursor + pw <= sw)
            .min_by_key(|(_, s)| sw - s.x_cursor - pw)
            .map(|(i, _)| i);

        if let Some(idx) = best_idx {
            let s = &mut shelves[idx];
            placements.push(Placement {
                sheet_idx: s.sheet_idx,
                piece_idx: orig_idx,
                x: s.x_cursor,
                y: s.y,
                rotated,
            });
            s.x_cursor += pw;
        } else {
            let target = sheet_y
                .iter()
                .enumerate()
                .find(|&(_, &y)| y + ph <= sh)
                .map(|(i, &y)| (i, y));
            let (sidx, shelf_y) = target.unwrap_or_else(|| {
                let sidx = sheet_y.len();
                sheet_y.push(0);
                (sidx, 0)
            });
            sheet_y[sidx] += ph;
            placements.push(Placement {
                sheet_idx: sidx,
                piece_idx: orig_idx,
                x: 0,
                y: shelf_y,
                rotated,
            });
            shelves.push(Shelf {
                sheet_idx: sidx,
                y: shelf_y,
                h: ph,
                x_cursor: pw,
            });
        }
    }

    Solution {
        placements,
        leftovers: vec![],
    }
}
