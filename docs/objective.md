# Objective function

The fitness of a solution is a three-level lexicographic tuple — **lower is better**
(except `layout_score` and `drop_consolidation_score`, which are maximized and
therefore reversed in `Ord`):

| Level | Field                      | Direction    | Meaning                                                                                                                                                                                                                 |
|-------|----------------------------|--------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 1     | `sheets_used`              | minimize     | Fused float: sheet count in the integer part, last-sheet fill fraction in the fractional part (see below). Any *k*-sheet solution is strictly better than any *(k+1)*-sheet solution regardless of the other levels.    |
| 2     | `layout_score`             | **maximize** | Cut-line concentration score + strip-structure score (see below). Higher means internal cuts align into a few long, reusable lines and pieces stack into mono-width strips — easier to cut and fewer fence repositions. |
| 3     | `drop_consolidation_score` | **maximize** | Sum of squared areas of the disjoint free-region partition of the **last sheet only**, computed straight from `placements`. Rewards a few large, reusable offcuts over many small scattered scraps.                     |

## sheets_used (fused sheet-count + last-sheet-fill encoding)

```
sheets_used = (sheets_used_int - 1) + piece_area_on_last_sheet / sheet_area
```

This single `f64` folds two priorities into one `total_cmp`-comparable value, replacing
what used to be a separate integer level plus a `piece_area_on_last_sheet` tie-break:
the integer part is `sheets_used_int - 1` (so it lies in `[sheets_used_int - 1,
sheets_used_int)`), and the fractional part is the fill fraction of the *last* sheet —
`placements` on `sheets_used() - 1` weighted by piece area, divided by `sheet_area`.
Because the fraction is always `< 1`, any *k*-sheet solution still strictly beats any
*(k+1)*-sheet solution, exactly as the plain integer count did. Within the same sheet
count, a *lower* fill fraction on the last sheet wins — i.e. the GA is pushed to
consolidate placed area onto the earlier, already-committed sheets and leave the last
one as empty as possible, rather than spreading the same total placed area evenly
across the last two sheets. `Objective::sheets_used_int()` recovers the plain integer
count when only that is needed (e.g. for display).

---

## drop_consolidation_score

See [`drop_consolidation_score`](drop_consolidation_score.md) — rewards a few large,
reusable offcuts on the last sheet over many small scattered scraps, and explains why
only the last sheet is considered.

This is the same "compute from `placements`, not `Solution.leftovers`" reasoning that
motivates computing `layout_score` from `placements` too, since guillotine free-space
decomposition is not unique across decoders (SLAS, GLAS, BFDH, ... can each split an
identical final set of `placements` into differently-shaped `FreeRect` lists).

---

## layout_score (cut_line_concentration_score + strip_structure_score)

A guillotine cut is a single straight line across the full width (or height) of a
rectangular region. In practice this means one fence setting on a panel saw. When
several pieces line up along a common cut, the operator makes one pass and gets
multiple pieces — no fence repositioning needed between them.

The objective therefore needs to distinguish two solutions that use the same number
of sheets and leave the same amount of waste but differ in *how* pieces are grouped:

```
  Good layout                        Bad layout
  ┌─────────┬─────────┬─────────┐   ┌─────────┬─────────┬─────────┐
  │   A     │   A     │   A     │   │   A     │   B     │   A     │
  ├─────────┼─────────┼─────────┤   ├─────────┼─────────┼─────────┤
  │   B     │   B     │   B     │   │   B     │   A     │   B     │
  └─────────┴─────────┴─────────┘   └─────────┴─────────┴─────────┘
  One horizontal cut separates        Every row needs different cuts;
  all A from all B; vertical cuts     horizontal cut cannot be shared
  align across full width.            across the whole sheet.
```

`leftover_area` alone cannot distinguish these two layouts — it only sees waste
rectangles. `layout_score` is the tiebreaker that drives the GA toward manufacturable,
"technological" groupings. It has two components, each computed per sheet as a sum of
squared run lengths (scaled by `/10_000`) and detailed in its own doc:

- [`cut_line_concentration_score`](cut_line_concentration_score.md) — rewards cuts that
  align into a few long, full-span lines at a shared coordinate, regardless of the
  piece sizes on either side.
- [`strip_structure_score`](strip_structure_score.md) — additionally requires the piece
  *size* to match, rewarding mono-width/mono-height strips ripped in a single pass.

Both are computed from `Solution::placements` after decoding (piece dimensions are in
the expanded flat coordinate system where kerf is already absorbed, so adjacent pieces
touch directly — decoders themselves stay kerf-agnostic; see `docs/slas.md`).

The two components are summed: `layout_score = CUT_LINE_WEIGHT · concentration + STRIP_WEIGHT · strip`,
with `CUT_LINE_WEIGHT = 2, STRIP_WEIGHT = 3` (i.e. strip weighs 1.5x concentration; both
are squared-length sums on the same scale, so the ratio is exact, not an approximation —
scaling `layout_score` by a constant factor doesn't change `Ord` outcomes). Calibrated on
`generator` instances; revisit if a different piece mix suggests otherwise.

---

## staircase_area (implemented, but not used in Objective)

`Solution::staircase_area` is implemented (`#[allow(dead_code)]`) but **not** part of the
current `Objective` and not called at runtime. It may be worth restoring as a secondary
signal or objective level in a future experiment.

The staircase is the Pareto frontier of bottom-right corners `(x + w, y + h)` of all
placed pieces on a sheet, integrated into a step function from the top. It measures how
far pieces stray from the top-left corner — a fully packed sheet has staircase area
equal to the sheet area, and a sparse sheet has a large staircase.

| Large staircase (worse)                         | Small staircase (better)                        |
|-------------------------------------------------|-------------------------------------------------|
| <img src="img/staircase_large.png" width="380"> | <img src="img/staircase_small.png" width="380"> |

Computed per sheet as `Σ x_i · (y_i − y_{i-1})` over the sorted Pareto frontier;
`Solution::staircase_area` returns the maximum across all sheets.

---

## largest_usable_drop_area (implemented, not used in Objective)

`Solution::largest_usable_drop_area` is `drop_consolidation_score`'s sibling — same
decoder-agnostic free-region reconstruction from `placements`, but it reports the area
of the single largest free rectangle instead of the sum of squares. Like
`staircase_area`, it is `#[allow(dead_code)]` and not part of `Objective`: finding it
requires a slab-pair sweep over every pair of y-band boundaries, `O(p³)` per sheet,
which is fine as a one-off report on the final/best solution but too expensive to run
on every individual every generation.
