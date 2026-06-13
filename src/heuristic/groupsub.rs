use super::common::{SORT_DIRS, SORT_KEYS, SortDir, SortKey, sort_cmp};
use crate::model::{FreeRect, Objective, Placement, Problem, Solution};

/// One deterministic GroupSub pass with a fixed piece ordering.
///
/// The head (first remaining) piece anchors a block in the Best-Area-Fit free
/// rect. Up to four block shapes are scored — (head orientation) x (axis:
/// vertical column / horizontal row) — by the total piece area packed once the
/// strip is topped up with a `best_group` knapsack fill; the densest block wins
/// (ties: vertical axis, then unrotated head). Members are laid flush from the
/// rect corner; the cross-axis trims of narrower members, the strip tail and
/// the rest of the rect become new free rects, all guillotine-valid (one full
/// cut along the strip, transverse cuts between members, one trim cut each).
/// A new sheet is opened when the head fits nowhere.
fn groupsub_pass(problem: &Problem, key: SortKey, dir: SortDir) -> Solution {
    use crate::{group_fill::best_group, slas::decoder::sheet_rect};

    struct Block {
        packed: u64,
        vertical: bool,
        head_w: u32,
        head_h: u32,
        head_rotated: bool,
        group: Vec<(usize, bool)>,
    }

    let mut remaining = (0..problem.pieces.len()).collect::<Vec<_>>();
    remaining.sort_by(|&a, &b| {
        let ord = sort_cmp(problem, a, b, key);
        match dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });

    let piece_area = |i: usize| {
        let p = &problem.pieces[i];
        p.width as u64 * p.height as u64
    };

    let mut placements: Vec<Placement> = Vec::with_capacity(remaining.len());
    let mut free: Vec<FreeRect> = vec![sheet_rect(problem, 0)];
    let mut sheets_open = 1usize;

    while let Some(&head) = remaining.first() {
        let piece = &problem.pieces[head];
        let both = [(piece.width, piece.height, false), (piece.height, piece.width, true)];
        let n_or = if piece.can_rotate && piece.width != piece.height {
            2
        } else {
            1
        };
        let orients = &both[..n_or];

        // Best-Area-Fit among rects where the head fits in any orientation
        // (min_by_key keeps the earliest minimum — deterministic).
        let find_rect = |free: &[FreeRect]| {
            free.iter()
                .enumerate()
                .filter(|(_, fr)| orients.iter().any(|&(pw, ph, _)| pw <= fr.w && ph <= fr.h))
                .min_by_key(|(_, fr)| fr.w as u64 * fr.h as u64)
                .map(|(i, _)| i)
        };
        let fi = match find_rect(&free) {
            Some(fi) => fi,
            None => {
                free.push(sheet_rect(problem, sheets_open));
                sheets_open += 1;
                free.len() - 1
            }
        };
        let fr = free.remove(fi);

        // Score up to 4 block shapes; enumeration order implements the tie-break.
        let rest = &remaining[1..];
        let mut best: Option<Block> = None;
        for vertical in [true, false] {
            for &(pw, ph, rotated) in orients {
                if pw > fr.w || ph > fr.h {
                    continue;
                }
                let (cap, budget) = if vertical { (pw, fr.h - ph) } else { (ph, fr.w - pw) };
                let group = best_group(&problem.pieces, rest, cap, budget, vertical);
                let packed = piece_area(head) + group.iter().map(|&(i, _)| piece_area(i)).sum::<u64>();
                if best.as_ref().is_none_or(|b| packed > b.packed) {
                    best = Some(Block {
                        packed,
                        vertical,
                        head_w: pw,
                        head_h: ph,
                        head_rotated: rotated,
                        group,
                    });
                }
            }
        }
        let block = best.expect("head fits the chosen rect");
        let vertical = block.vertical;
        let cross = if vertical { block.head_w } else { block.head_h };

        // Members laid flush from the rect corner, longest-first (then by index).
        let mut members: Vec<(usize, bool, u32, u32)> = Vec::with_capacity(block.group.len() + 1);
        members.push((head, block.head_rotated, block.head_w, block.head_h));
        for &(i, rotated) in &block.group {
            let p = &problem.pieces[i];
            let (w, h) = if rotated {
                (p.height, p.width)
            } else {
                (p.width, p.height)
            };
            members.push((i, rotated, w, h));
        }
        members.sort_by_key(|&(i, _, w, h)| (std::cmp::Reverse(if vertical { h } else { w }), i));

        let mut cursor = if vertical { fr.y } else { fr.x };
        for &(i, rotated, w, h) in &members {
            let (x, y) = if vertical { (fr.x, cursor) } else { (cursor, fr.y) };
            placements.push(Placement {
                sheet_idx: fr.sheet_idx,
                piece_idx: i,
                x,
                y,
                rotated,
            });
            let member_cross = if vertical { w } else { h };
            if member_cross < cross {
                free.push(if vertical {
                    FreeRect {
                        sheet_idx: fr.sheet_idx,
                        x: fr.x + w,
                        y: cursor,
                        w: cross - w,
                        h,
                    }
                } else {
                    FreeRect {
                        sheet_idx: fr.sheet_idx,
                        x: cursor,
                        y: fr.y + h,
                        w,
                        h: cross - h,
                    }
                });
            }
            cursor += if vertical { h } else { w };
        }

        // Strip tail and the rest of the rect.
        if vertical {
            let filled = cursor - fr.y;
            if filled < fr.h {
                free.push(FreeRect {
                    sheet_idx: fr.sheet_idx,
                    x: fr.x,
                    y: fr.y + filled,
                    w: cross,
                    h: fr.h - filled,
                });
            }
            if cross < fr.w {
                free.push(FreeRect {
                    sheet_idx: fr.sheet_idx,
                    x: fr.x + cross,
                    y: fr.y,
                    w: fr.w - cross,
                    h: fr.h,
                });
            }
        } else {
            let filled = cursor - fr.x;
            if filled < fr.w {
                free.push(FreeRect {
                    sheet_idx: fr.sheet_idx,
                    x: fr.x + filled,
                    y: fr.y,
                    w: fr.w - filled,
                    h: cross,
                });
            }
            if cross < fr.h {
                free.push(FreeRect {
                    sheet_idx: fr.sheet_idx,
                    x: fr.x,
                    y: fr.y + cross,
                    w: fr.w,
                    h: fr.h - cross,
                });
            }
        }

        remaining.retain(|i| !members.iter().any(|&(m, ..)| m == *i));
    }

    Solution {
        placements,
        leftovers: free,
    }
}

