use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::cut_tree::build_cut_tree;

/// Four-level lexicographic fitness (lower is better):
///   0. `sheets_used`
///   1. primary grouping criterion   (order determined by `CriteriaOrder`)
///   2. secondary grouping criterion (the other one)
///   3. `staircase_area_last_sheet`
pub type Objective = (usize, u64, u64, u64);

/// Order of the two grouping criteria inside `Objective`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CriteriaOrder {
    /// `(sheets, bbox_grouping_penalty, sheet_spread_penalty, staircase)` — default.
    /// Minimises within-sheet scatter first, then cross-sheet spread.
    #[default]
    BboxFirst,
    /// `(sheets, sheet_spread_penalty, bbox_grouping_penalty, staircase)`.
    /// Keeps same-type pieces on one sheet first, then compacts within each sheet.
    SpreadFirst,
}

impl fmt::Display for CriteriaOrder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BboxFirst => f.write_str("bbox-first"),
            Self::SpreadFirst => f.write_str("spread-first"),
        }
    }
}

impl FromStr for CriteriaOrder {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bbox-first" => Ok(Self::BboxFirst),
            "spread-first" => Ok(Self::SpreadFirst),
            _ => Err(format!("unknown criteria order '{s}'; expected bbox-first or spread-first")),
        }
    }
}

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
    pub piespecs: Vec<PieceSpec>,
}

impl ProblemSpec {
    /// Canonicalize piece specs so `(width, height, can_rotate)` triples are unique.
    ///
    /// For rotatable pieces: normalize dimensions to `(min(w,h), max(w,h))`.
    /// Then merge entries with identical `(width, height, can_rotate)` by summing `count`.
    /// First-appearance order is preserved.
    pub fn normalize(&mut self) {
        for ps in &mut self.piespecs {
            if ps.can_rotate && ps.width > ps.height {
                std::mem::swap(&mut ps.width, &mut ps.height);
            }
        }
        let mut seen: Vec<(u32, u32, bool)> = Vec::new();
        let mut merged: Vec<PieceSpec> = Vec::new();
        for ps in self.piespecs.drain(..) {
            let key = (ps.width, ps.height, ps.can_rotate);
            if let Some(pos) = seen.iter().position(|&k| k == key) {
                merged[pos].count += ps.count;
            } else {
                seen.push(key);
                merged.push(ps);
            }
        }
        self.piespecs = merged;
    }
}

