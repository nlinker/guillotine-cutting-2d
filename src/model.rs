use serde::{Deserialize, Serialize};

/// Lexicographic objective value `(sheets_used, last_sheet_area)`. Lower is better.
pub type Objective = (usize, i64);

/// Stock sheet - all sheets in the problem are identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sheet {
    pub width: u32,
    pub height: u32,
}

// == Spec types (user-facing, type-indexed) ====================================

/// A piece type: N copies of a rectangle to be cut from stock sheets.
/// `name`: caller-supplied label (empty string when parsed from CLI format).
/// `can_rotate`: when false, must be placed in original (width × height) orientation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceSpec {
    #[serde(default)]
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub count: u32,
    pub can_rotate: bool,
}

/// A cutting problem as supplied by the user: piece types with counts.
/// `margin`: border subtracted from each edge; algorithm sees `(width - 2·margin) × (height - 2·margin)`;
/// output coordinates are shifted back by `+margin`. Defaults to 0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemSpec {
    pub sheet: Sheet,
    pub kerf: u32,
    #[serde(default)]
    pub margin: u32,
    pub pieces: Vec<PieceSpec>,
}

impl ProblemSpec {
    /// Canonicalize piece specs so `(width, height, can_rotate)` triples are unique.
    ///
    /// For rotateable pieces: normalize dimensions to `(min(w,h), max(w,h))`.
    /// Then merge entries with identical `(width, height, can_rotate)` by summing `count`.
    /// First-appearance order is preserved.
    pub fn normalize(&mut self) {
        for ps in &mut self.pieces {
            if ps.can_rotate && ps.width > ps.height {
                std::mem::swap(&mut ps.width, &mut ps.height);
            }
        }
        let mut seen: Vec<(u32, u32, bool)> = Vec::new();
        let mut merged: Vec<PieceSpec> = Vec::new();
        for ps in self.pieces.drain(..) {
            let key = (ps.width, ps.height, ps.can_rotate);
            if let Some(pos) = seen.iter().position(|&k| k == key) {
                merged[pos].count += ps.count;
            } else {
                seen.push(key);
                merged.push(ps);
            }
        }
        self.pieces = merged;
    }
}

/// Position of a placed piece in a type-indexed solution.
/// `piece_idx` is the 0-based index into `ProblemSpec::pieces`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementSpec {
    pub sheet_idx: usize,
    pub piece_idx: usize,
    pub x: u32,
    pub y: u32,
    pub rotated: bool,
}

/// A solution expressed in terms of the user-supplied piece types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionSpec {
    pub placements: Vec<PlacementSpec>,
    pub leftovers: Vec<FreeRect>,
}

impl SolutionSpec {
    pub fn sheets_used(&self) -> usize {
        self.placements
            .iter()
            .map(|p| p.sheet_idx)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }
}

// == Flat types (internal, flat-indexed) =======================================

/// A single physical piece instance in the flat expanded problem.
/// `can_rotate`: when false, must be placed in original (width × height) orientation.
/// `name`: carried from the originating `PieceSpec` for display purposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Piece {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub can_rotate: bool,
}

/// Flat cutting problem: one `Piece` entry per physical copy (no counts).
/// Produced by `expand::expand_problem`; consumed by the decoder and GA internals.
/// Dimensions are pre-expanded by `ProblemSpec::kerf` so decoder and GLF treat all cuts as kerf=0.
#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub sheet: Sheet,
    pub pieces: Vec<Piece>,
}

/// Position of a placed piece in a flat solution.
/// `piece_idx` is the 0-based index into `Problem::pieces` (flat).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub sheet_idx: usize,
    pub piece_idx: usize,
    pub x: u32,
    pub y: u32,
    pub rotated: bool,
}

/// An unused rectangle remaining after all pieces are placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeRect {
    pub sheet_idx: usize,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Flat solution: one placement per physical piece.
