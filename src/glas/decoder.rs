use smallvec::{SmallVec, smallvec};

use crate::{
    expand,
    model::{FreeRect, Placement, Problem, ProblemSpec, Solution},
    slas::decoder::{CutCtx, SplitAxis, find_placement, guillotine_split, open_new_sheet, sheet_rect},
};

type FreeList = SmallVec<[(FreeRect, Option<CutCtx>); 16]>;

/// Per-copy free-rect selectors: `selectors[k] % |free|` picks the target free rect
/// at the start of the batch that begins when `k` pieces of this type are already placed.
pub type Selectors = SmallVec<[u32; 16]>;

/// Per-copy SLAS split flags: `inverses[k]` controls the guillotine split direction for
/// the batch that starts when `k` pieces of this type are already placed.
pub type Inverses = SmallVec<[bool; 16]>;

/// One gene per piece type: a permutation of `0..spec.piespecs.len()`.
///
/// Unlike `slas::Gene` (one gene per physical piece), a single `Gene` here drives
/// the placement of ALL copies of one piece type, batch by batch.
///
/// Each batch selects a free rect (via `selectors[placed]`), packs as many pieces as
/// possible into a rectangular **matrix** or a **matrix-minus-corner** arrangement
/// (see [`matrix_pack`]), then applies a SLAS guillotine split on the composite
/// bounding box.
///
/// `selectors` and `inverses` each have exactly `count` elements — one per physical
/// copy — but only batch-start positions are consulted at decode time.
/// Mid-batch entries are carried silently; this keeps the arrays symmetric and
/// lets crossover treat every index identically without special-casing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Gene {
    /// Index into `spec.piespecs` — which piece type this gene handles.
    pub type_idx: usize,
    /// Prefer rotated orientation for every piece in this group.
    pub rotate: bool,
    /// `selectors[k]`: free-rect selector used when `k` copies have been placed.
    pub selectors: Selectors,
    /// `inverses[k]`: SLAS split direction used when `k` copies have been placed.
    pub inverses: Inverses,
}

pub type Genome = Vec<Gene>;

