/// External label attached to a piece — used for display and round-tripping with
/// the caller's numbering scheme (e.g. an ERP part number).  The value carries no
/// structural meaning inside the solver: it need not be unique, need not be
/// contiguous, and is never used as an internal key.  Pieces are addressed
/// internally by their 0-based index in `Problem::pieces`.
pub type PieceId = u32;

/// Stock sheet — all sheets in the problem are identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sheet {
    pub width: u32,
    pub height: u32,
}

/// A rectangular piece to be cut from a sheet.
/// `can_rotate`: when false, must be placed in original (width × height) orientation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub id: PieceId,
    pub width: u32,
    pub height: u32,
    pub can_rotate: bool,
}

/// A cutting problem: identical sheets, pieces to place, and a blade kerf (mm)
/// subtracted at each guillotine split; boundary edges of the sheet are exempt.
#[derive(Debug, Clone)]
pub struct Problem {
    pub sheet: Sheet,
    pub kerf: u32,
    pub pieces: Vec<Piece>,
}

/// Position of a placed piece. `(x, y)` is the top-left corner of the piece on the sheet;
/// `rotated` means the piece was placed as (height × width) instead of (width × height).
/// `piece_idx` is the 0-based index into `Problem::pieces` — the unambiguous internal key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub sheet_idx: usize,
    pub piece_idx: usize,
    pub x: u32,
    pub y: u32,
    pub rotated: bool,
}

/// An unused rectangle remaining after all pieces are placed. `(x, y)` is the top-left corner;
/// `x` increases rightward, `y` increases downward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreeRect {
    pub sheet_idx: usize,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone)]
pub struct Solution {
    pub placements: Vec<Placement>,
    pub leftovers: Vec<FreeRect>,
}

impl Solution {
    pub fn sheets_used(&self) -> usize {
        self.placements
            .iter()
            .map(|p| p.sheet_idx)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }

    /// Two-level lexicographic fitness (lower is better):
    ///   1. minimize `sheets_used`
    ///   2. minimize piece area on the last sheet
    ///
    /// `objective = sheets_used * (sheet_area + 1) + piece_area_on_last_sheet`
    ///
    /// The `+ 1` ensures strict lexicographic priority: any k-sheet solution is better than
    /// any (k+1)-sheet solution, because `piece_area_on_last <= total_piece_area <= sheet_area`.
    pub fn objective(&self, problem: &Problem) -> i64 {
        if self.placements.is_empty() {
            return 0;
        }
        let sheet_area = problem.sheet.width as i64 * problem.sheet.height as i64;
        let last = self.sheets_used() - 1;
        let area_on_last: i64 = self
            .placements
            .iter()
            .filter(|pl| pl.sheet_idx == last)
            .map(|pl| {
                let p = &problem.pieces[pl.piece_idx];
                p.width as i64 * p.height as i64
            })
            .sum();
        self.sheets_used() as i64 * (sheet_area + 1) + area_on_last
    }
}
