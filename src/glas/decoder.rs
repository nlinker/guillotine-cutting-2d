use smallvec::{SmallVec, smallvec};

use crate::{
    expand,
    ga::Decodable,
    model::{FreeRect, Piece, Placement, Problem, ProblemSpec, Solution},
    slas::decoder::{find_placement, fits_in, split_directional},
};

/// `selectors[k] % |free|`: target free rect for the batch starting at `k` placed copies.
pub type Selectors = SmallVec<[u32; 16]>;

/// `inverses[k]`: split direction for the batch starting at `k` placed copies.
pub type Inverses = SmallVec<[bool; 16]>;

/// One gene per piece type, driving placement of ALL its copies, batch by batch (unlike
/// `slas::Gene`, one gene per physical piece). `selectors`/`inverses` have `count` elements
/// each, but only batch-start positions are read; mid-batch entries ride along unused so
/// crossover treats every index identically.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Gene {
    /// Index into `spec.piece_types` - which piece type this gene handles.
    pub type_idx: usize,
    /// Prefer rotated orientation for every piece in this group.
    pub rotate: bool,
    /// `selectors[k]`: free-leaf selector used when `k` copies have been placed.
    pub selectors: Selectors,
    /// `inverses[k]`: split direction used when `k` copies have been placed.
    pub inverses: Inverses,
}

/// Outer index = class priority (0=large, 1=medium, 2=small).
/// Each inner vec is a GA-evolved permutation of type indices within that class.
/// Classes decode in order, so large pieces are always placed before small.
pub type Genome = Vec<Vec<Gene>>;

pub fn decode_spec(spec: &ProblemSpec, genome: &Genome) -> crate::model::SolutionSpec {
    let problem = expand::expand_problem(spec);
    let sol = decode(&problem, spec, genome);
    expand::shrink_solution(&sol, spec)
}

impl Decodable for Genome {
    fn decode(&self, spec: &ProblemSpec) -> crate::model::SolutionSpec {
        decode_spec(spec, self)
    }
}

