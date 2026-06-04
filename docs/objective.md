# Objective function

The fitness of a solution is a three-level lexicographic tuple — **lower is better**
(except `shared_edge_score`, which is maximised and therefore reversed in `Ord`):

| Level | Field               | Direction    | Meaning                                                                                                                                                                             |
|-------|---------------------|--------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 1     | `sheets_used`       | minimize     | Number of stock sheets consumed. Any *k*-sheet solution is strictly better than any *(k+1)*-sheet solution regardless of the other levels.                                          |
| 2     | `shared_edge_score` | **maximize** | Weighted total length of shared cut lines between adjacent pieces (see below). Higher means pieces align along common guillotine cuts — easier to cut and fewer fence repositions.  |
| 3     | `leftover_area`     | minimize     | Area of the largest single leftover rectangle across all sheets. Prefers solutions where waste is concentrated in one big reusable offcut rather than scattered in many small ones. |

---

## leftover_area

`Solution::leftover_area` returns the area of the **single largest** unused rectangle
across all sheets — not the total waste. The intent: a large contiguous offcut is
commercially reusable (can be fed back into the stock), while the same area fragmented
into many small scraps is not. By minimising the largest offcut the GA is nudged toward
using that space for pieces rather than leaving a big gap.

```
  Low leftover_area (better)          High leftover_area (worse)
  ┌─────────┬─────────┬─────────┐    ┌─────────┬─────────┬─────────┐
  │    A    │    A    │    A    │    │    A    │    A    │ leftover│
  ├─────────┼─────────┼─────────┤    │         │         │  (big)  │
  │    B    │    B    │ scrap   │    ├─────────┼─────────┘         │
  └─────────┴─────────┴─────────┘    │    B    │                   │
  Many tiny scraps, largest is small └─────────┴───────────────────┘
```

Computed as `max(fr.w * fr.h)` over `Solution::leftovers`.

---

## shared_edge_score

A guillotine cut is a single straight line across the full width (or height) of a
rectangular region. In practice this means one fence setting on a panel saw. When
pieces of the same size line up along a common cut, the operator makes one pass and
gets multiple pieces — no fence repositioning needed between them.

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

In the good layout every internal cut line is shared by multiple pieces on both sides:
the horizontal cut is full-width and the vertical cuts repeat at the same positions in
both rows. `shared_edge_score` is high because `h == e1 == e2` (full flush match) on
every boundary and `d1 == d2` (same-size bonus) between same-type neighbours.

In the bad layout pieces of different sizes alternate, so no cut line is reusable
across rows. The score is low despite identical sheet count and waste.

`leftover_area` alone cannot distinguish these two layouts — it only sees waste
rectangles. `shared_edge_score` is the tiebreaker that drives the GA toward
manufacturable groupings.

### Pairwise term

For every pair of pieces on the same sheet that share a boundary segment of length `h`:

- `e1`, `e2` — full edge lengths of each piece along the shared boundary axis
- `d1`, `d2` — their dimensions perpendicular to that boundary

| Condition                      | Score                                        |
|--------------------------------|----------------------------------------------|
| `h == e1 == e2` and `d1 == d2` | `30 · h` — identical pieces, edges flush     |
| `h == e1 == e2`                | `20 · h` — edges flush, different other size |
| `h == e1` or `h == e2`         | `h` — one edge fully spans the other         |
| otherwise                      | `0` — partial overlap on both sides          |

```
  30·h case (d1==d2)       20·h case               1·h case (h==e2)
  ┌─────────┐ ┌─────────┐  ┌───────────┐ ┌──────┐  ┌────────────┐ ┌────┐
  │         │ │         │  │           │ │      │  │            │ │    │
  │  pw×ph  │ │  pw×ph  │  │   pw×ph   │ │pw×p2 │  │   pw×ph    │ │pw2 │
  │         │ │         │  │           │ │      │  │            │ │×p2 │
  └─────────┘ └─────────┘  └───────────┘ └──────┘  └────────────┘ │    │
                                                                  └────┘
  h=ph=e1=e2, d1=d2=pw     h=ph=e1=e2, pw≠pw2      h=ph=e1, h<e2=p2
```

## How is it being computed?

Computed from `Solution::placements` after decoding (piece dimensions are in the
expanded flat coordinate system where kerf is already absorbed, so adjacent pieces touch directly).

---

## staircase_area (implemented, not used in Objective)

`Solution::staircase_area` is implemented (`#[allow(dead_code)]`) but not part of the
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
