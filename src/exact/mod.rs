/// Exact 2BPP-G solver via column generation (branch-price-and-cut skeleton).
///
/// This module solves the *primary* objective only: minimize `sheets_used`.
/// The secondary metrics (`layout_score`, `drop_consolidation_score`) used by
/// the GA are not part of the BPC formulation.
///
/// ## Algorithm outline (Phases 0-6 from docs/plans/21_exact-bpc.md)
///
/// Phases 2-6 are implemented: LB0/UB0 (area bound / Jylanki heuristic), the
/// RLMP (`rlmp`), the pricing oracle (`pricing`), the root-node column
/// generation loop, and gap-closing rounding (`round_gap`) below. Phase 7
/// (Ryan-Foster branch-and-price) is out of scope — see
/// `docs/plans/21_exact-bpc.md`; a converged root node that still has
/// `lb < ub` after rounding is reported as an honest `BpcStatus::Gap`.
///
/// ## Soundness of the reported lower bound
///
/// `z_RLMP` (the current RLMP objective) is a valid lower bound on the full
/// LP relaxation **only once column generation has converged** (pricing
/// proved no improving pattern exists) — with a partial column pool it can
/// only ever be *larger* than the true LP optimum, so reporting `ceil(z_RLMP)`
/// as `lb` before convergence would be unsound. The CG loop below therefore
/// only ever computes `lb` from `z_RLMP` after `PriceOutcome::NoneExists`;
/// mid-loop progress events keep reporting the already-proven `lb0`.
///
/// ## Cancellation
///
/// Drop the returned `BpcHandle` (or set `handle.stop`) to request early
/// termination.  The background thread checks the flag between iterations and
/// emits `BpcStatus::Stopped` before exiting.
mod pricing;
mod rlmp;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use pricing::{PriceOutcome, Pricer, PricingLimits};
use rlmp::Rlmp;

use crate::{
    expand::{expand_problem, shrink_solution},
    heuristic::{bfdh_solve, jylanki_solve},
    model::{Placement, Problem, ProblemSpec, Solution, SolutionSpec},
    transport::{ProgressMessage, ProgressSink},
};

/// Safety margin subtracted from `z_RLMP` before rounding up to the integer
/// lower bound. Absorbs two sources of slack, both tiny and both one-sided
/// (they can only make `z_RLMP` *larger* than the true LP optimum, never
/// smaller — so subtracting is always safe, never invalidates the bound):
/// floating-point noise accumulated over the simplex pivots, and `Pricer`'s
/// own `RC_EPS` tolerance (it reports `NoneExists` once no column improves by
/// more than `RC_EPS`, not by exactly zero).
const EPS_ROUND: f64 = 1e-4;

// == Types =====================================================================

/// Hyperparameters for the BPC solver.
pub struct BpcConfig {
    /// Emit a `BpcProgress` event every this many column-generation iterations.
    pub progress_interval: usize,
}

impl Default for BpcConfig {
    fn default() -> Self {
        BpcConfig { progress_interval: 10 }
    }
}

/// Terminal status of the BPC solver, produced once at completion.
pub enum BpcStatus {
    /// LP lower bound == incumbent: proven optimal.
    Optimal { sheets: usize },
    /// Column generation converged but LB < UB: gap remains.
    Gap { lb: usize, ub: usize },
    /// Stop flag set before completion; best feasible solution is still valid.
    Stopped { lb: usize, ub: usize },
}

/// A single-sheet guillotine cutting pattern (internal use; used in Phases 4-6).
struct Pattern {
    items: Vec<usize>,
    placements: Vec<Placement>,
}

// == Handle ====================================================================

enum BpcInternalEvent {
    Progress { iteration: usize, lb: usize, ub: usize },
    Done { status: BpcStatus, solution: SolutionSpec },
}

/// Handle to a running BPC solver thread.
///
/// Dropping the handle sets the stop flag, causing the solver thread to finish
/// its current iteration and exit cleanly.
pub struct BpcHandle {
    /// Setting this to `true` requests early termination.
    pub stop: Arc<AtomicBool>,
    rx: std::sync::mpsc::Receiver<BpcInternalEvent>,
}

impl Drop for BpcHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// == Public API ================================================================

