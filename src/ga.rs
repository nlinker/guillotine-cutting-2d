use rand_core::Rng;

use crate::decoder::{Gene, Genome};

/// OX (Ordered Crossover) for two genomes.
///
/// A random segment `[lo, hi)` is copied from each donor into the corresponding child;
/// the remaining positions are filled from the other parent in order starting at `hi`
/// (wrapping), preserving relative order and skipping already-present `piece_idx` values.
/// The full `Gene` (including `rotate` and `point_selector`) travels with its `piece_idx`.
///
/// ```text
///          lo    hi
///           ↓     ↓
/// P1: [ 0 │ 1  2 │ 3  4 ]  ──→  C1: [ 4 │ 1  2 │ 3  0 ]
/// P2: [ 3 │ 0  4 │ 1  2 ]  ──→  C2: [ 2 │ 0  4 │ 3  1 ]
///
///   C1 segment ← P1;  remaining ← P2 from hi, wrapping, skipping dupes
///   C2 segment ← P2;  remaining ← P1 from hi, wrapping, skipping dupes
/// ```
///
/// `p1` and `p2` must be the same length and their `piece_idx` values must be a
/// permutation of `0..n`.
pub fn ox_crossover<R: Rng>(p1: &Genome, p2: &Genome, rng: &mut R) -> (Genome, Genome) {
    let n = p1.len();
    debug_assert_eq!(n, p2.len());
    if n < 2 {
        return (p1.clone(), p2.clone());
    }
    let lo = (rng.next_u64() as usize) % (n - 1);
    let hi = lo + 1 + (rng.next_u64() as usize) % (n - lo);
    ox_at(p1, p2, lo, hi)
}

/// Mutate a genome in-place. For each gene, independently:
/// - with probability `p_swap`: swap it with a random other gene (preserves permutation)
/// - with probability `p_flip`: flip `rotate`
/// - with probability `p_point`: assign a new random `point_selector`
pub fn mutate<R: Rng>(genome: &mut Genome, rng: &mut R, p_swap: f64, p_flip: f64, p_point: f64) {
    let n = genome.len();
    if n < 2 {
        return;
    }
    for i in 0..n {
        if rng_01(rng) < p_swap {
            let j = (i + 1 + (rng.next_u64() as usize) % (n - 1)) % n;
            genome.swap(i, j);
        }
        if rng_01(rng) < p_flip {
            genome[i].rotate = !genome[i].rotate;
        }
        if rng_01(rng) < p_point {
            genome[i].point_selector = rng.next_u64() as u32;
        }
    }
}

fn ox_at(p1: &Genome, p2: &Genome, lo: usize, hi: usize) -> (Genome, Genome) {
    (build_child(p1, p2, lo, hi), build_child(p2, p1, lo, hi))
}

/// get random float in (0, 1)
fn rng_01<R: Rng>(rng: &mut R) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

fn build_child(donor: &Genome, filler: &Genome, lo: usize, hi: usize) -> Genome {
    let n = donor.len();
    let mut in_segment = vec![false; n];
    for gene in &donor[lo..hi] {
        in_segment[gene.piece_idx] = true;
    }
    let fill_positions: Vec<usize> = (hi..n).chain(0..lo).collect();
    let fill_genes: Vec<&Gene> = (0..n)
        .map(|i| &filler[(hi + i) % n])
        .filter(|g| !in_segment[g.piece_idx])
        .collect();
    let mut child = donor.clone();
    for (pos, gene) in fill_positions.iter().zip(fill_genes.iter()) {
        child[*pos] = **gene;
    }
    child
}

#[cfg(test)]
mod tests {
    use rand_core::SeedableRng;
    use rand_xoshiro::Xoshiro256StarStar;

    use super::*;

    fn g(piece_idx: usize) -> Gene {
        Gene {
            piece_idx,
            rotate: false,
            point_selector: 0,
        }
    }

