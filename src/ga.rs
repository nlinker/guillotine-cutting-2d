use rand_core::Rng;

use crate::{
    decoder::{Gene, Genome, decode},
    model::Problem,
};

/// A genome paired with its cached `Solution::objective()` value to avoid re-decoding during selection.
#[derive(Debug, Clone)]
pub struct Individual {
    pub genome: Genome,
    pub objective: i64,
}

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

/// CX (Cycle Crossover) for two genomes. No RNG required — cycle structure is
/// fully determined by the two parents.
///
/// Traces cycles by following P2 values back to their positions in P1. Even cycles
/// keep their parent source; odd cycles swap it. O(n): one pass to invert P1,
/// one pass to trace all cycles.
///
/// Key property: within each cycle, {P1[i]} == {P2[i]}, so swapping sources
/// never breaks the permutation invariant.
///
/// ```text
/// pos:  0  1  2  3  4
/// P1: [ 0  1  2  3  4 ]
/// P2: [ 3  0  4  1  2 ]
/// cy:   0  0  1  0  1    (cycle 0: even, cycle 1: odd)
///
/// C1: [ 0  1  4  3  2 ]   even → P1, odd → P2
/// C2: [ 3  0  2  1  4 ]   even → P2, odd → P1
/// ```
pub fn cx_crossover(p1: &Genome, p2: &Genome) -> (Genome, Genome) {
    let n = p1.len();
    debug_assert_eq!(n, p2.len());
    if n < 2 {
        return (p1.clone(), p2.clone());
    }

    // Inverse of p1: pos_in_p1[v] = i where p1[i].piece_idx == v
    let mut pos_in_p1 = vec![0usize; n];
    for (i, gene) in p1.iter().enumerate() {
        pos_in_p1[gene.piece_idx] = i;
    }

    // Label each position with the parity of its cycle
    let mut odd_cycle = vec![false; n];
    let mut visited = vec![false; n];
    let mut odd = false;
    for start in 0..n {
        if visited[start] {
            continue;
        }
        let mut pos = start;
        loop {
            visited[pos] = true;
            odd_cycle[pos] = odd;
            pos = pos_in_p1[p2[pos].piece_idx];
            if pos == start {
                break;
            }
        }
        odd = !odd;
    }

    // Even cycles: C1 ← P1, C2 ← P2 (already correct from clone)
    // Odd cycles:  C1 ← P2, C2 ← P1
    let mut c1 = p1.clone();
    let mut c2 = p2.clone();
    for i in 0..n {
        if odd_cycle[i] {
            c1[i] = p2[i];
            c2[i] = p1[i];
        }
    }

    (c1, c2)
}

/// Returns the `n_elite` individuals with the lowest objective (lower is better),
/// sorted ascending. If `n_elite >= individuals.len()`, all are returned sorted.
/// Typical value: `n_elite = 1`.
pub fn select_elite(individuals: &[Individual], n_elite: usize) -> Vec<Individual> {
    let mut ranked: Vec<&Individual> = individuals.iter().collect();
    ranked.sort_unstable_by_key(|ind| ind.objective);
    ranked.into_iter().take(n_elite).cloned().collect()
}

/// Picks `k` individuals at random and returns the one with the lowest objective.
/// Typical value: `k = 2` or `k = 3`.
pub fn tournament_select<'a, R: Rng>(individuals: &'a [Individual], k: usize, rng: &mut R) -> &'a Individual {
    let n = individuals.len();
    debug_assert!(k >= 1 && k <= n);
    let first = (rng.next_u64() as usize) % n;
    let mut best = &individuals[first];
    for _ in 1..k {
        let idx = (rng.next_u64() as usize) % n;
        if individuals[idx].objective < best.objective {
            best = &individuals[idx];
        }
    }
    best
}

/// Generates `size` random individuals for `problem`, each with a shuffled genome
/// and a freshly computed objective.
pub fn init_population<R: Rng>(problem: &Problem, size: usize, rng: &mut R) -> Vec<Individual> {
    (0..size)
        .map(|_| {
            let genome = random_genome(problem, rng);
            let objective = decode(problem, &genome).objective(problem);
            Individual { genome, objective }
        })
        .collect()
}

/// GA hyperparameters.
///
/// `n_elite`: best individuals carried unchanged each generation (default 1).
/// `tournament_k`: tournament size for parent selection (default 2).
/// `p_crossover`: probability of applying OX crossover; otherwise children are parent clones.
#[derive(Debug, Clone)]
pub struct GaConfig {
    pub pop_size: usize,
    pub n_generations: usize,
    pub n_elite: usize,
    pub tournament_k: usize,
    pub p_crossover: f64,
    pub p_swap: f64,
    pub p_flip: f64,
    pub p_point: f64,
}

