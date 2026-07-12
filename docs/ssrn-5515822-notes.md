# Notes: "A Highly Efficient Numerically Exact Algorithm for Two-Dimensional Bin-Packing Problems"

Wang, Baldacci, Furini, Wei, Liu (SSRN preprint, 2025). Source: `ssrn-5515822.pdf`.

## What they solve

2BPP: pack `n` rectangular items into the minimum number of identical sheets, no rotation, no overlap (plain orthogonal packing). 2BPP-G is the same problem with the added requirement that the layout on each sheet be **guillotine-cuttable** (recursively: ≤3 items, or a through x/y cut exists that splits the sheet into two guillotine-cuttable sub-sheets). This is exactly the `cutting` crate's problem statement (minus the secondary criteria -- their objective is pure `min(sheets used)`, with no analogue of `layout_score`/`drop_consolidation_score`).

## Main technical contribution

**The first "numerically exact" branch-price-and-cut (BPC) algorithm for both problems at once.** The key idea isn't the formulation itself (a classic set-covering model over patterns, columns = feasible single-sheet packings), but that the whole arithmetic is guaranteed to avoid floating-point LP solver rounding errors:

- The master LP's dual solution (from CPLEX) is **scaled to integers** (`K=10^11`) and everything downstream is computed in fixed-point `i64`.
- Four formal propositions prove that the rounded integer lower bounds stay valid (never exceed the true optimum) even after scaling and truncation.
- Motivation: floating-point dual solutions from CPLEX/Gurobi can be slightly suboptimal or infeasible due to cancellation errors -- this either hangs column generation in an infinite loop, or produces an incorrect (too optimistic) lower bound, which is unacceptable for an "exact" algorithm. The authors give a concrete example (§5.4): their own floating-point version of `BPC` once "proves" an optimum that isn't actually correct (an 11th-decimal-digit discrepancy fooled CPLEX's tolerance).

**The pricing problem** (finding a pattern with negative reduced cost) is reduced to a generalized 2D Knapsack Problem (G2KP) with RF branching constraints (Ryan-Foster: a pair of items must be on the same/different sheets) -- solved by their own separate exact algorithm (Wang et al. 2025a), adapted for guillotine-cuttability by pruning the enumeration tree (dropping non-guillotine nodes) plus a BFH1 heuristic (a single-sheet guillotine packer) instead of a skyline heuristic.

**Primal heuristics** (a substantial part, not an afterthought): a hybrid heuristic (fit-based + triple-block + goal-driven iterated local search with tabu search and "squeeze" -- empty out 2 sheets and repack from scratch prioritizing area), a diving heuristic (incomplete depth-first search without expensive exact pricing), a rounding heuristic (rounding the fractional LP solution via CPLEX over a restricted pattern pool). An ablation (Tables 9-11) shows the algorithm is 18-23x slower without these heuristics -- they are not optional, but critical.

## Results (paper)

On the classic benchmark (Berkey&Wang 1987 + Martello&Vigo 1998, 500+500 instances, up to 100 items) -- 99%/97.6% solved within an hour, beating the prior SOTA (CHIP21 for 2BPP, PS07 for 2BPP-G) by **24 and 109 additional** proven optima and 77-87% faster. On a newer, larger benchmark (up to 500 items) -- 89%/84% within an hour, average gap 0.46-1.02%.

## Relevance to the `cutting` project

1. **This is an exact solver for pure `sheets_used`, not our multi-level Objective.** There is no direct port of the algorithm -- we have a GA plus decoders (SLAS/GLAS/BFDH/...) with secondary criteria (`layout_score`, `drop_consolidation_score`) that don't exist in this problem formulation at all. Trying to "embed" their whole BPC is not worthwhile.

2. **This is a direct continuation of the backlog idea "exact solver in the Martello-Vigo style on top of GLF"** (`project_improvement_ideas.md`, item 7) -- that reference pointed exactly at Pisinger-Sigurd 2007 and Martello-Vigo 1998 as baselines. This paper is the current (2025) successor of that exact line of work, specifically for the guillotine case, and explicitly notes that the previous best exact 2BPP-G algorithm (PS07) hadn't been updated in **20 years**. If we ever revisit this idea, this is the first source worth rereading in detail (Section 3, G2KP+pricing).

3. **GDRR22 (Gardeyn & Wauters) -- `reference_gdrr2bp.md`** -- appears in this paper as the current SOTA heuristic for 2BPP-G (Table 4, Fig 4b); the authors ran it themselves on their own hardware. Their own hybrid heuristic beats GDRR22 by only a small margin (7233 vs 7241 sheets over 500 instances -- a difference of ~8 sheets), confirming GDRR is a serious competitor, not obsolete.