/// Start the BPC solver in a background thread and return immediately.
///
/// Call [`drain_bpc`] on the same thread to receive progress events and block
/// until the solver finishes (or the handle is dropped).
pub fn run_bpc_bg(spec: Arc<ProblemSpec>, cfg: Arc<BpcConfig>) -> BpcHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<BpcInternalEvent>();
    let stop_bg = Arc::clone(&stop);
    std::thread::spawn(move || bpc_thread(&spec, &cfg, &stop_bg, &tx));
    BpcHandle { stop, rx }
}

/// Block until the BPC solver finishes, forwarding events to `sink`.
///
/// Progress events are throttled the same way GA progress is (see
/// `run_with_any_handle` in `main.rs`): at most one per `sink_interval_ms`,
/// with the latest pending one flushed right before `Done`. Without this, CG
/// iterations that complete in microseconds each would otherwise write one
/// line per `progress_interval` iterations regardless of real elapsed time.
///
/// Stops early if `sink.send` returns an error (e.g. SSE client disconnected),
/// which also drops `handle` and signals the solver thread to stop.
pub fn drain_bpc(
    handle: BpcHandle,
    spec: &ProblemSpec,
    sink: &mut dyn ProgressSink,
    sink_interval_ms: u64,
) -> Result<(), std::io::Error> {
    let throttle = Duration::from_millis(sink_interval_ms);
    let mut last_sent: Option<Instant> = None;
    let mut pending: Option<(usize, usize, usize)> = None;
    loop {
        match handle.rx.recv() {
            Err(_) => break,
            Ok(BpcInternalEvent::Progress { iteration, lb, ub }) => {
                pending = Some((iteration, lb, ub));
                let should_flush = sink_interval_ms == 0 || last_sent.is_none_or(|t| t.elapsed() >= throttle);
                if should_flush && let Some((iteration, lb, ub)) = pending.take() {
                    sink.send(&ProgressMessage::BpcProgress { iteration, lb, ub })?;
                    last_sent = Some(Instant::now());
                }
            }
            Ok(BpcInternalEvent::Done { status, solution }) => {
                if let Some((iteration, lb, ub)) = pending.take() {
                    sink.send(&ProgressMessage::BpcProgress { iteration, lb, ub })?;
                }
                let proven_optimal = matches!(status, BpcStatus::Optimal { .. });
                let sheets_used = match &status {
                    BpcStatus::Optimal { sheets } => *sheets,
                    BpcStatus::Gap { ub, .. } | BpcStatus::Stopped { ub, .. } => *ub,
                };
                let cut_lengths = solution.cut_lengths(spec);
                sink.send(&ProgressMessage::Done {
                    seed: 0,
                    sheets_used,
                    cut_lengths,
                    solution,
                    pieces: spec.piespecs.clone(),
                    genome: None,
                    proven_optimal: Some(proven_optimal),
                })?;
                break;
            }
        }
    }
    Ok(())
}

// == Solver thread =============================================================

