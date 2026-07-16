//! Pricing oracle for column generation: 2D guillotine knapsack.
//!
//! Given dual values `mu_i` per item (flat piece index), finds a single-sheet
//! guillotine pattern whose total profit `sum(mu_i)` exceeds `1 + RC_EPS`
//! (i.e. a column with negative reduced cost), or proves that none exists.
//!
//! Two stages:
//!
//! 1. **Greedy chain heuristic**: compose pieces one at a time via
//!    `min(h_cut, v_cut)` step functions, in profit-density and profit order.
//!    Sound (a fitting chain is an achievable layout, so only truly feasible
//!    patterns are returned) but incomplete: a left-associated chain can only
//!    express cut trees where one side of every cut is a single piece.  A 2x2
//!    grid of four unequal pieces that exactly tiles the sheet has no such
//!    tree, for any insertion order.
//! 2. **Exact branch-and-bound** over per-type copy counts (copies of a type
//!    are interchangeable, so an optimal pattern takes each type's top-mu
//!    copies as a prefix — branching is include-copy / block-type).  Pruning:
//!    fractional knapsack bound over the remaining area, plus exact guillotine
//!    feasibility via a memoized GLF DP over count vectors (`GlfOracle`).
//!    While the chain function of the current branch still fits the sheet it
//!    serves as a cheap sufficient feasibility witness; the oracle is invoked
//!    only once the chain loses track.  `NoneExists` is a proof that no
//!    pattern has profit > `1 + RC_EPS`; the CG loop (`mod.rs`) only trusts
//!    this as a certified LP bound once it holds, and separately subtracts
//!    `EPS_ROUND` (which must dominate `RC_EPS`) before rounding the RLMP
//!    objective down to an integer, so this margin never overstates the true
//!    lower bound. `Aborted` means a work budget ran out before either a
//!    column or the proof was found.
//!
//! The GLF DP is exponential in the number of pieces of a pattern — that is
//! the honest price of exactness without the paper's G2KP machinery — hence
//! the `max_nodes` / `max_cells` budgets and the `Aborted` outcome.
//!
//! Both piece and sheet dimensions must be kerf-expanded (`expand_problem`),
//! the same convention as the rest of the crate.

use std::collections::{HashMap, hash_map::Entry};

use super::bpc::Pattern;
use crate::{
    cut_tree::build_cut_tree,
    glf::{StepFn, eval_f, eval_f_inv, h_cut, min_fn, v_cut},
    model::{Piece, Placement, Problem, Sheet},
};

// Profit threshold: a pattern is improving iff sum(mu) > 1 + RC_EPS.
const RC_EPS: f64 = 1e-6;
// Copies with mu at or below this are never worth including in a pattern.
const MU_EPS: f64 = 1e-12;

// == Types =====================================================================

/// Work budgets for a single `price` call. The GLF cell and split budgets
/// belong to `GlfOracle`, which is shared across every node of the
/// branch-and-bound tree for one BPC run: the cache persists across the whole
/// run (that's the point of sharing it), but `max_nodes` and `max_splits` are
/// both per-`price`-call budgets -- see `GlfOracle`'s split-counter reset in
/// `Pricer::exact`. A lifetime-cumulative split budget was tried first and
/// rejected: one hard multiset key could exhaust it once and then every
/// later call -- even ones needing only cached or trivially cheap new keys --
/// would abort immediately for the rest of the run.
pub(crate) struct PricingLimits {
    /// Maximum branch-and-bound nodes explored per `price` call.
    pub max_nodes: usize,
    /// Maximum memoized GLF cells over the lifetime of the BPC run.
    pub max_cells: usize,
    /// Maximum GLF split-enumeration steps per `price` call (the counter is
    /// reset at the start of `Pricer::exact`). A single `GlfOracle::eval`
    /// call on a sparse key with `L` distinct types enumerates ~2^(L-1)
    /// splits even when every sub-key it recurses into is already cached (a
    /// cache hit is still one loop iteration), so `max_cells` alone does not
    /// bound this -- it only stops growth of the cache, not of an
    /// already-mostly-cached call's own enumeration.
    pub max_splits: usize,
}

impl Default for PricingLimits {
    fn default() -> Self {
        PricingLimits {
            max_nodes: 2_000_000,
            max_cells: 1_000_000,
            max_splits: 20_000_000,
        }
    }
}

/// Branch-and-price constraints restricting which patterns the pricing
/// oracle may return, expressed as pairs of *types* (not individual flat
/// items). Ryan-Foster branching normally operates on item pairs, but copies
/// of a type are interchangeable throughout `Pricer` (the exact search
/// already takes a mu-descending prefix of a type's copies, `DfsState`
/// tracks only per-type counts) -- branching on two copies of the same type
/// changes nothing (either copy is as good as the other), so the LP would
/// never converge; branching on types instead sidesteps that degeneracy.
/// Both fields hold `(a, b)` with `a < b`.
#[derive(Clone, Default)]
pub(crate) struct BranchConstraints {
    /// A pattern may not contain a copy of both types.
    forbidden: Vec<(usize, usize)>,
    /// A pattern must contain a copy of both types, or of neither. Checked
    /// only once a candidate pattern is complete (a leaf of the search) --
    /// unlike `forbidden`, there is no sound way to prune a partial pattern
    /// against this, since a currently-missing type might still be added.
    forced_together: Vec<(usize, usize)>,
}

impl BranchConstraints {
    /// A copy of `self` with an added forbidden type pair.
    pub(crate) fn with_forbidden(&self, a: usize, b: usize) -> Self {
        let mut c = self.clone();
        c.forbidden.push((a.min(b), a.max(b)));
        c
    }

    /// A copy of `self` with an added forced-together type pair.
    pub(crate) fn with_forced_together(&self, a: usize, b: usize) -> Self {
        let mut c = self.clone();
        c.forced_together.push((a.min(b), a.max(b)));
        c
    }
}

