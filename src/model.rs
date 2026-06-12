use serde::{Deserialize, Serialize};

use crate::cut_tree::build_cut_tree;

/// Three-level lexicographic fitness (lower is better overall).
///
///   0. `sheets_used`   — primary goal: minimize sheets
///   1. `layout_score`  — higher is better (reversed in `Ord`); rewards regular,
///      "technological" layouts: concentrated cut lines plus mono-width strips
///      (see `cut_line_concentration_score` and `strip_structure_score`)
///   2. `leftover_area` — area of the largest single leftover across all sheets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Objective {
    pub sheets_used: usize,
    pub layout_score: u64,
    pub leftover_area: u64,
}

impl PartialOrd for Objective {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Objective {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sheets_used
            .cmp(&other.sheets_used)
            .then(other.layout_score.cmp(&self.layout_score))
            .then(self.leftover_area.cmp(&other.leftover_area))
    }
}

impl Objective {
    /// Metric sent as `secondary_objective` in progress messages (web chart, Excel R3).
    /// Change the body here to switch what is displayed — no other files need updating.
    pub fn secondary(&self) -> u64 {
        self.layout_score
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

    /// Total guillotine cut length per sheet (mm).
    ///
    /// Pieces + leftovers tile the working area exactly (up to kerf rounding).
    /// Every internal boundary is shared by exactly two adjacent rectangles, so
    /// `total_cut_length = Σ(internal edge lengths) / 2`.
    /// "Internal" means the edge does not lie on the working-area boundary
    /// `[margin, sheet.width-margin] × [margin, sheet.height-margin]`.
    /// The working-area perimeter is added once per sheet when `margin > 0`.
    pub fn cut_lengths(&self, spec: &ProblemSpec) -> Vec<u64> {
        let n = self.sheets_used();
        if n == 0 {
            return vec![];
        }
        let m = spec.margin;
        let rw = spec.sheet.width - m;
        let bh = spec.sheet.height - m;
        let mut totals = vec![0u64; n];

        for pl in &self.placements {
            let ps = &spec.piespecs[pl.piespec_idx];
            let (pw, ph) = if pl.rotated {
                (ps.height, ps.width)
            } else {
                (ps.width, ps.height)
            };
            let si = pl.sheet_idx;
            if pl.x > m {
                totals[si] += ph as u64;
            }
            if pl.x + pw < rw {
                totals[si] += ph as u64;
            }
            if pl.y > m {
                totals[si] += pw as u64;
            }
            if pl.y + ph < bh {
                totals[si] += pw as u64;
            }
        }
        for lr in &self.leftovers {
            let si = lr.sheet_idx;
            if lr.x > m {
                totals[si] += lr.h as u64;
            }
            if lr.x + lr.w < rw {
                totals[si] += lr.h as u64;
            }
            if lr.y > m {
                totals[si] += lr.w as u64;
            }
            if lr.y + lr.h < bh {
                totals[si] += lr.w as u64;
            }
        }

        let margin_perim = if m > 0 {
            2 * ((spec.sheet.width - 2 * m) as u64 + (spec.sheet.height - 2 * m) as u64)
        } else {
            0
        };
        totals.into_iter().map(|t| t / 2 + margin_perim).collect()
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Weight of `strip_structure_score` relative to `cut_line_concentration_score` in
/// `layout_score`. Both are squared-length sums on the same `/10_000` scale, so 1 is
/// the natural starting point; calibrate on `generator` instances if needed.
const STRIP_WEIGHT: u64 = 1;

impl Solution {
    pub fn sheets_used(&self) -> usize {
        self.placements
            .iter()
            .map(|p| p.sheet_idx)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }

    pub fn eval(&self, problem: &Problem) -> Objective {
        if self.placements.is_empty() {
            return Objective {
                sheets_used: 0,
                leftover_area: 0,
                layout_score: 0,
            };
        }
        Objective {
            sheets_used: self.sheets_used(),
            leftover_area: self.leftover_area(),
            layout_score: self.cut_line_concentration_score(problem)
                + STRIP_WEIGHT * self.strip_structure_score(problem),
        }
    }

    /// Cut-line concentration score (higher = better) — rewards regular, "technological"
    /// layouts where internal cuts align into a few long, reusable lines rather than
    /// many short, misaligned ones (one fence setting → many pieces per pass).
    ///
    /// For each sheet: take every internal edge of every placed piece (edges on the
    /// sheet boundary are exempt, same as the kerf rule), group vertical edges by `x`
    /// and horizontal edges by `y`, merge overlapping/adjacent spans within each group
    /// into disjoint runs, and sum `length^2` per run. Squaring rewards concentrating
    /// a fixed amount of cut length into few long lines over spreading it across many
    /// short ones (Herfindahl-style). See `docs/plans/cut-line-concentration-score.md`.
    ///
    /// The raw sum can be very large (`O(sheet_dim^2)` per run), so it is divided by
    /// `100^2 = 10_000` and rounded to the nearest integer to keep values manageable.
    ///
    /// `O(n log n)` per sheet (sort + merge).
    pub fn cut_line_concentration_score(&self, problem: &Problem) -> u64 {
        let n_sheets = self.sheets_used();
        let sheet_w = problem.sheet.width;
        let sheet_h = problem.sheet.height;
        let mut total = 0u64;
        // (coordinate, span_lo, span_hi) triples, grouped by sorting rather than
        // hashing — this is the GA's hot path (called once per individual per
        // generation), and a sort over a couple dozen small tuples beats the
        // allocation/hashing overhead of a HashMap<u32, Vec<_>> per sheet.
        let mut v_edges: Vec<(u32, u32, u32)> = Vec::new();
        let mut h_edges: Vec<(u32, u32, u32)> = Vec::new();
        for sheet in 0..n_sheets {
            v_edges.clear();
            h_edges.clear();
            for pl in self.placements.iter().filter(|p| p.sheet_idx == sheet) {
                let (x, y, w, h) = placement_rect(pl, problem);
                if x > 0 {
                    v_edges.push((x, y, y + h));
                }
                if x + w < sheet_w {
                    v_edges.push((x + w, y, y + h));
                }
                if y > 0 {
                    h_edges.push((y, x, x + w));
                }
                if y + h < sheet_h {
                    h_edges.push((y + h, x, x + w));
                }
            }
            total += sum_squared_runs_by_coordinate(&mut v_edges);
            total += sum_squared_runs_by_coordinate(&mut h_edges);
        }
        (total + 5_000) / 10_000
    }

    /// Strip-structure score (higher = better) — rewards mono-width strips: stacks of
    /// pieces sharing the same `[x, x+w)` interval laid flush in `y` (and symmetric
    /// horizontal runs). Such a strip is ripped with one fence setting and then chopped
    /// to length, even when the pieces inside it differ. A k x m grid of identical
    /// pieces scores along both axes at once: `k*(m*h)^2 + m*(k*w)^2`.
    ///
    /// Sum of `run_length^2` over all flush runs in both orientations. Squaring is
    /// essential: every placement belongs to exactly one run per orientation, so the
    /// linear sum of run lengths is invariant for a fixed piece set and carries no
    /// signal; the superadditive square rewards consolidation (Herfindahl-style, same
    /// units and `/10_000` scale as `cut_line_concentration_score`).
    ///
    /// Kerf is baked into expanded piece dimensions, so "flush" is exact coordinate
    /// equality. See `docs/plans/12_strip-structure-score.md`.
    pub fn strip_structure_score(&self, problem: &Problem) -> u64 {
        let n_sheets = self.sheets_used();
        let mut total = 0u64;
        // Keyed by the strip's cross-axis interval; buffers reused across sheets
        // (GA hot path, same rationale as cut_line_concentration_score).
        let mut v_runs: Vec<((u32, u32), u32, u32)> = Vec::new();
        let mut h_runs: Vec<((u32, u32), u32, u32)> = Vec::new();
        for sheet in 0..n_sheets {
            v_runs.clear();
            h_runs.clear();
            for pl in self.placements.iter().filter(|p| p.sheet_idx == sheet) {
                let (x, y, w, h) = placement_rect(pl, problem);
                v_runs.push(((x, w), y, y + h));
                h_runs.push(((y, h), x, x + w));
            }
            total += sum_squared_runs_by_coordinate(&mut v_runs);
            total += sum_squared_runs_by_coordinate(&mut h_runs);
        }
        (total + 5_000) / 10_000
    }

    /// Area of the largest single leftover rectangle across all sheets (lower = better).
    pub fn leftover_area(&self) -> u64 {
        self.leftovers
            .iter()
            .map(|fr| fr.w as u64 * fr.h as u64)
            .max()
            .unwrap_or(0)
    }

    /// Inter-sheet spread penalty: sum over each canonical piece size of
    /// `(distinct_sheets_that_hold_that_size - 1)`.
    /// Zero when every piece size is confined to exactly one sheet.
    ///
    /// Canonical size normalises for rotation: key = `(min(w,h), max(w,h))`.
    pub fn sheet_spread_penalty(&self, problem: &Problem) -> u64 {
        if self.placements.is_empty() {
            return 0;
        }
        let mut pairs = self
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
            .collect::<Vec<_>>();
        pairs.sort_unstable();
        pairs.dedup();
        let total = pairs.len();
        let distinct_sizes = pairs.windows(2).filter(|w| w[0].0 != w[1].0).count() + 1;
        (total - distinct_sizes) as u64
    }

    /// Alias for `eval`. Kept for tests and benchmarks.
    pub fn objective(&self, problem: &Problem) -> Objective {
        self.eval(problem)
    }

    /// Maximum staircase-polygon area across all sheets (lower = better).
    ///
    /// For each sheet: build the Pareto-optimal set of bottom-right corners `(rx, ry)`
    /// of all placed pieces; integrate the resulting step function top-to-bottom.
    /// Returns the maximum of these per-sheet areas.
    ///
    /// Not part of the current `Objective`; see `docs/objective.md` for motivation.
    #[allow(dead_code)]
    pub fn staircase_area(&self, problem: &Problem) -> u64 {
        let n = self.sheets_used();
        if n == 0 {
            return 0;
        }
        let mut best = 0u64;
        for sheet in 0..n {
            let mut stairs: Vec<(u32, u32)> = Vec::new();
            for pl in self.placements.iter().filter(|p| p.sheet_idx == sheet) {
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
                continue;
            }
            stairs.sort_unstable_by_key(|&(_, y)| y);
            let mut prev_y = 0u32;
            let mut sheet_area = 0u64;
            for (x, y) in stairs {
                sheet_area += x as u64 * (y - prev_y) as u64;
                prev_y = y;
            }
            if sheet_area > best {
                best = sheet_area;
            }
        }
        best
    }
}

fn placement_rect(pl: &Placement, problem: &Problem) -> (u32, u32, u32, u32) {
    let p = &problem.pieces[pl.piece_idx];
    let (w, h) = if pl.rotated {
        (p.height, p.width)
    } else {
        (p.width, p.height)
    };
    (pl.x, pl.y, w, h)
}

/// `edges` are `(coordinate, span_lo, span_hi)` triples — e.g. all internal vertical
/// edges with their `x` coordinate and the `[y, y+h)` span they cover. Sorts by
/// `(coordinate, span_lo, span_hi)`. Then within each group of equal `coordinate` merges
/// overlapping/adjacent spans into disjoint runs (each run is one cut the saw can make
/// in a single pass) and sums `length^2` over all runs in all groups.
///
/// Generic over the grouping key: `cut_line_concentration_score` groups by a single
/// cut coordinate (u32), `strip_structure_score` by a cross-axis interval ((u32, u32)).
fn sum_squared_runs_by_coordinate<K: Copy + Ord>(edges: &mut [(K, u32, u32)]) -> u64 {
    edges.sort_unstable();
    let mut total = 0u64;
    let mut iter = edges.iter();
    let Some(&(mut coord, mut lo, mut hi)) = iter.next() else {
        return 0;
    };
    for &(c, s, e) in iter {
        if c != coord || s > hi {
            let len = (hi - lo) as u64;
            total += len * len;
            coord = c;
            lo = s;
            hi = e;
        } else {
            hi = hi.max(e);
        }
    }
    let len = (hi - lo) as u64;
    total += len * len;
    total
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

    // 2×2 grid of 50×50 pieces tiling a 100×100 sheet exactly (no leftovers, no margin).
    // Cuts: y=50 full-width (100) + x=50 in each strip (50+50) = 200.
    #[test]
    fn cut_lengths_no_margin() {
        let spec = ProblemSpec {
            sheet: Sheet {
                width: 100,
                height: 100,
            },
            kerf: 0,
            margin: 0,
            piespecs: vec![PieceSpec {
                name: String::new(),
                width: 50,
                height: 50,
                count: 4,
                can_rotate: false,
            }],
        };
        let sol = SolutionSpec {
            placements: vec![
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 50,
                    y: 0,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 0,
                    y: 50,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 50,
                    y: 50,
                    rotated: false,
                },
            ],
            leftovers: vec![],
        };
        assert_eq!(sol.cut_lengths(&spec), vec![200]);
    }

    // Same grid on a 120×120 sheet with margin=10.
    // Working area 100×100; pieces at margin-adjusted coords.
    // Inner cuts: 200; margin perimeter: 2*(100+100) = 400. Total: 600.
    #[test]
    fn cut_lengths_with_margin() {
        let spec = ProblemSpec {
            sheet: Sheet {
                width: 120,
                height: 120,
            },
            kerf: 0,
            margin: 10,
            piespecs: vec![PieceSpec {
                name: String::new(),
                width: 50,
                height: 50,
                count: 4,
                can_rotate: false,
            }],
        };
        let sol = SolutionSpec {
            placements: vec![
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 10,
                    y: 10,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 60,
                    y: 10,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 10,
                    y: 60,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 60,
                    y: 60,
                    rotated: false,
                },
            ],
            leftovers: vec![],
        };
        assert_eq!(sol.cut_lengths(&spec), vec![600]);
    }

    // Four 50×50 pieces in a 2×2 grid on a 120×120 sheet with kerf=10, no margin.
    // Expanded: each piece is 60×60 on a 130×130 sheet.
    // SLAS trace (inverse=false, point_selector=0):
    //   piece 0 → (0,0,130,130): lw=lh=70 → right=(60,0,70,60), bottom=(0,60,130,70)
    //   piece 1 → (60,0,70,60):  lw=10,lh=0, lw>lh → right=(120,0,10,60), no bottom
    //   piece 2 → (0,60,130,70): lw=70,lh=10, lw>lh → right=(60,60,70,70), bottom=(0,120,60,10)
    //   piece 3 → (60,60,70,70): lw=lh=10 → right=(120,60,10,60), bottom=(60,120,70,10)
    #[test]
    fn cut_lengths_with_kerf() {
        let spec = ProblemSpec {
            sheet: Sheet {
                width: 120,
                height: 120,
            },
            kerf: 10,
            margin: 0,
            piespecs: vec![PieceSpec {
                name: String::new(),
                width: 50,
                height: 50,
                count: 4,
                can_rotate: false,
            }],
        };
        let sol = SolutionSpec {
            placements: vec![
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 60,
                    y: 0,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 0,
                    y: 60,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 0,
                    piespec_idx: 0,
                    x: 60,
                    y: 60,
                    rotated: false,
                },
            ],
            leftovers: vec![
                FreeRect {
                    sheet_idx: 0,
                    x: 120,
                    y: 0,
                    w: 10,
                    h: 60,
                },
                FreeRect {
                    sheet_idx: 0,
                    x: 0,
                    y: 120,
                    w: 60,
                    h: 10,
                },
                FreeRect {
                    sheet_idx: 0,
                    x: 120,
                    y: 60,
                    w: 10,
                    h: 60,
                },
                FreeRect {
                    sheet_idx: 0,
                    x: 60,
                    y: 120,
                    w: 70,
                    h: 10,
                },
            ],
        };
        assert_eq!(sol.cut_lengths(&spec), vec![445]);
    }