/// Runs the GA for `config.n_generations` and returns the best `Individual` found.
///
/// Each generation: elite individuals are carried over unchanged; the remainder is
/// filled by tournament selection → OX crossover (with probability `p_crossover`) →
/// mutation → decode. The running best is tracked independently of elitism so that
/// `n_elite = 0` still returns a valid result.
pub fn run_ga<R: Rng>(problem: &Problem, config: &GaConfig, rng: &mut R) -> Individual {
    let mut pop = init_population(problem, config.pop_size, rng);
    let mut best = select_elite(&pop, 1).into_iter().next().unwrap();

    for _ in 0..config.n_generations {
        let elite = select_elite(&pop, config.n_elite);
        let mut next_pop = elite;

        while next_pop.len() < config.pop_size {
            let p1 = tournament_select(&pop, config.tournament_k, rng).genome.clone();
            let p2 = tournament_select(&pop, config.tournament_k, rng).genome.clone();

            let (mut g1, mut g2) = if rng_01(rng) < config.p_crossover {
                ox_crossover(&p1, &p2, rng)
            } else {
                (p1, p2)
            };

            mutate(&mut g1, rng, config.p_swap, config.p_flip, config.p_point);
            let obj1 = decode(problem, &g1).objective(problem);
            next_pop.push(Individual {
                genome: g1,
                objective: obj1,
            });

            if next_pop.len() < config.pop_size {
                mutate(&mut g2, rng, config.p_swap, config.p_flip, config.p_point);
                let obj2 = decode(problem, &g2).objective(problem);
                next_pop.push(Individual {
                    genome: g2,
                    objective: obj2,
                });
            }
        }

        pop = next_pop;
        let gen_best = select_elite(&pop, 1).into_iter().next().unwrap();
        if gen_best.objective < best.objective {
            best = gen_best;
        }
    }

    best
}

fn ox_at(p1: &Genome, p2: &Genome, lo: usize, hi: usize) -> (Genome, Genome) {
    (build_child(p1, p2, lo, hi), build_child(p2, p1, lo, hi))
}

/// get random float in (0, 1)
fn rng_01<R: Rng>(rng: &mut R) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

