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

## Our implementation (`src/exact/`, plan `docs/plans/21_exact-bpc.md`)

A scaled-down adaptation of BPC for this codebase -- CG + RLMP + a pricing oracle at the root node (Phases 0-6), without the paper's K-scaled integer arithmetic (replaced by an epsilon margin when rounding the LP bound) and without Ryan-Foster branch-and-price (Phase 7, explicitly deferred as a sketch). See the plan itself for the design-deviation details.

### Verification on `real1.json` (2026-07-04)

Command:
```
cargo run --release -- calc --json src/web/real1.json --algorithm bpc --progress 10 --sink stdout
```

Input: 178 items (76 types), 2800x2070 sheet.

Result: `sheets_used: 8`, `proven_optimal: true` -- a proven optimum, and the run finished almost instantly (the root LP bound converged to match the heuristic UB0 after column generation, with no need for Phase 6 rounding). This confirms the paper's observation (§EC.2.4) that the 2BPP(-G) LP relaxation is usually very tight -- for this instance, the root-node CG alone was enough; Ryan-Foster branching (Phase 7) wasn't needed.

### `real2.json`: the Class-10-32 instance (2026-07-04)

`src/web/real2.json` is reverse-engineered from 11 rendered-sheet PNGs the user provided (`tmp/png2/`, untracked), extracted via connected-component analysis of the piece/leftover fill colors. The images turned out to be **Figure 5(a)** of this paper, "Initial Solution Found by the Hybrid Heuristic Using 11 Bins" for the classic, decades-unsolved **Class-10-32** instance (Table 13: `n=80, W=100, H=100, LB0=10, UB0=11`) -- not an arbitrary instance and not the optimal layout: Figure 5(b)/(c) (not given to us) show the true 10-bin optimum found by NE-BPC (188 nodes, 5232.60s) and their non-numerically-exact BPC variant (19 nodes, 359.24s).

The PNGs render at 12.93 px per unit (1293px = 100 units) with a 4px border stroke per rectangle; after rescaling (dividing by 12.93, folding the border back into each piece's width/height since 2BPP has no kerf) the reconstruction reproduces the paper's numbers exactly: area lower bound = 10 = `LB0`, and `jylanki` (our heuristic, unrelated to the paper's) also lands on 11 = `UB0`. Our BPC (root node + rounding only, no Ryan-Foster branch-and-price) predictably cannot close the gap to 10 -- same as the paper's own algorithms without branching would need real search to get there. This makes `real2.json` a good stand-in "hard instance" for exercising Phase 7 whenever it gets built.