/// Outcome of one pricing round.
pub(crate) enum PriceOutcome {
    /// A guillotine-valid pattern with profit > 1 + RC_EPS.
    Column(Pattern),
    /// Proof: no single-sheet pattern has profit > 1 + RC_EPS.
    NoneExists,
    /// A work budget was exhausted before a column or a proof was found.
    Aborted,
}

/// Sparse count vector: sorted `(type_idx, count)` pairs, counts >= 1.
type CountKey = Vec<(u16, u16)>;

/// A piece placement expressed per type (flat copy assigned later).
struct TypePlacement {
    type_idx: usize,
    x: u32,
    y: u32,
    rotated: bool,
}

/// A distinct piece type: kerf-expanded dims + rotatability.
struct PType {
    width: u32,
    height: u32,
    can_rotate: bool,
    /// Flat indices of this type's copies, in `pieces` order.
    copies: Vec<usize>,
    /// Step function of a single copy (min over allowed orientations),
    /// clipped to the sheet.
    base: StepFn,
}

/// One candidate copy in the search, with its dual value as profit.
#[derive(Clone, Copy)]
struct Candidate {
    type_idx: usize,
    flat_idx: usize,
    profit: f64,
    area: u64,
    density: f64,
}

/// One accepted step of a greedy/exact chain: which candidate was added and
/// the chain step function after adding it.
struct ChainStep {
    candidate_pos: usize,
    f: StepFn,
}

/// Memoized GLF DP over sparse count vectors.
///
/// `eval` computes the guillotine layout function (min strip height per
/// width) for a multiset of pieces, clipped to the sheet: breakpoints with
/// `x > sheet_w` or `h > sheet_h` are dropped, so an empty function means
/// "does not fit on one sheet".  The cache depends only on geometry (not on
/// duals or branch constraints) and persists across pricing rounds -- owned
/// externally (one instance per BPC run, not per `Pricer`) so every node of
/// the branch-and-bound tree shares the same memoized cache instead of
/// rebuilding it from scratch per node.
pub(crate) struct GlfOracle {
    sheet_w: u32,
    sheet_h: u32,
    cache: HashMap<CountKey, StepFn>,
    max_cells: usize,
    /// Total split-enumeration steps taken so far, across all `eval` calls.
    splits: usize,
    max_splits: usize,
}

impl GlfOracle {
    pub(crate) fn new(sheet_w: u32, sheet_h: u32, max_cells: usize, max_splits: usize) -> Self {
        GlfOracle {
            sheet_w,
            sheet_h,
            cache: HashMap::new(),
            max_cells,
            splits: 0,
            max_splits,
        }
    }

    /// Reset the per-call split-enumeration counter. Called once at the
    /// start of every `Pricer::exact` invocation so a hard multiset key in
    /// one pricing call can't carry its exhaustion over into unrelated later
    /// calls -- see `PricingLimits::max_splits`'s doc.
    fn reset_splits(&mut self) {
        self.splits = 0;
    }
}

/// Signals that a work budget was exhausted mid-search.
struct Exhausted;

/// Result of attempting to include one more copy at a search node.
enum Include {
    Infeasible,
    /// Feasible; payload is the chain function after the include
    /// (`None` = the chain no longer tracks this branch, oracle took over).
    Feasible(Option<StepFn>),
}

/// Pricing oracle for a single BPC run. `GlfOracle` (the memoized GLF cache)
/// is owned externally and passed to `price` by `&mut` -- see its doc for why
/// it must be shared across branch-and-bound tree nodes rather than living
/// inside `Pricer`.
pub(crate) struct Pricer {
    pieces: Vec<Piece>,
    ptypes: Vec<PType>,
    sheet_w: u32,
    sheet_h: u32,
    limits: PricingLimits,
}

/// Mutable state of the exact branch-and-bound.
struct DfsState<'a> {
    candidates: &'a [Candidate],
    ptypes: &'a [PType],
    oracle: &'a mut GlfOracle,
    constraints: &'a BranchConstraints,
    sheet_area: u64,
    max_nodes: usize,
    nodes: usize,
    /// Copies included so far, per type.
    counts: Vec<u16>,
    /// Types whose remaining copies are excluded in the current subtree.
    blocked: Vec<bool>,
    /// Candidate positions included so far (in include order).
    includes: Vec<usize>,
    profit: f64,
    area: u64,
}

// == impl Pricer ===============================================================

impl Pricer {
    /// Group flat pieces into types and precompute their base step functions.
    /// Pieces that cannot fit the sheet in any orientation are dropped (such
    /// an instance is infeasible upstream anyway).
    pub(crate) fn new(pieces: &[Piece], sheet_w: u32, sheet_h: u32, limits: PricingLimits) -> Self {
        let mut type_map: HashMap<(u32, u32, bool), usize> = HashMap::new();
        let mut ptypes: Vec<PType> = Vec::new();
        for (idx, p) in pieces.iter().enumerate() {
            let key = (p.width, p.height, p.can_rotate);
            match type_map.entry(key) {
                Entry::Occupied(e) => {
                    ptypes[*e.get()].copies.push(idx);
                }
                Entry::Vacant(v) => {
                    let f_normal = vec![(p.width, p.height)];
                    let base = if p.can_rotate && p.width != p.height {
                        min_fn(&f_normal, &vec![(p.height, p.width)])
                    } else {
                        f_normal
                    };
                    let base = clip_to_sheet(base, sheet_w, sheet_h);
                    if base.is_empty() {
                        continue;
                    }
                    v.insert(ptypes.len());
                    ptypes.push(PType {
                        width: p.width,
                        height: p.height,
                        can_rotate: p.can_rotate,
                        copies: vec![idx],
                        base,
                    });
                }
            }
        }
        Pricer {
            pieces: pieces.to_vec(),
            ptypes,
            sheet_w,
            sheet_h,
            limits,
        }
    }