fn bpc_thread(spec: &ProblemSpec, cfg: &BpcConfig, stop: &AtomicBool, tx: &std::sync::mpsc::Sender<BpcInternalEvent>) {
    let problem = expand_problem(spec);

    // Phase 2: LB0 — continuous area lower bound
    let total_area: u64 = problem.pieces.iter().map(|p| p.width as u64 * p.height as u64).sum();
    let sheet_area = problem.sheet.width as u64 * problem.sheet.height as u64;
    let lb0 = if sheet_area == 0 {
        0usize
    } else {
        usize::try_from(total_area.div_ceil(sheet_area)).unwrap_or(usize::MAX)
    };

    // Phase 2: UB0 — best result from the Jylanki greedy portfolio.
    // Use Solution::sheets_used() (counts max sheet_idx) instead of
    // Objective::sheets_used_int() which has a float rounding issue when
    // the last sheet is filled to exactly 100%.
    let flat_sol = jylanki_solve(&problem);
    let ub0 = flat_sol.sheets_used();
    let best_sol = shrink_solution(&flat_sol, spec);

    // Early exit: area bound already matches heuristic UB
    if lb0 == ub0 {
        let _ = tx.send(BpcInternalEvent::Done {
            status: BpcStatus::Optimal { sheets: ub0 },
            solution: best_sol,
        });
        return;
    }

    // Emit initial bounds (iteration 0)
    if tx
        .send(BpcInternalEvent::Progress {
            iteration: 0,
            lb: lb0,
            ub: ub0,
        })
        .is_err()
    {
        return;
    }

    if stop.load(Ordering::Relaxed) {
        let _ = tx.send(BpcInternalEvent::Done {
            status: BpcStatus::Stopped { lb: lb0, ub: ub0 },
            solution: best_sol,
        });
        return;
    }

    // Phases 3-5: root-node column generation.
    let mut rlmp = Rlmp::new(problem.pieces.len());
    let mut pricer = Pricer::new(
        &problem.pieces,
        problem.sheet.width,
        problem.sheet.height,
        PricingLimits::default(),
    );
    // Parallel to `rlmp`'s column indexing (0..n singletons, then
    // `add_column` call order): the RLMP only needs each column's item set
    // for the LP, but Phase 6 rounding needs the actual placements, which
    // `Rlmp` never stores. `basic_patterns()` returns column indices into
    // this table.
    let mut column_patterns: Vec<Pattern> = (0..problem.pieces.len())
        .map(|i| singleton_pattern(i, &problem))
        .collect();

    // CG is finite in theory (finite pattern universe, strictly decreasing
    // z_RLMP per accepted column), but a generous cap protects against
    // pathological floating-point cycling; hitting it is reported honestly
    // as a Gap, not a crash or a hang.
    let max_iterations = 20 * problem.pieces.len().max(1) + 1_000;

    let mut iteration = 0usize;
    let mut converged = false;
    while iteration < max_iterations {
        if stop.load(Ordering::Relaxed) {
            let _ = tx.send(BpcInternalEvent::Done {
                status: BpcStatus::Stopped { lb: lb0, ub: ub0 },
                solution: best_sol,
            });
            return;
        }

        rlmp.solve();
        let mu = rlmp.duals();
        match pricer.price(&mu) {
            PriceOutcome::Column(pattern) => {
                rlmp.add_column(&pattern.items);
                column_patterns.push(pattern);
                iteration += 1;
                if iteration.is_multiple_of(cfg.progress_interval) {
                    // Not yet a proven bound (CG hasn't converged) — report
                    // the last proven value, per the module doc.
                    let progress = BpcInternalEvent::Progress {
                        iteration,
                        lb: lb0,
                        ub: ub0,
                    };
                    if tx.send(progress).is_err() {
                        return;
                    }
                }
            }
            PriceOutcome::NoneExists => {
                converged = true;
                break;
            }
            PriceOutcome::Aborted => break,
        }
    }

    let mut ub = ub0;
    let mut solution = best_sol;
    let status = if converged {
        let lb = lb0.max(round_down_lb(rlmp.objective()));
        debug_assert!(lb <= ub0, "computed LB {lb} exceeds known feasible UB {ub0}");
        if lb < ub0 {
            // Phase 6: try to beat UB0 by rounding the final fractional
            // RLMP solution instead of accepting the gap outright.
            if let Some((rounded_sheets, placements)) = round_gap(&rlmp, &column_patterns, &problem, ub0) {
                ub = rounded_sheets;
                solution = shrink_solution(
                    &Solution {
                        placements,
                        leftovers: vec![],
                    },
                    spec,
                );
            }
        }
        if lb >= ub {
            BpcStatus::Optimal { sheets: ub }
        } else {
            BpcStatus::Gap { lb, ub }
        }
    } else {
        // Aborted (pricing budget exhausted) or the iteration cap was hit:
        // z_RLMP is not a valid bound without convergence, so lb stays lb0.
        // Rounding needs a converged RLMP (its dual/primal split is only
        // meaningful at LP optimality), so it is skipped here too.
        BpcStatus::Gap { lb: lb0, ub: ub0 }
    };
    let _ = tx.send(BpcInternalEvent::Done { status, solution });
}

/// A single-item pattern occupying its own sheet at the origin. Used to seed
/// `column_patterns` for `Rlmp`'s initial singleton columns, which `Rlmp`
/// itself only tracks as item sets (see `column_patterns`'s doc at its call
/// site). Every piece is assumed to fit the sheet in some orientation — the
/// same assumption the rest of the crate already makes (e.g. `Pricer` silently
/// drops pieces that don't).
fn singleton_pattern(piece_idx: usize, problem: &Problem) -> Pattern {
    let p = &problem.pieces[piece_idx];
    let rotated = !(p.width <= problem.sheet.width && p.height <= problem.sheet.height)
        && p.can_rotate
        && p.height <= problem.sheet.width
        && p.width <= problem.sheet.height;
    Pattern {
        items: vec![piece_idx],
        placements: vec![Placement {
            sheet_idx: 0,
            piece_idx,
            x: 0,
            y: 0,
            rotated,
        }],
    }
}

