use smallvec::SmallVec;

use crate::{
    cut_tree::{Blueprint, CutForest},
    expand,
    model::{FreeRect, Placement, Problem, ProblemSpec, Solution},
};

/// Per-copy free-rect selectors: `selectors[k] % |free|` picks the target free rect
/// at the start of the batch that begins when `k` pieces of this type are already placed.
pub type Selectors = SmallVec<[u32; 16]>;

/// Per-copy inversion flags: `inverses[k]` selects TlH (`false`) or TlV (`true`) for
/// the batch that starts when `k` pieces of this type are already placed.
pub type Inverses = SmallVec<[bool; 16]>;

/// One gene per piece type: a permutation of `0..spec.piece_types.len()`.
///
/// Unlike `slas::Gene` (one gene per physical piece), a single `Gene` here drives
/// the placement of ALL copies of one piece type, batch by batch.
///
/// Each batch selects a free leaf (via `selectors[placed]`) and a split direction
/// (via `inverses[placed]`), packs a strip of pieces (see [`strip_fill`]) into the
/// composite box, then applies TlH or TlV to split the free leaf.
///
/// `selectors` and `inverses` each have exactly `count` elements — one per physical
/// copy — but only batch-start positions are consulted at decode time.
/// Mid-batch entries are carried silently; this keeps the arrays symmetric and
/// lets crossover treat every index identically without special-casing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Gene {
    /// Index into `spec.piece_types` — which piece type this gene handles.
    pub type_idx: usize,
    /// Prefer rotated orientation for every piece in this group.
    pub rotate: bool,
    /// `selectors[k]`: free-leaf selector used when `k` copies have been placed.
    pub selectors: Selectors,
    /// `inverses[k]`: `false` → TlH split, `true` → TlV split, for the batch
    /// starting when `k` copies have been placed.
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
///   4. Apply TlH (`inv=false`) or TlV (`inv=true`) to split the free leaf.
///   5. Advance `next[gene.type_idx]` by `count` and repeat.
pub fn decode(problem: &Problem, spec: &ProblemSpec, genome: &Genome) -> Solution {
    let forest = CutForest::new(problem.sheet.width, problem.sheet.height);
    decode_with_forest(problem, spec, genome, forest)
}

/// Like [`decode`] but starts from an already-initialized [`CutForest`].
///
/// Use when some sheets are pre-seeded from a partial solution.
/// GLAS opens additional sheets starting at `initial_forest.sheets_open()`.
pub fn decode_with_forest(problem: &Problem, spec: &ProblemSpec, genome: &Genome, mut forest: CutForest) -> Solution {
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
                let bp = if inv { Blueprint::TlV } else { Blueprint::TlH };

                let piece = &problem.pieces[next[gene.type_idx]];
                let remaining = end_idx - next[gene.type_idx];

                let sw = problem.sheet.width;
                let sh = problem.sheet.height;
                let found = if remaining > 1 {
                    // 1. same sheet, batch >= 2
                    last_sheet
                        .and_then(|sid| forest.find_fitting_leaf_min_batch_on_sheet(piece, gene.rotate, ps, 2, sid))
                        // 2. any sheet, batch >= 2
                        .or_else(|| forest.find_fitting_leaf_min_batch(piece, gene.rotate, ps, 2))
                        // 3. any sheet, single
                        .or_else(|| forest.find_fitting_leaf(piece, gene.rotate, ps))
                        .or_else(|| {
                            forest.open_new_sheet(sw, sh);
                            forest.find_fitting_leaf(piece, gene.rotate, ps)
                        })
                } else {
                    // 1. same sheet
                    last_sheet
                        .and_then(|sid| forest.find_fitting_leaf_on_sheet(piece, gene.rotate, ps, sid))
                        // 2. any sheet
                        .or_else(|| forest.find_fitting_leaf(piece, gene.rotate, ps))
                        .or_else(|| {
                            forest.open_new_sheet(sw, sh);
                            forest.find_fitting_leaf(piece, gene.rotate, ps)
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
                    let node = &forest.nodes[forest.free_leaves[free_pos]];
                    (node.w, node.h, node.sheet_idx)
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

                // Apply blueprint: splits the leaf, returns batch origin.
                let (batch_x, batch_y) = forest.apply_blueprint(free_pos, cw, ch, bp);

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
                    forest.push_free_leaf(sheet_idx, hx, hy, hw, hh);
                }
            }
        }
    }

    let leftovers = forest
        .free_leaves
        .iter()
        .map(|&idx| {
            let node = &forest.nodes[idx];
            FreeRect {
                sheet_idx: node.sheet_idx,
                x: node.x,
                y: node.y,
                w: node.w,
                h: node.h,
            }
        })
        .collect();

    Solution { placements, leftovers }
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
            selectors: std::iter::repeat(0u32).take(count).collect(),
            inverses: std::iter::repeat(false).take(count).collect(),
        }
    }

    #[test]
    fn two_identical_pieces_form_a_strip() {
        // Sheet 200×100, kerf=0. One type: 2 pieces 80×100.
        // strip_fill: fr_w=200, pw=80, remaining=2 → DP reachable={0,80,160}. best_w=160.
        //   cw=160, ch=100. TlH: right=(160,0,40,100).
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
        // Batch 1: cw=60, ch=80 (2 pieces). TlH: right=(60,0,40,80) + bottom=(0,80,100,20).
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
        // Genome [type_1, type_0]. type_1 fills (0,0)→TlH: right=(100,0,100,100) + no bottom.
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
        // TlH: right=none (lw=0), bottom=(0,200,300,200).
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
        // Sheet 300×400, kerf=0. Type 0: 5×(100×100) — same grid-with-hole as
        // `five_pieces_form_grid_with_corner_hole`, leaving free leaves
        // [bottom=(0,200,300,200), hole=(200,100,100,100)] (in that order).
        // Type 1: 1×(100×100r) — rotatable, so it stays a separate piece spec
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
        //   TlH: right=(150,0,200,100). A at (0,0).
        // Batch 2 (B): fr=(150,0,200,100), pw=100, remaining=2 → count=2, cw=200, ch=80.
        //   TlH: bottom=(150,80,200,20). B at (150,0), B at (250,0).
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
        //   type 0 (filler): 400×200/1f — strip count=floor(400/400)=1.
        //                                  TlH creates bottom=(0,200,400,100).
        //   type 1 (A):      200×100/1f — fits in the 100-high bottom; count=floor(400/200)=2, rem=1→1.
        //   type 2 (B):      200×200/1f — ph=200 > all remaining fr_h=100; goes to sheet 1.
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
}
