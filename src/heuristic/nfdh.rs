use crate::model::{Placement, Problem, Solution};

/// Standalone multi-sheet NFDH solver (Next-Fit Decreasing Height with in-row gap-fill).
///
/// Mirrors the VBA algorithm from `tmp/Module1.bas` (`dim_ras` sub):
/// sort pieces by height descending, place left-to-right in a single active
/// row; when the current piece does not fit, gap-fill the remaining row width
/// with any smaller unplaced piece that does fit, then advance to the next
/// row. Opens a new sheet when the piece would exceed the sheet height.
pub fn nfdh_solve(problem: &Problem) -> Solution {
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

    let n = items.len();
    let mut placements: Vec<Placement> = Vec::with_capacity(n);
    let mut placed = vec![false; n];
    let mut sheet_idx = 0usize;
    let mut x = 0u32;
    let mut y = 0u32;
    let mut row_h = 0u32;

    let mut i = 0;
    while i < n {
        if placed[i] {
            i += 1;
            continue;
        }
        let (orig_idx, pw, ph) = items[i];

        if x + pw <= sw && y + ph <= sh {
            let rotated = problem.pieces[orig_idx].can_rotate && problem.pieces[orig_idx].width != pw;
            placements.push(Placement {
                sheet_idx,
                piece_idx: orig_idx,
                x,
                y,
                rotated,
            });
            placed[i] = true;
            x += pw;
            row_h = row_h.max(ph);
            i += 1;
        } else {
            // Gap-fill remaining row width with any smaller unplaced piece that fits
            for j in (i + 1)..n {
                if placed[j] {
                    continue;
                }
                let (idx_j, pw_j, ph_j) = items[j];
                if x + pw_j <= sw && y + ph_j <= sh {
                    let rotated = problem.pieces[idx_j].can_rotate && problem.pieces[idx_j].width != pw_j;
                    placements.push(Placement {
                        sheet_idx,
                        piece_idx: idx_j,
                        x,
                        y,
                        rotated,
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

    Solution {
        placements,
        leftovers: vec![],
    }
}
