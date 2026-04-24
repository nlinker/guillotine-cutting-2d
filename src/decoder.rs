use crate::model::{Piece, Placement, Problem, Solution};

/// One element of the solution genome (V-vector encoding).
/// `rotate`: when true and `piece.can_rotate`, try (height × width) orientation first.
/// `point_selector`: selects the starting free rect as `free[point_selector % |free|]`;
/// without it the decoder always starts from `free[0]`, so this gives the
/// algorithm extra freedom to steer pieces to different regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gene {
    pub piece_idx: usize,
    pub rotate: bool,
    pub point_selector: u32,
}

/// Ordered genome — one gene per piece, defining placement order, rotation
/// preference, and free-rect selection. `piece_idx` values must be a permutation
/// of `0..problem.pieces.len()`.
pub type Genome = Vec<Gene>;

/// An available rectangle on a sheet. `(x, y)` is the top-left corner;
/// `x` increases rightward, `y` increases downward. `(0, 0)` is the sheet's
/// top-left corner.
struct FreeRect {
    sheet_idx: usize,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

/// Decode a genome into placements using strict guillotine splitting (no merge).
///
/// Pieces are placed in genome order. `point_selector % |free|` picks the
/// preferred free rect; if the piece does not fit there the decoder scans the
/// rest of the list in order. A new sheet is opened when nothing fits.
///
/// Precondition: every piece fits on an empty sheet.
pub fn decode(problem: &Problem, genome: &Genome) -> Solution {
    let mut free: Vec<FreeRect> = vec![sheet_rect(problem, 0)];
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
                piece_id: piece.id,
                x: fr.x,
                y: fr.y,
                rotated,
            });
            free.extend(guillotine_split(&fr, pw, ph, problem.kerf));
        } else {
            debug_assert!(
                false,
                "piece {}×{} does not fit on empty {}×{} sheet",
                piece.width, piece.height, problem.sheet.width, problem.sheet.height
            );
        }
    }

    Solution { placements }
}

fn open_new_sheet(
    free: &mut Vec<FreeRect>,
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
fn find_placement(
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
fn fits_in(fr: &FreeRect, piece: &Piece, prefer_rotate: bool) -> Option<(u32, u32, bool)> {
    let try_rotated = prefer_rotate && piece.can_rotate;
    let (pw_a, ph_a) = if try_rotated {
        (piece.height, piece.width)
    } else {
        (piece.width, piece.height)
    };
    if pw_a <= fr.width && ph_a <= fr.height {
        return Some((pw_a, ph_a, try_rotated));
    }
    if piece.can_rotate {
        let (pw_b, ph_b) = (ph_a, pw_a); // opposite orientation
        if pw_b <= fr.width && ph_b <= fr.height {
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
/// The blade kerf is subtracted from each internal cut; boundary edges are exempt.
fn guillotine_split(fr: &FreeRect, pw: u32, ph: u32, kerf: u32) -> Vec<FreeRect> {
    debug_assert!(pw <= fr.width && ph <= fr.height);
    let lw = fr.width - pw; // right leftover before kerf
    let lh = fr.height - ph; // bottom leftover before kerf
    let mut out = Vec::with_capacity(2);
    if lw <= lh {
        // right child: narrow (piece height); bottom child: full width:
        // ┌──────────┬─────────────────┐
        // │  piece   │  right          │
        // │  pw × ph │  ph × (lw-kerf) │
        // ├──────────┴─────────────────┤ kerf
        // │        bottom              │
        // │  fr.width × (lh - kerf)    │
        // └────────────────────────────┘
        if lw > kerf {
            out.push(FreeRect {
                sheet_idx: fr.sheet_idx,
                x: fr.x + pw + kerf,
                y: fr.y,
                width: lw - kerf,
                height: ph,
            });
        }
        if lh > kerf {
            out.push(FreeRect {
                sheet_idx: fr.sheet_idx,
                x: fr.x,
                y: fr.y + ph + kerf,
                width: fr.width,
                height: lh - kerf,
            });
        }
    } else {
        // right child: full height, bottom child: narrow (piece width)
        // ┌──────────────────┬──────────────┐
        // │  piece           │              │
        // │  pw × ph         │    right     │
        // ├──────────────────┤  (lw - kerf) │
        // │     bottom       │ × fr.height  │
        // │ pw × (lh - kerf) │              │
        // └──────────────────┴──────────────┘
        //                   kerf
        if lw > kerf {
            out.push(FreeRect {
                sheet_idx: fr.sheet_idx,
                x: fr.x + pw + kerf,
                y: fr.y,
                width: lw - kerf,
                height: fr.height,
            });
        }
        if lh > kerf {
            out.push(FreeRect {
                sheet_idx: fr.sheet_idx,
                x: fr.x,
                y: fr.y + ph + kerf,
                width: pw,
                height: lh - kerf,
            });
        }
    }
    out
}

fn sheet_rect(problem: &Problem, sheet_idx: usize) -> FreeRect {
    FreeRect {
        sheet_idx,
        x: 0,
        y: 0,
        width: problem.sheet.width,
        height: problem.sheet.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Piece, Problem, Sheet};

    fn make_problem(sw: u32, sh: u32, kerf: u32, pieces: &[(u32, u32, bool)]) -> Problem {
        Problem {
            sheet: Sheet { width: sw, height: sh },
            kerf,
            pieces: pieces
                .iter()
                .enumerate()
                .map(|(i, &(w, h, cr))| Piece {
                    id: i as u32,
                    width: w,
                    height: h,
                    can_rotate: cr,
                })
                .collect(),
        }
    }

    fn g(piece_id: usize, rotate: bool, point_selector: u32) -> Gene {
        Gene {
            piece_idx: piece_id,
            rotate,
            point_selector,
        }
    }

    #[test]
    fn decode_two_sheets() {
        // Sheet 200×150, kerf=5. Five pieces, expected placements:
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

        let problem = make_problem(
            200,
            150,
            5,
            &[
                (120, 80, false), // P0
                (60, 80, false),  // P1
                (200, 60, false), // P2
                (70, 100, true),  // P3
                (60, 70, true),   // P4
            ],
        );
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
        assert_eq!((p[0].sheet_idx, p[0].x, p[0].y, p[0].rotated), (0, 0, 0, false));
        assert_eq!((p[1].sheet_idx, p[1].x, p[1].y, p[1].rotated), (0, 125, 0, false));
        assert_eq!((p[2].sheet_idx, p[2].x, p[2].y, p[2].rotated), (1, 0, 0, false));
        assert_eq!((p[3].sheet_idx, p[3].x, p[3].y, p[3].rotated), (1, 0, 65, true));
        assert_eq!((p[4].sheet_idx, p[4].x, p[4].y, p[4].rotated), (0, 125, 85, true));
    }
}
