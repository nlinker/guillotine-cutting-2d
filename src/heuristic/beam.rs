use crate::model::{FreeRect, Placement, Problem, Solution};

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
