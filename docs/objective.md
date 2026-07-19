# Objective function

The fitness of a solution is a three-level lexicographic tuple — `sheets_used` is minimized,
`layout_score` and `drop_consolidation_score`, are maximized and the `Ord` is implemented correspondingly:

| Level | Field                      | Direction | Meaning                                                                                                                                                                                                                 |
|-------|----------------------------|-----------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 1     | `sheets_used`              | minimize  | Fused float: sheet count in the integer part, last-sheet fill fraction in the fractional part (see below). Any *k*-sheet solution is strictly better than any *(k+1)*-sheet solution regardless of the other levels.    |
| 2     | `layout_score`             | maximize  | Cut-line concentration score + strip-structure score (see below). Higher means internal cuts align into a few long, reusable lines and pieces stack into mono-width strips — easier to cut and fewer fence repositions. |
| 3     | `drop_consolidation_score` | maximize  | Sum of squared areas of the disjoint free-region partition of the **last sheet only**, computed straight from `placements`. Rewards a few large, reusable offcuts over many small scattered scraps.                     |

## sheets_used (fused sheet-count + last-sheet-fill encoding)

```
sheets_used = (sheets_used_int - 1) + piece_area_on_last_sheet / sheet_area
```

Example (`x` marks area occupied by a piece):
```
  ┌─────────┐ ┌─────────┐ ┌─────────┐
  │ x x x x │ │ x x x x │ │ x x┌────┤
  │ x x x x │ │ x x x x │ ├────┘    │
  └─────────┘ └─────────┘ └─────────┘
```
3 sheets total, the first two are considered fully packed and the last one half full, so
`sheets_used_int = 3` and `sheets_used = (3 - 1) + 0.5 = 2.5`.

Lower fraction wins within the same count and this pushes the GA to consolidate area onto
earlier sheets and leave the last one as empty as possible. `Objective::sheets_used_int()` recovers the
plain integer count for display.

---

## [drop_consolidation_score](drop_consolidation_score.md)

Rewards a few large, reusable offcuts on the last sheet over many small scattered
scraps; see the linked doc for the algorithm, why only the last sheet counts, and why
it's re-derived from `placements` rather than `Solution.leftovers`.

---

## layout_score ([cut_line_concentration_score](cut_line_concentration_score.md) + [strip_structure_score](strip_structure_score.md))

Metric distinguishing two same-sheet-count, same-waste solutions that differ in
*how* pieces are grouped — `leftover_area` alone can't see that, only waste rectangles.
Sum of two components, each a per-sheet sum of squared run lengths (scaled by
`/10_000`), detailed in their own docs:

- [cut_line_concentration_score](cut_line_concentration_score.md) — cuts aligning into
  a few long, full-span lines at a shared coordinate, any piece sizes.
- [strip_structure_score](strip_structure_score.md) — additionally requires the piece
  *size* to match, rewarding mono-width/mono-height strips.

Both computed from `Solution::placements` after decoding.

Combined as `layout_score = CUT_LINE_WEIGHT * concentration + STRIP_WEIGHT * strip`,
with `CUT_LINE_WEIGHT = 2, STRIP_WEIGHT = 3`. The constants 2 and 3 were gotten experimentally,
calibrated on `generator` instances; revisit if a different piece mix suggests otherwise.

---

## staircase_area (implemented, unused)

`Solution::staircase_area` (`#[allow(dead_code)]`) — not part of `Objective`, candidate
secondary metric for a future experiment. Pareto frontier of bottom-right corners
`(x+w, y+h)`, integrated into a step function from the top: measures how far pieces
stray from the top-left corner (full sheet = staircase area equals sheet area).

| Large staircase (worse)                         | Small staircase (better)                        |
|-------------------------------------------------|-------------------------------------------------|
| <img src="img/staircase_large.png" width="380"> | <img src="img/staircase_small.png" width="380"> |

Per sheet: `Σ x_i · (y_i − y_{i-1})` over the sorted frontier; returns the max across sheets.

---

## largest_usable_drop_area (implemented, unused)

`drop_consolidation_score`'s sibling — same free-region reconstruction from
`placements`, but reports the single largest free rectangle instead of sum of squares.
`#[allow(dead_code)]`: needs an `O(p³)`-per-sheet slab-pair sweep, fine as a one-off
report but too slow, since it has to be calculated per-individual for each GA generation.