    /// Which internal type index a flat piece belongs to, or `None` if it
    /// was dropped in `new` (cannot fit the sheet in any orientation). Used
    /// by the branch-and-bound tree driver (`mod.rs`) to translate a
    /// pattern's flat item indices into type indices for RF-style type-pair
    /// branching.
    pub(crate) fn type_of(&self, flat_idx: usize) -> Option<usize> {
        self.ptypes.iter().position(|t| t.copies.contains(&flat_idx))
    }

    /// One pricing round for duals `mu` (indexed by flat piece index), under
    /// the current node's branch constraints. `oracle` is shared across
    /// every node of the branch-and-bound tree for one BPC run (see
    /// `GlfOracle`'s doc) -- callers must pass the same instance every time.
    pub(crate) fn price(
        &mut self,
        mu: &[f64],
        constraints: &BranchConstraints,
        oracle: &mut GlfOracle,
    ) -> PriceOutcome {
        let candidates = self.build_candidates(mu);
        if candidates.is_empty() {
            return PriceOutcome::NoneExists;
        }

        // Stage 1: greedy chain heuristic in two insertion orders.
        let density_order = (0..candidates.len()).collect::<Vec<_>>();
        let mut profit_order = density_order.clone();
        profit_order.sort_by(|&a, &b| candidates[b].profit.total_cmp(&candidates[a].profit));
        for order in [&density_order, &profit_order] {
            if let Some(p) = self.try_chain(&candidates, order, constraints) {
                return PriceOutcome::Column(p);
            }
        }

        // Stage 2: exact branch-and-bound.
        self.exact(&candidates, constraints, oracle)
    }

    // -- Candidate construction ------------------------------------------------

    /// Positive-mu copies of each type, mu-descending within a type, sorted
    /// globally by profit density (required by the fractional knapsack bound).
    fn build_candidates(&self, mu: &[f64]) -> Vec<Candidate> {
        let mut candidates: Vec<Candidate> = Vec::new();
        for (ti, t) in self.ptypes.iter().enumerate() {
            let area = t.width as u64 * t.height as u64;
            for &flat in &t.copies {
                if mu[flat] > MU_EPS {
                    candidates.push(Candidate {
                        type_idx: ti,
                        flat_idx: flat,
                        profit: mu[flat],
                        area,
                        density: mu[flat] / area as f64,
                    });
                }
            }
        }
        candidates.sort_by(|a, b| {
            b.density
                .total_cmp(&a.density)
                .then(a.type_idx.cmp(&b.type_idx))
                .then(b.profit.total_cmp(&a.profit))
        });
        candidates
    }

    // -- Stage 1: greedy chain -------------------------------------------------

    /// Greedily chain-compose candidates in the given order, keeping every
    /// copy whose addition still fits the sheet and does not violate a
    /// `forbidden` type pair against a type already in the chain. Returns a
    /// pattern only if the accumulated profit is improving and the result
    /// respects every `forced_together` pair.
    fn try_chain(&self, candidates: &[Candidate], order: &[usize], constraints: &BranchConstraints) -> Option<Pattern> {
        let sheet_area = self.sheet_w as u64 * self.sheet_h as u64;
        let mut steps: Vec<ChainStep> = Vec::new();
        let mut profit = 0.0f64;
        let mut area = 0u64;
        let mut included = vec![false; self.ptypes.len()];
        for &pos in order {
            let c = &candidates[pos];
            if area + c.area > sheet_area {
                continue;
            }
            if chain_forbidden(constraints, &included, c.type_idx) {
                continue;
            }
            let base = &self.ptypes[c.type_idx].base;
            let f = match steps.last() {
                None => base.clone(),
                Some(last) => compose(&last.f, base, self.sheet_w, self.sheet_h),
            };
            if f.is_empty() {
                continue;
            }
            steps.push(ChainStep { candidate_pos: pos, f });
            included[c.type_idx] = true;
            profit += c.profit;
            area += c.area;
        }
        if profit <= 1.0 + RC_EPS || !chain_satisfies_forced(constraints, &included) {
            return None;
        }
        let tps = chain_reconstruct(&steps, candidates, &self.ptypes, self.sheet_w)?;
        let includes = steps.iter().map(|s| s.candidate_pos).collect::<Vec<_>>();
        self.assemble(&tps, &includes, candidates)
    }

    // -- Stage 2: exact search ------------------------------------------------

    fn exact(
        &mut self,
        candidates: &[Candidate],
        constraints: &BranchConstraints,
        oracle: &mut GlfOracle,
    ) -> PriceOutcome {
        oracle.reset_splits();
        let n_types = self.ptypes.len();
        let (found, includes, counts) = {
            let mut dfs = DfsState {
                candidates,
                ptypes: &self.ptypes,
                oracle,
                constraints,
                sheet_area: self.sheet_w as u64 * self.sheet_h as u64,
                max_nodes: self.limits.max_nodes,
                nodes: 0,
                counts: vec![0u16; n_types],
                blocked: vec![false; n_types],
                includes: Vec::new(),
                profit: 0.0,
                area: 0,
            };
            let empty_chain: StepFn = vec![];
            match dfs.dfs(0, Some(&empty_chain)) {
                Err(Exhausted) => return PriceOutcome::Aborted,
                Ok(false) => return PriceOutcome::NoneExists,
                Ok(true) => (true, dfs.includes.clone(), dfs.counts.clone()),
            }
        };
        debug_assert!(found);
        let pattern = self
            .reconstruct_found(candidates, &includes, &counts, oracle)
            .and_then(|tps| self.assemble(&tps, &includes, candidates));
        match pattern {
            Some(p) => PriceOutcome::Column(p),
            None => {
                debug_assert!(false, "exact search found an improving set but reconstruction failed");
                PriceOutcome::Aborted
            }
        }
    }

