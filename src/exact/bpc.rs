/// Exact 2BPP-G solver via column generation (branch-price-and-cut skeleton).
///
/// This module solves the *primary* objective only: minimize `sheets_used`.
/// The secondary metrics (`layout_score`, `drop_consolidation_score`) used by
/// the GA are not part of the BPC formulation.
///
/// ## Algorithm outline
///
/// LB0/UB0 (area bound / Jylanki heuristic), the RLMP (`rlmp`), the pricing
/// oracle (`pricing`), column generation, and gap-closing rounding
/// (`round_gap`) are implemented, plus Ryan-Foster-style branch-and-bound on
/// top of the root: when a converged node's LP bound still falls short of
/// the incumbent after rounding, `pick_fractional_type_pair` finds a pair of
/// *types* with a fractional aggregated co-occurrence and branches into a
/// "together" and an "apart" child (`BpcNode`), each with its own restricted
/// column pool. Branching on individual items (rather than types) doesn't
/// work here because copies of a type are interchangeable throughout
/// `Pricer` (the exact search only tracks per-type counts, not which
/// specific copy is used). `GlfOracle` and `Pricer` are shared across every
/// node of the tree (constructed once,
/// below) so the memoized GLF cache isn't rebuilt per node.
///
/// ## Soundness of the reported lower bound
///
/// `z_RLMP` (the current RLMP objective) is a valid lower bound on the full
/// LP relaxation **only once column generation has converged** (pricing
/// proved no improving pattern exists) - with a partial column pool it can
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
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use super::{
    pricing::{BranchConstraints, GlfOracle, PriceOutcome, Pricer, PricingLimits},
    rlmp::Rlmp,
};
use crate::{
    expand::{expand_problem, shrink_solution},
    heuristic::{bfdh_solve, jylanki_solve},
    model::{Placement, Problem, ProblemSpec, Solution, SolutionSpec},
    transport::{ProgressMessage, ProgressSink},
};

/// Safety margin subtracted from `z_RLMP` before rounding up to the integer
/// lower bound. Absorbs two sources of slack, both tiny and both one-sided
/// (they can only make `z_RLMP` *larger* than the true LP optimum, never
/// smaller - so subtracting is always safe, never invalidates the bound):
/// floating-point noise accumulated over the simplex pivots, and `Pricer`'s
/// own `RC_EPS` tolerance (it reports `NoneExists` once no column improves by
/// more than `RC_EPS`, not by exactly zero).
const EPS_ROUND: f64 = 1e-4;

// == Types =====================================================================

/// Hyperparameters for the BPC solver.
pub struct BpcConfig {
    /// Emit a `BpcProgress` event every this many column-generation iterations
    /// (counted across the whole branch-and-bound tree, not per node).
    pub progress_interval: usize,
    /// Maximum number of branch-and-bound tree nodes processed before giving
    /// up and reporting an honest `Gap`/`Stopped` instead of `Optimal`. Does
    /// not bound the work done *within* one node's column generation --
    /// that's `PricingLimits` (per pricing call) and the per-node CG
    /// iteration cap in `bpc_thread`.
    pub max_tree_nodes: usize,
}

