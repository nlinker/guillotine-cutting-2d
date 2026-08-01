use serde::{Deserialize, Serialize};

use crate::cut_tree::build_cut_tree;

/// Three-level lexicographic fitness (`Ord` impl is the key); see `docs/objective.md`.
#[derive(Debug, Clone, Copy)]
pub struct Objective {
    pub sheets_used: f64,
    pub layout_score: u64,
    pub drop_consolidation_score: u64,
}

impl Objective {
    /// Integer sheet count recovered from the float encoding.
    pub fn sheets_used_int(&self) -> usize {
        if self.sheets_used <= 0.0 {
            0
        } else {
            self.sheets_used as usize + 1
        }
    }

    /// Metric sent as `secondary_objective` in progress messages (web chart, Excel R3).
    pub fn secondary(&self) -> u64 {
        self.layout_score
    }
}

impl PartialEq for Objective {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}
impl Eq for Objective {}

impl PartialOrd for Objective {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Objective {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.sheets_used
            .total_cmp(&other.sheets_used)
            .then(other.layout_score.cmp(&self.layout_score))
            .then(other.drop_consolidation_score.cmp(&self.drop_consolidation_score))
    }
}

/// Stock sheet - all sheets in the problem are identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sheet {
    pub width: u32,
    pub height: u32,
}

/// A piece type: N copies of a rectangle to be cut from stock sheets.
/// `name`: caller-supplied label (empty string when parsed from CLI format).
/// `can_rotate`: when false, must be placed in original (width × height) orientation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PieceType {
    #[serde(default)]
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub count: u32,
    pub can_rotate: bool,
}

/// A cutting problem as supplied by the user: piece types with counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemSpec {
    pub sheet: Sheet,
    #[serde(default)]
    pub kerf: u32,
    #[serde(default)]
    pub margin: u32,
    pub piece_types: Vec<PieceType>,
}

impl ProblemSpec {
    /// Canonicalize piece specs so `(width, height, can_rotate)` triples are unique.
    ///
    /// For rotatable pieces: normalize dimensions to `(min(w,h), max(w,h))`.
    /// Then merge entries with identical `(width, height, can_rotate)` by summing `count`.
    /// Original order from the `piece_types` is preserved.
    pub fn normalize(&mut self) {
        for ps in &mut self.piece_types {
            if ps.can_rotate && ps.width > ps.height {
                std::mem::swap(&mut ps.width, &mut ps.height);
            }
        }
        let mut seen: Vec<(u32, u32, bool)> = Vec::new();
        let mut merged: Vec<PieceType> = Vec::new();
        for ps in self.piece_types.drain(..) {
            let key = (ps.width, ps.height, ps.can_rotate);
            if let Some(pos) = seen.iter().position(|&k| k == key) {
                merged[pos].count += ps.count;
            } else {
                seen.push(key);
                merged.push(ps);
            }
        }
        self.piece_types = merged;
    }
}

/// Position of a placed piece in a type-indexed solution.
/// `ptype_idx` is the 0-based index into `ProblemSpec::piece_types`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacementSpec {
    pub sheet_idx: usize,
    pub ptype_idx: usize,
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

    /// Total guillotine cut length per sheet - each shared internal edge of a
    /// piece/leftover counted once, plus the working-area perimeter when `margin > 0`.
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
            let ps = &spec.piece_types[pl.ptype_idx];
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

/// A single physical piece instance in the flat expanded problem.
/// `can_rotate`: when false, must be placed in original (width × height) orientation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Piece {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub can_rotate: bool,
}

/// Flat cutting problem: one `Piece` entry per physical copy (no counts).
/// Produced by `expand::expand_problem`; consumed by the decoders, GA,
/// heuristics and the GLF solver.
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

/// `layout_score` weights, as the integer ratio `STRIP_WEIGHT / CUT_LINE_WEIGHT`
/// (exact, since scaling by a constant doesn't change `Ord`). Calibrated on `generator` instances.
const CUT_LINE_WEIGHT: u64 = 2;
const STRIP_WEIGHT: u64 = 3;

impl Solution {
    pub fn sheets_used(&self) -> usize {
        self.placements
            .iter()
            .map(|p| p.sheet_idx)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }

    /// Buckets `placements` by `sheet_idx` in one `O(p)` pass (vs. `O(p * sheets_used)`
    /// for per-sheet scorers re-filtering the whole list each time).
    fn placements_by_sheet(&self, n_sheets: usize) -> Vec<Vec<&Placement>> {
        let mut buckets: Vec<Vec<&Placement>> = vec![Vec::new(); n_sheets];
        for pl in &self.placements {
            buckets[pl.sheet_idx].push(pl);
        }
        buckets
    }

    pub fn eval(&self, problem: &Problem) -> Objective {
        let n = self.sheets_used();
        if n == 0 {
            return Objective { sheets_used: 0.0, drop_consolidation_score: 0, layout_score: 0 };
        }
        let last_idx = n - 1;
        let piece_area_last: u64 = self
            .placements
            .iter()
            .filter(|p| p.sheet_idx == last_idx)
            .map(|p| {
                let pc = &problem.pieces[p.piece_idx];
                pc.width as u64 * pc.height as u64
            })
            .sum();
        let sheet_area = problem.sheet.width as u64 * problem.sheet.height as u64;
        let fill = piece_area_last as f64 / sheet_area as f64;
        Objective {
            sheets_used: (n - 1) as f64 + fill,
            drop_consolidation_score: self.drop_consolidation_score(problem),
            layout_score: CUT_LINE_WEIGHT * self.cut_line_concentration_score(problem)
                + STRIP_WEIGHT * self.strip_structure_score(problem),
        }
    }