    /// Reconstruct type-level placements for the found include set: first via
    /// chain recomposition (cheap), falling back to the exact oracle.
    fn reconstruct_found(
        &mut self,
        candidates: &[Candidate],
        includes: &[usize],
        counts: &[u16],
        oracle: &mut GlfOracle,
    ) -> Option<Vec<TypePlacement>> {
        let mut steps: Vec<ChainStep> = Vec::with_capacity(includes.len());
        for &pos in includes {
            let base = &self.ptypes[candidates[pos].type_idx].base;
            let f = match steps.last() {
                None => base.clone(),
                Some(last) => compose(&last.f, base, self.sheet_w, self.sheet_h),
            };
            if f.is_empty() {
                steps.clear();
                break;
            }
            steps.push(ChainStep { candidate_pos: pos, f });
        }
        if !steps.is_empty()
            && let Some(tps) = chain_reconstruct(&steps, candidates, &self.ptypes, self.sheet_w)
        {
            return Some(tps);
        }
        let key = sparse_key(counts);
        oracle.recon(&self.ptypes, &key)
    }

    // -- Pattern assembly & validation ------------------------------------------

    /// Map type-level placements to concrete flat copies (per-type queues in
    /// include order) and validate the result with the cut-tree checker.
    fn assemble(&self, tps: &[TypePlacement], includes: &[usize], candidates: &[Candidate]) -> Option<Pattern> {
        let mut queues: Vec<std::collections::VecDeque<usize>> =
            vec![std::collections::VecDeque::new(); self.ptypes.len()];
        for &pos in includes {
            let c = &candidates[pos];
            queues[c.type_idx].push_back(c.flat_idx);
        }
        let mut items = Vec::with_capacity(tps.len());
        let mut placements = Vec::with_capacity(tps.len());
        for tp in tps {
            let flat = queues[tp.type_idx].pop_front()?;
            items.push(flat);
            placements.push(Placement {
                sheet_idx: 0,
                piece_idx: flat,
                x: tp.x,
                y: tp.y,
                rotated: tp.rotated,
            });
        }
        if queues.iter().any(|q| !q.is_empty()) {
            debug_assert!(false, "placement/include count mismatch");
            return None;
        }
        if !self.validate(&placements) {
            debug_assert!(false, "pricing produced a non-guillotine pattern");
            return None;
        }
        Some(Pattern { items, placements })
    }

    /// Defensive guillotine check on a finished pattern (plan Phase 4 bullet).
    fn validate(&self, placements: &[Placement]) -> bool {
        let problem = Problem {
            sheet: Sheet {
                width: self.sheet_w,
                height: self.sheet_h,
            },
            pieces: self.pieces.clone(),
        };
        build_cut_tree(&problem, placements).is_ok()
    }
}

// == impl DfsState =============================================================

impl DfsState<'_> {
    /// Depth-first search over candidates. On `Ok(true)` the improving set is
    /// left in `self.includes` / `self.counts`.
    ///
    /// `chain`: `Some(f)` while the sequential chain still certifies the
    /// current branch (empty `f` = zero pieces so far); `None` once the chain
    /// failed and the oracle took over feasibility.
    fn dfs(&mut self, mut pos: usize, chain: Option<&StepFn>) -> Result<bool, Exhausted> {
        self.nodes += 1;
        if self.nodes > self.max_nodes {
            return Err(Exhausted);
        }
        while pos < self.candidates.len() && self.blocked[self.candidates[pos].type_idx] {
            pos += 1;
        }
        if pos == self.candidates.len() {
            // Reached with no more candidates to decide: accept only if the
            // set finalized this way is both improving and forced-consistent
            // (the `profit > ... ` shortcut below skips this path whenever
            // `forced_together` is trivially satisfied, i.e. on every node
            // without active constraints -- see `satisfies_forced`'s doc).
            return Ok(self.profit > 1.0 + RC_EPS && self.satisfies_forced());
        }
        if self.profit + self.frac_bound(pos) <= 1.0 + RC_EPS {
            return Ok(false);
        }
        let c = self.candidates[pos];
        // Include branch.
        if self.area + c.area <= self.sheet_area
            && !self.forbidden_with_included(c.type_idx)
            && let Include::Feasible(child_chain) = self.feasible_with(&c, chain)?
        {
            self.counts[c.type_idx] += 1;
            self.includes.push(pos);
            self.profit += c.profit;
            self.area += c.area;
            // Accepting here (instead of continuing to a longer, possibly
            // more-profitable set) is sound because the LP only needs *an*
            // improving column, not the best one -- but it is only valid
            // when `forced_together` already holds for the set as it
            // stands; otherwise a still-missing partner type might only be
            // reachable by continuing the search, so fall through to it.
            let accept_now = self.profit > 1.0 + RC_EPS && self.satisfies_forced();
            if accept_now || self.dfs(pos + 1, child_chain.as_ref())? {
                return Ok(true);
            }
            self.counts[c.type_idx] -= 1;
            self.includes.pop();
            self.profit -= c.profit;
            self.area -= c.area;
        }
        // Exclude branch: copies of a type are taken as a mu-descending
        // prefix, so skipping this copy blocks the type's remaining copies.
        self.blocked[c.type_idx] = true;
        let r = self.dfs(pos + 1, chain);
        self.blocked[c.type_idx] = false;
        r
    }

    /// Guillotine feasibility of `current set + c`: cheap chain witness first,
    /// exact oracle when the chain fails.  Chain infeasibility is *not* a
    /// proof (the chain is incomplete), oracle infeasibility is.
    fn feasible_with(&mut self, c: &Candidate, chain: Option<&StepFn>) -> Result<Include, Exhausted> {
        if let Some(prefix) = chain {
            let base = &self.ptypes[c.type_idx].base;
            let f = if prefix.is_empty() {
                base.clone()
            } else {
                compose(prefix, base, self.oracle.sheet_w, self.oracle.sheet_h)
            };
            if !f.is_empty() {
                return Ok(Include::Feasible(Some(f)));
            }
        }
        self.counts[c.type_idx] += 1;
        let key = sparse_key(&self.counts);
        let res = self.oracle.eval(self.ptypes, &key);
        self.counts[c.type_idx] -= 1;
        let f = res?;
        Ok(if f.is_empty() {
            Include::Infeasible
        } else {
            Include::Feasible(None)
        })
    }

    /// Fractional knapsack bound on the profit still obtainable from
    /// `candidates[pos..]` (skipping blocked types) within the remaining area.
    /// Valid because candidates are sorted by density descending.
    fn frac_bound(&self, pos: usize) -> f64 {
        let mut rem = self.sheet_area - self.area;
        let mut total = 0.0f64;
        for c in &self.candidates[pos..] {
            if self.blocked[c.type_idx] {
                continue;
            }
            if c.area <= rem {
                total += c.profit;
                rem -= c.area;
            } else {
                total += c.profit * (rem as f64 / c.area as f64);
                break;
            }
        }
        total
    }

    /// Whether including a copy of type `t` would put it in the same
    /// pattern as a type it is `forbidden` to accompany, given the types
    /// already included (`counts[other] > 0`).
    fn forbidden_with_included(&self, t: usize) -> bool {
        self.constraints.forbidden.iter().any(|&(a, b)| {
            let other = if a == t {
                b
            } else if b == t {
                a
            } else {
                return false;
            };
            self.counts[other] > 0
        })
    }

    /// Whether the current include set agrees with every `forced_together`
    /// pair (both types present, or both absent). Trivially `true` when
    /// `constraints.forced_together` is empty (the common case: no active
    /// branch constraint), which keeps this a no-op on the unbranched root.
    fn satisfies_forced(&self) -> bool {
        self.constraints
            .forced_together
            .iter()
            .all(|&(a, b)| (self.counts[a] > 0) == (self.counts[b] > 0))
    }
}

