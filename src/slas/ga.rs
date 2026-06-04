use std::sync::Arc;

use rand::Rng;

pub use crate::ga::{GaEvent, GaHandle, ProgressEvent, select_elite, tournament_select};
use crate::{
    expand::expand_problem,
    ga::{self, GaConfig, GaDecoder},
    model::{Objective, Problem, ProblemSpec},
    slas::decoder::{Gene, Genome, decode},
};

/// Concrete individual type for the SLAS decoder.
pub type Individual = crate::ga::Individual<Genome>;

/// SLAS GA decoder. Owns the expanded (flat) problem.
pub struct SlasDecoder {
    pub problem: Arc<Problem>,
}

impl GaDecoder for SlasDecoder {
    type Genome = Genome;

    fn random_genome<R: Rng>(&self, _config: &GaConfig, rng: &mut R) -> Genome {
        make_genome(self.problem.pieces.len(), rng)
    }

    fn eval(&self, genome: &Genome) -> Objective {
        decode(&self.problem, genome).eval(&self.problem)
    }

    fn crossover<R: Rng>(&self, p1: &Genome, p2: &Genome, rng: &mut R) -> (Genome, Genome) {
        ox_crossover(p1, p2, rng)
    }

    fn mutate<R: Rng>(&self, genome: &mut Genome, config: &GaConfig, rng: &mut R) {
        mutate(
            genome,
            config.swap_p,
            config.flip_p,
            config.point_p,
            config.point_delta,
            config.inverse_p,
            rng,
        );
    }
}

/// Single-threaded SLAS GA. Takes a flat `Problem` (already expanded from spec).
pub fn run_ga<R: Rng>(problem: &Problem, config: &GaConfig, rng: &mut R) -> Individual {
    let decoder = SlasDecoder {
        problem: Arc::new(problem.clone()),
    };
    ga::run_ga(&decoder, config, rng)
}

/// Multithreaded SLAS GA. Takes a `ProblemSpec` (expanded internally).
pub fn run_ga_mt(
    spec: Arc<ProblemSpec>,
    config: Arc<GaConfig>,
    seeds: Vec<u64>,
    progress_interval: usize,
    migration_interval: usize,
) -> GaHandle<Genome> {
    let decoder = Arc::new(SlasDecoder {
        problem: Arc::new(expand_problem(&spec)),
    });
    ga::run_ga_mt(decoder, config, seeds, progress_interval, migration_interval)
}

/// OX (Ordered Crossover) for two SLAS genomes.
///
/// A random segment `[lo, hi)` is copied from each donor into the corresponding child;
/// the remaining positions are filled from the other parent in order starting at `hi`
/// (wrapping), preserving relative order and skipping already-present `piece_idx` values.
/// The full `Gene` (including `rotate` and `point_selector`) travels with its `piece_idx`.
///
/// Both genomes must be permutations of `0..n` (each `piece_idx` appears exactly once).
///
/// ```text
///          lo    hi
///           |     |
/// P1: [ 0 | 1  2 | 3  4 ]  -->  C1: [ 4 | 1  2 | 3  0 ]
/// P2: [ 3 | 0  4 | 1  2 ]  -->  C2: [ 2 | 0  4 | 3  1 ]
///
///   C1 segment <- P1;  remaining <- P2 from hi, wrapping, skipping dupes
///   C2 segment <- P2;  remaining <- P1 from hi, wrapping, skipping dupes
/// ```
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

fn ox_at(p1: &Genome, p2: &Genome, lo: usize, hi: usize) -> (Genome, Genome) {
    fn build_child(donor: &Genome, filler: &Genome, lo: usize, hi: usize) -> Genome {
        let n = donor.len();
        let mut in_segment = vec![false; n];
        for gene in &donor[lo..hi] {
            in_segment[gene.piece_idx] = true;
        }
        let fill_positions = (hi..n).chain(0..lo).collect::<Vec<_>>();
        let fill_genes = (0..n)
            .map(|i| &filler[(hi + i) % n])
            .filter(|g| !in_segment[g.piece_idx])
            .collect::<Vec<_>>();
        let mut child = donor.clone();
        for (pos, gene) in fill_positions.iter().zip(fill_genes.iter()) {
            child[*pos] = **gene;
        }
        child
    }
    (build_child(p1, p2, lo, hi), build_child(p2, p1, lo, hi))
}