/// Decode a group genome into a flat `Solution`.
///
/// For each gene the decoder places all pieces of the given type one batch at a time:
///   1. Let `placed` = number of copies of this type already placed.
///      Consult `selectors[placed]` and `inverses[placed]`.
///   2. Find a fitting free leaf; open a new sheet if nothing fits anywhere.
///   3. Pack as many copies as fit side-by-side in the strip: `count = ⌊fr_w / pw⌋`.
///   4. Split the free leaf according to `inv` (see [`FreePool::apply_batch`]).
///   5. Advance `next[gene.type_idx]` by `count` and repeat.
pub fn decode(problem: &Problem, spec: &ProblemSpec, genome: &Genome) -> Solution {
    let mut pool = FreePool::new(problem.sheet.width, problem.sheet.height);
    debug_assert_eq!(genome.iter().map(|c| c.len()).sum::<usize>(), spec.piece_types.len());

    // end_idxs[i] = one-past-last flat index for type i
    // next[i]     = next unassigned flat index for type i (starts equal to start offset)
    let n_types = spec.piece_types.len();
    let mut end_idxs = Vec::with_capacity(n_types);
    let mut next = Vec::with_capacity(n_types);
    let mut acc = 0usize;
    for ps in &spec.piece_types {
        next.push(acc);
        acc += ps.count as usize;
        end_idxs.push(acc);
    }
    let mut placements: Vec<Placement> = Vec::with_capacity(problem.pieces.len());

    for class in genome {
        for gene in class {
            let count = spec.piece_types[gene.type_idx].count as usize;
            let end_idx = end_idxs[gene.type_idx];
            debug_assert_eq!(gene.selectors.len(), count);
            debug_assert_eq!(gene.inverses.len(), count);

            // Prefer the previous batch's sheet, to keep copies of one type together.
            let mut last_sheet: Option<usize> = None;

            while next[gene.type_idx] < end_idx {
                // placed = copies of this type already placed = index into selectors/inverses
                let placed = count - (end_idx - next[gene.type_idx]);

                let ps = gene.selectors[placed];
                let inv = gene.inverses[placed];

                let piece = &problem.pieces[next[gene.type_idx]];
                let remaining = end_idx - next[gene.type_idx];

                let sw = problem.sheet.width;
                let sh = problem.sheet.height;
                let found = if remaining > 1 {
                    // 1. same sheet, batch >= 2
                    last_sheet
                        .and_then(|sid| pool.find_fitting_leaf_min_batch_on_sheet(piece, gene.rotate, ps, 2, sid))
                        // 2. any sheet, batch >= 2
                        .or_else(|| pool.find_fitting_leaf_min_batch(piece, gene.rotate, ps, 2))
                        // 3. any sheet, single
                        .or_else(|| pool.find_fitting_leaf(piece, gene.rotate, ps))
                        .or_else(|| {
                            pool.open_new_sheet(sw, sh);
                            pool.find_fitting_leaf(piece, gene.rotate, ps)
                        })
                } else {
                    // 1. same sheet
                    last_sheet
                        .and_then(|sid| pool.find_fitting_leaf_on_sheet(piece, gene.rotate, ps, sid))
                        // 2. any sheet
                        .or_else(|| pool.find_fitting_leaf(piece, gene.rotate, ps))
                        .or_else(|| {
                            pool.open_new_sheet(sw, sh);
                            pool.find_fitting_leaf(piece, gene.rotate, ps)
                        })
                };

                let Some((free_pos, pw, ph, rotated)) = found else {
                    debug_assert!(
                        false,
                        "piece {}x{} does not fit on empty {}x{} sheet",
                        piece.width, piece.height, sw, sh
                    );
                    break;
                };

                let (fr_w, fr_h, sheet_idx) = {
                    let fr = &pool.free[free_pos];
                    (fr.w, fr.h, fr.sheet_idx)
                };
                last_sheet = Some(sheet_idx);

                // Grid geometry: try row-major (cols fixed by fr_w/pw) and column-major
                // (rows fixed by fr_h/ph), pick whichever leaves the smaller "hole" in the
                // partial last row/column (row-major wins ties). Degenerates to a 1xN strip
                // when cols==1, rows==1, or the grid divides remaining exactly.
                let cols = fr_w / pw;
                let rows = fr_h / ph;
                let grid_n = (rows * cols).min(remaining as u32);

                let rows_full = grid_n / cols;
                let extra_row = grid_n % cols;
                let cw_row = if rows_full >= 1 { cols * pw } else { extra_row * pw };
                let ch_row = (rows_full + (extra_row > 0) as u32) * ph;
                let hole_row_area = if extra_row > 0 && rows_full >= 1 {
                    (cw_row - extra_row * pw) * ph
                } else {
                    0
                };

                let cols_full = grid_n / rows;
                let extra_col = grid_n % rows;
                let ch_col = if cols_full >= 1 { rows * ph } else { extra_col * ph };
                let cw_col = (cols_full + (extra_col > 0) as u32) * pw;
                let hole_col_area = if extra_col > 0 && cols_full >= 1 {
                    pw * (ch_col - extra_col * ph)
                } else {
                    0
                };

                let row_major = hole_row_area <= hole_col_area;
                let (cw, ch, grid_cols, grid_lines_full, extra) = if row_major {
                    (cw_row, ch_row, cols, rows_full, extra_row)
                } else {
                    (cw_col, ch_col, rows, cols_full, extra_col)
                };

                // Split the leaf around the batch, returns batch origin.
                let (batch_x, batch_y) = pool.apply_batch(free_pos, cw, ch, inv);

                // Place the grid: row-major fills left-to-right then top-to-bottom;
                // column-major fills top-to-bottom then left-to-right.
                for i in 0..grid_n {
                    let line = i / grid_cols;
                    let pos = i % grid_cols;
                    let (x, y) = if row_major {
                        (batch_x + pos * pw, batch_y + line * ph)
                    } else {
                        (batch_x + line * pw, batch_y + pos * ph)
                    };
                    placements.push(Placement { sheet_idx, piece_idx: next[gene.type_idx], x, y, rotated });
                    next[gene.type_idx] += 1;
                }

                // Register the hole left by a partial last row/column, if any.
                if extra > 0 && grid_lines_full >= 1 {
                    let (hx, hy, hw, hh) = if row_major {
                        (
                            batch_x + extra * pw,
                            batch_y + grid_lines_full * ph,
                            cw - extra * pw,
                            ph,
                        )
                    } else {
                        (
                            batch_x + grid_lines_full * pw,
                            batch_y + extra * ph,
                            pw,
                            ch - extra * ph,
                        )
                    };
                    pool.push_free_leaf(sheet_idx, hx, hy, hw, hh);
                }
            }
        }
    }

    let leftovers = pool.free.into_iter().collect();

    Solution { placements, leftovers }
}