/// Produced by `decoder::decode`; convert to `SolutionSpec` via `expand::shrink_solution`.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    ///   2. minimize staircase area on the last sheet
    ///
    /// Returns `(sheets_used, staircase_area_last_sheet)`. Rust tuple `Ord` provides
    /// lexicographic comparison for free: any k-sheet solution is strictly better than
    /// any (k+1)-sheet solution regardless of `staircase_area_last_sheet`.
    pub fn objective(&self, problem: &Problem) -> Objective {
        if self.placements.is_empty() {
            return (0, 0);
        }
        (self.sheets_used(), self.staircase_area_last_sheet(problem) as i64)
    }

    /// Area of the staircase polygon from (0,0) bounding all pieces on the last sheet.
    ///
    /// Builds the Pareto-optimal set of bottom-right corners (rx, ry): a corner is kept
    /// iff no other corner has both x ≥ rx and y ≥ ry. The resulting step function is
    /// integrated top-to-bottom to give the enclosed area.
    pub fn staircase_area_last_sheet(&self, problem: &Problem) -> u64 {
        if self.placements.is_empty() {
            return 0;
        }
        let last = self.sheets_used() - 1;

        // Build Pareto-optimal set of bottom-right corners.
        let mut stairs: Vec<(u32, u32)> = Vec::new();
        for pl in self.placements.iter().filter(|p| p.sheet_idx == last) {
            let piece = &problem.pieces[pl.piece_idx];
            let (pw, ph) = if pl.rotated {
                (piece.height, piece.width)
            } else {
                (piece.width, piece.height)
            };
            let rx = pl.x + pw;
            let ry = pl.y + ph;

            if stairs.iter().any(|&(x, y)| x >= rx && y >= ry) {
                continue;
            }
            stairs.retain(|&(x, y)| !(x <= rx && y <= ry));
            stairs.push((rx, ry));
        }

        if stairs.is_empty() {
            return 0;
        }

        // Sort by y ascending → x descending (staircase top-to-bottom, widest first).
        stairs.sort_unstable_by_key(|&(_, y)| y);

        let mut area = 0u64;
        let mut prev_y = 0u32;
        for (x, y) in stairs {
            area += x as u64 * (y - prev_y) as u64;
            prev_y = y;
        }
        area
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(w: u32, h: u32) -> Piece {
        Piece {
            name: String::new(),
            width: w,
            height: h,
            can_rotate: false,
        }
    }

    fn pl(piece_idx: usize, x: u32, y: u32) -> Placement {
        Placement {
            sheet_idx: 0,
            piece_idx,
            x,
            y,
            rotated: false,
        }
    }

    fn problem(pieces: Vec<Piece>) -> Problem {
        Problem {
            sheet: Sheet {
                width: 1000,
                height: 1000,
            },
            pieces,
        }
    }

    /// Comprehensive staircase test covering all code paths.
    ///
    /// Layout (based on c38/c39 with btm split in two):
    ///   idx 0: btm_right (200, 0) 200×50  -> corner (400, 50) - added first
    ///   idx 1: btm_left  (  0, 0) 200×50  -> corner (200, 50) - dominated by btm_right: any-SKIP
    ///   idx 2: shelf     (400, 0) 200×100 -> corner (600,100) - retain removes (400,50)
    ///   idx 3: panel     (0, 100) 500×100 -> corner (500,200) - neither dominates shelf
    ///
    /// Final stairs sorted by y: [(600,100), (500,200)]
    /// Area = 600×100 + 500×100 = 110_000
    #[test]
    fn staircase_comprehensive() {
        let prob = problem(vec![
            piece(200, 50),  // 0: низ_right
            piece(200, 50),  // 1: низ_left  (same dims, different placement)
            piece(200, 100), // 2: полка
            piece(500, 100), // 3: стойка
        ]);
        let sol = Solution {
            placements: vec![
                pl(0, 200, 0), // низ_right: corner (400,  50) — added
                pl(1, 0, 0),   // низ_left:  corner (200,  50) — any-skip (400≥200 && 50≥50)
                pl(2, 400, 0), // полка:     corner (600, 100) — retain removes (400,50)
                pl(3, 0, 100), // стойка:    corner (500, 200)
            ],
            leftovers: vec![],
        };
        assert_eq!(sol.staircase_area_last_sheet(&prob), 600 * 100 + 500 * 100);
    }
}
