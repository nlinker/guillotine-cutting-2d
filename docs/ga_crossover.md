# OX / CX crossover

Interactive step-by-step demo: [demos/ga_ox_cx_gsap.html](../demos/ga_ox_cx_gsap.html).

Shared by `slas::ga` (key = `piece_idx`, permutation of physical pieces) and
`glas::ga` (key = `type_idx`, permutation of piece types per class). Mechanics
below are keyed by `piece_idx`; GLAS applies the same steps independently per
class, substituting `type_idx`.

## OX (Ordered Crossover)

Copy segment `[lo, hi)` from each parent into the matching child; fill the rest
from the *other* parent starting at `hi`, wrapping, skipping keys already placed.

```text
         lo    hi
          |     |
P1: [ 0 | 1  2 | 3  4 ]  -->  C1: [ 4 | 1  2 | 3  0 ]
P2: [ 3 | 0  4 | 1  2 ]  -->  C2: [ 2 | 0  4 | 3  1 ]

  C1 segment <- P1;  remaining <- P2 from hi, wrapping, skipping dupes
  C2 segment <- P2;  remaining <- P1 from hi, wrapping, skipping dupes
```

## CX (Cycle Crossover)

No RNG. Trace cycles by following P2 values back to their positions in P1.
Even cycles keep their parent source; odd cycles swap it.

```text
pos:  0  1  2  3  4
P1: [ 0  1  2  3  4 ]
P2: [ 3  0  4  1  2 ]
cy:   0  0  1  0  1    (cycle 0: even, cycle 1: odd)

C1: [ 0  1  4  3  2 ]   even from P1, odd from P2
C2: [ 3  0  2  1  4 ]   even from P2, odd from P1
```