/// Position of a placed piece in a type-indexed solution.
/// `piespec_idx` is the 0-based index into `ProblemSpec::piespecs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementSpec {
    pub sheet_idx: usize,
    pub piespec_idx: usize,
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

    /// Evaluates the objective under the given `CriteriaOrder`.
    /// Rust tuple `Ord` provides lexicographic comparison for free.
    pub fn eval(&self, problem: &Problem, order: CriteriaOrder) -> Objective {
        if self.placements.is_empty() {
            return (0, 0, 0, 0);
        }
        let sheets = self.sheets_used();
        let bbox = self.bbox_grouping_penalty(problem);
        let spread = self.sheet_spread_penalty(problem);
        let staircase = self.staircase_area_last_sheet(problem);
        match order {
            CriteriaOrder::BboxFirst => (sheets, bbox, spread, staircase),
            CriteriaOrder::SpreadFirst => (sheets, spread, bbox, staircase),
        }
    }

    /// Canonical objective with `CriteriaOrder::BboxFirst`. Kept for tests and benchmarks.
    pub fn objective(&self, problem: &Problem) -> Objective {
        self.eval(problem, CriteriaOrder::BboxFirst)
    }

    /// Inter-sheet spread penalty: sum over each canonical piece size of
    /// `(distinct_sheets_that_hold_that_size - 1)`.
    /// Zero when every piece size is confined to exactly one sheet.
    ///
    /// Canonical size normalises for rotation: key = `(min(pw,ph), max(pw,ph))`.
    pub fn sheet_spread_penalty(&self, problem: &Problem) -> u64 {
        if self.placements.is_empty() {
            return 0;
        }
        let mut pairs: Vec<((u32, u32), usize)> = self
            .placements
            .iter()
            .map(|p| {
                let piece = &problem.pieces[p.piece_idx];
                let (pw, ph) = if p.rotated {
                    (piece.height, piece.width)
                } else {
                    (piece.width, piece.height)
                };
                ((pw.min(ph), pw.max(ph)), p.sheet_idx)
            })
            .collect();
        pairs.sort_unstable();
        pairs.dedup(); // now each (canonical_size, sheet_idx) pair is unique
        // penalty = Σ (distinct_sheets_per_size - 1) = total_pairs - distinct_sizes
        let total = pairs.len();
        let distinct_sizes = pairs.windows(2).filter(|w| w[0].0 != w[1].0).count() + 1;
        (total - distinct_sizes) as u64
    }

    /// Sort placements by `(sheet_idx, pw, ph)` so that `bbox_grouping_penalty` can
    /// do a zero-alloc linear scan. Called by `decode()` after building the solution.
    pub fn sort_placements(&mut self, problem: &Problem) {
        self.placements.sort_unstable_by_key(|p| {
            let piece = &problem.pieces[p.piece_idx];
            let (pw, ph) = if p.rotated {
                (piece.height, piece.width)
            } else {
                (piece.width, piece.height)
            };
            (p.sheet_idx, pw, ph)
        });
    }

    /// Spatial grouping penalty: sum over groups `(sheet_idx, pw, ph)` of
    /// `bbox_area - sum_piece_area`. Zero when all same-size pieces on a sheet are
    /// clustered together; large when they are scattered.
    ///
    /// Requires `placements` sorted by `(sheet_idx, pw, ph)` — call `sort_placements`
    /// first. On unsorted input the result is undefined (not a panic).
    pub fn bbox_grouping_penalty(&self, problem: &Problem) -> u64 {
        let group_key = |p: &Placement| -> (usize, u32, u32) {
            let piece = &problem.pieces[p.piece_idx];
            let (pw, ph) = if p.rotated {
                (piece.height, piece.width)
            } else {
                (piece.width, piece.height)
            };
            (p.sheet_idx, pw, ph)
        };
        debug_assert!(
            self.placements.windows(2).all(|w| group_key(&w[0]) <= group_key(&w[1])),
            "placements must be sorted by (sheet_idx, pw, ph) before calling bbox_grouping_penalty"
        );
        let mut penalty = 0u64;
        let mut start = 0;
        let n = self.placements.len();
        while start < n {
            let pl0 = &self.placements[start];
            let (pw0, ph0, _) = {
                let (a, b, c) = group_key(pl0);
                (b, c, a)
            };
            let key0 = group_key(pl0);
            let mut min_x = pl0.x;
            let mut min_y = pl0.y;
            let mut max_x = pl0.x + pw0;
            let mut max_y = pl0.y + ph0;
            let mut area = pw0 as u64 * ph0 as u64;
            let mut end = start + 1;
            while end < n {
                let key = group_key(&self.placements[end]);
                if key != key0 {
                    break;
                }
                let pl = &self.placements[end];
                let pw = key.1;
                let ph = key.2;
                min_x = min_x.min(pl.x);
                min_y = min_y.min(pl.y);
                max_x = max_x.max(pl.x + pw);
                max_y = max_y.max(pl.y + ph);
                area += pw as u64 * ph as u64;
                end += 1;
            }
            penalty += (max_x - min_x) as u64 * (max_y - min_y) as u64 - area;
            start = end;
        }
        penalty
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

        // Sort by y ascending -> x descending (staircase top-to-bottom, widest first).
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

/// Check that no piece exceeds the sheet and no two pieces on the same sheet overlap
/// and the solution is guillotine-constrained.
pub fn validate_solution(problem: &Problem, sol: &Solution) -> Vec<String> {
    let sw = problem.sheet.width;
    let sh = problem.sheet.height;
    let mut errors = Vec::new();
    for (i, pl) in sol.placements.iter().enumerate() {
        let piece = &problem.pieces[pl.piece_idx];
        let (pw, ph) = if pl.rotated {
            (piece.height, piece.width)
        } else {
            (piece.width, piece.height)
        };
        if pl.x + pw > sw || pl.y + ph > sh {
            errors.push(format!(
                "piece[{i}] at ({},{}) {}×{} exceeds sheet {}×{}",
                pl.x, pl.y, pw, ph, sw, sh,
            ));
        }
        for (j, other) in sol.placements[..i].iter().enumerate() {
            if other.sheet_idx != pl.sheet_idx {
                continue;
            }
            let op = &problem.pieces[other.piece_idx];
            let (ow, oh) = if other.rotated {
                (op.height, op.width)
            } else {
                (op.width, op.height)
            };
            let no_overlap =
                pl.x + pw <= other.x || other.x + ow <= pl.x || pl.y + ph <= other.y || other.y + oh <= pl.y;
            if !no_overlap {
                errors.push(format!(
                    "piece[{i}] ({},{} {}×{}) overlaps piece[{j}] ({},{} {}×{}) on sheet {}",
                    pl.x, pl.y, pw, ph, other.x, other.y, ow, oh, pl.sheet_idx,
                ));
            }
        }
    }
    // Guillotine validity.
    if let Err(e) = build_cut_tree(problem, &sol.placements) {
        errors.push(format!("not guillotine: {e}"));
    }
    errors
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