// == impl GlfOracle ============================================================

impl GlfOracle {
    /// GLF step function for the multiset `key`, clipped to the sheet.
    /// Empty result = the multiset cannot be guillotine-cut from one sheet.
    fn eval(&mut self, types: &[PType], key: &[(u16, u16)]) -> Result<StepFn, Exhausted> {
        if let Some(f) = self.cache.get(key) {
            return Ok(f.clone());
        }
        let total: u32 = key.iter().map(|&(_, k)| k as u32).sum();
        let f = if total == 1 {
            // Base functions are pre-clipped in Pricer::new.
            types[key[0].0 as usize].base.clone()
        } else {
            let mut best: StepFn = vec![];
            let mut split = vec![0u16; key.len()];
            while next_split(&mut split, key) {
                self.splits += 1;
                if self.splits > self.max_splits {
                    return Err(Exhausted);
                }
                if is_full_split(&split, key) || !is_canonical_split(&split, key) {
                    continue;
                }
                let (a_key, b_key) = split_keys(key, &split);
                let fa = self.eval(types, &a_key)?;
                if fa.is_empty() {
                    continue;
                }
                let fb = self.eval(types, &b_key)?;
                if fb.is_empty() {
                    continue;
                }
                let cand = min_fn(&h_cut(&fa, &fb), &v_cut(&fa, &fb));
                best = min_fn(&best, &cand);
            }
            clip_to_sheet(best, self.sheet_w, self.sheet_h)
        };
        if self.cache.len() >= self.max_cells {
            return Err(Exhausted);
        }
        self.cache.insert(key.to_vec(), f.clone());
        Ok(f)
    }

    /// Reconstruct type-level placements for `key` on a strip of the full
    /// sheet width. Sub-cells are served from the cache warmed by `eval`.
    fn recon(&mut self, types: &[PType], key: &[(u16, u16)]) -> Option<Vec<TypePlacement>> {
        let f = self.eval(types, key).ok()?;
        let h = eval_f(&f, self.sheet_w)?;
        let mut out = Vec::new();
        if self.rec(types, key, 0, 0, self.sheet_w, h, &mut out) {
            Some(out)
        } else {
            None
        }
    }

    /// Mirror of `GlfTable::recon_flat` over sparse count vectors: find a
    /// split whose H- or V-combination reproduces the region, recurse.
    #[allow(clippy::too_many_arguments)]
    fn rec(
        &mut self,
        types: &[PType],
        key: &[(u16, u16)],
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        out: &mut Vec<TypePlacement>,
    ) -> bool {
        let total: u32 = key.iter().map(|&(_, k)| k as u32).sum();
        if total == 1 {
            let ti = key[0].0 as usize;
            let t = &types[ti];
            let rotated = if t.width <= w && t.height <= h {
                false
            } else if t.can_rotate && t.height <= w && t.width <= h {
                true
            } else {
                return false;
            };
            out.push(TypePlacement {
                type_idx: ti,
                x,
                y,
                rotated,
            });
            return true;
        }
        // Ordered splits here (both (a,b) and (b,a)), unlike the DP in `eval`.
        let mut split = vec![0u16; key.len()];
        while next_split(&mut split, key) {
            if is_full_split(&split, key) {
                continue;
            }
            let (a_key, b_key) = split_keys(key, &split);
            let Ok(fa) = self.eval(types, &a_key) else {
                return false;
            };
            if fa.is_empty() {
                continue;
            }
            let Ok(fb) = self.eval(types, &b_key) else {
                return false;
            };
            if fb.is_empty() {
                continue;
            }
            if let (Some(h1), Some(h2)) = (eval_f(&fa, w), eval_f(&fb, w))
                && h1 + h2 == h
            {
                let snap = out.len();
                if self.rec(types, &a_key, x, y, w, h1, out) && self.rec(types, &b_key, x, y + h1, w, h2, out) {
                    return true;
                }
                out.truncate(snap);
            }
            if let (Some(w1), Some(w2)) = (eval_f_inv(&fa, h), eval_f_inv(&fb, h))
                && w1 + w2 <= w
            {
                let h1 = eval_f(&fa, w1).unwrap_or(h);
                let h2 = eval_f(&fb, w2).unwrap_or(h);
                let snap = out.len();
                if self.rec(types, &a_key, x, y, w1, h1, out) && self.rec(types, &b_key, x + w1, y, w2, h2, out) {
                    return true;
                }
                out.truncate(snap);
            }
        }
        false
    }
}