impl Default for BpcConfig {
    fn default() -> Self {
        BpcConfig {
            progress_interval: 10,
            max_tree_nodes: 10_000,
        }
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
#[derive(Clone)]
pub(super) struct Pattern {
    pub(super) items: Vec<usize>,
    pub(super) placements: Vec<Placement>,
}

/// One node of the branch-and-bound tree (Phase 7). `rlmp`/`column_patterns`
/// are a from-scratch RLMP restricted to patterns compatible with
/// `constraints` -- child nodes are built by re-adding only the parent's
/// patterns that still satisfy the (tightened) constraint set, never by
/// mutating the parent's RLMP in place.
struct BpcNode {
    constraints: BranchConstraints,
    rlmp: Rlmp,
    /// Parallel to `rlmp`'s column indexing, same convention as
    /// `bpc_thread`'s original root-only `column_patterns` (see its doc).
    column_patterns: Vec<Pattern>,
    /// Lower bound for this subtree inherited from the parent at creation
    /// time, before this node's own CG has run. A safe placeholder for
    /// progress reporting: a child's LP bound can only be >= the parent's
    /// (constraints only ever shrink the column pool), so this never
    /// overstates what the subtree can prove.
    pending_lb: usize,
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
                    pieces: spec.piece_types.clone(),
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

    // Phase 2: LB0 - continuous area lower bound
    let total_area: u64 = problem.pieces.iter().map(|p| p.width as u64 * p.height as u64).sum();
    let sheet_area = problem.sheet.width as u64 * problem.sheet.height as u64;
    let lb0 = if sheet_area == 0 {
        0usize
    } else {
        usize::try_from(total_area.div_ceil(sheet_area)).unwrap_or(usize::MAX)
    };

    // Phase 2: UB0 - best result from the Jylanki greedy portfolio.
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

    // Phases 3-7: branch-and-bound tree of column-generation nodes. `oracle`
    // and `pricer` are shared across every node (Phase 2) so the memoized GLF
    // cache and the pricing work budgets aren't rebuilt/reset per node.
    let pricing_limits = PricingLimits::default();
    let mut oracle = GlfOracle::new(
        problem.sheet.width,
        problem.sheet.height,
        pricing_limits.max_cells,
        pricing_limits.max_splits,
    );
    let mut pricer = Pricer::new(
        &problem.pieces,
        problem.sheet.width,
        problem.sheet.height,
        pricing_limits,
    );

    // Parallel to each node's `rlmp` column indexing (0..n singletons, then
    // `add_column` call order): the RLMP only needs each column's item set
    // for the LP, but Phase 6 rounding needs the actual placements, which
    // `Rlmp` never stores. `basic_patterns()` returns column indices into
    // this table.
    let root = BpcNode {
        constraints: BranchConstraints::default(),
        rlmp: Rlmp::new(problem.pieces.len()),
        column_patterns: (0..problem.pieces.len())
            .map(|i| singleton_pattern(i, &problem))
            .collect(),
        pending_lb: lb0,
    };
    let mut open: Vec<BpcNode> = vec![root];

    // Per-node CG iteration cap (finite in theory: finite pattern universe,
    // strictly decreasing z_RLMP per accepted column, but a generous cap
    // protects against pathological floating-point cycling); applied fresh
    // to every node's own column generation.
    let max_cg_iterations = 20 * problem.pieces.len().max(1) + 1_000;

    let mut iteration = 0usize;
    let mut global_ub = ub0;
    let mut best_solution = best_sol;
    let mut tree_nodes_processed = 0usize;
    // Set once a node's CG is aborted (pricing budget exhausted) or the tree
    // node budget runs out with nodes still pending: an honest signal that
    // the final status must be `Gap`, not `Optimal`, even once `open` empties
    // out (a dropped/unexplored subtree might have hidden a better solution).
    let mut any_node_incomplete = false;

    while let Some(mut node) = open.pop() {
        if stop.load(Ordering::Relaxed) {
            let lb = frontier_lb(&open, lb0).min(node.pending_lb);
            let _ = tx.send(BpcInternalEvent::Done {
                status: BpcStatus::Stopped { lb, ub: global_ub },
                solution: best_solution,
            });
            return;
        }

        tree_nodes_processed += 1;
        if tree_nodes_processed > cfg.max_tree_nodes {
            open.push(node);
            let lb = frontier_lb(&open, lb0);
            let _ = tx.send(BpcInternalEvent::Done {
                status: BpcStatus::Gap { lb, ub: global_ub },
                solution: best_solution,
            });
            return;
        }

        let progress_lb = frontier_lb(&open, lb0).min(node.pending_lb);
        match run_cg_at_node(
            &mut node,
            &mut pricer,
            &mut oracle,
            stop,
            tx,
            &mut iteration,
            cfg.progress_interval,
            progress_lb,
            global_ub,
            max_cg_iterations,
        ) {
            CgOutcome::Disconnected => return,
            CgOutcome::StopRequested => {
                let lb = frontier_lb(&open, lb0).min(node.pending_lb);
                let _ = tx.send(BpcInternalEvent::Done {
                    status: BpcStatus::Stopped { lb, ub: global_ub },
                    solution: best_solution,
                });
                return;
            }
            // z_RLMP is not a valid bound without convergence, so this
            // subtree is simply dropped rather than trusted or branched on.
            CgOutcome::Aborted => any_node_incomplete = true,
            CgOutcome::Converged => {
                let node_lb = lb0.max(round_down_lb(node.rlmp.objective()));
                if node_lb < global_ub {
                    // Phase 6: try to beat the incumbent by rounding this
                    // node's fractional RLMP solution before deciding whether
                    // to branch.
                    if let Some((rounded_sheets, placements)) =
                        round_gap(&node.rlmp, &node.column_patterns, &problem, global_ub)
                    {
                        global_ub = rounded_sheets;
                        best_solution = shrink_solution(
                            &Solution {
                                placements,
                                leftovers: vec![],
                            },
                            spec,
                        );
                    }
                }
                if node_lb < global_ub {
                    match pick_fractional_type_pair(&node, &pricer) {
                        Some((t, t2)) => {
                            let has_type =
                                |p: &Pattern, ty: usize| p.items.iter().any(|&i| pricer.type_of(i) == Some(ty));
                            let together = child_node(
                                &node,
                                node.constraints.with_forced_together(t, t2),
                                node_lb,
                                &problem,
                                |p| has_type(p, t) == has_type(p, t2),
                            );
                            let apart =
                                child_node(&node, node.constraints.with_forbidden(t, t2), node_lb, &problem, |p| {
                                    !(has_type(p, t) && has_type(p, t2))
                                });
                            open.push(together);
                            open.push(apart);
                        }
                        None => {
                            // Either this node's LP solution is already
                            // integral (round_gap above already turned it
                            // into a UB candidate if it improved things, and
                            // node_lb is now a fully resolved bound for this
                            // subtree -- nothing left to explore), or the
                            // rare edge case where some lambda is still
                            // fractional but no type pair captures it. The
                            // Vanderbeck-style aggregated fallback for that
                            // case isn't implemented; detect it and report it
                            // honestly instead of silently over-claiming
                            // optimality.
                            if node_has_fractional_lambda(&node) {
                                any_node_incomplete = true;
                            }
                        }
                    }
                }
                // else: fathomed by bound -- no solution in this subtree can
                // beat the incumbent, so it needs no further exploration.
            }
        }
    }

    let status = if any_node_incomplete {
        BpcStatus::Gap { lb: lb0, ub: global_ub }
    } else {
        BpcStatus::Optimal { sheets: global_ub }
    };
    let _ = tx.send(BpcInternalEvent::Done {
        status,
        solution: best_solution,
    });
}

/// Lower bound over every node still pending in `open`, or `fallback` if
/// `open` is empty. Nodes not yet processed by column generation only carry
/// their `pending_lb` placeholder (inherited from their parent), never a
/// tighter bound -- see `BpcNode::pending_lb`'s doc.
fn frontier_lb(open: &[BpcNode], fallback: usize) -> usize {
    open.iter().map(|n| n.pending_lb).min().unwrap_or(fallback)
}

/// Outcome of running column generation to convergence (or budget
/// exhaustion) at one branch-and-bound tree node.
enum CgOutcome {
    /// Pricing proved no improving column exists: `node.rlmp.objective()` is
    /// now a valid LP bound for this node's subtree.
    Converged,
    /// A work budget (pricing or the per-node iteration cap) ran out before
    /// convergence; this node's LP state cannot be trusted as a bound.
    Aborted,
    /// The stop flag was set; the whole solve is winding down.
    StopRequested,
    /// The progress channel's receiver is gone (e.g. SSE client
    /// disconnected); the caller should stop immediately without sending.
    Disconnected,
}

/// Run column generation at one tree node to convergence, forwarding
/// progress events (throttled by `progress_interval`, counted against the
/// shared `iteration` counter). `global_lb`/`global_ub` are a snapshot for
/// progress reporting only -- they don't change mid-call.
#[allow(clippy::too_many_arguments)]
fn run_cg_at_node(
    node: &mut BpcNode,
    pricer: &mut Pricer,
    oracle: &mut GlfOracle,
    stop: &AtomicBool,
    tx: &std::sync::mpsc::Sender<BpcInternalEvent>,
    iteration: &mut usize,
    progress_interval: usize,
    global_lb: usize,
    global_ub: usize,
    max_iterations: usize,
) -> CgOutcome {
    let mut local_iterations = 0usize;
    loop {
        if stop.load(Ordering::Relaxed) {
            return CgOutcome::StopRequested;
        }
        if local_iterations >= max_iterations {
            return CgOutcome::Aborted;
        }

        node.rlmp.solve();
        let mu = node.rlmp.duals();
        match pricer.price(&mu, &node.constraints, oracle) {
            PriceOutcome::Column(pattern) => {
                node.rlmp.add_column(&pattern.items);
                node.column_patterns.push(pattern);
                *iteration += 1;
                local_iterations += 1;
                if iteration.is_multiple_of(progress_interval) {
                    // Not yet a proven bound (this node's CG hasn't
                    // converged) -- report the last proven frontier value.
                    let progress = BpcInternalEvent::Progress {
                        iteration: *iteration,
                        lb: global_lb,
                        ub: global_ub,
                    };
                    if tx.send(progress).is_err() {
                        return CgOutcome::Disconnected;
                    }
                }
            }
            PriceOutcome::NoneExists => return CgOutcome::Converged,
            PriceOutcome::Aborted => return CgOutcome::Aborted,
        }
    }
}

/// Find a pair of distinct types with a fractional aggregated co-occurrence
/// `y_{t,t'} = sum_p lambda_p * [pattern p contains a copy of both t and
/// t']`, picking the one closest to 0.5 (classic "most fractional"
/// tie-break). `None` if every type pair present in the node's basic
/// patterns is already integral.
fn pick_fractional_type_pair(node: &BpcNode, pricer: &Pricer) -> Option<(usize, usize)> {
    let mut y: std::collections::HashMap<(usize, usize), f64> = std::collections::HashMap::new();
    for (col, lambda) in node.rlmp.basic_patterns() {
        let mut types: Vec<usize> = node.column_patterns[col]
            .items
            .iter()
            .filter_map(|&i| pricer.type_of(i))
            .collect();
        types.sort_unstable();
        types.dedup();
        for i in 0..types.len() {
            for &tj in &types[i + 1..] {
                *y.entry((types[i], tj)).or_insert(0.0) += lambda;
            }
        }
    }
    y.into_iter()
        .filter(|&(_, v)| (v - v.round()).abs() > EPS_ROUND)
        .min_by(|a, b| (a.1 - 0.5).abs().total_cmp(&(b.1 - 0.5).abs()))
        .map(|(pair, _)| pair)
}

/// Whether any basic pattern in `node` has a non-integral lambda -- used
/// only to distinguish "this node is already integral, nothing to branch on"
/// from the rare edge case where `pick_fractional_type_pair` finds nothing
/// even though the LP solution is still fractional (see the call site).
fn node_has_fractional_lambda(node: &BpcNode) -> bool {
    node.rlmp
        .basic_patterns()
        .iter()
        .any(|&(_, lambda)| (lambda - lambda.round()).abs() > EPS_ROUND)
}

/// Build a child node: a from-scratch RLMP seeded with the parent's patterns
/// that satisfy `keep` (plus the usual singletons), under a tightened
/// constraint set.
fn child_node(
    parent: &BpcNode,
    constraints: BranchConstraints,
    pending_lb: usize,
    problem: &Problem,
    keep: impl Fn(&Pattern) -> bool,
) -> BpcNode {
    let n = problem.pieces.len();
    let mut rlmp = Rlmp::new(n);
    let mut column_patterns: Vec<Pattern> = (0..n).map(|i| singleton_pattern(i, problem)).collect();
    for pattern in parent.column_patterns.iter().skip(n) {
        if keep(pattern) {
            rlmp.add_column(&pattern.items);
            column_patterns.push(pattern.clone());
        }
    }
    BpcNode {
        constraints,
        rlmp,
        column_patterns,
        pending_lb,
    }
}

/// A single-item pattern occupying its own sheet at the origin. Used to seed
/// `column_patterns` for `Rlmp`'s initial singleton columns, which `Rlmp`
/// itself only tracks as item sets (see `column_patterns`'s doc at its call
/// site). Every piece is assumed to fit the sheet in some orientation - the
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
/// by a higher-lambda pattern - each accepted pattern gets its own new sheet,
/// so no cross-pattern overlap checking is needed. This is deliberately not
/// the paper's tabu-search rounding, just a cheap best-effort improvement
/// over `incumbent_sheets`.
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
        parser::compact::parse_problem,
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

    fn piece(w: u32, h: u32) -> Piece {
        Piece {
            name: String::new(),
            width: w,
            height: h,
            can_rotate: false,
        }
    }

    fn dummy_pattern(items: Vec<usize>) -> Pattern {
        Pattern {
            items,
            placements: vec![],
        }
    }

    // The textbook example motivating Ryan-Foster branching in the first
    // place: 3 items, one of each (distinct) type, with only pairwise
    // patterns available (no full-triple or singleton pattern competitive at
    // the optimum) -- the LP relaxation's unique optimum is the classic
    // lambda=0.5 on each of the 3 pairs, objective 1.5, with every type pair
    // equally (and only) fractional. Verifies `pick_fractional_type_pair`
    // finds one of them via the real `Pricer::type_of` mapping, not just
    // `Rlmp`'s raw column indices.
    #[test]
    fn pick_fractional_type_pair_finds_a_fractional_pair() {
        let pieces = vec![piece(5, 5), piece(4, 4), piece(3, 3)];
        let pricer = Pricer::new(&pieces, 10, 10, PricingLimits::default());

        let mut rlmp = Rlmp::new(3);
        rlmp.add_column(&[0, 1]);
        rlmp.add_column(&[0, 2]);
        rlmp.add_column(&[1, 2]);
        rlmp.solve();
        assert!(
            (rlmp.objective() - 1.5).abs() < 1e-6,
            "expected the classic 1.5 fractional optimum, got {}",
            rlmp.objective()
        );

        let column_patterns = vec![
            dummy_pattern(vec![0]),
            dummy_pattern(vec![1]),
            dummy_pattern(vec![2]),
            dummy_pattern(vec![0, 1]),
            dummy_pattern(vec![0, 2]),
            dummy_pattern(vec![1, 2]),
        ];
        let node = BpcNode {
            constraints: BranchConstraints::default(),
            rlmp,
            column_patterns,
            pending_lb: 0,
        };
        let (t, t2) = pick_fractional_type_pair(&node, &pricer).expect("must find a fractional type pair");
        let pair = (t.min(t2), t.max(t2));
        assert!([(0, 1), (0, 2), (1, 2)].contains(&pair), "unexpected pair {pair:?}");
    }

    // A child node must both filter `column_patterns` *and* actually leave
    // the excluded pattern out of the rebuilt `Rlmp` (not just out of the
    // bookkeeping Vec) -- checked behaviorally: dropping the only pattern
    // that covers both items forces the child's LP optimum back up to 2,
    // same as if the combined pattern had never been generated at all.
    #[test]
    fn child_node_drops_the_filtered_pattern_from_the_rebuilt_rlmp() {
        let problem = tiny_problem(2);
        let mut parent_rlmp = Rlmp::new(2);
        parent_rlmp.add_column(&[0, 1]);
        parent_rlmp.solve();
        assert!(
            (parent_rlmp.objective() - 1.0).abs() < 1e-9,
            "parent should use the combined pattern"
        );

        let parent = BpcNode {
            constraints: BranchConstraints::default(),
            rlmp: parent_rlmp,
            column_patterns: vec![
                singleton_pattern(0, &problem),
                singleton_pattern(1, &problem),
                dummy_pattern(vec![0, 1]),
            ],
            pending_lb: 0,
        };

        let mut child = child_node(&parent, BranchConstraints::default(), 1, &problem, |p| {
            p.items.len() == 1
        });
        assert_eq!(
            child.column_patterns.len(),
            2,
            "the combined pattern must not survive the filter"
        );
        assert_eq!(child.pending_lb, 1);
        child.rlmp.solve();
        assert!(
            (child.rlmp.objective() - 2.0).abs() < 1e-9,
            "without the combined pattern the child must fall back to 2 singleton sheets, got {}",
            child.rlmp.objective()
        );
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
        // `problem` has a third piece with no corresponding column at all -
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
        let spec = parse_problem("10x10F::10x10").unwrap();
        let (sheets, proven) = run_sync(spec);
        assert_eq!(sheets, 1, "should fit in 1 sheet");
        assert_eq!(proven, Some(true), "should be proven optimal");
    }

    #[test]
    fn lp_bound_tighter_than_area_bound_proves_optimal() {
        // 20 copies of 30x30 in a 100x100 sheet: only a 3x3 grid (9 copies)
        // fits per sheet, so area_bound = ceil(20*900/10000) = 2 is loose -
        // the true optimum is 3 (ceil(20/9)). Column generation must price a
        // pattern of 9 identical pieces, drive z_RLMP down to 20/9 = 2.22,
        // and round up to LB = 3 = UB0: a genuine LP-bound proof, not just
        // the area heuristic from Phase 2.
        let spec = parse_problem("100x100F::30x30/20").unwrap();
        let (sheets, proven) = run_sync(spec);
        assert_eq!(sheets, 3, "3x3 grids cap every sheet at 9 copies");
        assert_eq!(proven, Some(true), "LP bound must tighten to match UB0");
    }

    #[test]
    fn stop_flag_respected() {
        let spec = Arc::new(parse_problem("100x100F::30x30/20").unwrap());
        let cfg = Arc::new(BpcConfig {
            progress_interval: 1,
            ..BpcConfig::default()
        });
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
