use smallvec::{SmallVec, smallvec};

use crate::{
    expand,
    model::{FreeRect, Piece, Placement, Problem, ProblemSpec, Solution},
    slas::decoder::{find_placement, fits_in, split_directional},
};

/// Per-copy free-rect selectors: `selectors[k] % |free|` picks the target free rect
/// at the start of the batch that begins when `k` pieces of this type are already placed.
pub type Selectors = SmallVec<[u32; 16]>;

/// Per-copy inversion flags: `inverses[k]` selects the split direction (`false`/`true`,
/// see [`FreePool::apply_batch`]) for the batch that starts when `k` pieces of this
/// type are already placed.
pub type Inverses = SmallVec<[bool; 16]>;

/// One gene per piece type: a permutation of `0..spec.piece_types.len()`.
///
/// Unlike `slas::Gene` (one gene per physical piece), a single `Gene` here drives
/// the placement of ALL copies of one piece type, batch by batch.
///
/// Each batch selects a free leaf (via `selectors[placed]`) and a split direction
/// (via `inverses[placed]`), packs a strip of pieces into the composite box, then
/// splits the free leaf according to that direction.
///
/// `selectors` and `inverses` each have exactly `count` elements - one per physical
/// copy, but only batch-start positions are consulted at decode time.
/// Mid-batch entries are carried silently; this keeps the arrays symmetric and
/// lets crossover treat every index identically without special-casing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Gene {
    /// Index into `spec.piece_types` - which piece type this gene handles.
    pub type_idx: usize,
    /// Prefer rotated orientation for every piece in this group.
    pub rotate: bool,
    /// `selectors[k]`: free-leaf selector used when `k` copies have been placed.
    pub selectors: Selectors,
    /// `inverses[k]`: split direction (`false`/`true`, see [`FreePool::apply_batch`])
    /// for the batch starting when `k` copies have been placed.
    pub inverses: Inverses,
}

/// Outer index = class priority (0=large, 1=medium, 2=small).
/// Each inner vec is a GA-evolved permutation of type indices within that class.
/// The decoder processes classes in order, so large pieces are always placed before small.
pub type Genome = Vec<Vec<Gene>>;

/// High-level entry point: decode a group genome into a `SolutionSpec`.
pub fn decode_spec(spec: &ProblemSpec, genome: &Genome) -> crate::model::SolutionSpec {
    let problem = expand::expand_problem(spec);
    let sol = decode(&problem, spec, genome);
    expand::shrink_solution(&sol, spec)
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
    let pool = FreePool::new(problem.sheet.width, problem.sheet.height);
    decode_with_pool(problem, spec, genome, pool)
}

