use super::common::{
    SELECTION_RULES, SORT_DIRS, SORT_KEYS, SPLIT_RULES, SelectionRule, SortDir, SortKey, SplitRule, selection_score,
    sort_cmp,
};
use crate::model::{FreeRect, Objective, Placement, Problem, Solution};

/// One deterministic greedy pass with a fixed strategy combination.
///
/// Pieces are sorted by `key`/`dir`, then placed one by one: every
/// (free rect, orientation) pair the piece fits into is scored with `sel`,
/// the minimum wins (ties: earlier rect index, then non-rotated). The chosen
/// rect is split along the direction dictated by `split`. A new sheet is
/// opened when nothing fits.
fn jylanki_pass(problem: &Problem, key: SortKey, dir: SortDir, sel: SelectionRule, split: SplitRule) -> Solution {
    use crate::slas::decoder::{sheet_rect, split_directional};

    let mut order = (0..problem.pieces.len()).collect::<Vec<_>>();
    order.sort_by(|&a, &b| {
        let ord = sort_cmp(problem, a, b, key);
        match dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });

    let mut placements: Vec<Placement> = Vec::with_capacity(order.len());
    let mut free: Vec<FreeRect> = vec![sheet_rect(problem, 0)];
    let mut sheets_open = 1usize;

    for orig_idx in order {
        let piece = &problem.pieces[orig_idx];
        let both = [(piece.width, piece.height, false), (piece.height, piece.width, true)];
        let n_or = if piece.can_rotate && piece.width != piece.height {
            2
        } else {
            1
        };
        let orients = &both[..n_or];

        // Strict < keeps the earlier rect index and the non-rotated orientation on ties.
        let find_best = |free: &[FreeRect], skip: usize| {
            let mut best: Option<(u64, usize, u32, u32, bool)> = None;
            for (i, fr) in free.iter().enumerate().skip(skip) {
                for &(pw, ph, rotated) in orients {
                    if pw <= fr.w && ph <= fr.h {
                        let score = selection_score(sel, fr, pw, ph);
                        if best.is_none_or(|(s, ..)| score < s) {
                            best = Some((score, i, pw, ph, rotated));
                        }
                    }
                }
            }
            best
        };

        let (fi, pw, ph, rotated) = match find_best(&free, 0) {
            Some((_, fi, pw, ph, rotated)) => (fi, pw, ph, rotated),
            None => {
                free.push(sheet_rect(problem, sheets_open));
                sheets_open += 1;
                let (_, fi, pw, ph, rotated) = find_best(&free, free.len() - 1).expect("piece must fit on empty sheet");
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

        let lw = fr.w - pw;
        let lh = fr.h - ph;
        let horizontal = match split {
            SplitRule::ShorterLeftover => lw <= lh,
            SplitRule::LongerLeftover => lw > lh,
            SplitRule::ShortAxis => fr.w < fr.h,
            SplitRule::LongAxis => fr.w > fr.h,
        };
        free.extend(split_directional(&fr, pw, ph, horizontal));
    }

    Solution {
        placements,
        leftovers: free,
    }
}

/// Portfolio guillotine packer after Jylanki's "A Thousand Ways to Pack the Bin".
///
/// Runs a deterministic greedy pass for every strategy combination
/// (6 sort keys x 2 directions x 3 selection rules x 4 split rules = 144)
/// and returns the solution with the best `Objective`. With strict comparison
/// and a fixed iteration order the result is fully deterministic.
pub fn jylanki_solve(problem: &Problem) -> Solution {
    let mut best: Option<(Objective, Solution)> = None;
    for key in SORT_KEYS {
        for dir in SORT_DIRS {
            for sel in SELECTION_RULES {
                for split in SPLIT_RULES {
                    let sol = jylanki_pass(problem, key, dir, sel, split);
                    let obj = sol.eval(problem);
                    if best.as_ref().is_none_or(|(b, _)| obj < *b) {
                        best = Some((obj, sol));
                    }
                }
            }
        }
    }
    best.expect("at least one pass always runs").1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, parse_compact::parse_problem};

    fn problem(s: &str) -> Problem {
        expand_problem(&parse_problem(s).expect("Error parsing problem"))
    }

    #[test]
    fn jylanki_four_quarters_fill_one_sheet() {
        let p = problem("100x100F::50x50/4");
        let sol = jylanki_solve(&p);
        assert_eq!(sol.placements.len(), 4);
        assert_eq!(sol.sheets_used(), 1);
    }

    #[test]
    fn jylanki_piece_requiring_rotation_is_rotated() {
        // Rotatable pieces are normalized to (min, max) = 10x20 portrait, which
        // only fits the 20x10 landscape sheet when rotated.
        let p = problem("20x10F::20x10r");
        let sol = jylanki_solve(&p);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 1);
        assert!(sol.placements[0].rotated);
    }

    #[test]
    fn jylanki_same_input_produces_identical_solution() {
        let p = problem("2600x1800F:3,0:400x400/6,495x495/6,270x320/10,150x450/17r");
        let a = jylanki_solve(&p);
        let b = jylanki_solve(&p);
        assert_eq!(a.placements, b.placements);
        assert_eq!(a.leftovers, b.leftovers);
    }
}
