use smallvec::{SmallVec, smallvec};

use crate::{
    expand,
    model::{FreeRect, Placement, Problem, ProblemSpec, Solution},
    slas::decoder::{CutCtx, SplitAxis, find_placement, guillotine_split, open_new_sheet, sheet_rect},
};

type FreeList = SmallVec<[(FreeRect, Option<CutCtx>); 16]>;

/// Per-copy free-rect selectors: `selectors[k] % |free|` picks the target free rect
/// at the start of the strip that begins when `k` pieces of this type are already placed.
pub type Selectors = SmallVec<[u32; 16]>;

/// Per-copy strip-direction flags: `inverses[k]` governs whether the strip
/// starting at copy #k is packed vertically (true) or horizontally (false).
pub type Inverses = SmallVec<[bool; 16]>;

/// One gene per piece type: a permutation of `0..spec.piespecs.len()`.
///
/// Unlike `slas::Gene` (one gene per physical piece), a single `Gene` here drives
/// the placement of ALL copies of one piece type, strip by strip.
///
/// `selectors` and `inverses` each have exactly `count` elements — one per physical
/// copy — but only strip-start positions are consulted at decode time.
/// Mid-strip entries are carried silently; this keeps the arrays symmetric and
/// lets crossover treat every index identically without special-casing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Gene {
    /// Index into `spec.piespecs` — which piece type this gene handles.
    pub type_idx: usize,
    /// Prefer rotated orientation for every piece in this group.
    pub rotate: bool,
    /// `selectors[k]`: free-rect selector used when `k` copies have been placed.
    pub selectors: Selectors,
    /// `inverses[k]`: strip orientation used when `k` copies have been placed.
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
/// For each gene the decoder places all pieces of the given type one strip at a time:
///   1. Let `placed` = number of copies of this type already placed.
///      Consult `selectors[placed]` and `inverses[placed]`.
///   2. Find a free rect where at least one piece fits, scanning from
///      `selectors[placed] % |free|`.
///   3. Pack as many pieces as fit in a row (or column when `inverses[placed]`) —
///      the *composite piece*.
///   4. Apply the standard SLAS guillotine split on the composite dimensions.
///   5. Advance `placed` by the strip size and repeat until the group is exhausted.
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
    let mfg_cost = 0u32; // TODO: adapt mfg_cost_increment for composite strip placements

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
            let (n, cw, ch) = strip_pack(&fr, pw, ph, remaining, inv);

            for j in 0..n {
                let (x, y) = if !inv {
                    (fr.x + j as u32 * pw, fr.y) // horizontal: pieces in a row
                } else {
                    (fr.x, fr.y + j as u32 * ph) // vertical: pieces in a column
                };
                placements.push(Placement {
                    sheet_idx: fr.sheet_idx,
                    piece_idx: next[gene.type_idx] + j,
                    x,
                    y,
                    rotated,
                });
            }
            next[gene.type_idx] += n;

            // Guillotine split on composite rect (cw × ch).
            let lw = fr.w.saturating_sub(cw);
            let lh = fr.h.saturating_sub(ch);
            let splits = guillotine_split(&fr, cw, ch, inv);
            let mut si = 0;
            if lw > 0 {
                free.push((splits[si], Some((SplitAxis::V, fr.x + cw))));
                si += 1;
            }
            if lh > 0 {
                free.push((splits[si], Some((SplitAxis::H, fr.y + ch))));
            }
        }
    }

    let leftovers = free.into_iter().map(|(fr, _)| fr).collect();
    Solution { placements, leftovers, mfg_cost }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// How many pieces fit in one strip and the composite strip dimensions.
///
/// Horizontal (default): pieces side by side in a row -> composite is `n·pw × ph`.
/// Vertical (`inverse`): pieces stacked in a column -> composite is `pw × n·ph`.
///
/// `pw`/`ph` are kerf-inclusive (from `expand_problem`), so `n·pw` is exact.
/// Guaranteed `n >= 1` because `find_placement` already confirmed `pw <= fr.w` / `ph <= fr.h`.
fn strip_pack(fr: &FreeRect, pw: u32, ph: u32, remaining: usize, inverse: bool) -> (usize, u32, u32) {
    if !inverse {
        let cols = (fr.w / pw) as usize;
        let n = remaining.min(cols);
        (n, n as u32 * pw, ph)
    } else {
        let rows = (fr.h / ph) as usize;
        let n = remaining.min(rows);
        (n, pw, n as u32 * ph)
    }
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
        // Both fit in a horizontal strip on sheet 0:
        //   piece 0 at (0,0), piece 1 at (80,0), right leftover (160,0,40,100).
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
        // cols = 100/60 = 1 per row. Each strip places exactly 1 piece:
        //   placed=0: selectors[0]=0 -> free[0]=(0,0,100,100) -> (0,0), bottom (0,40,100,60).
        //   placed=1: selectors[1]=0 -> scans from 0, right leftover (60,0,40,100):
        //             60×40 doesn't fit (w=40<60). bottom (0,40,100,60): fits -> (0,40).
        //   placed=2: selectors[2]=0 -> nothing fits on sheet 0 -> opens sheet 1 -> (0,0).
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
        // Genome: [type_1, type_0]. type_1 places first at (0,0), right leftover (100,0,100,100).
        // type_0, placed=0: selectors[0]=1 -> free[1]=(100,0,100,100) -> fits 1 piece at (100,0).
        // type_0, placed=1: selectors[1]=0 -> nothing fits on sheet 0 -> opens sheet 1.
        let spec = parse_problem("200x100F:0:90x100/2,100x100/1").expect("parse");
        let problem = expand_problem(&spec);
        let mut genome = vec![gg(1, 1), gg(0, 2)];
        genome[1].selectors[0] = 1; // steer first strip of type_0 to right leftover
        let sol = decode(&problem, &spec, &genome);
        assert_eq!(sol.sheets_used(), 2);
        let b = sol.placements.iter().find(|p| p.piece_idx == 2).unwrap(); // type_1
        assert_eq!((b.x, b.y, b.sheet_idx), (0, 0, 0));
        let a0 = sol.placements.iter().find(|p| p.piece_idx == 0).unwrap(); // first type_0
        assert_eq!((a0.x, a0.y, a0.sheet_idx), (100, 0, 0));
        let a1 = sol.placements.iter().find(|p| p.piece_idx == 1).unwrap(); // second type_0
        assert_eq!(a1.sheet_idx, 1);
    }
}