/// Like [`decode`] but starts from an already-initialized [`FreePool`].
///
/// Use when some sheets are pre-seeded from a partial solution.
/// GLAS opens additional sheets starting at `initial_pool.sheets_open()`.
pub(crate) fn decode_with_pool(problem: &Problem, spec: &ProblemSpec, genome: &Genome, mut pool: FreePool) -> Solution {
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

            // Prefer the sheet used by the previous batch of this type so that
            // all copies of one type stay as close together as possible.
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

                // Grid geometry: try filling complete rows (cols fixed by fr_w/pw)
                // and complete columns (rows fixed by fr_h/ph); pick whichever
                // leaves the smaller "hole" in the partial last row/column
                // (row-major wins ties). Degenerates to a 1xN strip when
                // cols==1, rows==1, or the grid divides remaining exactly.
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
                    placements.push(Placement {
                        sheet_idx,
                        piece_idx: next[gene.type_idx],
                        x,
                        y,
                        rotated,
                    });
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

/// Free-rectangle pool across all open sheets.
///
/// Tracks only what the decode loop actually needs: the current free leaves and how
/// many sheets are open. Replaces a former arena-based cut tree that retained every
/// consumed leaf forever without anyone ever reading it back; for verifying that a
/// finished placement is genuinely guillotine-splittable, see
/// [`crate::cut_tree::build_cut_tree`] instead, which reconstructs a tree from a
/// `(Problem, Solution)` pair independently of how the pieces were decoded.
pub(crate) struct FreePool {
    free: SmallVec<[FreeRect; 16]>,
    sheets_open: usize,
}

impl FreePool {
    /// Create a pool with one free leaf `(0, 0, w, h)` on sheet 0.
    fn new(w: u32, h: u32) -> Self {
        FreePool {
            free: smallvec![FreeRect {
                sheet_idx: 0,
                x: 0,
                y: 0,
                w,
                h,
            }],
            sheets_open: 1,
        }
    }

    /// Create a pool pre-seeded with `used_heights.len()` GLF sheets.
    ///
    /// Each sheet gets up to two free rects around the GLF-placed bounding box
    /// `(0, 0, used_w, used_h)`:
    ///   - bottom: `(0, used_h, sw, sh - used_h)`
    ///   - right:  `(used_w, 0, sw - used_w, used_h)`
    ///
    /// Either (or both) is omitted if degenerate (zero width/height).
    /// `used_widths[i]` is clamped to `sw` (and `used_heights[i]` to `sh`) for safety.
    /// Sheet dimensions `sw` and `sh` must be kerf-expanded (as returned by `expand_problem`).
    #[allow(dead_code)]
    pub(crate) fn from_preplaced_sheets(sw: u32, sh: u32, used_heights: &[u32], used_widths: &[u32]) -> Self {
        let mut free = SmallVec::new();
        for (sheet_idx, (&used_h, &used_w)) in used_heights.iter().zip(used_widths).enumerate() {
            let used_h = used_h.min(sh);
            let used_w = used_w.min(sw);
            let free_h = sh - used_h;
            let free_w = sw - used_w;
            if free_h > 0 {
                free.push(FreeRect {
                    sheet_idx,
                    x: 0,
                    y: used_h,
                    w: sw,
                    h: free_h,
                });
            }
            if free_w > 0 && used_h > 0 {
                free.push(FreeRect {
                    sheet_idx,
                    x: used_w,
                    y: 0,
                    w: free_w,
                    h: used_h,
                });
            }
        }
        FreePool {
            free,
            sheets_open: used_heights.len(),
        }
    }

    /// Number of sheets currently open.
    #[allow(dead_code)]
    fn sheets_open(&self) -> usize {
        self.sheets_open
    }

    /// Open a new sheet and add a full-size free leaf `(0, 0, w, h)` for it.
    /// Returns the new sheet index.
    fn open_new_sheet(&mut self, w: u32, h: u32) -> usize {
        let sheet_idx = self.sheets_open;
        self.free.push(FreeRect {
            sheet_idx,
            x: 0,
            y: 0,
            w,
            h,
        });
        self.sheets_open += 1;
        sheet_idx
    }

    /// Register an additional free leaf not produced by `apply_batch`
    /// (used for the "hole" left by a partial grid row/column in matrix
    /// batches). No-op if degenerate (`w == 0 || h == 0`).
    fn push_free_leaf(&mut self, sheet_idx: usize, x: u32, y: u32, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.free.push(FreeRect { sheet_idx, x, y, w, h });
    }

    /// Remove the free rect at `free_pos` and split it around a `cw x ch` batch placed
    /// at its top-left corner `(x, y)`. Returns the batch origin `(x, y)`.
    ///
    /// Given the free rect `(x, y, nw, nh)`, `lw = nw - cw`, `lh = nh - ch`:
    /// - `inv = false` → horizontal cut: right rect is narrow (`lw x ch`),
    ///   bottom rect spans the full width (`nw x lh`).
    /// - `inv = true` → vertical cut: right rect spans the full height
    ///   (`lw x nh`), bottom rect is narrow (`cw x lh`).
    ///
    /// Delegates to [`split_directional`] with `horizontal = !inv`; see
    /// [docs/glas.md](../../docs/glas.md) for the ASCII diagrams.
    ///
    /// `free_pos` is the index returned by one of the `find_fitting_leaf*` methods;
    /// passing it directly avoids a re-scan.
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
    use super::*;
    use crate::{expand::expand_problem, parse_compact::parse_problem};

    /// Build a default gene for `type_idx` with `count` selectors/inverses all zeroed/false.
    fn gg(type_idx: usize, count: usize) -> Gene {
        Gene {
            type_idx,
            rotate: false,
            selectors: std::iter::repeat_n(0u32, count).collect(),
            inverses: std::iter::repeat_n(false, count).collect(),
        }
    }

    #[test]
    fn two_identical_pieces_form_a_strip() {
        // Sheet 200×100, kerf=0. One type: 2 pieces 80×100.
        // strip_fill: fr_w=200, pw=80, remaining=2 → DP reachable={0,80,160}. best_w=160.
        //   cw=160, ch=100. inverse=false: right=(160,0,40,100).
        //   piece 0 at (0,0), piece 1 at (80,0).
        let spec = parse_problem("200x100F:0:80x100/2").expect("parse");
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
        // Sheet 100×100, kerf=0. One type: 3 pieces 60×40.
        // count_h=floor(100/60)=1, count_v=floor(100/40)=2 → vertical strip wins.
        // Batch 1: cw=60, ch=80 (2 pieces). inverse=false: right=(60,0,40,80) + bottom=(0,80,100,20).
        //   piece 0 at (0,0), piece 1 at (0,40).
        // Batch 2: selector=0 → free[0]=(60,0,40,80): pw=60>40 ✗.
        //   free[1]=(0,80,100,20): ph=40>20 ✗. Neither fits → opens sheet 1.
        //   piece 2 at (0,0).
        let spec = parse_problem("100x100F:0:60x40/3").expect("parse");
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
        // Sheet 200×100, kerf=0. Type 0: 2×(90×100). Type 1: 1×(100×100).
        // Genome [type_1, type_0]. type_1 fills (0,0) with inverse=false: right=(100,0,100,100) + no bottom.
        // type_0 placed=0: selectors[0]=1 → free[1]=(100,0,100,100).
        //   strip: fr_w=100, pw=90, 1 piece (90<100, 180>100). piece 0 at (100,0).
        // type_0 placed=1: selectors[1]=0 → nothing fits sheet 0 → opens sheet 1.
        let spec = parse_problem("200x100F:0:90x100/2,100x100/1").expect("parse");
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
        // Sheet 200×200, kerf=0. One type: 4 pieces 100×100.
        // cols=2, rows=2, grid_n=4. Both orientations give cw=200, ch=200, hole=0
        // (rows_full==cols_full==2, extra==0) → one batch, a 2x2 grid filling the sheet.
        //   p0=(0,0), p1=(100,0), p2=(0,100), p3=(100,100).
        let spec = parse_problem("200x200F:0:100x100/4").expect("parse");
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
    fn five_pieces_form_grid_with_corner_hole() {
        // Sheet 300×400, kerf=0. One type: 5 pieces 100×100.
        // fr=(0,0,300,400): cols=3, rows=4, grid_n=min(5,12)=5.
        // row-major: rows_full=1, extra_row=2 → cw=300, ch=200, hole=(300-200)*100=10000.
        // column-major: cols_full=1, extra_col=1 → cw=200, ch=400, hole=100*(400-100)=30000.
        // row-major wins (smaller hole) → single batch, cw=300, ch=200.
        // inverse=false: right=none (lw=0), bottom=(0,200,300,200).
        // Grid (row-major, 3 cols): p0=(0,0), p1=(100,0), p2=(200,0), p3=(0,100), p4=(100,100).
        // Corner hole = (200,100,100,100), pushed as an extra free leaf.
        let spec = parse_problem("300x400F:0:100x100/5").expect("parse");
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
    }

    #[test]
    fn matrix_batch_hole_is_reused_by_next_gene() {
        // Sheet 300×400, kerf=0. Type 0: 5×(100×100) - same grid-with-hole as
        // `five_pieces_form_grid_with_corner_hole`, leaving free leaves
        // [bottom=(0,200,300,200), hole=(200,100,100,100)] (in that order).
        // Type 1: 1×(100×100r) - rotatable, so it stays a separate piece spec
        // after normalization despite matching type 0's dimensions.
        // selectors[0]=1 → find_fitting_leaf starts at free_pos=1, which is
        // the hole; the piece fits it exactly (no leftover).
        let spec = parse_problem("300x400F:0:100x100/5,100x100/1r").expect("parse");
        let problem = expand_problem(&spec);
        let mut genome = vec![vec![gg(0, 5), gg(1, 1)]];
        genome[0][1].selectors[0] = 1;
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 6);
        let p5 = sol.placements.iter().find(|p| p.piece_idx == 5).unwrap(); // type_1
        assert_eq!((p5.x, p5.y, p5.sheet_idx), (200, 100, 0));

        let errors = crate::model::validate_solution(&problem, &sol);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn strip_mixes_two_types() {
        // Sheet 350×100, kerf=0. Type A: 150×100/1. Type B: 100×80/2.
        // Genome: [A, B]. Each type gets its own batch.
        // Batch 1 (A): fr_w=350, pw=150, remaining=1 → count=1, cw=150, ch=100.
        //   inverse=false: right=(150,0,200,100). A at (0,0).
        // Batch 2 (B): fr=(150,0,200,100), pw=100, remaining=2 → count=2, cw=200, ch=80.
        //   inverse=false: bottom=(150,80,200,20). B at (150,0), B at (250,0).
        // All pieces on sheet 0.
        let spec = parse_problem("350x100F:0:150x100/1f,100x80/2f").expect("parse");
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
        // Sheet 400×300, kerf=0. Three types:
        //   type 0 (filler): 400×200/1f - strip count=floor(400/400)=1.
        //                                  inverse=false creates bottom=(0,200,400,100).
        //   type 1 (A):      200×100/1f - fits in the 100-high bottom; count=floor(400/200)=2, rem=1→1.
        //   type 2 (B):      200×200/1f - ph=200 > all remaining fr_h=100; goes to sheet 1.
        let spec = parse_problem("400x300F:0:400x200/1f,200x100/1f,200x200/1f").expect("parse");
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

    /// Helper: extract (x, y, w, h) of every free leaf in insertion order.
    fn free_rects(pool: &FreePool) -> Vec<(u32, u32, u32, u32)> {
        pool.free.iter().map(|fr| (fr.x, fr.y, fr.w, fr.h)).collect()
    }

    #[test]
    fn pool_inverse_false() {
        // Sheet 200×300, batch 100×100.  lw=100, lh=200.
        // inverse=false: batch at (0,0); free: (100,0,100,100) + (0,100,200,200).
        let mut pool = FreePool::new(200, 300);
        let (bx, by) = pool.apply_batch(0, 100, 100, false);
        assert_eq!((bx, by), (0, 0));
        let rects = free_rects(&pool);
        assert_eq!(rects.len(), 2);
        assert!(rects.contains(&(100, 0, 100, 100)));
        assert!(rects.contains(&(0, 100, 200, 200)));
    }

    #[test]
    fn pool_inverse_true() {
        // Sheet 200×300, batch 100×100.  lw=100, lh=200.
        // inverse=true: batch at (0,0); free: (100,0,100,300) + (0,100,100,200).
        let mut pool = FreePool::new(200, 300);
        let (bx, by) = pool.apply_batch(0, 100, 100, true);
        assert_eq!((bx, by), (0, 0));
        let rects = free_rects(&pool);
        assert_eq!(rects.len(), 2);
        assert!(rects.contains(&(100, 0, 100, 300)));
        assert!(rects.contains(&(0, 100, 100, 200)));
    }

    #[test]
    fn pool_exact_fit() {
        // Sheet 100×100, batch 100×100: exact fit - no free leaves after.
        let mut pool = FreePool::new(100, 100);
        let (bx, by) = pool.apply_batch(0, 100, 100, false);
        assert_eq!((bx, by), (0, 0));
        assert!(pool.free.is_empty());
    }

    #[test]
    fn pool_exact_w() {
        // Sheet 100×200, batch 100×100: lw=0, lh=100.
        // inverse=false: batch at (0,0), only bottom free: (0,100,100,100).
        let mut pool = FreePool::new(100, 200);
        let (bx, by) = pool.apply_batch(0, 100, 100, false);
        assert_eq!((bx, by), (0, 0));
        assert_eq!(free_rects(&pool), vec![(0, 100, 100, 100)]);
    }

    #[test]
    fn pool_exact_h() {
        // Sheet 200×100, batch 100×100: lw=100, lh=0.
        // inverse=false: batch at (0,0), only right free: (100,0,100,100).
        let mut pool = FreePool::new(200, 100);
        let (bx, by) = pool.apply_batch(0, 100, 100, false);
        assert_eq!((bx, by), (0, 0));
        assert_eq!(free_rects(&pool), vec![(100, 0, 100, 100)]);
    }
}
