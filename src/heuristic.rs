use std::cmp::Ordering;

use crate::model::{FreeRect, Objective, Placement, Problem, Solution};

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
            if p.can_rotate && p.width > p.height {
                (i, p.height, p.width)
            } else {
                (i, p.width, p.height)
            }
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
            let rotated = problem.pieces[*orig_idx].can_rotate && problem.pieces[*orig_idx].width != *pw;
            placements.push(Placement {
                sheet_idx,
                piece_idx: *orig_idx,
                x: shelf.x_cursor,
                y: shelf.y,
                rotated,
            });
            shelf.x_cursor += pw;
        } else if y_cursor + ph <= sh {
            let rotated = problem.pieces[*orig_idx].can_rotate && problem.pieces[*orig_idx].width != *pw;
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
        } else {
            sheet_idx += 1;
            shelves.clear();
            y_cursor = *ph;
            let rotated = problem.pieces[*orig_idx].can_rotate && problem.pieces[*orig_idx].width != *pw;
            placements.push(Placement {
                sheet_idx,
                piece_idx: *orig_idx,
                x: 0,
                y: 0,
                rotated,
            });
            shelves.push(Shelf {
                y: 0,
                h: *ph,
                x_cursor: *pw,
            });
        }
    }
    Solution {
        placements,
        leftovers: vec![],
    }
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
            let State {
                mut free,
                placements,
                sheets_open,
            } = state;

            let best = free
                .iter()
                .enumerate()
                .filter_map(|(i, fr)| {
                    fits_in(fr, piece, false).map(|(pw, ph, rotated)| (fr.w * fr.h - pw * ph, i, pw, ph, rotated))
                })
                .min_by_key(|&(waste, _, _, _, _)| waste);

            let (fi, pw, ph, rotated, new_sheets) = match best {
                Some((_, fi, pw, ph, rotated)) => (fi, pw, ph, rotated, sheets_open),
                None => {
                    let new_fr = sheet_rect(problem, sheets_open);
                    let (pw, ph, rotated) = fits_in(&new_fr, piece, false).expect("piece must fit on empty sheet");
                    let fi = free.len();
                    free.push(new_fr);
                    (fi, pw, ph, rotated, sheets_open + 1)
                }
            };

            let fr = free.remove(fi);
            let placement = Placement {
                sheet_idx: fr.sheet_idx,
                piece_idx: orig_idx,
                x: fr.x,
                y: fr.y,
                rotated,
            };
            // Both splits are identical when lw==0 or lh==0; only one candidate needed.
            let inversions: &[bool] = if fr.w - pw > 0 && fr.h - ph > 0 {
                &[false, true]
            } else {
                &[false]
            };

            for &inverse in inversions {
                let mut nf = free.clone();
                nf.extend(guillotine_split(&fr, pw, ph, inverse));
                let mut np = placements.clone();
                np.push(placement);
                candidates.push(State {
                    free: nf,
                    placements: np,
                    sheets_open: new_sheets,
                });
            }
        }

        // Score: (sheets_open, stranded_free_area, -max_rect_on_last_sheet). Lower is better.
        // stranded = total area of free rects on every sheet except the newest one;
        // minimising it forces the beam to fill existing sheets before opening new ones.
        // As a tie-breaker we maximise the largest single free rect on the current sheet,
        // which preserves room for future wide/tall pieces and avoids bad-split fragmentation.
        candidates.sort_by_key(|s| {
            let last = s.sheets_open.saturating_sub(1);
            let stranded: u32 = s.free.iter().filter(|r| r.sheet_idx < last).map(|r| r.w * r.h).sum();
            let max_last: u32 = s
                .free
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
    Solution {
        placements: best.placements,
        leftovers: best.free,
    }
}

/// Piece ordering criterion for a Jylanki pass. All keys compare on u64 to
/// avoid overflow; Ratio compares w/h by cross-multiplication (no floats).
#[derive(Clone, Copy)]
enum SortKey {
    Area,
    ShortSide,
    LongSide,
    Perimeter,
    Diff,
    Ratio,
}

#[derive(Clone, Copy)]
enum SortDir {
    Asc,
    Desc,
}

/// Free-rect selection criterion; the candidate (rect, orientation) with the
/// minimum score wins.
#[derive(Clone, Copy)]
enum SelectionRule {
    /// Smallest rect area (Best-Area-Fit); orientation-independent, ties resolved by tie-break.
    Area,
    /// Smallest min(fr.w - pw, fr.h - ph) — tightest short-side fit.
    ShortSide,
    /// Smallest max(fr.w - pw, fr.h - ph) — tightest long-side fit.
    LongSide,
}

/// Cut direction rule applied after placing a piece into a free rect.
/// `horizontal` in `split_directional` terms: the full-span cut runs horizontally.
#[derive(Clone, Copy)]
enum SplitRule {
    /// Horizontal when the right leftover strip is the narrower one (== SLAS).
    ShorterLeftover,
    /// Horizontal when the right leftover strip is the wider one (== LLAS).
    LongerLeftover,
    /// Horizontal when the free rect is taller than wide.
    ShortAxis,
    /// Horizontal when the free rect is wider than tall.
    LongAxis,
}

const SORT_KEYS: [SortKey; 6] = [
    SortKey::Area,
    SortKey::ShortSide,
    SortKey::LongSide,
    SortKey::Perimeter,
    SortKey::Diff,
    SortKey::Ratio,
];
const SORT_DIRS: [SortDir; 2] = [SortDir::Asc, SortDir::Desc];
const SELECTION_RULES: [SelectionRule; 3] = [SelectionRule::Area, SelectionRule::ShortSide, SelectionRule::LongSide];
const SPLIT_RULES: [SplitRule; 4] = [
    SplitRule::ShorterLeftover,
    SplitRule::LongerLeftover,
    SplitRule::ShortAxis,
    SplitRule::LongAxis,
];

fn sort_cmp(problem: &Problem, a: usize, b: usize, key: SortKey) -> Ordering {
    let pa = &problem.pieces[a];
    let pb = &problem.pieces[b];
    let (aw, ah) = (pa.width as u64, pa.height as u64);
    let (bw, bh) = (pb.width as u64, pb.height as u64);
    match key {
        SortKey::Area => (aw * ah).cmp(&(bw * bh)),
        SortKey::ShortSide => (aw.min(ah), aw.max(ah)).cmp(&(bw.min(bh), bw.max(bh))),
        SortKey::LongSide => (aw.max(ah), aw.min(ah)).cmp(&(bw.max(bh), bw.min(bh))),
        SortKey::Perimeter => (aw + ah).cmp(&(bw + bh)),
        SortKey::Diff => aw.abs_diff(ah).cmp(&bw.abs_diff(bh)),
        SortKey::Ratio => (aw * bh).cmp(&(bw * ah)),
    }
}

fn selection_score(sel: SelectionRule, fr: &FreeRect, pw: u32, ph: u32) -> u64 {
    let lw = (fr.w - pw) as u64;
    let lh = (fr.h - ph) as u64;
    match sel {
        SelectionRule::Area => fr.w as u64 * fr.h as u64,
        SelectionRule::ShortSide => lw.min(lh),
        SelectionRule::LongSide => lw.max(lh),
    }
}

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
    use crate::{expand::expand_problem, parse::parse_problem};

    fn problem(s: &str) -> Problem {
        expand_problem(&parse_problem(s).expect("Error parsing problem"))
    }

    #[test]
    fn jylanki_four_quarters_fill_one_sheet() {
        let p = problem("100x100F:0:50x50/4");
        let sol = jylanki_solve(&p);
        assert_eq!(sol.placements.len(), 4);
        assert_eq!(sol.sheets_used(), 1);
    }

    #[test]
    fn jylanki_piece_requiring_rotation_is_rotated() {
        // Rotatable pieces are normalized to (min, max) = 10x20 portrait, which
        // only fits the 20x10 landscape sheet when rotated.
        let p = problem("20x10F:0:20x10r");
        let sol = jylanki_solve(&p);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 1);
        assert!(sol.placements[0].rotated);
    }

    #[test]
    fn jylanki_not_worse_than_gbaf_on_sheets() {
        let p = problem("2600x1800F:3:400x400/6,495x495/6,270x320/10,150x450/17r");
        let j = jylanki_solve(&p);
        let g = gbaf_solve(&p);
        assert!(
            j.sheets_used() <= g.sheets_used(),
            "jylanki used {} sheets, gbaf used {}",
            j.sheets_used(),
            g.sheets_used()
        );
    }

    #[test]
    fn jylanki_same_input_produces_identical_solution() {
        let p = problem("2600x1800F:3:400x400/6,495x495/6,270x320/10,150x450/17r");
        let a = jylanki_solve(&p);
        let b = jylanki_solve(&p);
        assert_eq!(a.placements, b.placements);
        assert_eq!(a.leftovers, b.leftovers);
    }
}