// ── entry points ─────────────────────────────────────────────────────────────

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
///   2. Find a free rect where at least one piece fits, scanning from
///      `selectors[placed] % |free|`.
///   3. Pack as many pieces as fit into a rectangular **matrix** or a
///      **matrix-minus-corner** arrangement (see [`matrix_pack`]).
///   4. Apply the standard SLAS guillotine split on the composite bounding box.
///   5. If a minus-corner arrangement was chosen, return the unused bottom-right
///      cell to the free list.
///   6. Advance `placed` by the batch size and repeat until the group is exhausted.
///
/// Opening a new sheet when nothing fits mirrors `slas::decoder::decode`.
pub fn decode(problem: &Problem, spec: &ProblemSpec, genome: &Genome) -> Solution {
    debug_assert_eq!(genome.len(), spec.piespecs.len());

    // flat index range for type i: problem.pieces[offsets[i] .. offsets[i] + count_i]
    let offsets: Vec<usize> = {
        let mut acc = 0usize;
        spec.piespecs
            .iter()
            .map(|ps| {
                let start = acc;
                acc += ps.count as usize;
                start
            })
            .collect()
    };

    let mut next = offsets.clone(); // next[i] = next unassigned flat index for type i
    let mut free: FreeList = smallvec![(sheet_rect(problem, 0), None)];
    let mut placements: Vec<Placement> = Vec::with_capacity(problem.pieces.len());
    let mut sheets_open = 1usize;
    let mfg_cost = 0u32; // TODO: adapt mfg_cost_increment for composite batch placements

    for gene in genome {
        let count = spec.piespecs[gene.type_idx].count as usize;
        let end_idx = offsets[gene.type_idx] + count;
        debug_assert_eq!(gene.selectors.len(), count);
        debug_assert_eq!(gene.inverses.len(), count);

        while next[gene.type_idx] < end_idx {
            // placed = copies of this type already placed = index into selectors/inverses
            let placed = next[gene.type_idx] - offsets[gene.type_idx];
            let remaining = end_idx - next[gene.type_idx];

            let ps = gene.selectors[placed];
            let inv = gene.inverses[placed];

            let piece = &problem.pieces[next[gene.type_idx]];
            let found = find_placement(&free, piece, gene.rotate, ps)
                .or_else(|| open_new_sheet(&mut free, &mut sheets_open, problem, piece, gene.rotate));

            let Some((idx, pw, ph, rotated)) = found else {
                debug_assert!(
                    false,
                    "piece {}×{} does not fit on empty {}×{} sheet",
                    piece.width, piece.height, problem.sheet.width, problem.sheet.height
                );
                break;
            };

            let (fr, _) = free.remove(idx);
            let mp = matrix_pack(fr.w, fr.h, pw, ph, remaining);

            // Place pieces in row-major order; skip bottom-right cell for minus-corner.
            for row in 0..mp.rows as usize {
                for col in 0..mp.cols as usize {
                    let piece_offset = row * mp.cols as usize + col;
                    let is_missing =
                        mp.minus_corner && row == mp.rows as usize - 1 && col == mp.cols as usize - 1;
                    if !is_missing {
                        placements.push(Placement {
                            sheet_idx: fr.sheet_idx,
                            piece_idx: next[gene.type_idx] + piece_offset,
                            x: fr.x + col as u32 * pw,
                            y: fr.y + row as u32 * ph,
                            rotated,
                        });
                    }
                }
            }
            next[gene.type_idx] += mp.n;

            // SLAS guillotine split on composite bounding box (mp.cw × mp.ch).
            let lw = fr.w.saturating_sub(mp.cw);
            let lh = fr.h.saturating_sub(mp.ch);
            let splits = guillotine_split(&fr, mp.cw, mp.ch, inv);
            let mut si = 0;
            if lw > 0 {
                free.push((splits[si], Some((SplitAxis::V, fr.x + mp.cw))));
                si += 1;
            }
            if lh > 0 {
                free.push((splits[si], Some((SplitAxis::H, fr.y + mp.ch))));
            }
            // Minus-corner: recover the unused bottom-right cell as a new free rect.
            if mp.minus_corner {
                free.push((
                    FreeRect {
                        sheet_idx: fr.sheet_idx,
                        x: fr.x + (mp.cols - 1) * pw,
                        y: fr.y + (mp.rows - 1) * ph,
                        w: pw,
                        h: ph,
                    },
                    None,
                ));
            }
        }
    }

    let leftovers = free.into_iter().map(|(fr, _)| fr).collect();
    Solution { placements, leftovers, mfg_cost }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Result of [`matrix_pack`]: layout for one batch of same-type pieces.
struct MatrixPack {
    /// Pieces to place in this batch (≥ 1).
    n: usize,
    /// Columns in the bounding box.
    cols: u32,
    /// Rows in the bounding box.
    rows: u32,
    /// Composite width  = `cols × pw` (used for SLAS split).
    cw: u32,
    /// Composite height = `rows × ph` (used for SLAS split).
    ch: u32,
    /// `true` when the bottom-right cell is vacant (`n == cols × rows − 1`).
    minus_corner: bool,
}

/// Choose the densest matrix or minus-corner arrangement for a single batch.
///
/// Considers all `(cols, rows)` grid sizes that fit within the `fr_w × fr_h` free rect:
/// - **Full matrix** (`cols × rows`): valid when `remaining ≥ cols × rows`.
/// - **Minus-corner** (`cols × rows − 1`): valid when `remaining + 1 == cols × rows`
///   and `cols ≥ 2`, `rows ≥ 2`.  Degenerate single-row/column cases are already
///   covered by smaller full matrices.
///
/// Selection priority (highest first):
/// 1. More pieces placed.
/// 2. Full matrix over minus-corner (equal count).
/// 3. Wider arrangement (larger `cols`) over taller (equal count and type).
fn matrix_pack(fr_w: u32, fr_h: u32, pw: u32, ph: u32, remaining: usize) -> MatrixPack {
    // Both ≥ 1 because find_placement already confirmed the piece fits.
    let max_cols = (fr_w / pw) as usize;
    let max_rows = (fr_h / ph) as usize;

    let mut best: Option<MatrixPack> = None;

    for rows in 1..=max_rows {
        for cols in 1..=max_cols {
            let full = cols * rows;

            // Option A: full matrix — place exactly `full` pieces.
            if remaining >= full {
                let cand = MatrixPack {
                    n: full,
                    cols: cols as u32,
                    rows: rows as u32,
                    cw: cols as u32 * pw,
                    ch: rows as u32 * ph,
                    minus_corner: false,
                };
                if matrix_is_better(&cand, best.as_ref()) {
                    best = Some(cand);
                }
            }

            // Option B: minus-corner — place `full − 1` pieces (skip bottom-right cell).
            // Only meaningful for true 2-D grids (cols ≥ 2 and rows ≥ 2).
            if cols >= 2 && rows >= 2 && remaining + 1 == full {
                let cand = MatrixPack {
                    n: remaining,
                    cols: cols as u32,
                    rows: rows as u32,
                    cw: cols as u32 * pw,
                    ch: rows as u32 * ph,
                    minus_corner: true,
                };
                if matrix_is_better(&cand, best.as_ref()) {
                    best = Some(cand);
                }
            }
        }
    }

    // Fallback: 1 × 1 (guaranteed to fit by find_placement).
    best.unwrap_or(MatrixPack { n: 1, cols: 1, rows: 1, cw: pw, ch: ph, minus_corner: false })
}

/// `true` when `new` is strictly better than the current `best` pack.
///
/// Priority: 1) more pieces, 2) full matrix over minus-corner, 3) wider (`cols` larger).
fn matrix_is_better(new: &MatrixPack, best: Option<&MatrixPack>) -> bool {
    let Some(best) = best else { return true };
    if new.n != best.n {
        return new.n > best.n;
    }
    // Equal count: prefer full matrix over minus-corner.
    if new.minus_corner != best.minus_corner {
        return !new.minus_corner;
    }
    // Same count and type: prefer wider (more columns).
    new.cols > best.cols
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, parse::parse_problem};

    /// Build a default gene for `type_idx` with `count` selectors/inverses all zeroed.
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
        // matrix_pack chooses (cols=2, rows=1) full: both pieces in one row.
        //   piece 0 at (0,0), piece 1 at (80,0); right leftover (160,0,40,100).
        let spec = parse_problem("200x100F:0:80x100/2").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 2)];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 2);
        let p0 = sol.placements.iter().find(|p| p.piece_idx == 0).unwrap();
        let p1 = sol.placements.iter().find(|p| p.piece_idx == 1).unwrap();
        assert_eq!((p0.x, p0.y, p0.sheet_idx), (0,  0, 0));
        assert_eq!((p1.x, p1.y, p1.sheet_idx), (80, 0, 0));
    }

    #[test]
    fn strip_overflows_to_next_rect() {
        // Sheet 100×100, kerf=0. One type: 3 pieces 60×40.
        // max_cols=1, max_rows=2: first batch places (cols=1, rows=2) = 2 pieces vertically.
        // The 3rd piece finds nothing on sheet 0, goes to sheet 1.
        let spec = parse_problem("100x100F:0:60x40/3").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 3)];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 2);
        assert_eq!(sol.placements.len(), 3);
        assert_eq!((sol.placements[0].x, sol.placements[0].y, sol.placements[0].sheet_idx), (0,  0, 0));
        assert_eq!((sol.placements[1].x, sol.placements[1].y, sol.placements[1].sheet_idx), (0, 40, 0));
        assert_eq!((sol.placements[2].x, sol.placements[2].y, sol.placements[2].sheet_idx), (0,  0, 1));
    }

    #[test]
    fn selector_steers_strip_to_different_rect() {
        // Sheet 200×100, kerf=0. Type 0: 2×(90×100). Type 1: 1×(100×100).
        // Genome [type_1, type_0]. type_1 fills (0,0)→right leftover (100,0,100,100).
        // type_0 placed=0: selectors[0]=1 → free[1]=(100,0,100,100).
        //   max_cols=100/90=1, so matrix_pack places n=1 piece at (100,0).
        // type_0 placed=1: selectors[1]=0 → nothing fits sheet 0 → opens sheet 1.
        let spec = parse_problem("200x100F:0:90x100/2,100x100/1").expect("parse");
        let problem = expand_problem(&spec);
        let mut genome = vec![gg(1, 1), gg(0, 2)];
        genome[1].selectors[0] = 1; // steer first batch of type_0 to right leftover
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
    fn matrix_2x2_full() {
        // Sheet 200×200, kerf=0. One type: 4 pieces 100×100.
        // matrix_pack picks (cols=2, rows=2) full: all 4 pieces in one batch.
        let spec = parse_problem("200x200F:0:100x100/4").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 4)];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 4);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        assert_eq!((f(0).x, f(0).y), (0,   0));
        assert_eq!((f(1).x, f(1).y), (100, 0));
        assert_eq!((f(2).x, f(2).y), (0,   100));
        assert_eq!((f(3).x, f(3).y), (100, 100));
    }

    #[test]
    fn matrix_3x2_minus_corner() {
        // Sheet 300×200, kerf=0. One type: 5 pieces 100×100.
        // (3×2) minus-corner: row 0 = 3 pieces, row 1 = 2 (missing bottom-right at (200,100)).
        // The unused cell (200,100,100,100) is returned to the free list.
        let spec = parse_problem("300x200F:0:100x100/5").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 5)];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 5);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        // Row 0
        assert_eq!((f(0).x, f(0).y), (0,   0));
        assert_eq!((f(1).x, f(1).y), (100, 0));
        assert_eq!((f(2).x, f(2).y), (200, 0));
        // Row 1 — partial (missing bottom-right)
        assert_eq!((f(3).x, f(3).y), (0,   100));
        assert_eq!((f(4).x, f(4).y), (100, 100));
        // Corner cell must appear in leftovers
        assert!(
            sol.leftovers.iter().any(|fr| fr.x == 200 && fr.y == 100 && fr.w == 100 && fr.h == 100),
            "expected corner free rect at (200,100,100×100)"
        );
    }

    #[test]
    fn prefer_strip_over_minus_corner() {
        // Sheet 300×200, kerf=0. One type: 3 pieces 100×100.
        // (3,1) strip and (2,2)-minus-corner both give 3 pieces.
        // Full strip (3,1) is preferred: all pieces in row 0.
        let spec = parse_problem("300x200F:0:100x100/3").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 3)];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 3);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        assert_eq!((f(0).x, f(0).y), (0,   0));
        assert_eq!((f(1).x, f(1).y), (100, 0));
        assert_eq!((f(2).x, f(2).y), (200, 0));
        // No corner leftover at (100,100) — that would signal minus-corner was chosen
        assert!(
            !sol.leftovers.iter().any(|fr| fr.x == 100 && fr.y == 100),
            "strip should be chosen, not (2,2) minus-corner"
        );
    }

    #[test]
    fn prefer_wider_matrix_over_taller() {
        // Sheet 300×400, kerf=0. One type: 5 pieces 100×100.
        // Both (3,2)-minus-corner and (2,3)-minus-corner give 5 pieces.
        // (3,2) is wider (cols=3 > cols=2) so it should be chosen.
        let spec = parse_problem("300x400F:0:100x100/5").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 5)];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 5);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        // (3,2) layout: pieces 0,1,2 in row 0; pieces 3,4 in row 1
        assert_eq!((f(2).x, f(2).y), (200, 0),   "piece 2 at col 2 → (3,2) chosen over (2,3)");
        assert_eq!((f(3).x, f(3).y), (0,   100), "piece 3 starts row 1");
        assert_eq!((f(4).x, f(4).y), (100, 100), "piece 4 is row 1 col 1");
    }
}
