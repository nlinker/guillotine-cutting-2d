use serde::{Deserialize, Serialize};
use smallvec::{SmallVec, smallvec};

use crate::{
    expand, model,
    model::{FreeRect, Piece, Placement, Problem, Solution},
};

type FreeList = SmallVec<[FreeRect; 16]>;
type FreePair = SmallVec<[FreeRect; 2]>;

/// One element of the solution genome (V-vector encoding).
/// `piece_idx`: index into the flat `Problem::pieces` list (one entry per physical copy).
/// `rotate`: when true and `piece.can_rotate`, try (height × width) orientation first.
/// `point_selector`: selects the starting free rect as `free[point_selector % |free|]`;
/// scanning from a variable offset gives the GA freedom to steer pieces to different regions.
/// `inverse`: when true, flips the SLAS split direction — `lw <= lh` picks vertical instead
/// of horizontal. Lets the GA represent cut trees that SLAS cannot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gene {
    pub piece_idx: usize,
    pub rotate: bool,
    pub point_selector: u32,
    pub inverse: bool,
}

/// Ordered genome - one gene per piece, defining placement order, rotation
/// preference, and free-rect selection. `piece_idx` values form a permutation
/// of `0..problem.pieces.len()` (one gene per physical piece).
pub type Genome = Vec<Gene>;

/// Decode a genome into a type-indexed `SolutionSpec`.
///
/// Expands `spec` into a flat `Problem`, runs `decoder::decode`, then maps each
/// `Placement.piece_idx` from flat index back to the type index in `spec`.
pub fn decode_spec(spec: &model::ProblemSpec, genome: &Genome) -> model::SolutionSpec {
    let problem = expand::expand_problem(spec);
    let sol = decode(&problem, genome);
    expand::shrink_solution(&sol, spec)
}

/// Decode a genome into placements using strict guillotine splitting (no merge).
///
/// Pieces are placed in genome order. `point_selector % |free|` picks the
/// preferred free rect; if the piece does not fit there the decoder scans the
/// rest of the list in order. A new sheet is opened when nothing fits.
///
/// Precondition: every piece fits on an empty sheet.
pub fn decode(problem: &Problem, genome: &Genome) -> Solution {
    let mut free: FreeList = smallvec![sheet_rect(problem, 0)];
    let mut placements: Vec<Placement> = Vec::with_capacity(genome.len());
    let mut sheets_open: usize = 1;

    for gene in genome {
        let piece = &problem.pieces[gene.piece_idx];

        let found = find_placement(&free, piece, gene.rotate, gene.point_selector)
            .or_else(|| open_new_sheet(&mut free, &mut sheets_open, problem, piece, gene.rotate));

        if let Some((idx, pw, ph, rotated)) = found {
            let fr = free.remove(idx);
            placements.push(Placement {
                sheet_idx: fr.sheet_idx,
                piece_idx: gene.piece_idx,
                x: fr.x,
                y: fr.y,
                rotated,
            });

            let lw = fr.w - pw;
            let lh = fr.h - ph;
            let splits = guillotine_split(&fr, pw, ph, gene.inverse);
            let mut si = 0;
            if lw > 0 {
                free.push(splits[si]);
                si += 1;
            }
            if lh > 0 {
                free.push(splits[si]);
            }
        } else {
            debug_assert!(
                false,
                "piece {}×{} does not fit on empty {}×{} sheet",
                piece.width, piece.height, problem.sheet.width, problem.sheet.height
            );
        }
    }
    Solution {
        placements,
        leftovers: free.into_vec(),
    }
}

pub(crate) fn open_new_sheet(
    free: &mut FreeList,
    sheets_open: &mut usize,
    problem: &Problem,
    piece: &Piece,
    prefer_rotate: bool,
) -> Option<(usize, u32, u32, bool)> {
    let sheet_idx = *sheets_open;
    let new_fr = sheet_rect(problem, sheet_idx);
    let (pw, ph, rotated) = fits_in(&new_fr, piece, prefer_rotate)?;
    let idx = free.len();
    free.push(new_fr);
    *sheets_open += 1;
    Some((idx, pw, ph, rotated))
}

/// Scan `free` starting at `point_selector % |free|`, wrapping around.
/// Returns `(index, placed_w, placed_h, rotated)` for the first fitting rect, or `None`.
pub(crate) fn find_placement(
    free: &[FreeRect],
    piece: &Piece,
    prefer_rotate: bool,
    point_selector: u32,
) -> Option<(usize, u32, u32, bool)> {
    if free.is_empty() {
        return None;
    }
    let n = free.len();
    let start = (point_selector as usize) % n;
    for i in 0..n {
        let idx = (start + i) % n;
        if let Some((pw, ph, rotated)) = fits_in(&free[idx], piece, prefer_rotate) {
            return Some((idx, pw, ph, rotated));
        }
    }
    None
}

/// Check whether `piece` fits in `fr`, trying preferred orientation first.
/// Returns `(placed_width, placed_height, rotated)` or `None`.
pub(crate) fn fits_in(fr: &FreeRect, piece: &Piece, prefer_rotate: bool) -> Option<(u32, u32, bool)> {
    let try_rotated = prefer_rotate && piece.can_rotate;
    let (pw_a, ph_a) = if try_rotated {
        (piece.height, piece.width)
    } else {
        (piece.width, piece.height)
    };
    if pw_a <= fr.w && ph_a <= fr.h {
        return Some((pw_a, ph_a, try_rotated));
    }
    if piece.can_rotate {
        let (pw_b, ph_b) = (ph_a, pw_a); // opposite orientation
        if pw_b <= fr.w && ph_b <= fr.h {
            return Some((pw_b, ph_b, !try_rotated));
        }
    }
    None
}