/// CX (Cycle Crossover) for two SLAS genomes. No RNG required - cycle structure is
/// fully determined by the two parents.
///
/// Each `piece_idx` value appears exactly once in the genome (bijection).
/// Traces cycles by following P2 values back to their positions in P1. Even cycles
/// keep their parent source; odd cycles swap it. O(n): one pass to invert P1,
/// one pass to trace all cycles.
///
/// ```text
/// pos:  0  1  2  3  4
/// P1: [ 0  1  2  3  4 ]
/// P2: [ 3  0  4  1  2 ]
/// cy:   0  0  1  0  1    (cycle 0: even, cycle 1: odd)
///
/// C1: [ 0  1  4  3  2 ]   even from P1, odd from P2
/// C2: [ 3  0  2  1  4 ]   even from P2, odd from P1
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

/// Mutate a SLAS genome in-place. For each gene, independently:
/// - with probability `swap_p`: swap it with a random other gene (preserves permutation)
/// - with probability `flip_p`: flip `rotate`
/// - with probability `point_p`: nudge `point_selector` by +/-`point_delta` wrapping
/// - with probability `inverse_p`: flip `inverse` (reverses SLAS split direction)
pub fn mutate<R: Rng>(
    genome: &mut Genome,
    swap_p: f64,
    flip_p: f64,
    point_p: f64,
    point_delta: (u32, u32),
    inverse_p: f64,
    rng: &mut R,
) {
    let n = genome.len();
    if n < 2 {
        return;
    }
    let span = (point_delta.1.saturating_sub(point_delta.0) + 1).max(1) as u64;
    for i in 0..n {
        if rng_01(rng) < swap_p {
            let j = (i + 1 + (rng.next_u64() as usize) % (n - 1)) % n;
            genome.swap(i, j);
        }
        if rng_01(rng) < flip_p {
            genome[i].rotate = !genome[i].rotate;
        }
        if rng_01(rng) < point_p {
            let delta = point_delta.0 + (rng.next_u64() % span) as u32;
            genome[i].point_selector = if rng.next_u64() & 1 == 0 {
                genome[i].point_selector.wrapping_add(delta)
            } else {
                genome[i].point_selector.wrapping_sub(delta)
            };
        }
        if rng_01(rng) < inverse_p {
            genome[i].inverse = !genome[i].inverse;
        }
    }
}

fn make_genome<R: Rng>(n: usize, rng: &mut R) -> Genome {
    let mut indices = (0..n).collect::<Vec<_>>();
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
            inverse: false,
        })
        .collect()
}

