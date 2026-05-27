use smallvec::SmallVec;

use crate::{
    cut_tree::{Blueprint, CutForest},
    expand,
    glas::dim_index::DimIndex,
    model::{FreeRect, Placement, Problem, ProblemSpec, Solution},
};

/// Per-copy free-rect selectors: `selectors[k] % |free|` picks the target free rect
/// at the start of the batch that begins when `k` pieces of this type are already placed.
pub type Selectors = SmallVec<[u32; 16]>;

/// Per-copy blueprint bytes: `blueprints[k]` (value 0–3) selects the corner-placement
/// blueprint for the batch that starts when `k` pieces of this type are already placed.
pub type Blueprints = SmallVec<[u8; 16]>;

/// One gene per piece type: a permutation of `0..spec.piespecs.len()`.
///
/// Unlike `slas::Gene` (one gene per physical piece), a single `Gene` here drives
/// the placement of ALL copies of one piece type, batch by batch.
///
/// Each batch selects a free leaf (via `selectors[placed]`) and a corner-placement
/// blueprint (via `blueprints[placed]`), packs as many pieces as possible into a
/// rectangular **matrix** arrangement (see [`matrix_pack`]), then applies the chosen
/// blueprint to split the free leaf.
///
/// `selectors` and `blueprints` each have exactly `count` elements — one per physical
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
    /// `blueprints[k]`: corner-placement blueprint used when `k` copies have been placed.
    pub blueprints: Blueprints,
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
///      Consult `selectors[placed]` and `blueprints[placed]`.
///   2. Find a free leaf where at least one piece fits, preferring a snug fit first.
///      Open a new sheet if nothing fits anywhere.
///   3. Pack as many pieces as fit into the best full-matrix arrangement from
///      the priority table (see [`matrix_pack`]).
///   4. Apply the chosen blueprint to split the free leaf around the composite box.
///   5. Advance `placed` by the batch size and repeat until the group is exhausted.
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

    // Dimension index: maps expanded piece dimensions → type matches.
    let dim_idx = DimIndex::build(spec, problem);

    let mut next = offsets.clone(); // next[i] = next unassigned flat index for type i
    let mut forest = CutForest::new(problem.sheet.width, problem.sheet.height);
    let mut placements: Vec<Placement> = Vec::with_capacity(problem.pieces.len());
    let mfg_cost = 0u32; // TODO: compute via forest.to_cut_trees() once implemented

    for gene in genome {
        let count = spec.piespecs[gene.type_idx].count as usize;
        let end_idx = offsets[gene.type_idx] + count;
        debug_assert_eq!(gene.selectors.len(), count);
        debug_assert_eq!(gene.blueprints.len(), count);

        while next[gene.type_idx] < end_idx {
            // placed = copies of this type already placed = index into selectors/blueprints
            let placed = next[gene.type_idx] - offsets[gene.type_idx];
            let remaining = end_idx - next[gene.type_idx];

            let ps = gene.selectors[placed];
            let bp = Blueprint::from_u8(gene.blueprints[placed]);

            let piece = &problem.pieces[next[gene.type_idx]];

            // Prefer a snug-fit leaf (exact dimension match eliminates one cut),
            // then any fitting leaf, then open a new sheet.
            let found = forest
                .find_snug_leaf(piece, gene.type_idx, gene.rotate, ps, &dim_idx)
                .or_else(|| forest.find_fitting_leaf(piece, gene.rotate, ps))
                .or_else(|| {
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
            let mp = matrix_pack(fr_w, fr_h, pw, ph, remaining);

            // Apply blueprint: splits the leaf, returns batch origin.
            let (batch_x, batch_y) = forest.apply_blueprint(leaf_idx, mp.cw, mp.ch, bp);

            // Place pieces in row-major order within the batch box.
            for row in 0..mp.rows as usize {
                for col in 0..mp.cols as usize {
                    let piece_offset = row * mp.cols as usize + col;
                    placements.push(Placement {
                        sheet_idx,
                        piece_idx: next[gene.type_idx] + piece_offset,
                        x: batch_x + col as u32 * pw,
                        y: batch_y + row as u32 * ph,
                        rotated,
                    });
                }
            }
            next[gene.type_idx] += mp.n;
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

    Solution {
        placements,
        leftovers,
        mfg_cost,
    }
}

// ── matrix layout table ───────────────────────────────────────────────────────

/// A full-matrix layout candidate: `cols` columns × `rows` rows = `cols * rows` pieces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MatrixLayout {
    cols: u32,
    rows: u32,
}

const fn ml(cols: u32, rows: u32) -> MatrixLayout {
    MatrixLayout { cols, rows }
}

// Priority-ordered layout lists for each piece count n = 1..=12.
// Within a count, wider arrangements (larger cols) come first, except where
// the user's priority ordering explicitly differs (e.g. n=4: 4×1 before 2×2 before 1×4).
static ML1: [MatrixLayout; 1] = [ml(1, 1)];
static ML2: [MatrixLayout; 2] = [ml(2, 1), ml(1, 2)];
static ML3: [MatrixLayout; 2] = [ml(3, 1), ml(1, 3)];
static ML4: [MatrixLayout; 3] = [ml(4, 1), ml(2, 2), ml(1, 4)];
static ML5: [MatrixLayout; 2] = [ml(5, 1), ml(1, 5)];
static ML6: [MatrixLayout; 4] = [ml(6, 1), ml(3, 2), ml(2, 3), ml(1, 6)];
static ML7: [MatrixLayout; 2] = [ml(7, 1), ml(1, 7)];
static ML8: [MatrixLayout; 4] = [ml(8, 1), ml(4, 2), ml(2, 4), ml(1, 8)];
static ML9: [MatrixLayout; 3] = [ml(9, 1), ml(3, 3), ml(1, 9)];
static ML10: [MatrixLayout; 4] = [ml(10, 1), ml(5, 2), ml(2, 5), ml(1, 10)];
static ML11: [MatrixLayout; 2] = [ml(11, 1), ml(1, 11)];
static ML12: [MatrixLayout; 5] = [ml(12, 1), ml(6, 2), ml(4, 3), ml(3, 4), ml(2, 6)];

static LAYOUTS: [&[MatrixLayout]; 13] = [
    &[],   // n=0  (unused)
    &ML1,  // n=1
    &ML2,  // n=2
    &ML3,  // n=3
    &ML4,  // n=4
    &ML5,  // n=5
    &ML6,  // n=6
    &ML7,  // n=7
    &ML8,  // n=8
    &ML9,  // n=9
    &ML10, // n=10
    &ML11, // n=11
    &ML12, // n=12
];

/// Iterator over candidate layouts for a given piece count.
///
/// - For `n ≤ 12`: yields from the static priority table `LAYOUTS[n]`.
/// - For `n ≥ 13`: yields only exact full matrices `M×N` where `M*N == n`,
///   ordered by decreasing `cols` (widest first).
enum CandidateIter {
    Small(std::iter::Copied<std::slice::Iter<'static, MatrixLayout>>),
    Large { n: usize, c: usize },
}

impl Iterator for CandidateIter {
    type Item = MatrixLayout;

    fn next(&mut self) -> Option<MatrixLayout> {
        match self {
            CandidateIter::Small(it) => it.next(),
            CandidateIter::Large { n, c } => {
                while *c >= 1 {
                    let col = *c;
                    *c -= 1;
                    if *n % col == 0 {
                        return Some(MatrixLayout {
                            cols: col as u32,
                            rows: (*n / col) as u32,
                        });
                    }
                }
                None
            }
        }
    }
}

fn candidate_layouts(n: usize) -> CandidateIter {
    if n <= 12 {
        CandidateIter::Small(LAYOUTS[n].iter().copied())
    } else {
        CandidateIter::Large { n, c: n }
    }
}

// ── MatrixPack ────────────────────────────────────────────────────────────────

/// Result of [`matrix_pack`]: layout for one batch of same-type pieces.
struct MatrixPack {
    /// Pieces to place in this batch (≥ 1); always equals `cols × rows`.
    n: usize,
    /// Columns in the bounding box.
    cols: u32,
    /// Rows in the bounding box.
    rows: u32,
    /// Composite width  = `cols × pw` (used for SLAS split).
    cw: u32,
    /// Composite height = `rows × ph` (used for SLAS split).
    ch: u32,
}

/// Choose the best full-matrix arrangement for placing up to `remaining` pieces
/// of size `pw × ph` inside a free rect of size `fr_w × fr_h`.
///
/// Layouts are tried in priority order from the [`LAYOUTS`] table (for n ≤ 12)
/// or by descending column count (for n ≥ 13, exact divisor pairs only).
/// The first layout that physically fits (`cols*pw ≤ fr_w` and `rows*ph ≤ fr_h`)
/// and does not exceed `remaining` is selected.
///
/// Priority: place as many pieces as possible; within the same count, earlier
/// entries in the priority table win (wider arrangements first by default).
fn matrix_pack(fr_w: u32, fr_h: u32, pw: u32, ph: u32, remaining: usize) -> MatrixPack {
    let max_cols = (fr_w / pw) as usize;
    let max_rows = (fr_h / ph) as usize;
    // Maximum pieces that physically fit; also the upper bound on n to try.
    let target = remaining.min(max_cols * max_rows);

    for n in (1..=target).rev() {
        for layout in candidate_layouts(n) {
            if layout.cols as usize <= max_cols && layout.rows as usize <= max_rows {
                return MatrixPack {
                    n,
                    cols: layout.cols,
                    rows: layout.rows,
                    cw: layout.cols * pw,
                    ch: layout.rows * ph,
                };
            }
        }
    }
    // Unreachable: target ≥ 1 and candidate_layouts(1) = [1×1] always fits because
    // find_placement already confirmed the piece fits in this free rect.
    unreachable!("1×1 always fits — find_placement confirmed a fit")
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, parse::parse_problem};

    /// Build a default gene for `type_idx` with `count` selectors/blueprints all zeroed.
    fn gg(type_idx: usize, count: usize) -> Gene {
        Gene {
            type_idx,
            rotate: false,
            selectors: std::iter::repeat(0u32).take(count).collect(),
            blueprints: std::iter::repeat(0u8).take(count).collect(),
        }
    }

    #[test]
    fn two_identical_pieces_form_a_strip() {
        // Sheet 200×100, kerf=0. One type: 2 pieces 80×100.
        // matrix_pack: target=2, n=2, LAYOUTS[2]=[2×1,1×2]. 2×1: cols=2≤2, rows=1≤1 → chosen.
        //   piece 0 at (0,0), piece 1 at (80,0); right leftover (160,0,40,100).
        let spec = parse_problem("200x100F:0:80x100/2").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 2)];
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
        // max_cols=1, max_rows=2; target=min(3,2)=2. n=2: [2×1(no), 1×2(yes)] → 1×2.
        // First batch places 2 pieces vertically: (0,0) and (0,40).
        // The 3rd piece finds nothing on sheet 0, goes to sheet 1.
        let spec = parse_problem("100x100F:0:60x40/3").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 3)];
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
        // target=4. n=4: LAYOUTS[4]=[4×1(no,4>2), 2×2(yes)] → 2×2.
        let spec = parse_problem("200x200F:0:100x100/4").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 4)];
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
    fn five_pieces_use_two_batches_when_no_exact_layout_fits() {
        // Sheet 300×400, kerf=0. One type: 5 pieces 100×100.
        // max_cols=3, max_rows=4, target=5.
        // n=5: LAYOUTS[5]=[5×1(need 5 cols,no), 1×5(need 5 rows,no)]. None fit.
        // n=4: LAYOUTS[4]=[4×1(no), 2×2(2≤3,2≤4 yes)] → batch 1: 2×2 = 4 pieces.
        //   lw=100, lh=200; SLAS lw≤lh → H-first: right=(200,0,100,200) + bottom=(0,200,300,200).
        // n=1: batch 2 (placed=4, sel=0): free[0]=(200,0,100,200) → 1×1 → piece 4 at (200,0).
        let spec = parse_problem("300x400F:0:100x100/5").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 5)];
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 5);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        // Batch 1: 2×2 grid at (0,0)
        assert_eq!((f(0).x, f(0).y), (0, 0));
        assert_eq!((f(1).x, f(1).y), (100, 0));
        assert_eq!((f(2).x, f(2).y), (0, 100));
        assert_eq!((f(3).x, f(3).y), (100, 100));
        // Batch 2: 5th piece in the first available free rect (200,0,100,200)
        assert_eq!((f(4).x, f(4).y), (200, 0));
    }

    #[test]
    fn snug_rect_preferred_over_selector() {
        // Sheet 600×300, kerf=0. Three types:
        //   type 0: 1× 100×200f  — placed first with Blueprint::TlV
        //   type 1: 1× 200×300f  — placed second using selector=1 (skips smaller rect)
        //   type 2: 1× 100×100f  — placed third with selector=1
        //
        // TlV for type 0 (cw=100, ch=200, lw=500, lh=100):
        //   batch at (0,0); free: (100,0,500,300) + (0,200,100,100)
        //
        // type 1 selector=1 → start at free[1]=(0,200,100,100): 200×300 doesn't fit →
        //   free[0]=(100,0,500,300): 200×300 fits. Placed with TlH (default blueprint=0):
        //   lw=300, lh=0 → only right free: (300,0,300,300).
        //   free: [(0,200,100,100), (300,0,300,300)]
        //
        // type 2 selector=1 → find_snug skips (300,0,300,300) (w≠100, h≠100),
        //   picks (0,200,100,100) (w=100=pw) → snug, fits.
        // Expected: type 2 at (0,200), not (300,0).
        let spec = parse_problem("600x300F:0:100x200/1f,200x300/1f,100x100/1f").expect("parse");
        let problem = expand_problem(&spec);
        let mut genome = vec![gg(0, 1), gg(1, 1), gg(2, 1)];
        genome[0].blueprints[0] = 1; // TlV: full-height right strip (100,0,500,300) + narrow bottom (0,200,100,100)
        genome[1].selectors[0] = 1; // steer type 1 past the small rect to the large one
        genome[2].selectors[0] = 1; // without snug: picks (300,0); with snug: picks (0,200)
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 1);
        let p2 = sol.placements.iter().find(|p| p.piece_idx == 2).unwrap();
        assert_eq!(
            (p2.x, p2.y),
            (0, 200),
            "snug rect (0,200,100×100) must be preferred over selector-chosen (300,0,300×300)"
        );
    }

    #[test]
    fn layout_table_counts_correct() {
        // Every entry in LAYOUTS[n] must satisfy cols * rows == n.
        for n in 1usize..=12 {
            for &layout in LAYOUTS[n] {
                assert_eq!(
                    layout.cols as usize * layout.rows as usize,
                    n,
                    "LAYOUTS[{n}] has entry {}×{} whose product is {}",
                    layout.cols,
                    layout.rows,
                    layout.cols * layout.rows,
                );
            }
        }
    }

    #[test]
    fn large_n_candidate_layouts_are_full_matrices() {
        // For n ≥ 13, candidate_layouts yields only exact divisor pairs (no minus-corner).
        for n in 13usize..=20 {
            for layout in candidate_layouts(n) {
                assert_eq!(
                    layout.cols as usize * layout.rows as usize,
                    n,
                    "candidate_layouts({n}) contains {}×{} which is not exact (product {})",
                    layout.cols,
                    layout.rows,
                    layout.cols * layout.rows,
                );
            }
        }
    }
}
