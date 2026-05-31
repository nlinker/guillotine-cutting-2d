use smallvec::SmallVec;

use crate::{
    cut_tree::{Blueprint, CutForest, improve_tl_corners},
    expand,
    model::{FreeRect, Placement, Problem, ProblemSpec, Solution},
};

/// Per-copy free-rect selectors: `selectors[k] % |free|` picks the target free rect
/// at the start of the batch that begins when `k` pieces of this type are already placed.
pub type Selectors = SmallVec<[u32; 16]>;

/// Per-copy inversion flags: `inverses[k]` selects TlH (`false`) or TlV (`true`) for
/// the batch that starts when `k` pieces of this type are already placed.
pub type Inverses = SmallVec<[bool; 16]>;

/// One gene per piece type: a permutation of `0..spec.piespecs.len()`.
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
    /// Index into `spec.piespecs` — which piece type this gene handles.
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
    debug_assert_eq!(genome.iter().map(|c| c.len()).sum::<usize>(), spec.piespecs.len());

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
    let mut forest = CutForest::new(problem.sheet.width, problem.sheet.height);
    let mut placements: Vec<Placement> = Vec::with_capacity(problem.pieces.len());
    let mfg_cost = 0u32;

    for class in genome {
        for gene in class {
            let count = spec.piespecs[gene.type_idx].count as usize;
            let end_idx = offsets[gene.type_idx] + count;
            debug_assert_eq!(gene.selectors.len(), count);
            debug_assert_eq!(gene.inverses.len(), count);

            while next[gene.type_idx] < end_idx {
                // placed = copies of this type already placed = index into selectors/inverses
                let placed = next[gene.type_idx] - offsets[gene.type_idx];

                let ps = gene.selectors[placed];
                let inv = gene.inverses[placed];
                let bp = if inv { Blueprint::TlV } else { Blueprint::TlH };

                let piece = &problem.pieces[next[gene.type_idx]];

                let found = forest.find_fitting_leaf(piece, gene.rotate, ps).or_else(|| {
                    forest.open_new_sheet(problem.sheet.width, problem.sheet.height);
                    forest.find_fitting_leaf(piece, gene.rotate, ps)
                });

                let Some((leaf_idx, pw, ph, rotated)) = found else {
                    debug_assert!(
                        false,
                        "piece {}×{} does not fit on empty {}×{} sheet",
                        piece.width, piece.height, problem.sheet.width, problem.sheet.height
                    );
                    break;
                };

                let (fr_w, fr_h, sheet_idx) = {
                    let node = &forest.nodes[leaf_idx];
                    (node.w, node.h, node.sheet_idx)
                };

                // Choose strip orientation: horizontal (left-to-right) or vertical
                // (top-to-bottom), whichever fits more copies in one batch.
                let remaining = end_idx - next[gene.type_idx];
                let count_h = (fr_w / pw).min(remaining as u32) as usize;
                let count_v = (fr_h / ph).min(remaining as u32) as usize;
                let vertical = count_v > count_h;
                let (count, cw, ch) = if vertical {
                    (count_v, pw, ph * count_v as u32)
                } else {
                    (count_h, pw * count_h as u32, ph)
                };

                // Apply blueprint: splits the leaf, returns batch origin.
                let (batch_x, batch_y) = forest.apply_blueprint(leaf_idx, cw, ch, bp);

                // Place all pieces in the strip (left-to-right or top-to-bottom).
                let mut cursor = if vertical { batch_y } else { batch_x };
                for _ in 0..count {
                    let (x, y) = if vertical { (batch_x, cursor) } else { (cursor, batch_y) };
                    placements.push(Placement {
                        sheet_idx,
                        piece_idx: next[gene.type_idx],
                        x,
                        y,
                        rotated,
                    });
                    cursor += if vertical { ph } else { pw };
                    next[gene.type_idx] += 1;
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

    improve_tl_corners(
        problem,
        Solution {
            placements,
            leftovers,
            mfg_cost,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, parse::parse_problem};

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
        // Batch 1: strip fr_w=200, pw=100 → 2 pieces, cw=200, ch=100. TlH: bottom=(0,100,200,100).
        //   p0=(0,0), p1=(100,0).
        // Batch 2: fr=(0,100,200,100). strip → 2 pieces, cw=200, ch=100. TlH exact fit.
        //   p2=(0,100), p3=(100,100).
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
    fn five_pieces_vertical_then_horizontal() {
        // Sheet 300×400, kerf=0. One type: 5 pieces 100×100.
        // Batch 1: fr (0,0,300×400). count_h=3, count_v=4 → vertical strip (4 pieces).
        //   cw=100, ch=400. TlH: right=(100,0,200,400). bottom=none (lh=0).
        //   p0=(0,0), p1=(0,100), p2=(0,200), p3=(0,300).
        // Batch 2: fr=(100,0,200×400), remaining=1. count_h=min(2,1)=1, count_v=min(4,1)=1 → horizontal.
        //   cw=100, ch=100. TlH: right=(200,0,100,100), bottom=(100,100,200,300). p4=(100,0).
        let spec = parse_problem("300x400F:0:100x100/5").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![vec![gg(0, 5)]];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 5);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        assert_eq!((f(0).x, f(0).y), (0, 0));
        assert_eq!((f(1).x, f(1).y), (0, 100));
        assert_eq!((f(2).x, f(2).y), (0, 200));
        assert_eq!((f(3).x, f(3).y), (0, 300));
        assert_eq!((f(4).x, f(4).y), (100, 0));
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
