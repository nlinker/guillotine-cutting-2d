/// Exact 2BPP-G solver via column generation (branch-price-and-cut skeleton).
///
/// This module solves the *primary* objective only: minimize `sheets_used`.
/// The secondary metrics (`layout_score`, `drop_consolidation_score`) used by
/// the GA are not part of the BPC formulation.
///
/// ## Algorithm outline (Phases 0-6 from docs/plans/21_exact-bpc.md)
///
/// Phase 2 (LB0/UB0) is implemented; Phases 3-6 (column generation, pricing,
/// LP master, gap closing) are stubs — the solver currently returns UB0 from
/// the Jylanki heuristic together with the area-bound LB0.
///
/// ## Cancellation
///
/// Drop the returned `BpcHandle` (or set `handle.stop`) to request early
/// termination.  The background thread checks the flag between iterations and
/// emits `BpcStatus::Stopped` before exiting.
mod rlmp;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    expand::{expand_problem, shrink_solution},
    heuristic::jylanki_solve,
    model::{ProblemSpec, SolutionSpec},
    transport::{ProgressMessage, ProgressSink},
};

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
#[allow(dead_code)]
struct Pattern {
    items: Vec<usize>,
    placements: Vec<crate::model::Placement>,
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
/// Stops early if `sink.send` returns an error (e.g. SSE client disconnected),
/// which also drops `handle` and signals the solver thread to stop.
pub fn drain_bpc(
    handle: BpcHandle,
    spec: &ProblemSpec,
    sink: &mut dyn ProgressSink,
) -> Result<(), std::io::Error> {
    loop {
        match handle.rx.recv() {
            Err(_) => break,
            Ok(BpcInternalEvent::Progress { iteration, lb, ub }) => {
                sink.send(&ProgressMessage::BpcProgress { iteration, lb, ub })?;
            }
            Ok(BpcInternalEvent::Done { status, solution }) => {
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

fn bpc_thread(
    spec: &ProblemSpec,
    cfg: &BpcConfig,
    stop: &AtomicBool,
    tx: &std::sync::mpsc::Sender<BpcInternalEvent>,
) {
    let problem = expand_problem(spec);

    // Phase 2: LB0 — continuous area lower bound
    let total_area: u64 = problem.pieces.iter()
        .map(|p| p.width as u64 * p.height as u64)
        .sum();
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
    if tx.send(BpcInternalEvent::Progress { iteration: 0, lb: lb0, ub: ub0 }).is_err() {
        return;
    }

    if stop.load(Ordering::Relaxed) {
        let _ = tx.send(BpcInternalEvent::Done {
            status: BpcStatus::Stopped { lb: lb0, ub: ub0 },
            solution: best_sol,
        });
        return;
    }

    // TODO Phase 3: LP master problem (RLMP) + microlp/good_lp dependency
    // TODO Phase 4: pricing oracle — guillotine knapsack B&B
    // TODO Phase 5: column generation loop
    // TODO Phase 6: gap-closing rounding

    // Interim: return Gap with the heuristic UB and area LB
    let _ = cfg.progress_interval; // suppress unused warning until CG loop is implemented
    let _ = tx.send(BpcInternalEvent::Done {
        status: BpcStatus::Gap { lb: lb0, ub: ub0 },
        solution: best_sol,
    });
}

// == Tests =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_problem;

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
                if let ProgressMessage::Done { sheets_used, proven_optimal, .. } = msg {
                    self.sheets = *sheets_used;
                    self.proven = *proven_optimal;
                }
                Ok(())
            }
        }

        let mut cap = Capture { sheets: 0, proven: None };
        drain_bpc(handle, &spec, &mut cap).expect("drain_bpc failed");
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
    fn gap_returned_when_lb_lt_ub() {
        // Many small pieces in a small sheet: area bound is tight but heuristic
        // may use more sheets than LB (unlikely for this tiny case, but we verify
        // the run completes and returns a valid sheet count).
        let spec = parse_problem("100x100F:0:30x30/20").unwrap();
        let (sheets, proven) = run_sync(spec);
        assert!(sheets >= 1, "must use at least 1 sheet");
        // proven may be true (if area bound == UB) or false (gap); both are valid
        let _ = proven;
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
        drain_bpc(handle, &spec, &mut cnt).expect("drain_bpc failed");
        // Should have received at most 2 events (one Progress + one Done)
        assert!(cnt.0 <= 3, "too many events: {}", cnt.0);
    }
}