fn rng_01<R: Rng>(rng: &mut R) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256StarStar;

    use super::*;
    use crate::{expand::expand_problem, model::validate_solution, parse::parse_problem};

    fn g(piece_idx: usize) -> Gene {
        Gene {
            piece_idx,
            rotate: false,
            point_selector: 0,
            inverse: false,
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

    fn ind(piece_idx: usize, objective: crate::model::Objective) -> Individual {
        Individual {
            genome: vec![g(piece_idx)],
            objective,
        }
    }

    fn default_config() -> GaConfig {
        GaConfig {
            pop_size: 20,
            n_generations: 10,
            n_elite: 1,
            tournament_k: 2,
            crossover_p: 0.8,
            swap_p: 0.1,
            point_p: 0.05,
            ..GaConfig::default()
        }
    }

    // --- mutate ---

    #[test]
    fn mutate_preserves_permutation() {
        let n = 8;
        let mut genome: Genome = (0..n).map(g).collect();
        mutate(
            &mut genome,
            1.0,
            1.0,
            1.0,
            (1, 3),
            1.0,
            &mut Xoshiro256StarStar::seed_from_u64(1),
        );
        assert_eq!(sorted_ids(&genome), (0..n).collect::<Vec<_>>());
    }

    #[test]
    fn mutate_no_op_at_zero_prob() {
        let orig: Genome = (0..5usize).map(g).collect();
        let mut genome = orig.clone();
        mutate(
            &mut genome,
            0.0,
            0.0,
            0.0,
            (1, 3),
            0.0,
            &mut Xoshiro256StarStar::seed_from_u64(2),
        );
        assert_eq!(genome, orig);
    }

    #[test]
    fn mutate_flips_all_rotate() {
        let n = 4;
        let mut genome: Genome = (0..n).map(g).collect();
        mutate(
            &mut genome,
            0.0,
            1.0,
            0.0,
            (1, 3),
            0.0,
            &mut Xoshiro256StarStar::seed_from_u64(3),
        );
        assert!(genome.iter().all(|g| g.rotate));
    }

    #[test]
    fn mutate_flips_all_inverse() {
        let n = 4;
        let mut genome: Genome = (0..n).map(g).collect();
        mutate(
            &mut genome,
            0.0,
            0.0,
            0.0,
            (1, 3),
            1.0,
            &mut Xoshiro256StarStar::seed_from_u64(3),
        );
        assert!(genome.iter().all(|g| g.inverse));
    }

    #[test]
    fn mutate_is_deterministic() {
        let orig: Genome = (0..6usize).map(g).collect();
        let mut g1 = orig.clone();
        let mut g2 = orig.clone();
        mutate(
            &mut g1,
            0.3,
            0.2,
            0.2,
            (1, 3),
            0.1,
            &mut Xoshiro256StarStar::seed_from_u64(42),
        );
        mutate(
            &mut g2,
            0.3,
            0.2,
            0.2,
            (1, 3),
            0.1,
            &mut Xoshiro256StarStar::seed_from_u64(42),
        );
        assert_eq!(g1, g2);
    }

    // --- selection ---

    #[test]
    fn tournament_full_k_returns_best() {
        let o = |la| crate::model::Objective { sheets_used: 0, leftover_area: la, shared_edge_score: 0 };
        let pop = vec![ind(0, o(30)), ind(1, o(10)), ind(2, o(20))];
        let mut rng = Xoshiro256StarStar::seed_from_u64(1);
        let winner = tournament_select(&pop, 3, &mut rng);
        assert_eq!((winner.objective.sheets_used, winner.objective.leftover_area), (0, 10));
    }

    #[test]
    fn tournament_is_deterministic() {
        let o = |la| crate::model::Objective { sheets_used: 0, leftover_area: la, shared_edge_score: 0 };
        let pop = vec![ind(0, o(5)), ind(1, o(3)), ind(2, o(8)), ind(3, o(1))];
        let w1 = tournament_select(&pop, 2, &mut Xoshiro256StarStar::seed_from_u64(42));
        let w2 = tournament_select(&pop, 2, &mut Xoshiro256StarStar::seed_from_u64(42));
        assert_eq!(w1.objective, w2.objective);
    }

    #[test]
    fn elite_returns_best() {
        let o = |la| crate::model::Objective { sheets_used: 0, leftover_area: la, shared_edge_score: 0 };
        let pop = vec![ind(0, o(30)), ind(1, o(10)), ind(2, o(20))];
        let elite = select_elite(&pop, 1);
        assert_eq!(elite.len(), 1);
        assert_eq!((elite[0].objective.sheets_used, elite[0].objective.leftover_area), (0, 10));
    }

    #[test]
    fn elite_top_k_sorted() {
        let o = |la| crate::model::Objective { sheets_used: 0, leftover_area: la, shared_edge_score: 0 };
        let pop = vec![ind(0, o(50)), ind(1, o(10)), ind(2, o(30)), ind(3, o(20))];
        let elite = select_elite(&pop, 2);
        assert_eq!(
            elite.iter().map(|e| (e.objective.sheets_used, e.objective.leftover_area)).collect::<Vec<_>>(),
            [(0, 10), (0, 20)]
        );
    }

    #[test]
    fn elite_n_exceeds_pop() {
        let o = |la| crate::model::Objective { sheets_used: 0, leftover_area: la, shared_edge_score: 0 };
        let pop = vec![ind(0, o(5)), ind(1, o(3))];
        let elite = select_elite(&pop, 10);
        assert_eq!(elite.len(), 2);
        assert_eq!((elite[0].objective.sheets_used, elite[0].objective.leftover_area), (0, 3));
    }

    // --- genome generation ---

    #[test]
    fn random_genome_valid_permutation() {
        let spec = parse_problem("10x10R:0:3x2,4x3,2x2f,5x1").unwrap();
        let flat = expand_problem(&spec);
        let n = flat.pieces.len();
        let decoder = SlasDecoder {
            problem: Arc::new(flat),
        };
        let cfg = default_config();
        let mut rng = Xoshiro256StarStar::seed_from_u64(99);
        for _ in 0..20 {
            let genome = decoder.random_genome(&cfg, &mut rng);
            assert_eq!(sorted_ids(&genome), (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn random_genome_is_deterministic() {
        let spec = parse_problem("10x10R:0:3x2,4x3,2x2f").unwrap();
        let flat = expand_problem(&spec);
        let decoder = SlasDecoder {
            problem: Arc::new(flat),
        };
        let cfg = default_config();
        let g1 = decoder.random_genome(&cfg, &mut Xoshiro256StarStar::seed_from_u64(7));
        let g2 = decoder.random_genome(&cfg, &mut Xoshiro256StarStar::seed_from_u64(7));
        assert_eq!(g1, g2);
    }

    // --- CX crossover ---

    #[test]
    fn cx_known() {
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
        let n = 6;
        let p1: Genome = (0..n).map(g).collect();
        let p2: Genome = [5, 4, 3, 2, 1, 0].into_iter().map(g).collect();
        let (c1, c2) = cx_crossover(&p1, &p2);
        assert_eq!(sorted_ids(&c1), (0..n).collect::<Vec<_>>());
        assert_eq!(sorted_ids(&c2), (0..n).collect::<Vec<_>>());
    }

    // --- OX crossover ---

    #[test]
    fn ox_at_known() {
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

    // --- run_ga ---

    #[test]
    fn run_ga_smoke() {
        let spec = parse_problem("10x10R:0:3x2,4x3,2x2f,5x1").unwrap();
        let problem = expand_problem(&spec);
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let _best = run_ga(&problem, &default_config(), &mut rng);
    }

    #[test]
    fn run_ga_is_deterministic() {
        let spec = parse_problem("10x10R:0:3x2,4x3,2x2f,5x1").unwrap();
        let problem = expand_problem(&spec);
        let b1 = run_ga(&problem, &default_config(), &mut Xoshiro256StarStar::seed_from_u64(123));
        let b2 = run_ga(&problem, &default_config(), &mut Xoshiro256StarStar::seed_from_u64(123));
        assert_eq!(b1.objective, b2.objective);
        assert_eq!(b1.genome, b2.genome);
    }

    #[test]
    fn run_ga_mt_is_deterministic() {
        let spec = Arc::new(parse_problem("10x10R:0:3x2,4x3,2x2f,5x1").unwrap());
        let cfg = Arc::new(GaConfig {
            pop_size: 20,
            n_generations: 30,
            ..GaConfig::default()
        });
        let seeds = vec![0u64, 1, 2];

        let collect = || {
            let mut h = run_ga_mt(Arc::clone(&spec), Arc::clone(&cfg), seeds.clone(), 0, 10);
            loop {
                match h.rx.blocking_recv() {
                    Some(GaEvent::Done(r)) => break r,
                    _ => {}
                }
            }
        };

        let r1 = collect();
        let r2 = collect();
        assert_eq!(r1.len(), r2.len());
        for ((s1, i1), (s2, i2)) in r1.iter().zip(r2.iter()) {
            assert_eq!(s1, s2, "seed order differs");
            assert_eq!(i1.objective, i2.objective, "objective differs for seed {s1}");
            assert_eq!(i1.genome, i2.genome, "genome differs for seed {s1}");
        }
    }

    /// GA solutions must be non-overlapping and in-bounds.
    #[test]
    fn ga_cut_promotion_solutions_are_valid() {
        let specs = [
            "15x35F:0:12x3/2,3x12/2,8x4/4r,7x5/4r,6x4/4r",
            "17x31F:0:12x3/2,3x12/2,8x4/4r,7x5/4r,6x4/4r",
            "22x24F:0:12x3/2,3x12/2,8x4/4r,7x5/4r,6x4/4r",
            "28x19F:0:12x3/2,3x12/2,8x4/4r,7x5/4r,6x4/4r",
        ];
        let cfg = GaConfig {
            pop_size: 50,
            n_generations: 200,
            ..GaConfig::default()
        };
        for spec_str in specs {
            let problem = expand_problem(&parse_problem(spec_str).unwrap());
            for seed in 0u64..3 {
                let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
                let best = run_ga(&problem, &cfg, &mut rng);
                let sol = decode(&problem, &best.genome);
                let errors = validate_solution(&problem, &sol);
                assert!(
                    errors.is_empty(),
                    "spec={spec_str} seed={seed}: {} error(s):\n  {}",
                    errors.len(),
                    errors.join("\n  "),
                );
            }
        }
    }
}