/// Split `fr` after placing a `pw × ph` piece at its top-left origin.
///
/// Uses the Shorter Leftover Axis (SLAS) heuristic: if the right leftover is
/// narrower than the bottom leftover, the right child gets the piece's height
/// and the bottom child spans the full rect width (and vice versa).
/// When `inverse` is true the direction is flipped: `lw <= lh` picks vertical instead.
/// Piece dimensions already include kerf (baked in by `expand_problem`), so splits are flush.
pub(crate) fn guillotine_split(fr: &FreeRect, pw: u32, ph: u32, inverse: bool) -> FreePair {
    debug_assert!(pw <= fr.w && ph <= fr.h);
    let lw = fr.w - pw;
    let lh = fr.h - ph;
    let mut out = SmallVec::new();
    // equivalent to (!inverse && lw <= lh) || (inverse && lw > lh)
    if (lw <= lh) != inverse {
        // right child: narrow (piece height); bottom child: full width
        // ┌──────────┬──────────┐
        // │  piece   │  right   │
        // │  pw × ph │ lw × ph  │
        // ├──────────┴──────────┤
        // │       bottom        │
        // │   fr.width × lh     │
        // └─────────────────────┘
        if lw > 0 {
            out.push(FreeRect {
                sheet_idx: fr.sheet_idx,
                x: fr.x + pw,
                y: fr.y,
                w: lw,
                h: ph,
            });
        }
        if lh > 0 {
            out.push(FreeRect {
                sheet_idx: fr.sheet_idx,
                x: fr.x,
                y: fr.y + ph,
                w: fr.w,
                h: lh,
            });
        }
    } else {
        // right child: full height; bottom child: narrow (piece width)
        // ┌──────────┬──────────┐
        // │  piece   │          │
        // │  pw × ph │  right   │
        // ├──────────┤ lw×fr.h  │
        // │  bottom  │          │
        // │ pw × lh  │          │
        // └──────────┴──────────┘
        if lw > 0 {
            out.push(FreeRect {
                sheet_idx: fr.sheet_idx,
                x: fr.x + pw,
                y: fr.y,
                w: lw,
                h: fr.h,
            });
        }
        if lh > 0 {
            out.push(FreeRect {
                sheet_idx: fr.sheet_idx,
                x: fr.x,
                y: fr.y + ph,
                w: pw,
                h: lh,
            });
        }
    }
    out
}

pub(crate) fn sheet_rect(problem: &Problem, sheet_idx: usize) -> FreeRect {
    FreeRect {
        sheet_idx,
        x: 0,
        y: 0,
        w: problem.sheet.width,
        h: problem.sheet.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, parse::parse_problem};

    fn g(piece_id: usize, rotate: bool, point_selector: u32) -> Gene {
        Gene {
            piece_idx: piece_id,
            rotate,
            point_selector,
            inverse: false,
        }
    }

    #[test]
    fn decode_two_sheets() {
        // Sheet 200×150, kerf=5. Five pieces (each count=1), expected placements:
        //
        //  Sheet 0 (200×150)                Sheet 1 (200×150)
        // ┌─────────────┬──────────┬──┐    ┌────────────────────────────┐
        // │     P0      │    P1    │  │    │        P2  200×60          │
        // │   120×80    │  60×80   │  │    ├────────────┬───────────────┤
        // ├─────────────┼──────────┴──┤    │   P3(r)    │               │
        // │             │    P4(r)    │    │   100×70   │   free 95×85  │
        // │ free 120×65 │    70×60    │    ├────────────┤               │
        // │             ├─────────────┤    │ free 100×10│               │
        // └─────────────┴─────────────┘    └────────────┴───────────────┘
        // kerf = 5 between every pair of pieces
        // improve_tl_corners checks only the root (top-level) split per sheet.
        // Sheet 0 root is HSplit: tl(top)=P0 9600 > tl(bottom)=0 — no swap.
        // Sheet 1 root is HSplit: tl(top)=P2 12000 > tl(bottom)=P3r 7000 — no swap.
        let spec = parse_problem("200x150F:5:120x80,60x80,200x60,70x100r,60x70r").expect("Error parsing problem");
        let problem = expand_problem(&spec);
        let genome = vec![
            g(0, false, 0),
            g(1, false, 0),
            g(2, false, 0),
            g(3, true, 0),
            g(4, true, 2),
        ];
        let sol = decode(&problem, &genome);
        assert_eq!(sol.sheets_used(), 2);
        let p = &sol.placements;
        assert_eq!(p.len(), 5);
        let find = |idx: usize| p.iter().find(|pl| pl.piece_idx == idx).unwrap();
        let tuple = |pl: &Placement| (pl.sheet_idx, pl.x, pl.y, pl.rotated);
        assert_eq!(tuple(find(0)), (0, 0, 0, false)); // P0  at (0,0) sheet 0
        assert_eq!(tuple(find(1)), (0, 125, 0, false)); // P1  at (125,0) sheet 0
        assert_eq!(tuple(find(2)), (1, 0, 0, false)); // P2  at (0,0) sheet 1
        assert_eq!(tuple(find(3)), (1, 0, 65, true)); // P3r at (0,65) sheet 1
        assert_eq!(tuple(find(4)), (0, 125, 85, true)); // P4r at (125,85) sheet 0
    }
}