    fn ids(genome: &Genome) -> Vec<usize> {
        genome.iter().map(|g| g.piece_idx).collect()
    }

    fn sorted_ids(genome: &Genome) -> Vec<usize> {
        let mut v = ids(genome);
        v.sort_unstable();
        v
    }

    #[test]
    fn mutate_preserves_permutation() {
        let n = 8;
        let mut genome: Genome = (0..n).map(g).collect();
        mutate(&mut genome, &mut Xoshiro256StarStar::seed_from_u64(1), 1.0, 1.0, 1.0);
        assert_eq!(sorted_ids(&genome), (0..n).collect::<Vec<_>>());
    }

    #[test]
    fn mutate_no_op_at_zero_prob() {
        let orig: Genome = (0..5usize).map(g).collect();
        let mut genome = orig.clone();
        mutate(&mut genome, &mut Xoshiro256StarStar::seed_from_u64(2), 0.0, 0.0, 0.0);
        assert_eq!(genome, orig);
    }

    #[test]
    fn mutate_flips_all_rotate() {
        let n = 4;
        let mut genome: Genome = (0..n).map(g).collect();
        mutate(&mut genome, &mut Xoshiro256StarStar::seed_from_u64(3), 0.0, 1.0, 0.0);
        assert!(genome.iter().all(|g| g.rotate));
    }

    #[test]
    fn mutate_is_deterministic() {
        let orig: Genome = (0..6usize).map(g).collect();
        let mut g1 = orig.clone();
        let mut g2 = orig.clone();
        mutate(&mut g1, &mut Xoshiro256StarStar::seed_from_u64(42), 0.3, 0.2, 0.2);
        mutate(&mut g2, &mut Xoshiro256StarStar::seed_from_u64(42), 0.3, 0.2, 0.2);
        assert_eq!(g1, g2);
    }

    #[test]
    fn ox_at_known() {
        // p1=[0,1,2,3,4], p2=[3,0,4,1,2], segment [lo=1, hi=3)
        //
        // child1: segment from p1 → [_,1,2,_,_]
        //   filler p2 from pos 3: 1(skip),2(skip),3,0,4 → fill [3,4,0] with [3,0,4]
        //   → [4,1,2,3,0]
        //
        // child2: segment from p2 → [_,0,4,_,_]
        //   filler p1 from pos 3: 3,4(skip),0(skip),1,2 → fill [3,4,0] with [3,1,2]
        //   → [2,0,4,3,1]
        let p1: Genome = (0..5usize).map(g).collect();
        let p2: Genome = [3, 0, 4, 1, 2].into_iter().map(g).collect();
        let (c1, c2) = ox_at(&p1, &p2, 1, 3);
        assert_eq!(ids(&c1), [4, 1, 2, 3, 0]);
        assert_eq!(ids(&c2), [2, 0, 4, 3, 1]);
    }

    #[test]
    fn ox_produces_valid_permutations() {
        let n = 7;
        let p1: Genome = (0..n).map(g).collect();
        let p2: Genome = [6, 2, 4, 0, 3, 5, 1].into_iter().map(g).collect();
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        for _ in 0..50 {
            let (c1, c2) = ox_crossover(&p1, &p2, &mut rng);
            let mut s1 = ids(&c1);
            let mut s2 = ids(&c2);
            s1.sort_unstable();
            s2.sort_unstable();
            assert_eq!(s1, (0..n).collect::<Vec<_>>());
            assert_eq!(s2, (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn ox_is_deterministic() {
        let p1: Genome = (0..5usize).map(g).collect();
        let p2: Genome = [4, 3, 2, 1, 0].into_iter().map(g).collect();
        let (c1a, c2a) = ox_crossover(&p1, &p2, &mut Xoshiro256StarStar::seed_from_u64(7));
        let (c1b, c2b) = ox_crossover(&p1, &p2, &mut Xoshiro256StarStar::seed_from_u64(7));
        assert_eq!(c1a, c1b);
        assert_eq!(c2a, c2b);
    }
}