    fn flat_problem(sheet_w: u32, sheet_h: u32, dims: &[(u32, u32)]) -> Problem {
        Problem {
            sheet: Sheet {
                width: sheet_w,
                height: sheet_h,
            },
            pieces: dims
                .iter()
                .map(|&(w, h)| Piece {
                    name: String::new(),
                    width: w,
                    height: h,
                    can_rotate: false,
                })
                .collect(),
        }
    }

    fn place(piece_idx: usize, x: u32, y: u32) -> Placement {
        Placement {
            sheet_idx: 0,
            piece_idx,
            x,
            y,
            rotated: false,
        }
    }

    // 2x2 grid of 400x400 pieces: 2 vertical runs of 800 + 2 horizontal runs of 800
    // = 4 * 800^2 = 2_560_000 -> /10^4 = 256. The same pieces scattered (no flush
    // contacts) give 8 singleton runs of 400 = 8 * 400^2 = 1_280_000 -> 128.
    #[test]
    fn strip_score_grid_beats_scattered() {
        let problem = flat_problem(1000, 1000, &[(400, 400); 4]);
        let grid = Solution {
            placements: vec![place(0, 0, 0), place(1, 400, 0), place(2, 0, 400), place(3, 400, 400)],
            leftovers: vec![],
        };
        let scattered = Solution {
            placements: vec![place(0, 0, 0), place(1, 600, 0), place(2, 0, 600), place(3, 600, 600)],
            leftovers: vec![],
        };
        assert_eq!(grid.strip_structure_score(&problem), 256);
        assert_eq!(scattered.strip_structure_score(&problem), 128);
    }