4. **A potentially portable idea -- the "squeeze" subroutine** (§EC.1.3.1, Algorithm 2, Figure EC.1): take 2 random sheets, empty them out, repack from scratch with a greedy packer starting from the largest item. A simple, cheap local move -- could be added as another kind of mutation/local-improvement in the GA (separate from OX/CX); conceptually similar to "ruin & recreate" from GDRR, but simpler to implement.

5. **The formal definition of guillotine-cuttability** (recursion: ≤3 items OR a through cut exists) -- the same definition implicitly implemented via `cut_tree`/decoders. Could be used as an independent oracle validator ("check that an arbitrary set of placements is guillotine-cuttable") separate from our decoders -- useful for tests if we ever need to validate solutions produced by algorithms other than our own (e.g., when comparing against GDRR/PS07).

## Our implementation (`src/exact/`)

A scaled-down adaptation of BPC for this codebase -- CG + RLMP + a pricing oracle at the root node, plus Ryan-Foster branch-and-price on top (see below), without the paper's K-scaled integer arithmetic (replaced by an epsilon margin when rounding the LP bound).

### Verification on `real1.json` (2026-07-04)

Command:
```
cargo run --release -- calc --json src/web/real1.json --algorithm bpc --progress 10 --sink stdout
```

Input: 178 items (76 types), 2800x2070 sheet.

Result at the time: `sheets_used: 8`, `proven_optimal: true`, almost instantly (`lb0 == ub0` at the area-bound/Jylanki-heuristic stage, before any column generation). **Update (2026-07-05, Phase 7 verification):** re-running the same command against the current `real1.json` no longer reproduces this -- `jylanki` alone now returns `sheets_used: 9` (not 8), so `lb0 = 8 < ub0 = 9` and the solver falls through into full column generation, which does not converge within tens of thousands of iterations (same behavior confirmed on both the pre-Phase-7 root-only code and the new branch-and-bound code, so this is not a Phase 7 regression). Neither `real1.json`'s content (`git log` shows no changes since before this note was written) nor the BPC code path leading to the early-exit check changed in a way that would explain the heuristic's answer changing from 8 to 9 -- this is flagged here as an open discrepancy against this note's original claim, not yet root-caused.

### `real2.json`: the Class-10-32 instance (2026-07-04)

`src/web/real2.json` is reverse-engineered from 11 rendered-sheet PNGs the user provided (`tmp/png2/`, untracked), extracted via connected-component analysis of the piece/leftover fill colors. The images turned out to be **Figure 5(a)** of this paper, "Initial Solution Found by the Hybrid Heuristic Using 11 Bins" for the classic, decades-unsolved **Class-10-32** instance (Table 13: `n=80, W=100, H=100, LB0=10, UB0=11`) -- not an arbitrary instance and not the optimal layout: Figure 5(b)/(c) (not given to us) show the true 10-bin optimum found by NE-BPC (188 nodes, 5232.60s) and their non-numerically-exact BPC variant (19 nodes, 359.24s).