    /// Cut-line concentration score (higher = better) - rewards regular, "technological"
    /// layouts where internal cuts align into a few long, reusable lines rather than
    /// many short, misaligned ones (one fence setting → many pieces per pass).
    /// The raw sum can be very large (`O(sheet_dim^2)` per run), so it is divided by
    /// `100^2 = 10_000` and rounded to the nearest integer to keep values manageable.
    ///
    /// `O(n log n)` per sheet (sort + merge).
    ///
    /// See `docs/cut-line-concentration-score.md`.
    pub fn cut_line_concentration_score(&self, problem: &Problem) -> u64 {
        let n_sheets = self.sheets_used();
        let sheet_w = problem.sheet.width;
        let sheet_h = problem.sheet.height;
        let mut total = 0u64;
        // (coordinate, span_lo, span_hi) triples, grouped by sorting rather than
        // hashing - this is the GA's hot path (called once per individual per
        // generation), and a sort over a couple dozen small tuples beats the
        // allocation/hashing overhead of a HashMap<u32, Vec<_>> per sheet.
        let mut v_edges: Vec<(u32, u32, u32)> = Vec::new();
        let mut h_edges: Vec<(u32, u32, u32)> = Vec::new();
        for sheet_placements in self.placements_by_sheet(n_sheets) {
            v_edges.clear();
            h_edges.clear();
            for pl in sheet_placements {
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

    /// Strip-structure score (higher = better) - rewards mono-width strips: stacks of
    /// pieces sharing the same `[x, x+w)` interval laid flush in `y` (and symmetric
    /// horizontal runs). The raw sum can be very large (`O(sheet_dim^2)` per run), so
    /// it is divided by `100^2 = 10_000` and rounded to the nearest integer (same
    /// units and scale as `cut_line_concentration_score`).
    ///
    /// `O(n log n)` per sheet (sort + merge).
    ///
    /// See `docs/strip_structure_score.md`.
    pub fn strip_structure_score(&self, problem: &Problem) -> u64 {
        let n_sheets = self.sheets_used();
        let mut total = 0u64;
        // Keyed by the strip's cross-axis interval; buffers reused across sheets
        // (GA hot path, same rationale as cut_line_concentration_score).
        let mut v_runs: Vec<((u32, u32), u32, u32)> = Vec::new();
        let mut h_runs: Vec<((u32, u32), u32, u32)> = Vec::new();
        for sheet_placements in self.placements_by_sheet(n_sheets) {
            v_runs.clear();
            h_runs.clear();
            for pl in sheet_placements {
                let (x, y, w, h) = placement_rect(pl, problem);
                v_runs.push(((x, w), y, y + h));
                h_runs.push(((y, h), x, x + w));
            }
            total += sum_squared_runs_by_coordinate(&mut v_runs);
            total += sum_squared_runs_by_coordinate(&mut h_runs);
        }
        (total + 5_000) / 10_000
    }

    /// Drop-consolidation score (higher = better) - rewards a few large, reusable
    /// offcuts over many small scattered scraps. Computed for the **last sheet only**
    /// (`sheets_used() - 1`, re-derived from `placements`, not summed across the
    /// solution); sum of `area^2` over that sheet's free rectangles.
    ///
    /// `O(p^2)` in the number of placements on the last sheet.
    ///
    /// See `docs/drop_consolidation_score.md`.
    pub fn drop_consolidation_score(&self, problem: &Problem) -> u64 {
        let n_sheets = self.sheets_used();
        if n_sheets == 0 {
            return 0;
        }
        let last_sheet = n_sheets - 1;
        let sheet_w = problem.sheet.width;
        let sheet_h = problem.sheet.height;
        let mut total = 0u64;

        let rects: Vec<(u32, u32, u32, u32)> = self
            .placements
            .iter()
            .filter(|p| p.sheet_idx == last_sheet)
            .map(|pl| placement_rect(pl, problem))
            .collect();

        let mut ys: Vec<u32> = Vec::new();
        ys.push(0);
        ys.push(sheet_h);
        for &(_, y, _, h) in &rects {
            ys.push(y.min(sheet_h));
            ys.push((y + h).min(sheet_h));
        }
        ys.sort_unstable();
        ys.dedup();

        let mut occupied: Vec<(u32, u32)> = Vec::new();
        for window in ys.windows(2) {
            let (y_lo, y_hi) = (window[0], window[1]);
            if y_hi == y_lo {
                continue;
            }
            let band_h = (y_hi - y_lo) as u64;

            occupied.clear();
            occupied.extend(
                rects
                    .iter()
                    .filter(|&&(_, y, _, h)| y <= y_lo && y + h >= y_hi)
                    .map(|&(x, _, w, _)| (x, (x + w).min(sheet_w))),
            );
            occupied.sort_unstable();

            let mut cursor = 0u32;
            for &(start, end) in &occupied {
                if start > cursor {
                    let area = (start - cursor) as u64 * band_h;
                    total += area * area;
                }
                cursor = cursor.max(end);
            }
            if sheet_w > cursor {
                let area = (sheet_w - cursor) as u64 * band_h;
                total += area * area;
            }
        }
        total
    }

    /// Inter-sheet spread penalty: sum over each canonical piece size of
    /// `(distinct_sheets_that_hold_that_size - 1)`.
    /// Zero when every piece size is confined to exactly one sheet.
    ///
    /// Canonical size normalizes for rotation: key = `(min(w,h), max(w,h))`.
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

    /// Maximum staircase-polygon area across all sheets (lower = better). Not part
    /// of the current `Objective`; see `docs/objective.md`.
    #[allow(dead_code)]
    pub fn staircase_area(&self, problem: &Problem) -> u64 {
        let n = self.sheets_used();
        if n == 0 {
            return 0;
        }
        let mut best = 0u64;
        for sheet_placements in self.placements_by_sheet(n) {
            let mut stairs: Vec<(u32, u32)> = Vec::new();
            for pl in sheet_placements {
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

/// Groups `(key, span_lo, span_hi)` edges by `key`, merges overlapping/adjacent spans into runs,
/// sums `run_length^2`.
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

    // 2x2 grid of 50x50 pieces, placed at (ox+i*step, oy+j*step) for i,j in 0..2.
    fn grid_2x2(ox: u32, oy: u32, step: u32) -> Vec<PlacementSpec> {
        [(ox, oy), (ox + step, oy), (ox, oy + step), (ox + step, oy + step)]
            .map(|(x, y)| PlacementSpec { sheet_idx: 0, ptype_idx: 0, x, y, rotated: false })
            .into()
    }

    fn spec_50x50x4(sheet_w: u32, sheet_h: u32, kerf: u32, margin: u32) -> ProblemSpec {
        ProblemSpec {
            sheet: Sheet { width: sheet_w, height: sheet_h },
            kerf,
            margin,
            piece_types: vec![PieceType { name: String::new(), width: 50, height: 50, count: 4, can_rotate: false }],
        }
    }

    #[test]
    fn cut_lengths_calculation() {
        // No margin, no kerf: 100x100 sheet, pieces flush at (0,0)/(50,0)/(0,50)/(50,50).
        let spec = spec_50x50x4(100, 100, 0, 0);
        let sol = SolutionSpec { placements: grid_2x2(0, 0, 50), leftovers: vec![] };
        assert_eq!(sol.cut_lengths(&spec), vec![200]);

        // Same grid on a 120x120 sheet with margin=10. Working area 100x100; pieces
        // at margin-adjusted coords. Inner cuts: 200; margin perimeter:
        // 2*(100+100) = 400. Total: 600.
        let spec = spec_50x50x4(120, 120, 0, 10);
        let sol = SolutionSpec { placements: grid_2x2(10, 10, 50), leftovers: vec![] };
        assert_eq!(sol.cut_lengths(&spec), vec![600]);

        // Same grid on a 120x120 sheet with kerf=10, no margin. Expanded: each piece
        // is 60x60 on a 130x130 sheet. SLAS trace (inverse=false, point_selector=0):
        //   piece 0 -> (0,0,130,130): lw=lh=70 -> right=(60,0,70,60), bottom=(0,60,130,70)
        //   piece 1 -> (60,0,70,60):  lw=10,lh=0, lw>lh -> right=(120,0,10,60), no bottom
        //   piece 2 -> (0,60,130,70): lw=70,lh=10, lw>lh -> right=(60,60,70,70), bottom=(0,120,60,10)
        //   piece 3 -> (60,60,70,70): lw=lh=10 -> right=(120,60,10,60), bottom=(60,120,70,10)
        let spec = spec_50x50x4(120, 120, 10, 0);
        let sol = SolutionSpec {
            placements: grid_2x2(0, 0, 60),
            leftovers: vec![
                FreeRect { sheet_idx: 0, x: 120, y: 0, w: 10, h: 60 },
                FreeRect { sheet_idx: 0, x: 0, y: 120, w: 60, h: 10 },
                FreeRect { sheet_idx: 0, x: 120, y: 60, w: 10, h: 60 },
                FreeRect { sheet_idx: 0, x: 60, y: 120, w: 70, h: 10 },
            ],
        };
        assert_eq!(sol.cut_lengths(&spec), vec![445]);
    }

    fn flat_problem(sheet_w: u32, sheet_h: u32, dims: &[(u32, u32)]) -> Problem {
        Problem {
            sheet: Sheet { width: sheet_w, height: sheet_h },
            pieces: dims
                .iter()
                .map(|&(w, h)| Piece { name: String::new(), width: w, height: h, can_rotate: false })
                .collect(),
        }
    }

    fn place(piece_idx: usize, x: u32, y: u32) -> Placement {
        Placement { sheet_idx: 0, piece_idx, x, y, rotated: false }
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
        let flush =
            Solution { placements: vec![place(0, 0, 0), place(1, 0, 300), place(2, 0, 800)], leftovers: vec![] };
        let scattered =
            Solution { placements: vec![place(0, 0, 0), place(1, 500, 400), place(2, 50, 750)], leftovers: vec![] };
        assert_eq!(flush.strip_structure_score(&problem), 148);
        assert_eq!(scattered.strip_structure_score(&problem), 86);
    }

    // Stacked pieces of different widths share x but not the cross interval, so they
    // stay separate vertical runs: 300^2 + 500^2 + horizontal singletons 400^2 + 300^2
    // = 590_000 -> 59 (no merge bonus).
    #[test]
    fn strip_runs_require_equal_width() {
        let problem = flat_problem(1000, 1000, &[(400, 300), (300, 500)]);
        let stacked = Solution { placements: vec![place(0, 0, 0), place(1, 0, 300)], leftovers: vec![] };
        assert_eq!(stacked.strip_structure_score(&problem), 59);
    }

    #[test]
    fn full_sheet_placement_yields_zero_drop_score() {
        let problem = flat_problem(10, 10, &[(10, 10)]);
        let sol = Solution { placements: vec![place(0, 0, 0)], leftovers: vec![] };
        assert_eq!(sol.drop_consolidation_score(&problem), 0);
    }

    // 10x10 sheet, 4x4 piece in the top-left corner. Free region is an L-shape.
    // Horizontal-strip partition (bands at y=0,4,10):
    //   band y=0..4: x free in [4..10] -> 6x4 = 24
    //   band y=4..10: x free in [0..10] -> 10x6 = 60
    // sum_sq = 24^2 + 60^2 = 576 + 3600 = 4176.
    #[test]
    fn corner_placement_l_shape() {
        let problem = flat_problem(10, 10, &[(4, 4)]);
        let sol = Solution { placements: vec![place(0, 0, 0)], leftovers: vec![] };
        assert_eq!(sol.drop_consolidation_score(&problem), 4_176);
    }

    // 10x10 sheet, two 3x3 pieces at opposite corners (top-left, bottom-right).
    // y-band boundaries: 0, 3, 7, 10.
    //   band y=0..3: x free in [3..10] -> 7x3 = 21
    //   band y=3..7: x free in [0..10] -> 10x4 = 40
    //   band y=7..10: x free in [0..7]  -> 7x3 = 21
    // sum_sq = 21^2 + 40^2 + 21^2 = 441 + 1600 + 441 = 2482.
    #[test]
    fn two_opposite_corner_placements() {
        let problem = flat_problem(10, 10, &[(3, 3), (3, 3)]);
        let sol = Solution { placements: vec![place(0, 0, 0), place(1, 7, 7)], leftovers: vec![] };
        assert_eq!(sol.drop_consolidation_score(&problem), 2_482);
    }

    // Consolidation property: merging two equal-area free strips into one (same total
    // waste) must strictly increase the score. 6x2 sheet:
    //   fragmented: piece (2,0) 2x2 -> two 2x2 free strips, sum_sq = 4^2 + 4^2 = 32
    //   merged:     piece (0,0) 2x2 -> one 4x2 free strip,   sum_sq = 8^2 = 64
    #[test]
    fn drop_score_strictly_increases_when_drops_merge() {
        let problem = flat_problem(6, 2, &[(2, 2)]);
        let fragmented = Solution { placements: vec![place(0, 2, 0)], leftovers: vec![] };
        let merged = Solution { placements: vec![place(0, 0, 0)], leftovers: vec![] };
        let fragmented_score = fragmented.drop_consolidation_score(&problem);
        let merged_score = merged.drop_consolidation_score(&problem);
        assert_eq!(fragmented_score, 32);
        assert_eq!(merged_score, 64);
        assert!(merged_score > fragmented_score);
    }

    // Two-sheet solution: only the last sheet (sheet_idx 1, fully covered) is scored.
    // The earlier sheet's L-shape leftover (which alone would score 4_176, see
    // `corner_placement_l_shape`) is ignored.
    #[test]
    fn drop_score_only_last_sheet() {
        let problem = flat_problem(10, 10, &[(4, 4), (10, 10)]);
        let sol = Solution {
            placements: vec![
                Placement { sheet_idx: 0, piece_idx: 0, x: 0, y: 0, rotated: false },
                Placement { sheet_idx: 1, piece_idx: 1, x: 0, y: 0, rotated: false },
            ],
            leftovers: vec![],
        };
        assert_eq!(sol.drop_consolidation_score(&problem), 0);
    }

    // Same two sheets, roles swapped: the last sheet now has the L-shape leftover, so
    // its score (4_176) is the one that counts - confirms the function picks the sheet
    // by `sheets_used() - 1`, not by "least full".
    #[test]
    fn drop_score_picks_last_sheet_not_emptiest() {
        let problem = flat_problem(10, 10, &[(10, 10), (4, 4)]);
        let sol = Solution {
            placements: vec![
                Placement { sheet_idx: 0, piece_idx: 0, x: 0, y: 0, rotated: false },
                Placement { sheet_idx: 1, piece_idx: 1, x: 0, y: 0, rotated: false },
            ],
            leftovers: vec![],
        };
        assert_eq!(sol.drop_consolidation_score(&problem), 4_176);
    }
}
