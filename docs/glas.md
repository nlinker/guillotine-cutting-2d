# GLAS — Group-based SLAS

GLAS (Group-based SLAS) is the decoder the genetic algorithm uses by default
(`src/glas/decoder.rs::decode_spec`, wired in as the GA's decoder in `main.rs`): it
turns a `Genome` into a `Solution`, and every individual the GA evaluates in the
default `calc`/`serve` path is decoded through it — it is a required part of the GA
loop, not optional tooling. Unlike [SLAS](slas.md), which places one physical piece per
gene, GLAS groups all copies of the same piece *type* into a single gene and places
them together in strips — one batch per free leaf.

## Piece classification

Before the GA runs, each piece type is assigned to one of three priority classes:

| Class | Name   | Condition                                                              |
|-------|--------|------------------------------------------------------------------------|
| 0     | Large  | `max_dim >= long_dim_threshold`  AND  `area >= large_area_threshold^2` |
| 1     | Medium | `max_dim >= long_dim_threshold`  AND  `area <  large_area_threshold^2` |
| 2     | Small  | `max_dim <  long_dim_threshold`                                        |

Auto-derived thresholds (when the config or CLI supplies `0`):

```
long_dim_threshold   = sheet_max * 0.3
large_area_threshold = sqrt(sheet_area * 0.05)
```

The decoder always processes classes in order 0 → 1 → 2, so large pieces are placed before
medium and small ones.

## Genome structure

```
Genome = Vec<Vec<Gene>>
          ^    ^
          |    +-- GA-evolved permutation of type indices within this class
          +-- class priority (0 = Large, 1 = Medium, 2 = Small)
```

Each `Gene` drives all copies of one piece type:

```rust
struct Gene {
    type_idx:  usize,                // index into ProblemSpec.piece_types
    rotate:    bool,                 // prefer rotated orientation for every copy
    selectors: SmallVec<[u32; 16]>,  // selectors[k]: free-leaf selector for batch starting at copy k
    inverses:  SmallVec<[bool; 16]>, // inverses[k]:  split direction for that batch (see below)
}
```

Both `selectors` and `inverses` have exactly `count` elements (one per physical copy).
Only the element at the batch-start position is consulted each round; mid-batch entries
are carried silently. This symmetry lets OX/CX crossover treat every index uniformly
without special-casing batch boundaries.

## Decoder loop

For each class in order, then for each gene within that class:

1. `placed` = number of copies of this type already placed.
2. Look up `selector = selectors[placed]`, `inv = inverses[placed]`.
3. Find the fitting free leaf: `selector % |free_leaves|` (wraps around).
   If nothing fits anywhere → open a new sheet and retry.
4. **Strip orientation** — count how many copies fit side-by-side:

   ```
   count_h = floor(fr_w / pw)   (horizontal strip, left-to-right)
   count_v = floor(fr_h / ph)   (vertical strip,   top-to-bottom)
   ```

   Pick vertical if `count_v > count_h`; otherwise horizontal.

5. **Split the free leaf** around the composite box `cw x ch`:
   - `inv = false` → horizontal cut
   - `inv = true`  → vertical cut

6. Place all `count` pieces in the strip (left-to-right or top-to-bottom).
7. Advance `placed` by `count` and go back to step 1 until all copies are placed.

## Splitting the free leaf (`inverse` flag)

Given a free leaf at `(x, y)` with dimensions `nw x nh`, composite box `cw x ch`,
`lw = nw - cw`, `lh = nh - ch`:

`inv = false` — horizontal cut; right strip is capped to the batch height:

```
+------------+----------+
|  batch     |  right   |
|  cw  x ch  | lw x ch  |
+------------+----------+
|         bottom        |
|         nw  x  lh     |
+-----------------------+
```

`inv = true` — vertical cut; right strip keeps the full leaf height:

```
+------------+----------+
|  batch     |          |
|  cw  x ch  |  right   |
+------------+ lw x nh  |
|  bottom    |          |
|  cw  x lh  |          |
+------------+----------+
```

This is the same `inverse` flag as in [SLAS](slas.md), but applied
to a composite box instead of a single piece.

There is currently no post-processing pass after decoding (an earlier `improve_tl_corners`
pass that reordered pieces within committed slots to cluster same-size pieces was tried
and removed; see git history around `8570d81`).

## Crossover and mutation

**Crossover** (OX or CX) is applied **independently per class**. The permutation of type
indices in class 0 is never mixed with class 1 or class 2. Each class is crossed over
as if it were a standalone genome, which preserves the class invariant across generations.

**Mutation** per gene:

| Parameter   | Effect                                                                       |
|-------------|------------------------------------------------------------------------------|
| `swap_p`    | swap gene with a random other gene **within the same class**                 |
| `flip_p`    | flip `rotate`                                                                |
| `point_p`   | per selector: nudge `selectors[k]` by ±`point_delta` (wrapping)              |
| `inverse_p` | per inverse: flip `inverses[k]` (toggles the split direction for that batch) |

## Comparison with SLAS

| Aspect           | SLAS                          | GLAS                                              |
|------------------|-------------------------------|---------------------------------------------------|
| Gene granularity | one gene per physical piece   | one gene per piece *type*                         |
| Genome size      | N genes (N = total copies)    | T genes across 3 classes (T = types)              |
| Placement order  | single GA-evolved permutation | Large -> Medium -> Small, GA within class         |
| Split heuristic  | SLAS shorter-leftover-axis    | strip fill + GA-evolved split direction per batch |
| Batch placement  | no (one piece per step)       | yes (all remaining copies of one type)            |
| `inverse` array  | one bool per gene             | one bool per copy (`inverses[k]`)                 |