/// Free-rectangle pool across all open sheets: tracks only the current free leaves and
/// sheet count, not a full cut tree. For verifying a finished placement is genuinely
/// guillotine-splittable, see [`crate::cut_tree::build_cut_tree`] instead, which
/// reconstructs a tree from a `(Problem, Solution)` pair independently of decoding.
pub(crate) struct FreePool {
    free: SmallVec<[FreeRect; 16]>,
    sheets_open: usize,
}

impl FreePool {
    fn new(w: u32, h: u32) -> Self {
        FreePool { free: smallvec![FreeRect { sheet_idx: 0, x: 0, y: 0, w, h }], sheets_open: 1 }
    }

    fn open_new_sheet(&mut self, w: u32, h: u32) -> usize {
        let sheet_idx = self.sheets_open;
        self.free.push(FreeRect { sheet_idx, x: 0, y: 0, w, h });
        self.sheets_open += 1;
        sheet_idx
    }

    /// Registers a free leaf not produced by `apply_batch` (the "hole" left by a partial
    /// grid row/column).
    fn push_free_leaf(&mut self, sheet_idx: usize, x: u32, y: u32, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.free.push(FreeRect { sheet_idx, x, y, w, h });
    }

    /// Remove the free rect at `free_pos` and split it around a `cw x ch` batch placed at
    /// its top-left corner; returns that origin `(x, y)`.
    ///
    /// Given the free rect `(x, y, nw, nh)`, `lw = nw - cw`, `lh = nh - ch`: `inv = false`
    /// cuts horizontally (right = `lw x ch`, bottom spans full width `nw x lh`); `inv = true`
    /// cuts vertically (right spans full height `lw x nh`, bottom = `cw x lh`). Delegates to
    /// [`split_directional`] with `horizontal = !inv`; see
    /// [docs/glas.md](../../docs/glas.md) for ASCII diagrams.
    ///
    /// `free_pos` comes from a `find_fitting_leaf*` call, passed directly to avoid a re-scan.
    fn apply_batch(&mut self, free_pos: usize, cw: u32, ch: u32, inv: bool) -> (u32, u32) {
        let fr = self.free.swap_remove(free_pos);
        debug_assert!(
            cw <= fr.w && ch <= fr.h,
            "batch ({cw}×{ch}) exceeds free rect ({}×{})",
            fr.w,
            fr.h
        );
        let origin = (fr.x, fr.y);
        for child in split_directional(&fr, cw, ch, !inv) {
            self.free.push(child);
        }
        origin
    }

    /// Scan free leaves starting at `selector % |free|` (wrapping) and return the first
    /// that fits `piece`.
    fn find_fitting_leaf(&self, piece: &Piece, prefer_rotate: bool, selector: u32) -> Option<(usize, u32, u32, bool)> {
        find_placement(&self.free, piece, prefer_rotate, selector)
    }