/// Phase 6: greedy set-cover rounding of the converged RLMP's basic patterns,
/// falling back to `bfdh_solve` for whatever items no accepted pattern covers.
///
/// Patterns are taken in decreasing lambda order (the LP's own preference);
/// a pattern is accepted wholesale (keeping its internally-validated
/// guillotine layout intact) only if none of its items were already placed
/// by a higher-lambda pattern — each accepted pattern gets its own new sheet,
/// so no cross-pattern overlap checking is needed. This is deliberately not
/// the paper's tabu-search rounding (see `docs/plans/21_exact-bpc.md`), just
/// a cheap best-effort improvement over `incumbent_sheets`.
///
/// Returns `None` if the rounded solution does not strictly improve on
/// `incumbent_sheets` (the caller keeps the existing incumbent).
fn round_gap(
    rlmp: &Rlmp,
    column_patterns: &[Pattern],
    problem: &Problem,
    incumbent_sheets: usize,
) -> Option<(usize, Vec<Placement>)> {
    let n = problem.pieces.len();
    let mut basic = rlmp.basic_patterns();
    basic.sort_by(|a, b| b.1.total_cmp(&a.1));

    let mut covered = vec![false; n];
    let mut placements = Vec::new();
    let mut sheets_used = 0usize;
    for (col, _lambda) in basic {
        let pattern = &column_patterns[col];
        if pattern.items.iter().any(|&i| covered[i]) {
            continue;
        }
        for &i in &pattern.items {
            covered[i] = true;
        }
        placements.extend(pattern.placements.iter().map(|pl| Placement {
            sheet_idx: sheets_used,
            ..*pl
        }));
        sheets_used += 1;
    }

    let remaining: Vec<usize> = (0..n).filter(|&i| !covered[i]).collect();
    if !remaining.is_empty() {
        let remaining_problem = Problem {
            sheet: problem.sheet,
            pieces: remaining.iter().map(|&i| problem.pieces[i].clone()).collect(),
        };
        let remaining_sol = bfdh_solve(&remaining_problem);
        placements.extend(remaining_sol.placements.iter().map(|pl| Placement {
            sheet_idx: pl.sheet_idx + sheets_used,
            piece_idx: remaining[pl.piece_idx],
            ..*pl
        }));
        sheets_used += remaining_sol.sheets_used();
    }

    (sheets_used < incumbent_sheets).then_some((sheets_used, placements))
}

/// `ceil(z_rlmp - EPS_ROUND)`, clamped to 0 (see `EPS_ROUND` for why the
/// subtraction is always safe). Only valid to call once CG has converged.
fn round_down_lb(z_rlmp: f64) -> usize {
    let v = (z_rlmp - EPS_ROUND).ceil();
    if v <= 0.0 { 0 } else { v as usize }
}

