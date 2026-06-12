# Objective function

The fitness of a solution is a three-level lexicographic tuple — **lower is better**
(except `layout_score`, which is maximised and therefore reversed in `Ord`):

| Level | Field           | Direction    | Meaning                                                                                                                                                                             |
|-------|-----------------|--------------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| 1     | `sheets_used`   | minimize     | Number of stock sheets consumed. Any *k*-sheet solution is strictly better than any *(k+1)*-sheet solution regardless of the other levels.                                          |
| 2     | `layout_score`  | **maximize** | Cut-line concentration score + strip-structure score (see below). Higher means internal cuts align into a few long, reusable lines and pieces stack into mono-width strips — easier to cut and fewer fence repositions. |
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
loop that the previous metric (`shared_edge_score`) used — see
`docs/plans/cut-line-concentration-score.md` for the original brainstorm and the
comparison that motivated the replacement (that metric was pairwise and local, so a
chaotic region could rack up nonzero scores from incidental local matches, and it
could not tell "several pairwise matches on the same coordinate — one continuous,
reusable cut line" from "several unrelated short cuts that add up to a similar total").

## How is it being computed?

Computed from `Solution::placements` after decoding (piece dimensions are in the
expanded flat coordinate system where kerf is already absorbed, so adjacent pieces touch directly).

---

## layout_score, part 2 (strip_structure_score)

The concentration score is blind to *what* lines up along a cut: a strip of pieces
sharing a common side and an incidental alignment of unrelated pieces score the same.
The strip-structure component closes that gap. It rewards **mono-width strips** — stacks
of pieces sharing the same `[x, x+w)` interval laid flush in `y` (plus the symmetric
horizontal runs). Such a strip is ripped with a single fence setting and then chopped
to length, even when the pieces inside it differ — the "block" structure of the
GroupSub decoder (Faizrakhmanov et al., 2014; see
`docs/plans/12_strip-structure-score.md`).

### How it is computed

For each sheet, group placements by their cross-axis interval (`(x, w)` for vertical
runs, `(y, h)` for horizontal), merge flush spans into runs (kerf is absorbed into the
expanded dimensions, so "flush" is exact coordinate equality), and add `length²` per
run. Same merge helper, same `/10_000` scaling as the concentration score.

Squaring is again essential — and here a linear sum would be not just weaker but
*useless*: every placement belongs to exactly one run per orientation, so the total run
length is invariant for a fixed piece set; only the superadditive square distinguishes
consolidated strips from scattered singletons.

A k×m grid of identical w×h pieces scores along both axes at once,
`k·(m·h)² + m·(k·w)²` — the global maximum for that piece set — which is exactly the
incentive that drives the GA toward grids of identical pieces. Mixed pieces sharing a
width still earn partial credit, giving the GA a smoother gradient than an
identical-pieces-only reward.

The two components are summed: `layout_score = concentration + STRIP_WEIGHT · strip`,
with `STRIP_WEIGHT = 1` (both are squared-length sums on the same scale).

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