    /// Like [`Self::find_fitting_leaf`], but restricted to leaves on `sheet_idx`.
    fn find_fitting_leaf_on_sheet(
        &self,
        piece: &Piece,
        prefer_rotate: bool,
        selector: u32,
        sheet_idx: usize,
    ) -> Option<(usize, u32, u32, bool)> {
        let candidates: SmallVec<[(usize, u32, u32, bool); 16]> = (0..self.free.len())
            .filter_map(|free_pos| {
                let fr = &self.free[free_pos];
                if fr.sheet_idx != sheet_idx {
                    return None;
                }
                let (pw, ph, rotated) = fits_in(fr, piece, prefer_rotate)?;
                Some((free_pos, pw, ph, rotated))
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        Some(candidates[(selector as usize) % candidates.len()])
    }

    /// Like [`Self::find_fitting_leaf_min_batch`], but restricted to leaves on `sheet_idx`.
    fn find_fitting_leaf_min_batch_on_sheet(
        &self,
        piece: &Piece,
        prefer_rotate: bool,
        selector: u32,
        min_fit: u32,
        sheet_idx: usize,
    ) -> Option<(usize, u32, u32, bool)> {
        let candidates: SmallVec<[(usize, u32, u32, bool); 16]> = (0..self.free.len())
            .filter_map(|free_pos| {
                let fr = &self.free[free_pos];
                if fr.sheet_idx != sheet_idx {
                    return None;
                }
                let (pw, ph, rotated) = fits_in(fr, piece, prefer_rotate)?;
                let fits = (fr.w / pw).max(fr.h / ph);
                (fits >= min_fit).then_some((free_pos, pw, ph, rotated))
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        Some(candidates[(selector as usize) % candidates.len()])
    }

    /// Like [`Self::find_fitting_leaf`], but only considers leaves where the batch count
    /// `max(fr_w / pw, fr_h / ph)` is >= `min_fit`. Returns `None` if no such leaf exists.
    fn find_fitting_leaf_min_batch(
        &self,
        piece: &Piece,
        prefer_rotate: bool,
        selector: u32,
        min_fit: u32,
    ) -> Option<(usize, u32, u32, bool)> {
        let candidates: SmallVec<[(usize, u32, u32, bool); 16]> = (0..self.free.len())
            .filter_map(|free_pos| {
                let fr = &self.free[free_pos];
                let (pw, ph, rotated) = fits_in(fr, piece, prefer_rotate)?;
                let fits = (fr.w / pw).max(fr.h / ph);
                (fits >= min_fit).then_some((free_pos, pw, ph, rotated))
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        Some(candidates[(selector as usize) % candidates.len()])
    }
}

#[cfg(test)]
mod tests {
    use std::iter::repeat_n;

    use super::*;
    use crate::{expand::expand_problem, parser::compact::parse_problem};

    fn gg(type_idx: usize, count: usize) -> Gene {
        Gene {
            type_idx,
            rotate: false,
            selectors: repeat_n(0u32, count).collect(),
            inverses: repeat_n(false, count).collect(),
        }
    }

    #[test]
    fn two_identical_pieces_form_a_strip() {
        // Sheet 200×100, one type: 2×(80×100).
        // ┌────────┬────────┬──────┐
        // │   P0   │   P1   │ free │
        // │ 80×100 │ 80×100 │40×100│
        // └────────┴────────┴──────┘
        let spec = parse_problem("200x100F::80x100/2").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![vec![gg(0, 2)]];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 2);
        let p0 = sol.placements.iter().find(|p| p.piece_idx == 0).unwrap();
        let p1 = sol.placements.iter().find(|p| p.piece_idx == 1).unwrap();
        assert_eq!((p0.x, p0.y, p0.sheet_idx), (0, 0, 0));
        assert_eq!((p1.x, p1.y, p1.sheet_idx), (80, 0, 0));
    }

    #[test]
    fn strip_overflows_to_next_rect() {
        // Sheet 100×100, one type: 3×(60×40) - vertical strip fits 2, third overflows.
        // Sheet 0 (100×100)      Sheet 1 (100×100)
        // ┌────────┬──────┐      ┌────────┬──────┐
        // │   P0   │      │      │   P2   │      │
        // │ 60×40  │ free │      │ 60×40  │ free │
        // ├────────┤40×80 │      └────────┴──────┘
        // │   P1   │      │
        // │ 60×40  │      │
        // ├────────┴──────┤
        // │  free 100×20  │
        // └───────────────┘
        // P2 fits neither leftover of sheet 0 (40×80 too narrow, 100×20 too short).
        let spec = parse_problem("100x100F::60x40/3").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![vec![gg(0, 3)]];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 2);
        assert_eq!(sol.placements.len(), 3);
        assert_eq!(
            (sol.placements[0].x, sol.placements[0].y, sol.placements[0].sheet_idx),
            (0, 0, 0)
        );
        assert_eq!(
            (sol.placements[1].x, sol.placements[1].y, sol.placements[1].sheet_idx),
            (0, 40, 0)
        );
        assert_eq!(
            (sol.placements[2].x, sol.placements[2].y, sol.placements[2].sheet_idx),
            (0, 0, 1)
        );
    }

    #[test]
    fn selector_steers_strip_to_different_rect() {
        // Sheet 200×100. Type 1 (B): 1×(100×100). Type 0 (A): 2×(90×100).
        // Genome [B, A], selectors[0]=1 steers A's first batch onto B's right leftover.
        // Sheet 0 (200×100)          Sheet 1 (200×100)
        // ┌────────┬────────┬──┐    ┌────────┬─────┐
        // │   B    │   A0   │  │    │   A1   │     │
        // │100×100 │ 90×100 │10│    │ 90×100 │ free│
        // └────────┴────────┴──┘    └────────┴─────┘
        // A1 (selectors[1]=0, default) fits nothing left on sheet 0 -> opens sheet 1.
        let spec = parse_problem("200x100F::90x100/2,100x100/1").expect("parse");
        let problem = expand_problem(&spec);
        let mut genome = vec![vec![gg(1, 1), gg(0, 2)]];
        genome[0][1].selectors[0] = 1; // steer first batch of type_0 to right leftover
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 2);
        let b = sol.placements.iter().find(|p| p.piece_idx == 2).unwrap(); // type_1
        assert_eq!((b.x, b.y, b.sheet_idx), (0, 0, 0));
        let a0 = sol.placements.iter().find(|p| p.piece_idx == 0).unwrap(); // first type_0
        assert_eq!((a0.x, a0.y, a0.sheet_idx), (100, 0, 0));
        let a1 = sol.placements.iter().find(|p| p.piece_idx == 1).unwrap(); // second type_0
        assert_eq!(a1.sheet_idx, 1);
    }

    #[test]
    fn four_pieces_two_rows() {
        // Sheet 200×200, one type: 4×(100×100) - exact 2x2 grid, no leftover hole.
        // ┌────────┬────────┐
        // │   P0   │   P1   │
        // │100×100 │100×100 │
        // ├────────┼────────┤
        // │   P2   │   P3   │
        // │100×100 │100×100 │
        // └────────┴────────┘
        let spec = parse_problem("200x200F::100x100/4").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![vec![gg(0, 4)]];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 4);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        assert_eq!((f(0).x, f(0).y), (0, 0));
        assert_eq!((f(1).x, f(1).y), (100, 0));
        assert_eq!((f(2).x, f(2).y), (0, 100));
        assert_eq!((f(3).x, f(3).y), (100, 100));
    }

    #[test]
    fn five_pieces_grid_hole_reused_by_next_gene() {
        // Sheet 300×400, one type: 5×(100×100) - row-major grid (3 cols) leaves a
        // corner hole plus a bottom strip; row-major beats column-major (smaller hole).
        // ┌────────┬────────┬────────┐
        // │   P0   │   P1   │   P2   │
        // │100×100 │100×100 │100×100 │
        // ├────────┼────────┼────────┤
        // │   P3   │   P4   │  hole  │
        // │100×100 │100×100 │100×100 │
        // ├────────┴────────┴────────┤
        // │       free 300×200       │
        // └──────────────────────────┘
        let spec = parse_problem("300x400F::100x100/5").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![vec![gg(0, 5)]];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 5);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        assert_eq!((f(0).x, f(0).y), (0, 0));
        assert_eq!((f(1).x, f(1).y), (100, 0));
        assert_eq!((f(2).x, f(2).y), (200, 0));
        assert_eq!((f(3).x, f(3).y), (0, 100));
        assert_eq!((f(4).x, f(4).y), (100, 100));

        let errors = crate::model::validate_solution(&problem, &sol);
        assert!(errors.is_empty(), "{errors:?}");

        let bottom = sol
            .leftovers
            .iter()
            .find(|r| (r.x, r.y, r.w, r.h) == (0, 200, 300, 200))
            .expect("bottom leftover from the outer split");
        assert_eq!(bottom.sheet_idx, 0);
        let hole = sol
            .leftovers
            .iter()
            .find(|r| (r.x, r.y, r.w, r.h) == (200, 100, 100, 100))
            .expect("corner hole left by the partial grid row");
        assert_eq!(hole.sheet_idx, 0);

        // A second, rotatable gene of the same 100×100 dims (kept distinct by normalization)
        // steered (selectors[0]=1) at the hole free-leaf fills it exactly - no new leftover.
        // ┌────────┬────────┬────────┐
        // │   P0   │   P1   │   P2   │
        // ├────────┼────────┼────────┤
        // │   P3   │   P4   │   P5   │
        // ├────────┴────────┴────────┤
        // │       free 300×200       │
        // └──────────────────────────┘
        let spec2 = parse_problem("300x400F::100x100/5,100x100/1r").expect("parse");
        let problem2 = expand_problem(&spec2);
        let mut genome2 = vec![vec![gg(0, 5), gg(1, 1)]];
        genome2[0][1].selectors[0] = 1;
        let sol2 = decode(&problem2, &spec2, &genome2);
        assert_eq!(sol2.sheets_used(), 1);
        assert_eq!(sol2.placements.len(), 6);
        let p5 = sol2.placements.iter().find(|p| p.piece_idx == 5).unwrap(); // type_1
        assert_eq!((p5.x, p5.y, p5.sheet_idx), (200, 100, 0));

        let errors2 = crate::model::validate_solution(&problem2, &sol2);
        assert!(errors2.is_empty(), "{errors2:?}");
    }

    #[test]
    fn strip_mixes_two_types() {
        // Sheet 350×100. Type A: 1×(150×100). Type B: 2×(100×80). Genome [A, B]:
        // B's batch reuses A's right leftover, so all three land on sheet 0.
        // ┌─────────────┬────────┬────────┐
        // │             │   B0   │   B1   │
        // │      A      │ 100×80 │ 100×80 │
        // │   150×100   ├────────┴────────┤
        // │             │   free 200×20   │
        // └─────────────┴─────────────────┘
        let spec = parse_problem("350x100F::150x100/1f,100x80/2f").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![vec![gg(0, 1), gg(1, 2)]];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 3);
        // Type A (piece 0) must be in the strip.
        let pa = sol.placements.iter().find(|p| p.piece_idx == 0).unwrap();
        assert_eq!(pa.sheet_idx, 0);
        // All pieces on one sheet → strip filler combined them.
        assert!(sol.placements.iter().all(|p| p.sheet_idx == 0));
    }