fn random_genome<R: Rng>(problem: &Problem, rng: &mut R) -> Genome {
    let n = problem.pieces.len();
    let mut indices: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        let j = (rng.next_u64() as usize) % (i + 1);
        indices.swap(i, j);
    }
    indices
        .into_iter()
        .map(|piece_idx| Gene {
            piece_idx,
            rotate: rng.next_u64() & 1 != 0,
            point_selector: rng.next_u64() as u32,
        })
        .collect()
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
    fn tournament_full_k_returns_best() {
        let pop = vec![ind(0, 30), ind(1, 10), ind(2, 20)];
        let mut rng = Xoshiro256StarStar::seed_from_u64(1);
        let winner = tournament_select(&pop, 3, &mut rng);
        assert_eq!(winner.objective, 10);
    }

    #[test]
    fn tournament_is_deterministic() {
        let pop = vec![ind(0, 5), ind(1, 3), ind(2, 8), ind(3, 1)];
        let w1 = tournament_select(&pop, 2, &mut Xoshiro256StarStar::seed_from_u64(42));
        let w2 = tournament_select(&pop, 2, &mut Xoshiro256StarStar::seed_from_u64(42));
        assert_eq!(w1.objective, w2.objective);
    }

    #[test]
    fn init_population_size_and_valid_permutations() {
        use crate::parse::parse_problem;
        let problem = parse_problem("10x10:3x2,4x3,2x2n,5x1", 0).unwrap();
        let n = problem.pieces.len();
        let mut rng = Xoshiro256StarStar::seed_from_u64(99);
        let pop = init_population(&problem, 20, &mut rng);
        assert_eq!(pop.len(), 20);
        for ind in &pop {
            assert_eq!(sorted_ids(&ind.genome), (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn init_population_is_deterministic() {
        use crate::parse::parse_problem;
        let problem = parse_problem("10x10:3x2,4x3,2x2n", 0).unwrap();
        let pop1 = init_population(&problem, 5, &mut Xoshiro256StarStar::seed_from_u64(7));
        let pop2 = init_population(&problem, 5, &mut Xoshiro256StarStar::seed_from_u64(7));
        assert!(pop1.iter().zip(&pop2).all(|(a, b)| a.genome == b.genome));
    }

    fn default_config() -> GaConfig {
        GaConfig {
            pop_size: 20,
            n_generations: 10,
            n_elite: 1,
            tournament_k: 2,
            p_crossover: 0.8,
            p_swap: 0.1,
            p_flip: 0.05,
            p_point: 0.05,
        }
    }

    #[test]
    fn run_ga_smoke() {
        use crate::parse::parse_problem;
        let problem = parse_problem("10x10:3x2,4x3,2x2n,5x1", 0).unwrap();
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let _best = run_ga(&problem, &default_config(), &mut rng);
    }

    #[test]
    fn run_ga_is_deterministic() {
        use crate::parse::parse_problem;
        let problem = parse_problem("10x10:3x2,4x3,2x2n,5x1", 0).unwrap();
        let b1 = run_ga(&problem, &default_config(), &mut Xoshiro256StarStar::seed_from_u64(123));
        let b2 = run_ga(&problem, &default_config(), &mut Xoshiro256StarStar::seed_from_u64(123));
        assert_eq!(b1.objective, b2.objective);
        assert_eq!(b1.genome, b2.genome);
    }

    fn ind(piece_idx: usize, objective: i64) -> Individual {
        Individual {
            genome: vec![g(piece_idx)],
            objective,
        }
    }

    #[test]
    fn elite_returns_best() {
        let pop = vec![ind(0, 30), ind(1, 10), ind(2, 20)];
        let elite = select_elite(&pop, 1);
        assert_eq!(elite.len(), 1);
        assert_eq!(elite[0].objective, 10);
    }

    #[test]
    fn elite_top_k_sorted() {
        let pop = vec![ind(0, 50), ind(1, 10), ind(2, 30), ind(3, 20)];
        let elite = select_elite(&pop, 2);
        assert_eq!(elite.iter().map(|e| e.objective).collect::<Vec<_>>(), [10, 20]);
    }

    #[test]
    fn elite_n_exceeds_pop() {
        let pop = vec![ind(0, 5), ind(1, 3)];
        let elite = select_elite(&pop, 10);
        assert_eq!(elite.len(), 2);
        assert_eq!(elite[0].objective, 3);
    }

    #[test]
    fn cx_known() {
        // P1=[0,1,2,3,4], P2=[3,0,4,1,2]
        // cycle 0 (even): positions {0,3,1} — trace: 0→pos_of(3)=3→pos_of(1)=1→pos_of(0)=0
        // cycle 1 (odd):  positions {2,4}   — trace: 2→pos_of(4)=4→pos_of(2)=2
        // C1 = [0,1,4,3,2],  C2 = [3,0,2,1,4]
        let p1: Genome = (0..5usize).map(g).collect();
        let p2: Genome = [3, 0, 4, 1, 2].into_iter().map(g).collect();
        let (c1, c2) = cx_crossover(&p1, &p2);
        assert_eq!(ids(&c1), [0, 1, 4, 3, 2]);
        assert_eq!(ids(&c2), [3, 0, 2, 1, 4]);
    }

    #[test]
    fn cx_produces_valid_permutations() {
        let n = 7;
        let p1: Genome = (0..n).map(g).collect();
        let p2: Genome = [6, 2, 4, 0, 3, 5, 1].into_iter().map(g).collect();
        let (c1, c2) = cx_crossover(&p1, &p2);
        assert_eq!(sorted_ids(&c1), (0..n).collect::<Vec<_>>());
        assert_eq!(sorted_ids(&c2), (0..n).collect::<Vec<_>>());
    }

    #[test]
    fn cx_identity_parent_gives_self() {
        // P1 = identity → cycles are all singletons, alternating parity
        // Each position i: pos_in_p1[P2[i]] = P2[i] (since P1 is identity)
        // So cycle of pos i is just {i}, parity alternates 0,1,0,1,...
        // C1: even positions from P1, odd from P2
        // C2: even positions from P2, odd from P1
        let n = 6;
        let p1: Genome = (0..n).map(g).collect();
        let p2: Genome = [5, 4, 3, 2, 1, 0].into_iter().map(g).collect();
        let (c1, c2) = cx_crossover(&p1, &p2);
        assert_eq!(sorted_ids(&c1), (0..n).collect::<Vec<_>>());
        assert_eq!(sorted_ids(&c2), (0..n).collect::<Vec<_>>());
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