// == Tests =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{Piece, Sheet},
        parse::parse_problem,
    };

    fn tiny_problem(n_pieces: usize) -> Problem {
        Problem {
            sheet: Sheet { width: 10, height: 10 },
            pieces: (0..n_pieces)
                .map(|_| Piece {
                    name: String::new(),
                    width: 5,
                    height: 5,
                    can_rotate: false,
                })
                .collect(),
        }
    }

    #[test]
    fn round_gap_prefers_the_combining_pattern_over_singletons() {
        let problem = tiny_problem(2);
        let mut rlmp = Rlmp::new(2);
        let combined = rlmp.add_column(&[0, 1]);
        rlmp.solve();
        assert!(
            (rlmp.objective() - 1.0).abs() < 1e-9,
            "expected the combined pattern to dominate"
        );

        let column_patterns = vec![
            singleton_pattern(0, &problem),
            singleton_pattern(1, &problem),
            Pattern {
                items: vec![0, 1],
                placements: vec![
                    Placement {
                        sheet_idx: 0,
                        piece_idx: 0,
                        x: 0,
                        y: 0,
                        rotated: false,
                    },
                    Placement {
                        sheet_idx: 0,
                        piece_idx: 1,
                        x: 5,
                        y: 0,
                        rotated: false,
                    },
                ],
            },
        ];
        assert_eq!(rlmp.basic_patterns(), vec![(combined, 1.0)]);

        // Strictly improves on a 2-sheet incumbent: both items fit one sheet.
        let (sheets, placements) = round_gap(&rlmp, &column_patterns, &problem, 2).expect("must improve on 2 sheets");
        assert_eq!(sheets, 1);
        assert_eq!(placements.len(), 2);
        assert!(placements.iter().all(|p| p.sheet_idx == 0));

        // Does not "improve" on an incumbent that is already as good.
        assert!(round_gap(&rlmp, &column_patterns, &problem, 1).is_none());
    }

    #[test]
    fn round_gap_falls_back_to_bfdh_for_items_the_pool_never_covers() {
        // `rlmp` only knows about items 0 and 1 (a 2-item RLMP), while
        // `problem` has a third piece with no corresponding column at all —
        // isolates the "no basic pattern covers this item" branch without
        // needing to engineer genuine LP fractional-cover degeneracy.
        let problem = tiny_problem(3);
        let mut rlmp = Rlmp::new(2);
        rlmp.solve();
        let column_patterns = vec![singleton_pattern(0, &problem), singleton_pattern(1, &problem)];

        let (sheets, placements) =
            round_gap(&rlmp, &column_patterns, &problem, usize::MAX).expect("must produce a solution");
        // 2 singleton sheets (items 0, 1) + however many bfdh needs for item 2.
        assert!(sheets >= 3, "expected at least 3 sheets, got {sheets}");
        assert_eq!(placements.len(), 3);
        assert!(
            placements.iter().any(|p| p.piece_idx == 2),
            "item 2 must be placed by the bfdh fallback"
        );
    }

    fn run_sync(spec: ProblemSpec) -> (usize, Option<bool>) {
        let spec = Arc::new(spec);
        let cfg = Arc::new(BpcConfig::default());
        let handle = run_bpc_bg(Arc::clone(&spec), cfg);

        struct Capture {
            sheets: usize,
            proven: Option<bool>,
        }
        impl ProgressSink for Capture {
            fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error> {
                if let ProgressMessage::Done {
                    sheets_used,
                    proven_optimal,
                    ..
                } = msg
                {
                    self.sheets = *sheets_used;
                    self.proven = *proven_optimal;
                }
                Ok(())
            }
        }

        let mut cap = Capture {
            sheets: 0,
            proven: None,
        };
        drain_bpc(handle, &spec, &mut cap, 0).expect("drain_bpc failed");
        (cap.sheets, cap.proven)
    }

    #[test]
    fn trivially_optimal_single_sheet() {
        // One piece that exactly fills the sheet: area bound == jylanki UB == 1
        let spec = parse_problem("10x10F:0:10x10").unwrap();
        let (sheets, proven) = run_sync(spec);
        assert_eq!(sheets, 1, "should fit in 1 sheet");
        assert_eq!(proven, Some(true), "should be proven optimal");
    }

    #[test]
    fn lp_bound_tighter_than_area_bound_proves_optimal() {
        // 20 copies of 30x30 in a 100x100 sheet: only a 3x3 grid (9 copies)
        // fits per sheet, so area_bound = ceil(20*900/10000) = 2 is loose —
        // the true optimum is 3 (ceil(20/9)). Column generation must price a
        // pattern of 9 identical pieces, drive z_RLMP down to 20/9 = 2.22,
        // and round up to LB = 3 = UB0: a genuine LP-bound proof, not just
        // the area heuristic from Phase 2.
        let spec = parse_problem("100x100F:0:30x30/20").unwrap();
        let (sheets, proven) = run_sync(spec);
        assert_eq!(sheets, 3, "3x3 grids cap every sheet at 9 copies");
        assert_eq!(proven, Some(true), "LP bound must tighten to match UB0");
    }

    #[test]
    fn stop_flag_respected() {
        let spec = Arc::new(parse_problem("100x100F:0:30x30/20").unwrap());
        let cfg = Arc::new(BpcConfig { progress_interval: 1 });
        let handle = run_bpc_bg(Arc::clone(&spec), cfg);
        handle.stop.store(true, Ordering::Relaxed);

        struct Count(usize);
        impl ProgressSink for Count {
            fn send(&mut self, _: &ProgressMessage) -> Result<(), std::io::Error> {
                self.0 += 1;
                Ok(())
            }
        }
        let mut cnt = Count(0);
        drain_bpc(handle, &spec, &mut cnt, 0).expect("drain_bpc failed");
        // Should have received at most 2 events (one Progress + one Done)
        assert!(cnt.0 <= 3, "too many events: {}", cnt.0);
    }
}