    #[test]
    fn strip_respects_height_constraint() {
        // Sheet 400×300. Type 0 (filler): 400×200. Type 1 (A): 200×100. Type 2 (B): 200×200.
        // Sheet 0 (400×300)                  Sheet 1 (400×300)
        // ┌───────────────────────────┐      ┌────────┬──────┐
        // │      filler (type 0)      │      │   B    │      │
        // │          400×200          │      │200×200 │ free │
        // ├────────────┬──────────────┤      └────────┴──────┘
        // │     A      │              │
        // │  200×100   │ free 200×100 │
        // └────────────┴──────────────┘
        // B (200×200) fits no leftover of sheet 0 (max free height there is 100) -> sheet 1.
        let spec = parse_problem("400x300F::400x200/1f,200x100/1f,200x200/1f").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![vec![gg(0, 1), gg(1, 1), gg(2, 1)]];
        let sol = decode(&problem, &spec, &genome);
        // B (piece_idx=2) cannot fit in the leftover rects (all h≤100); opens sheet 1.
        let pb = sol.placements.iter().find(|p| p.piece_idx == 2).unwrap();
        assert_eq!(
            pb.sheet_idx, 1,
            "200×200 piece must go to a new sheet (fr_h=100 < ph=200)"
        );
    }

    #[test]
    fn pool_apply_batch_free_rects() {
        fn free_rects(pool: &FreePool) -> Vec<(u32, u32, u32, u32)> {
            pool.free.iter().map(|fr| (fr.x, fr.y, fr.w, fr.h)).collect()
        }
        // (sheet_w, sheet_h, inverse) -> expected free rects after placing a 100x100
        // batch at (0,0). Covers both-leftovers (inverse false/true), exact fit,
        // exact-width (lw=0), and exact-height (lh=0).
        let cases = [
            (200, 300, false, vec![(100, 0, 100, 100), (0, 100, 200, 200)]),
            (200, 300, true, vec![(100, 0, 100, 300), (0, 100, 100, 200)]),
            (100, 100, false, vec![]),
            (100, 200, false, vec![(0, 100, 100, 100)]),
            (200, 100, false, vec![(100, 0, 100, 100)]),
        ];
        for (sheet_w, sheet_h, inverse, expected) in cases {
            let mut pool = FreePool::new(sheet_w, sheet_h);
            let (bx, by) = pool.apply_batch(0, 100, 100, inverse);
            assert_eq!((bx, by), (0, 0));
            let mut rects = free_rects(&pool);
            rects.sort();
            let mut expected = expected;
            expected.sort();
            assert_eq!(rects, expected, "sheet {sheet_w}x{sheet_h} inverse={inverse}");
        }
    }
}