/// Portfolio GroupSub solver after Faizrakhmanov et al. (2014): strips are
/// filled with piece GROUPS found by an exact 1D knapsack DP instead of one
/// piece at a time.
///
/// Runs `groupsub_pass` for every piece ordering (6 sort keys x 2 directions
/// = 12 passes) and returns the solution with the best `Objective`. Axis and
/// head orientation are chosen adaptively per block inside a pass, so they are
/// not portfolio dimensions. Fully deterministic.
pub fn groupsub_solve(problem: &Problem) -> Solution {
    let mut best: Option<(Objective, Solution)> = None;
    for key in SORT_KEYS {
        for dir in SORT_DIRS {
            let sol = groupsub_pass(problem, key, dir);
            let obj = sol.eval(problem);
            if best.as_ref().is_none_or(|(b, _)| obj < *b) {
                best = Some((obj, sol));
            }
        }
    }
    best.expect("at least one pass always runs").1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, heuristic::gbaf::gbaf_solve, parse::parse_problem};

    fn problem(s: &str) -> Problem {
        expand_problem(&parse_problem(s).expect("Error parsing problem"))
    }

    // Head 50x100 fills one column; the DP must top up the second column with
    // the exact 30+70 group — a fit one-piece-at-a-time decoders can miss.
    #[test]
    fn groupsub_mixed_column_fills_one_sheet() {
        let p = problem("100x100F:0:50x100/1f,50x30/1f,50x70/1f");
        let sol = groupsub_solve(&p);
        assert_eq!(sol.placements.len(), 3);
        assert_eq!(sol.sheets_used(), 1);
    }

    #[test]
    fn groupsub_solution_is_valid() {
        let p = problem("2600x1800F:3:400x400/6,495x495/6,270x320/10,150x450/17r");
        let sol = groupsub_solve(&p);
        assert_eq!(sol.placements.len(), p.pieces.len());
        let errors = crate::model::validate_solution(&p, &sol);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn groupsub_not_worse_than_gbaf_on_sheets() {
        let p = problem("2600x1800F:3:400x400/6,495x495/6,270x320/10,150x450/17r");
        let g = groupsub_solve(&p);
        let b = gbaf_solve(&p);
        assert!(
            g.sheets_used() <= b.sheets_used(),
            "groupsub used {} sheets, gbaf used {}",
            g.sheets_used(),
            b.sheets_used()
        );
    }

    #[test]
    fn groupsub_same_input_produces_identical_solution() {
        let p = problem("2600x1800F:3:400x400/6,495x495/6,270x320/10,150x450/17r");
        let a = groupsub_solve(&p);
        let b = groupsub_solve(&p);
        assert_eq!(a.placements, b.placements);
        assert_eq!(a.leftovers, b.leftovers);
    }
}