    // A column of equal-width pieces with different heights laid flush merges into one
    // vertical run (1000^2) + 3 horizontal singletons (3 * 400^2) = 1_480_000 -> 148.
    // The same pieces scattered keep separate vertical runs: 300^2 + 500^2 + 200^2
    // + the same horizontal singletons = 860_000 -> 86.
    #[test]
    fn strip_score_mono_width_mixed_heights() {
        let problem = flat_problem(1000, 1000, &[(400, 300), (400, 500), (400, 200)]);
        let flush = Solution {
            placements: vec![place(0, 0, 0), place(1, 0, 300), place(2, 0, 800)],
            leftovers: vec![],
        };
        let scattered = Solution {
            placements: vec![place(0, 0, 0), place(1, 500, 400), place(2, 50, 750)],
            leftovers: vec![],
        };
        assert_eq!(flush.strip_structure_score(&problem), 148);
        assert_eq!(scattered.strip_structure_score(&problem), 86);
    }

    // Stacked pieces of different widths share x but not the cross interval, so they
    // stay separate vertical runs: 300^2 + 500^2 + horizontal singletons 400^2 + 300^2
    // = 590_000 -> 59 (no merge bonus).
    #[test]
    fn strip_runs_require_equal_width() {
        let problem = flat_problem(1000, 1000, &[(400, 300), (300, 500)]);
        let stacked = Solution {
            placements: vec![place(0, 0, 0), place(1, 0, 300)],
            leftovers: vec![],
        };
        assert_eq!(stacked.strip_structure_score(&problem), 59);
    }
}
