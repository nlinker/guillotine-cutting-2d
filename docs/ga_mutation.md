# GA mutation

Interactive step-by-step demo: [demos/ga_mutation_gsap.html](../demos/ga_mutation_gsap.html).

Shared by `slas::ga::mutate` (one field per gene) and `glas::ga::mutate` (per-gene
arrays `selectors`/`inverses`, applied independently per class). For every gene,
four independent coin flips - a gene can be swapped *and* flipped *and* nudged in
the same call:

- `swap_p`: swap with a random *other* gene (within the class, for GLAS) -
  preserves the permutation.
- `flip_p`: flip `rotate`.
- `point_p`: nudge `point_selector` (SLAS) / each `selectors[k]` (GLAS) by
  +/-`point_delta`, wrapping.
- `inverse_p`: flip `inverse` (SLAS) / each `inverses[k]` (GLAS) - reverses the
  split direction for that batch.

GLAS classes (outer `Vec`, large/medium/small) mutate independently: `swap_p`
never moves a gene across a class boundary, so decode order between classes is
never disturbed. A class with fewer than 2 genes is skipped (nothing to swap).