The PNGs render at 12.93 px per unit (1293px = 100 units) with a 4px border stroke per rectangle; after rescaling (dividing by 12.93, folding the border back into each piece's width/height since 2BPP has no kerf) the reconstruction reproduces the paper's numbers exactly: area lower bound = 10 = `LB0`, and `jylanki` (our heuristic, unrelated to the paper's) also lands on 11 = `UB0`. Our BPC (root node + rounding only, no Ryan-Foster branch-and-price) predictably cannot close the gap to 10 -- same as the paper's own algorithms without branching would need real search to get there. This made `real2.json` the intended target for Phase 7.

#### Phase 7 (Ryan-Foster-style branch-and-price) implementation (2026-07-05)

Implemented as `BranchConstraints` (forbidden/forced-together *type* pairs, not individual items -- branching on individual items doesn't work here since copies of a type are interchangeable throughout `Pricer`, so the LP would never converge), a shared `GlfOracle`/`Pricer` across the whole branch-and-bound tree, and a DFS tree of `BpcNode`s with `pick_fractional_type_pair` selecting the branching variable. Verified via:

- Targeted `pricing.rs` unit tests (`forbidden_pair_blocks_the_combining_pattern`, `forced_together_pair_requires_the_full_grid`) confirming the pricing oracle actually respects both constraint kinds.
- A `mod.rs` unit test (`pick_fractional_type_pair_finds_a_fractional_pair`) reproducing the textbook Ryan-Foster example (3 items, pairwise patterns only, LP optimum 1.5) and confirming the right type pair is found from a hand-built `Rlmp`/`BpcNode` fixture.
- A `child_node` unit test confirming a filtered-out pattern is actually dropped from the rebuilt RLMP, not just from bookkeeping.
- Full regression: all 134 existing tests still pass; `real1.json`'s early-exit path (`lb0 == ub0` in the old, now-inaccurate sense above) is unaffected since the tree is never even constructed when that check fires.

**Running against `real2.json` does not yet reach `Optimal`.** The root node's own column generation exhausts `PricingLimits`'s pricing budget (`PriceOutcome::Aborted`, not `NoneExists`) before it ever converges once -- branching only ever triggers *after* a node's CG converges, so Phase 7 never gets a chance to run on this instance; the result is the same `Gap { ub: 11 }` as before, honestly reported (not a hang, not a crash, terminates in well under a minute). This is a pre-existing Phase 4/5 bottleneck (the exact pricing search giving up on a genuinely large instance -- 80 items, mostly count=1 types), not a defect in Phase 7 itself.

A deliberate side effect of this: the branch-and-bound code path (child node construction, `pending_lb` bookkeeping, `any_node_incomplete` tracking) has only been exercised by the unit tests above, not by an actual end-to-end solve reaching a real branch. Several small hand-crafted 2D cutting instances were tried (grid-tiling variants of the `6x4/4x4/6x3/4x3` pieces used in `pricing.rs`'s own tests, at various counts and sheet sizes) specifically to find one that converges at the root with a genuine gap needing a branch -- none did; the root LP bound turned out tight enough to resolve via `round_gap` (Phase 6) alone every time. This matches the paper's own observation (§EC.2.4) that 2BPP(-G) LP relaxations are usually very tight in practice, which is a plausible (if inconclusive) explanation for why an easy small counterexample is hard to construct by hand.

#### Root-cause investigation of the pricing abort on `real2.json` (2026-07-09)

Instrumented `GlfOracle`'s three exhaustion points (`max_nodes`, `max_splits`, `max_cells`) with temporary tracing to see exactly which budget kills the root node's column generation. Found and fixed a real bug: `max_splits` (the GLF split-enumeration counter) was, by design, cumulative over the *entire* BPC run's lifetime, same as `max_cells` -- but unlike `max_cells` (whose cache persisting is exactly the point), a lifetime-cumulative split counter means **one** combinatorially hard multiset key anywhere in the run permanently exhausts it, after which *every* later `price` call aborts immediately, even ones that only need already-cached or trivially cheap new keys. `max_nodes` (the DFS search-node budget) was already correctly scoped per `price` call. Fixed by resetting `GlfOracle`'s split counter at the start of every `Pricer::exact` call (`GlfOracle::reset_splits`), keeping the cache itself lifetime-scoped. This is a genuine correctness/robustness improvement independent of whether it closes the gap on any particular instance: confirmed via `cargo test` (134/134 still pass) and by rerunning `real2.json`, where the root node now gets roughly twice as far (~1900-2000 CG iterations before aborting, versus ~1045 before the fix) before hitting the *next* wall.

That next wall is `max_cells`: the memoized GLF cache reaches its 1,000,000-entry cap after ~1900 root-node CG iterations on `real2.json`, confirmed by re-instrumenting that specific exhaustion path. Once full, no new multiset key can ever be cached again, so the root's CG aborts for good (branching, which only fires after a node's CG *converges*, never gets a chance to run). Tried raising `max_cells` 5x (to 5,000,000) as an experiment: no meaningful improvement -- the run didn't converge even after a 280s timeout (versus 80s to hit the wall at the 1M default), and per-iteration throughput fell roughly 3x (~24 iter/s to ~7 iter/s), consistent with each new cache insertion getting more expensive as the cache grows and as CG iterations progress toward harder-to-price dual vectors. Reverted `max_cells` back to 1,000,000 -- raising it further looks like a bad trade (much more memory and CPU for uncertain, possibly negative, payoff) rather than a path to convergence.

**Conclusion:** this is a genuine scale limitation of the current pricing oracle (memoized GLF DP + backtracking search over individual items), not a shallow bug or a tunable-constant problem. An 80-item, mostly-count-1 instance has a combinatorial multiset space that the current oracle cannot exhaustively cache through before the root LP converges. Closing this gap for real would need an algorithmic upgrade considered out of scope for this project (the paper's own G2KP solver with meet-in-the-middle, Cote & Iori 2018) -- not further constant-tuning of the existing oracle.
