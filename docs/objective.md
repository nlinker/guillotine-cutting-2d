# Objective function

The fitness of a solution is a three-level lexicographic tuple — **lower is better**
(except `layout_score`, which is maximised and therefore reversed in `Ord`):

| Level | Field           | Direction    | Meaning                                                                                                                                                                             |
|-------|-----------------|--------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 1     | `sheets_used`   | minimize     | Number of stock sheets consumed. Any *k*-sheet solution is strictly better than any *(k+1)*-sheet solution regardless of the other levels.                                          |
| 2     | `layout_score`  | **maximize** | Cut-line concentration score (see below). Higher means internal cuts align into a few long, reusable lines — easier to cut and fewer fence repositions.                            |
| 3     | `leftover_area` | minimize     | Area of the largest single leftover rectangle across all sheets. Prefers solutions where waste is concentrated in one big reusable offcut rather than scattered in many small ones. |

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

## layout_score (cut_line_concentration_score)

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
"technological" groupings.

### How it is computed

For each sheet:

1. Take every internal edge of every placed piece (edges lying on the sheet boundary
   are exempt — the same exemption the kerf computation uses).
2. Group vertical edges by their `x` coordinate, horizontal edges by their `y`
   coordinate — each group is a candidate single cut line.
3. Within each group, sort the covered spans and merge overlapping/adjacent ones into
   disjoint runs (a continuous run is one cut the saw can make in a single pass).
4. Add `length²` for every merged run.

The result is summed across all sheets, divided by `100² = 10_000` and rounded to the
nearest integer to keep the numbers in a manageable range.

Squaring is the key: for a fixed total amount of cut length, concentrating it into one
long run scores far higher than spreading it across many short ones (Herfindahl-style
concentration). In the good layout above, the horizontal cut spans the full sheet width
in one run; in the bad layout the same total length is fragmented into several short,
misaligned runs that score much lower when squared individually. This rewards exactly
the "few long, reusable lines" intuition that drives manufacturability — fewer fence
repositions, more pieces cut per pass.

It is also `O(n log n)` per sheet (sort + merge), cheaper than the pairwise `O(n²)`
loop that the previous metric (`shared_edge_score`, see below) used.

See `docs/plans/cut-line-concentration-score.md` for the original brainstorm.

## How is it being computed?

Computed from `Solution::placements` after decoding (piece dimensions are in the
expanded flat coordinate system where kerf is already absorbed, so adjacent pieces touch directly).

---

## shared_edge_score (implemented, not used in Objective)

`Solution::shared_edge_score` is the metric `layout_score` replaced — kept around
(`#[allow(dead_code)]`) in case we want to revisit it.

It is **pairwise and local**: `O(n²)` per sheet, looping over every pair of pieces and
scoring each shared boundary segment independently. This made it racy — a chaotic
region could rack up nonzero pairwise scores from incidental local matches that rival
the sum from a clean grid — and blind to the difference between "several pairwise
matches that lie on the same coordinate, i.e. one continuous, reusable cut line" and
"several unrelated short cuts that happen to add up to a similar total". `layout_score`
was designed specifically to fix these two weaknesses; see
`docs/plans/cut-line-concentration-score.md` for the comparison that motivated the
change.

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
