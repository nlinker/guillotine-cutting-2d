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

pub type Genome = Vec<Gene>;



/// High-level entry point: decode a group genome into a `SolutionSpec`.
pub fn decode_spec(spec: &ProblemSpec, genome: &Genome, strip_delta: u32) -> crate::model::SolutionSpec {
    let problem = expand::expand_problem(spec);
    let sol = decode(&problem, spec, genome, strip_delta);
    expand::shrink_solution(&sol, spec)
}

/// Decode a group genome into a flat `Solution`.
///
/// For each gene the decoder places all pieces of the given type one batch at a time:
///   1. Let `placed` = number of copies of this type already placed.
///      Consult `selectors[placed]` and `inverses[placed]`.
///   2. Find a free leaf where at least one piece fits, preferring a snug fit first.
///      Open a new sheet if nothing fits anywhere.
///   3. Fill a strip of pieces (see [`strip_fill`]) maximising the composite-box width.
///   4. Apply TlH (`inv=false`) or TlV (`inv=true`) to split the free leaf.
///   5. Advance `next` counters for all types used in the strip and repeat.
pub fn decode(problem: &Problem, spec: &ProblemSpec, genome: &Genome, strip_delta: u32) -> Solution {
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
        debug_assert_eq!(gene.inverses.len(), count);

        while next[gene.type_idx] < end_idx {
            // placed = copies of this type already placed = index into selectors/inverses
            let placed = next[gene.type_idx] - offsets[gene.type_idx];

            let ps = gene.selectors[placed];
            let inv = gene.inverses[placed];
            let bp = if inv { Blueprint::TlV } else { Blueprint::TlH };

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

            let Some((leaf_idx, _pw, _ph, _rotated)) = found else {
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

            let strip = strip_fill(
                fr_w,
                fr_h,
                spec,
                &next,
                &offsets,
                gene.type_idx,
                gene.rotate,
                strip_delta,
            );

            // Apply blueprint: splits the leaf, returns batch origin.
            let (batch_x, batch_y) = forest.apply_blueprint(leaf_idx, strip.cw, strip.ch, bp);

            // Place all pieces in the strip left-to-right.
            let mut x_cursor = batch_x;
            for item in &strip.items {
                for _ in 0..item.count {
                    placements.push(Placement {
                        sheet_idx,
                        piece_idx: next[item.type_idx],
                        x: x_cursor,
                        y: batch_y,
                        rotated: item.rotated,
                    });
                    x_cursor += item.pw;
                    next[item.type_idx] += 1;
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

    Solution {
        placements,
        leftovers,
        mfg_cost,
    }
}



/// One piece type's contribution to a strip.
struct StripItem {
    type_idx: usize,
    pw: u32,
    ph: u32,
    rotated: bool,
    count: usize,
}

/// Result of [`strip_fill`]: a single horizontal strip of mixed piece types.
///
/// `items` are sorted by `ph` descending (tallest first, leftmost in the row).
/// `cw` = sum of `item.pw * item.count`; `ch` = max `item.ph`.
struct StripResult {
    items: Vec<StripItem>,
    /// Composite width: total occupied width of the strip.
    cw: u32,
    /// Composite height: height of the tallest piece in the strip.
    ch: u32,
}

/// Fill a free rect `(fr_w × fr_h)` with a horizontal strip of pieces drawn
/// from all available piece types, maximising the total occupied width.
///
/// Only the primary type uses `prefer_rotate`; all others are tried in natural
/// orientation first (then rotated if allowed by the piece spec).
///
/// `strip_delta` sets the minimum acceptable total width:
/// the function searches for the highest reachable `cw ∈ [fr_w − strip_delta, fr_w]`.
/// If nothing meets the threshold, the highest reachable width ≥ 1 is used instead
/// (guaranteed because `find_fitting_leaf` already confirmed the primary piece fits).
///
/// Algorithm: bounded 0/1 knapsack DP.
/// Complexity: O(Σ remaining_i × fr_w) — typically < 300 K operations.
#[allow(clippy::too_many_arguments)]
fn strip_fill(
    fr_w: u32,
    fr_h: u32,
    spec: &ProblemSpec,
    next: &[usize],
    offsets: &[usize],
    primary_type_idx: usize,
    prefer_rotate: bool,
    strip_delta: u32,
) -> StripResult {
    struct Candidate {
        type_idx: usize,
        pw: u32,
        ph: u32,
        rotated: bool,
        remaining: usize,
    }

    let candidates: Vec<Candidate> = (0..spec.piespecs.len())
        .filter_map(|i| {
            let remaining = spec.piespecs[i].count as usize - (next[i] - offsets[i]);
            if remaining == 0 {
                return None;
            }
            let ps = &spec.piespecs[i];
            let prefer_rot = i == primary_type_idx && prefer_rotate;
            // Build a temporary Piece to reuse piece_fits_in from cut_tree.
            let piece = crate::model::Piece {
                name: String::new(),
                width: ps.width,
                height: ps.height,
                can_rotate: ps.can_rotate,
            };
            let (pw, ph, rotated) = crate::cut_tree::piece_fits_in(fr_w, fr_h, &piece, prefer_rot)?;
            Some(Candidate {
                type_idx: i,
                pw,
                ph,
                rotated,
                remaining,
            })
        })
        .collect();

    let cap = fr_w as usize;
    let mut reachable = vec![false; cap + 1];
    let mut from: Vec<Option<(usize, u32)>> = vec![None; cap + 1]; // (type_idx, pw)
    reachable[0] = true;

    for c in &candidates {
        for _copy in 0..c.remaining {
            let pw = c.pw as usize;
            if pw > cap {
                continue;
            }
            for w in (0..=(cap - pw)).rev() {
                if reachable[w] && !reachable[w + pw] {
                    reachable[w + pw] = true;
                    from[w + pw] = Some((c.type_idx, c.pw));
                }
            }
        }
    }

    let lo = fr_w.saturating_sub(strip_delta) as usize;
    let best_w = (lo..=cap)
        .rev()
        .find(|&w| reachable[w])
        .or_else(|| (1..lo).rev().find(|&w| reachable[w]))
        .expect("primary piece always fits, so w ≥ 1 is always reachable");

    let mut counts: std::collections::HashMap<usize, (u32, u32, bool, usize)> =
        std::collections::HashMap::new(); // type_idx → (pw, ph, rotated, count)
    let mut w = best_w;
    while w > 0 {
        let (t_idx, pw) = from[w].expect("backtrack must follow a valid path");
        let entry = counts.entry(t_idx).or_insert_with(|| {
            let c = candidates
                .iter()
                .find(|c| c.type_idx == t_idx)
                .expect("backtrack always follows a candidate that was added to `candidates`");
            (c.pw, c.ph, c.rotated, 0)
        });
        entry.3 += 1;
        w -= pw as usize;
    }

    let mut items: Vec<StripItem> = counts
        .into_iter()
        .map(|(type_idx, (pw, ph, rotated, count))| StripItem {
            type_idx,
            pw,
            ph,
            rotated,
            count,
        })
        .collect();
    // Sort tallest first, then widest, then by type_idx — fully deterministic.
    items.sort_unstable_by(|a, b| b.ph.cmp(&a.ph).then(b.pw.cmp(&a.pw)).then(a.type_idx.cmp(&b.type_idx)));

    let cw = best_w as u32;
    let ch = items.iter().map(|i| i.ph).max().unwrap_or(0);
    StripResult { items, cw, ch }
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
        let genome = vec![gg(0, 2)];
        let sol = decode(&problem, &spec, &genome, 0);
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
        // Batch 1: fr_w=100, pw=60 → only 1 piece fits (60<100 but 120>100). cw=60, ch=40.
        //   TlH: right=(60,0,40,40) + bottom=(0,40,100,60). piece 0 at (0,0).
        // Batch 2: selector=0 → free[0]=(60,0,40,40): pw=60>40, skip.
        //   free[1]=(0,40,100,60): pw=60≤100, ph=40≤60, fits. piece 1 at (0,40).
        // Batch 3: remaining rects all too small. Opens sheet 1. piece 2 at (0,0).
        let spec = parse_problem("100x100F:0:60x40/3").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 3)];
        let sol = decode(&problem, &spec, &genome, 0);
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
        let mut genome = vec![gg(1, 1), gg(0, 2)];
        genome[1].selectors[0] = 1; // steer first batch of type_0 to right leftover
        let sol = decode(&problem, &spec, &genome, 0);
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
        let genome = vec![gg(0, 4)];
        let sol = decode(&problem, &spec, &genome, 0);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 4);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        assert_eq!((f(0).x, f(0).y), (0, 0));
        assert_eq!((f(1).x, f(1).y), (100, 0));
        assert_eq!((f(2).x, f(2).y), (0, 100));
        assert_eq!((f(3).x, f(3).y), (100, 100));
    }

    #[test]
    fn five_pieces_three_plus_two_batches() {
        // Sheet 300×400, kerf=0. One type: 5 pieces 100×100.
        // Batch 1: fr_w=300, pw=100, remaining=5 → 3 pieces (cw=300). TlH: bottom=(0,100,300,300).
        //   p0=(0,0), p1=(100,0), p2=(200,0).
        // Batch 2: fr=(0,100,300,300), remaining=2 → 2 pieces (cw=200). TlH: right=(200,100,100,200)+bottom=(0,200,300,200).
        //   p3=(0,100), p4=(100,100).
        let spec = parse_problem("300x400F:0:100x100/5").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 5)];
        let sol = decode(&problem, &spec, &genome, 0);
        assert_eq!(sol.sheets_used(), 1);
        assert_eq!(sol.placements.len(), 5);
        let f = |idx: usize| sol.placements.iter().find(|p| p.piece_idx == idx).unwrap();
        assert_eq!((f(0).x, f(0).y), (0, 0));
        assert_eq!((f(1).x, f(1).y), (100, 0));
        assert_eq!((f(2).x, f(2).y), (200, 0));
        assert_eq!((f(3).x, f(3).y), (0, 100));
        assert_eq!((f(4).x, f(4).y), (100, 100));
    }

    #[test]
    fn snug_rect_preferred_over_selector() {
        // Verify that find_snug_leaf prefers an exact-fit rect over the selector-chosen one.
        //
        // Manual setup (no decode involved): apply TlV(200×200) to a 500×300 sheet,
        // which produces two free leaves:
        //   free[0] = (200, 0, 300, 300) — NOT snug for a 100×100 piece (neither dim = 100)
        //   free[1] = (0, 200, 200, 100) — SNUG  for a 100×100 piece (h = 100 = ph)
        //
        // selector=0 → find_fitting_leaf returns free[0] (first fitting rect, non-snug).
        // find_snug_leaf with selector=0 must skip free[0] and return free[1] (snug).
        use crate::{
            cut_tree::{Blueprint, CutForest},
            glas::dim_index::DimIndex,
            model::Piece,
        };

        let spec = parse_problem("500x300F:0:100x100/1f").expect("parse");
        let problem = expand_problem(&spec);
        let dim_idx = DimIndex::build(&spec, &problem);

        let mut forest = CutForest::new(500, 300);
        let root = forest.free_leaves[0];
        // TlV(200×200): right=(200,0,300,300) + bottom=(0,200,200,100).
        forest.apply_blueprint(root, 200, 200, Blueprint::TlV);
        // free_leaves: [idx_right=(200,0,300,300), idx_snug=(0,200,200,100)]

        let piece = Piece {
            name: String::new(),
            width: 100,
            height: 100,
            can_rotate: false,
        };

        // Without snug preference, selector=0 picks free[0] = (200,0,300,300).
        let (fit_idx, _, _, _) = forest
            .find_fitting_leaf(&piece, false, 0)
            .expect("must find a fitting leaf");
        assert_eq!(
            (forest.nodes[fit_idx].x, forest.nodes[fit_idx].y),
            (200, 0),
            "find_fitting_leaf(sel=0) must return the first fitting rect at (200,0)"
        );

        // With snug preference, selector=0 is overridden — snug rect (0,200) wins.
        let (snug_idx, _, _, _) = forest
            .find_snug_leaf(&piece, 0, false, 0, &dim_idx)
            .expect("must find a snug leaf");
        assert_eq!(
            (forest.nodes[snug_idx].x, forest.nodes[snug_idx].y),
            (0, 200),
            "find_snug_leaf must prefer (0,200,200×100) over selector-chosen (200,0,300×300)"
        );
    }

    #[test]
    fn strip_mixes_two_types() {
        // Sheet 350×100, kerf=0. Type A: 150×100/1. Type B: 100×80/2.
        // Genome: [A, B]. After A is consumed in batch 1, B takes over.
        // Batch 1 (primary=A): candidates = A(pw=150,ph=100,rem=1), B(pw=100,ph=80,rem=2).
        //   DP: reachable={0,100,150,200,250,300,350}.
        //   best_w=350 (150+100+100). cw=350, ch=max(100,80)=100.
        //   TlH exact fit (cw=fr_w, ch=fr_h → lw=0, lh=0). No free rects remain.
        //   Pieces placed: A at x=0, B at x=150, B at x=250. y=0.
        let spec = parse_problem("350x100F:0:150x100/1f,100x80/2f").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 1), gg(1, 2)];
        let sol = decode(&problem, &spec, &genome, 0);
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
        //   type 0 (filler): 400×200/1f — pw=400=fr_w, so strip_fill selects only this.
        //                                  TlH creates bottom=(0,200,400,100).
        //   type 1 (A):      200×100/1f — fits in the 100-high bottom rect.
        //                                  strip_fill excludes type 2: ph=200 > fr_h=100. ✓
        //   type 2 (B):      200×200/1f — no rect on sheet 0 has h≥200; goes to sheet 1.
        let spec = parse_problem("400x300F:0:400x200/1f,200x100/1f,200x200/1f").expect("parse");
        let problem = expand_problem(&spec);
        let genome = vec![gg(0, 1), gg(1, 1), gg(2, 1)];
        let sol = decode(&problem, &spec, &genome, 0);
        // B (piece_idx=2) cannot fit in the leftover rects (all h≤100); opens sheet 1.
        let pb = sol.placements.iter().find(|p| p.piece_idx == 2).unwrap();
        assert_eq!(
            pb.sheet_idx, 1,
            "200×200 piece must go to a new sheet (fr_h=100 < ph=200)"
        );
    }
}