// == Free functions ============================================================

/// Walk a chain of compositions backwards, peeling one piece per step.
/// The prefix block is always anchored at the top-left of the region.
fn chain_reconstruct(
    steps: &[ChainStep],
    candidates: &[Candidate],
    ptypes: &[PType],
    sheet_w: u32,
) -> Option<Vec<TypePlacement>> {
    let last = steps.last()?;
    let mut w = sheet_w;
    let mut h = eval_f(&last.f, sheet_w)?;
    let mut out = Vec::with_capacity(steps.len());
    for i in (1..steps.len()).rev() {
        let type_idx = candidates[steps[i].candidate_pos].type_idx;
        let t = &ptypes[type_idx];
        let fp = &steps[i - 1].f;
        // H-branch: prefix block on top, the peeled piece below it.
        if let (Some(h1), Some(h2)) = (eval_f(fp, w), eval_f(&t.base, w))
            && h1 + h2 == h
        {
            out.push(TypePlacement {
                type_idx,
                x: 0,
                y: h1,
                rotated: rotated_for_height(t, h2, w)?,
            });
            h = h1;
            continue;
        }
        // V-branch: prefix block on the left, the peeled piece to its right.
        let (w1, w2) = (eval_f_inv(fp, h)?, eval_f_inv(&t.base, h)?);
        if w1 + w2 > w {
            return None;
        }
        out.push(TypePlacement {
            type_idx,
            x: w1,
            y: 0,
            rotated: rotated_for_width(t, w2, h)?,
        });
        h = eval_f(fp, w1).unwrap_or(h);
        w = w1;
    }
    let first_type = candidates[steps[0].candidate_pos].type_idx;
    let t0 = &ptypes[first_type];
    let rotated = if t0.width <= w && t0.height <= h {
        false
    } else if t0.can_rotate && t0.height <= w && t0.width <= h {
        true
    } else {
        return None;
    };
    out.push(TypePlacement {
        type_idx: first_type,
        x: 0,
        y: 0,
        rotated,
    });
    Some(out)
}

/// Orientation whose height is exactly `h` and whose width fits `max_w`.
fn rotated_for_height(t: &PType, h: u32, max_w: u32) -> Option<bool> {
    if t.height == h && t.width <= max_w {
        Some(false)
    } else if t.can_rotate && t.width == h && t.height <= max_w {
        Some(true)
    } else {
        None
    }
}

/// Orientation whose width is exactly `w` and whose height fits `max_h`.
fn rotated_for_width(t: &PType, w: u32, max_h: u32) -> Option<bool> {
    if t.width == w && t.height <= max_h {
        Some(false)
    } else if t.can_rotate && t.height == w && t.width <= max_h {
        Some(true)
    } else {
        None
    }
}

/// `chain_forbidden`/`chain_satisfies_forced` mirror
/// `DfsState::forbidden_with_included`/`satisfies_forced` for `try_chain`,
/// which tracks presence as a `Vec<bool>` rather than per-type counts.
fn chain_forbidden(constraints: &BranchConstraints, included: &[bool], t: usize) -> bool {
    constraints.forbidden.iter().any(|&(a, b)| {
        let other = if a == t {
            b
        } else if b == t {
            a
        } else {
            return false;
        };
        included[other]
    })
}

fn chain_satisfies_forced(constraints: &BranchConstraints, included: &[bool]) -> bool {
    constraints
        .forced_together
        .iter()
        .all(|&(a, b)| included[a] == included[b])
}

/// `min(h_cut, v_cut)` of a prefix block and a piece, clipped to the sheet.
/// Empty result = the combination cannot fit on one sheet.
fn compose(prefix: &StepFn, base: &StepFn, sheet_w: u32, sheet_h: u32) -> StepFn {
    clip_to_sheet(min_fn(&h_cut(prefix, base), &v_cut(prefix, base)), sheet_w, sheet_h)
}

/// Drop breakpoints that exceed the sheet in either dimension. Sound and
/// complete for sheet-bounded layouts: any sub-block of a fitting layout has
/// width <= sheet_w and height <= sheet_h, so only unusable combinations are
/// lost (see the module doc of `GlfOracle`).
fn clip_to_sheet(mut f: StepFn, sheet_w: u32, sheet_h: u32) -> StepFn {
    f.retain(|&(x, h)| x <= sheet_w && h <= sheet_h);
    f
}

/// Dense per-type counts -> sparse `CountKey`.
fn sparse_key(counts: &[u16]) -> CountKey {
    counts
        .iter()
        .enumerate()
        .filter(|&(_, &k)| k > 0)
        .map(|(t, &k)| (t as u16, k))
        .collect()
}

/// Odometer over sub-vectors of `key` (digit i in `0..=key[i].1`).
/// Returns `false` once the odometer rolls over (all-zero again).
/// The first call after an all-zero `split` yields the first nonzero vector.
fn next_split(split: &mut [u16], key: &[(u16, u16)]) -> bool {
    for (s, &(_, k)) in split.iter_mut().zip(key) {
        if *s < k {
            *s += 1;
            return true;
        }
        *s = 0;
    }
    false
}

fn is_full_split(split: &[u16], key: &[(u16, u16)]) -> bool {
    split.iter().zip(key).all(|(&s, &(_, k))| s == k)
}

/// Deduplicate unordered split pairs: keep `a` only if `a <= key - a`
/// lexicographically.
fn is_canonical_split(split: &[u16], key: &[(u16, u16)]) -> bool {
    for (&s, &(_, k)) in split.iter().zip(key) {
        match s.cmp(&(k - s)) {
            std::cmp::Ordering::Less => return true,
            std::cmp::Ordering::Greater => return false,
            std::cmp::Ordering::Equal => {}
        }
    }
    true
}

/// Split `key` into the sub-vector selected by `split` and its complement.
fn split_keys(key: &[(u16, u16)], split: &[u16]) -> (CountKey, CountKey) {
    let mut a = Vec::new();
    let mut b = Vec::new();
    for (&(t, k), &s) in key.iter().zip(split) {
        if s > 0 {
            a.push((t, s));
        }
        if k - s > 0 {
            b.push((t, k - s));
        }
    }
    (a, b)
}

// == Tests =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(w: u32, h: u32, rot: bool) -> Piece {
        Piece {
            name: String::new(),
            width: w,
            height: h,
            can_rotate: rot,
        }
    }

    fn pricer(pieces: &[Piece], w: u32, h: u32) -> (Pricer, GlfOracle) {
        let limits = PricingLimits::default();
        let oracle = GlfOracle::new(w, h, limits.max_cells, limits.max_splits);
        (Pricer::new(pieces, w, h, limits), oracle)
    }

    /// Assert the pattern is internally consistent and guillotine-valid.
    fn check_pattern(pieces: &[Piece], w: u32, h: u32, pat: &Pattern, mu: &[f64]) {
        assert_eq!(pat.items.len(), pat.placements.len());
        let profit: f64 = pat.items.iter().map(|&i| mu[i]).sum();
        assert!(profit > 1.0, "pattern profit {profit} must exceed 1");
        // No duplicate items.
        let mut seen = vec![false; pieces.len()];
        for &i in &pat.items {
            assert!(!seen[i], "duplicate item {i}");
            seen[i] = true;
        }
        // Placements within the sheet.
        for pl in &pat.placements {
            let p = &pieces[pl.piece_idx];
            let (pw, ph) = if pl.rotated {
                (p.height, p.width)
            } else {
                (p.width, p.height)
            };
            assert!(pl.x + pw <= w && pl.y + ph <= h, "placement out of sheet bounds");
        }
        // Independent guillotine check.
        let problem = Problem {
            sheet: Sheet { width: w, height: h },
            pieces: pieces.to_vec(),
        };
        build_cut_tree(&problem, &pat.placements).expect("pattern must be guillotine-valid");
    }

    // Two copies of one type side by side; found by the chain heuristic.
    #[test]
    fn column_found_for_two_copies_same_type() {
        let pieces = vec![piece(5, 5, false), piece(5, 5, false)];
        let mu = [0.9, 0.9];
        let (mut pr, mut oracle) = pricer(&pieces, 10, 5);
        match pr.price(&mu, &BranchConstraints::default(), &mut oracle) {
            PriceOutcome::Column(p) => {
                assert_eq!(p.items.len(), 2);
                check_pattern(&pieces, 10, 5, &p, &mu);
            }
            _ => panic!("expected a column"),
        }
    }

    // Total dual mass below 1: nothing to find, proven at the root bound.
    #[test]
    fn none_exists_when_duals_low() {
        let pieces = vec![piece(5, 5, false), piece(5, 5, false)];
        let (mut pr, mut oracle) = pricer(&pieces, 10, 5);
        assert!(matches!(
            pr.price(&[0.4, 0.4], &BranchConstraints::default(), &mut oracle),
            PriceOutcome::NoneExists
        ));
    }

    // A single piece whose dual alone exceeds 1: singleton pattern.
    #[test]
    fn singleton_column_when_dual_exceeds_one() {
        let pieces = vec![piece(5, 5, false)];
        let mu = [1.5];
        let (mut pr, mut oracle) = pricer(&pieces, 10, 10);
        match pr.price(&mu, &BranchConstraints::default(), &mut oracle) {
            PriceOutcome::Column(p) => {
                assert_eq!(p.items, vec![0]);
                check_pattern(&pieces, 10, 10, &p, &mu);
            }
            _ => panic!("expected a singleton column"),
        }
    }

    // 2x2 grid of unequal pieces exactly tiling 10x7. No left-associated
    // chain expresses this tree (both sides of the first cut hold 2 pieces),
    // so the greedy stage fails and the exact search + oracle must find it.
    #[test]
    fn grid_pattern_beyond_chain_heuristic() {
        let pieces = vec![
            piece(6, 4, false),
            piece(4, 4, false),
            piece(6, 3, false),
            piece(4, 3, false),
        ];
        let mu = [0.3; 4];
        let (mut pr, mut oracle) = pricer(&pieces, 10, 7);
        match pr.price(&mu, &BranchConstraints::default(), &mut oracle) {
            PriceOutcome::Column(p) => {
                assert_eq!(p.items.len(), 4, "all four grid pieces must be in the pattern");
                check_pattern(&pieces, 10, 7, &p, &mu);
            }
            _ => panic!("expected the full grid column"),
        }
    }

    // Same pieces on a 10x6 sheet. Feasible subsets top out at:
    // {B,C,D} = 0.9 (D stacked on C -> 6x6 block, B alongside) and
    // pairs with A = 0.85; every triple containing A is infeasible and the
    // full quad exceeds the sheet area. The root bound is ~1.28 > 1, so the
    // proof requires actually running the exact search to completion.
    #[test]
    fn none_exists_requires_search() {
        let pieces = vec![
            piece(6, 4, false),
            piece(4, 4, false),
            piece(6, 3, false),
            piece(4, 3, false),
        ];
        let (mut pr, mut oracle) = pricer(&pieces, 10, 6);
        assert!(matches!(
            pr.price(&[0.55, 0.3, 0.3, 0.3], &BranchConstraints::default(), &mut oracle),
            PriceOutcome::NoneExists
        ));
    }

    // Same grid instance, uniform mu = 0.3 each (full grid profit = 1.2).
    // Forbidding types 0 and 1 from co-occurring rules out the full grid;
    // any triple excluding one of them only reaches profit 0.9, never
    // exceeding the threshold -- so no improving pattern remains.
    #[test]
    fn forbidden_pair_blocks_the_combining_pattern() {
        let pieces = vec![
            piece(6, 4, false),
            piece(4, 4, false),
            piece(6, 3, false),
            piece(4, 3, false),
        ];
        let (mut pr, mut oracle) = pricer(&pieces, 10, 7);
        let constraints = BranchConstraints::default().with_forbidden(0, 1);
        assert!(matches!(
            pr.price(&[0.3; 4], &constraints, &mut oracle),
            PriceOutcome::NoneExists
        ));
    }

    // mu tuned so the unconstrained chain heuristic finds the triple
    // {0, 2, 3} (profit 1.05, excluding type 1) before the exact search is
    // ever needed. Forcing types 0 and 1 together rules that triple out (it
    // has 0 but not 1); the only pattern that both clears the profit
    // threshold and satisfies the constraint is the full 4-item grid
    // (profit 1.15) -- this exercises the leaf-level `satisfies_forced`
    // check deferring acceptance past the triple, in both the chain
    // heuristic and the exact search.
    #[test]
    fn forced_together_pair_requires_the_full_grid() {
        let pieces = vec![
            piece(6, 4, false),
            piece(4, 4, false),
            piece(6, 3, false),
            piece(4, 3, false),
        ];
        let mu = [0.35, 0.1, 0.35, 0.35];

        let (mut pr, mut oracle) = pricer(&pieces, 10, 7);
        match pr.price(&mu, &BranchConstraints::default(), &mut oracle) {
            PriceOutcome::Column(p) => {
                let mut items = p.items.clone();
                items.sort_unstable();
                assert_eq!(
                    items,
                    vec![0, 2, 3],
                    "expected the unconstrained triple, excluding type 1"
                );
            }
            _ => panic!("expected a column"),
        }

        let (mut pr2, mut oracle2) = pricer(&pieces, 10, 7);
        let constraints = BranchConstraints::default().with_forced_together(0, 1);
        match pr2.price(&mu, &constraints, &mut oracle2) {
            PriceOutcome::Column(p) => {
                let mut items = p.items.clone();
                items.sort_unstable();
                assert_eq!(
                    items,
                    vec![0, 1, 2, 3],
                    "must include the full grid to satisfy forced-together"
                );
                check_pattern(&pieces, 10, 7, &p, &mu);
            }
            _ => panic!("expected the full grid under forced-together"),
        }
    }

    // Rotatable 4x6 pieces on a 6x8 sheet: only the rotated stack fits.
    #[test]
    fn rotation_used_in_pattern() {
        let pieces = vec![piece(4, 6, true), piece(4, 6, true)];
        let mu = [0.6, 0.6];
        let (mut pr, mut oracle) = pricer(&pieces, 6, 8);
        match pr.price(&mu, &BranchConstraints::default(), &mut oracle) {
            PriceOutcome::Column(p) => {
                assert_eq!(p.items.len(), 2);
                assert!(p.placements.iter().all(|pl| pl.rotated), "both pieces must be rotated");
                check_pattern(&pieces, 6, 8, &p, &mu);
            }
            _ => panic!("expected a column with rotated pieces"),
        }
    }

    // Node budget too small for the grid instance (chain can't find it,
    // exact search is cut off): honest Aborted.
    #[test]
    fn aborted_when_node_budget_tiny() {
        let pieces = vec![
            piece(6, 4, false),
            piece(4, 4, false),
            piece(6, 3, false),
            piece(4, 3, false),
        ];
        let limits = PricingLimits {
            max_nodes: 2,
            max_cells: 200_000,
            max_splits: 2_000_000,
        };
        let mut oracle = GlfOracle::new(10, 7, limits.max_cells, limits.max_splits);
        let mut pr = Pricer::new(&pieces, 10, 7, limits);
        assert!(matches!(
            pr.price(&[0.3; 4], &BranchConstraints::default(), &mut oracle),
            PriceOutcome::Aborted
        ));
    }

    // GLF cell budget too small: the oracle aborts mid-DP.
    #[test]
    fn aborted_when_cell_budget_tiny() {
        let pieces = vec![
            piece(6, 4, false),
            piece(4, 4, false),
            piece(6, 3, false),
            piece(4, 3, false),
        ];
        let limits = PricingLimits {
            max_nodes: 200_000,
            max_cells: 3,
            max_splits: 2_000_000,
        };
        let mut oracle = GlfOracle::new(10, 7, limits.max_cells, limits.max_splits);
        let mut pr = Pricer::new(&pieces, 10, 7, limits);
        assert!(matches!(
            pr.price(&[0.3; 4], &BranchConstraints::default(), &mut oracle),
            PriceOutcome::Aborted
        ));
    }

    // GLF split budget too small: even though every sub-key the top-level
    // call recurses into ends up cached, the top-level split-enumeration
    // loop itself is cut off -- this is the case max_cells alone cannot
    // catch (see PricingLimits::max_splits doc).
    #[test]
    fn aborted_when_split_budget_tiny() {
        let pieces = vec![
            piece(6, 4, false),
            piece(4, 4, false),
            piece(6, 3, false),
            piece(4, 3, false),
        ];
        let limits = PricingLimits {
            max_nodes: 200_000,
            max_cells: 200_000,
            max_splits: 1,
        };
        let mut oracle = GlfOracle::new(10, 7, limits.max_cells, limits.max_splits);
        let mut pr = Pricer::new(&pieces, 10, 7, limits);
        assert!(matches!(
            pr.price(&[0.3; 4], &BranchConstraints::default(), &mut oracle),
            PriceOutcome::Aborted
        ));
    }

    // All duals are zero: no candidates at all.
    #[test]
    fn none_exists_for_zero_duals() {
        let pieces = vec![piece(5, 5, false)];
        let (mut pr, mut oracle) = pricer(&pieces, 10, 10);
        assert!(matches!(
            pr.price(&[0.0], &BranchConstraints::default(), &mut oracle),
            PriceOutcome::NoneExists
        ));
    }
}
