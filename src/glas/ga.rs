use std::sync::Arc;

use rand::Rng;

pub use crate::ga::{GaEvent, GaHandle, ProgressEvent, select_elite, tournament_select};
use crate::{
    expand::expand_problem,
    ga::{self, GaConfig, GaDecoder},
    glas::decoder::{Gene, Genome, decode},
    model::{Objective, PieceType, Problem, ProblemSpec},
};

/// Concrete individual type for the GLAS decoder.
pub type Individual = crate::ga::Individual<Genome>;

/// GLAS GA decoder. Owns the spec (for genome generation) and the expanded problem.
pub struct GlasDecoder {
    pub spec: Arc<ProblemSpec>,
    pub problem: Arc<Problem>,
}

impl GaDecoder for GlasDecoder {
    type Genome = Genome;

    fn random_genome<R: Rng>(&self, config: &GaConfig, rng: &mut R) -> Genome {
        make_genome(&self.spec, config.long_dim_threshold, config.large_area_threshold, rng)
    }

    fn eval(&self, genome: &Genome) -> Objective {
        decode(&self.problem, &self.spec, genome).eval(&self.problem)
    }

    fn crossover<R: Rng>(&self, p1: &Genome, p2: &Genome, rng: &mut R) -> (Genome, Genome) {
        ox_crossover(p1, p2, self.spec.piece_types.len(), rng)
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

    fn seed_genomes(&self, config: &GaConfig) -> Vec<Genome> {
        let g = greedy_genome(&self.spec, config.long_dim_threshold, config.large_area_threshold);
        vec![g]
    }
}

/// Single-threaded GLAS GA. Takes `ProblemSpec` and the expanded `Problem`.
pub fn run_ga<R: Rng>(spec: &ProblemSpec, problem: &Problem, config: &GaConfig, rng: &mut R) -> Individual {
    let decoder = GlasDecoder {
        spec: Arc::new(spec.clone()),
        problem: Arc::new(problem.clone()),
    };
    ga::run_ga(&decoder, config, rng)
}

/// Multithreaded GLAS GA. Takes a `ProblemSpec` (expanded internally).
pub fn run_ga_mt(
    spec: Arc<ProblemSpec>,
    config: Arc<GaConfig>,
    seeds: Vec<u64>,
    progress_interval: usize,
    migration_interval: usize,
) -> GaHandle<Genome> {
    let problem = Arc::new(expand_problem(&spec));
    let decoder = Arc::new(GlasDecoder { spec, problem });
    ga::run_ga_mt(decoder, config, seeds, progress_interval, migration_interval)
}

/// OX (Ordered Crossover) for two GLAS genomes.
///
/// Applied independently per class (outer vec). Within each class the permutation
/// key is `type_idx`; the gene payload (`rotate`, `selectors`, `inverses`) travels
/// with its gene. A random segment `[lo, hi)` is copied from each donor class into
/// the corresponding child class; remaining positions are filled from the other parent
/// in order starting at `hi` (wrapping), skipping already-present `type_idx` values.
///
/// `n_types` = total number of piece types (used to size the `in_segment` bitmap).
pub fn ox_crossover<R: Rng>(p1: &Genome, p2: &Genome, n_types: usize, rng: &mut R) -> (Genome, Genome) {
    let mut g1 = Genome::with_capacity(p1.len());
    let mut g2 = Genome::with_capacity(p2.len());
    for (c1, c2) in p1.iter().zip(p2.iter()) {
        let n = c1.len();
        let (rc1, rc2) = if n < 2 {
            (c1.clone(), c2.clone())
        } else {
            let lo = (rng.next_u64() as usize) % (n - 1);
            let hi = lo + 1 + (rng.next_u64() as usize) % (n - lo);
            ox_at(c1, c2, lo, hi, n_types)
        };
        g1.push(rc1);
        g2.push(rc2);
    }
    (g1, g2)
}

/// CX (Cycle Crossover) for two GLAS genomes.
///
/// Applied independently per class. Traces cycles via `type_idx`; even cycles keep
/// their parent source; odd cycles swap. No RNG required.
///
/// `n_types` = total number of piece types (used to size the position-inverse array).
pub fn cx_crossover(p1: &Genome, p2: &Genome, n_types: usize) -> (Genome, Genome) {
    let mut g1 = Genome::with_capacity(p1.len());
    let mut g2 = Genome::with_capacity(p2.len());
    for (c1, c2) in p1.iter().zip(p2.iter()) {
        let n = c1.len();
        if n < 2 {
            g1.push(c1.clone());
            g2.push(c2.clone());
            continue;
        }
        let mut pos_in_c1 = vec![0usize; n_types];
        for (i, gene) in c1.iter().enumerate() {
            pos_in_c1[gene.type_idx] = i;
        }
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
                pos = pos_in_c1[c2[pos].type_idx];
                if pos == start {
                    break;
                }
            }
            odd = !odd;
        }
        let mut rc1 = c1.clone();
        let mut rc2 = c2.clone();
        for i in 0..n {
            if odd_cycle[i] {
                rc1[i] = c2[i].clone();
                rc2[i] = c1[i].clone();
            }
        }
        g1.push(rc1);
        g2.push(rc2);
    }
    (g1, g2)
}

/// Mutate a GLAS genome in-place. Applied independently per class.
/// For each gene within a class (when class has >= 2 genes):
/// - with probability `swap_p`: swap it with a random other gene within the same class
/// - with probability `flip_p`: flip `rotate`
/// - for each `selectors[k]` with probability `point_p`: nudge by +/-`point_delta` (wrapping)
/// - for each `inverses[k]` with probability `inverse_p`: flip the boolean
pub fn mutate<R: Rng>(
    genome: &mut Genome,
    swap_p: f64,
    flip_p: f64,
    point_p: f64,
    point_delta: (u32, u32),
    inverse_p: f64,
    rng: &mut R,
) {
    let span = (point_delta.1.saturating_sub(point_delta.0) + 1).max(1) as u64;
    for class in genome.iter_mut() {
        let n = class.len();
        if n < 2 {
            continue;
        }
        for i in 0..n {
            if rng_01(rng) < swap_p {
                let j = (i + 1 + (rng.next_u64() as usize) % (n - 1)) % n;
                class.swap(i, j);
            }
            if rng_01(rng) < flip_p {
                class[i].rotate = !class[i].rotate;
            }
            let count = class[i].selectors.len();
            for k in 0..count {
                if rng_01(rng) < point_p {
                    let delta = point_delta.0 + (rng.next_u64() % span) as u32;
                    class[i].selectors[k] = if rng.next_u64() & 1 == 0 {
                        class[i].selectors[k].wrapping_add(delta)
                    } else {
                        class[i].selectors[k].wrapping_sub(delta)
                    };
                }
            }
            for k in 0..count {
                if rng_01(rng) < inverse_p {
                    class[i].inverses[k] = !class[i].inverses[k];
                }
            }
        }
    }
}

fn ox_at(p1: &[Gene], p2: &[Gene], lo: usize, hi: usize, n_types: usize) -> (Vec<Gene>, Vec<Gene>) {
    fn build_child(donor: &[Gene], filler: &[Gene], lo: usize, hi: usize, n_types: usize) -> Vec<Gene> {
        let n = donor.len();
        let mut in_segment = vec![false; n_types];
        for gene in &donor[lo..hi] {
            in_segment[gene.type_idx] = true;
        }
        let mut fill_iter = (0..n)
            .map(|i| &filler[(hi + i) % n])
            .filter(|g| !in_segment[g.type_idx]);
        let mut child = donor.to_vec();
        for pos in (hi..n).chain(0..lo) {
            child[pos] = fill_iter.next().expect("filler exhausted").clone();
        }
        child
    }
    (
        build_child(p1, p2, lo, hi, n_types),
        build_child(p2, p1, lo, hi, n_types),
    )
}

fn piece_class(ps: &PieceType, spec: &ProblemSpec, long_dim_threshold: u32, large_area_threshold: u32) -> usize {
    let sheet_max = spec.sheet.width.max(spec.sheet.height);
    let long_dim_threshold = if long_dim_threshold == 0 {
        (sheet_max as f64 * 0.3) as u32
    } else {
        long_dim_threshold
    };
    let large_area_threshold = if large_area_threshold == 0 {
        ((spec.sheet.width as f64 * spec.sheet.height as f64 * 0.05).sqrt()) as u32
    } else {
        large_area_threshold
    };
    let max_dim = ps.width.max(ps.height);
    let area = ps.width as u64 * ps.height as u64;
    if max_dim >= long_dim_threshold {
        if area >= large_area_threshold as u64 * large_area_threshold as u64 {
            0
        } else {
            1
        }
    } else {
        2
    }
}

/// Deterministic seed genome: within each size class, types sorted by max(w,h) desc,
/// then area desc. Rotation preference: true when height > width.
pub fn greedy_genome(spec: &ProblemSpec, long_dim_threshold: u32, large_area_threshold: u32) -> Genome {
    let n = spec.piece_types.len();
    let mut classes: [Vec<usize>; 3] = [vec![], vec![], vec![]];
    for i in 0..n {
        classes[piece_class(&spec.piece_types[i], spec, long_dim_threshold, large_area_threshold)].push(i);
    }
    let mut genome = Genome::with_capacity(3);
    for class_indices in classes {
        let mut sorted = class_indices;
        sorted.sort_by(|&a, &b| {
            let pa = &spec.piece_types[a];
            let pb = &spec.piece_types[b];
            let ka = (pa.width.max(pa.height), pa.width * pa.height);
            let kb = (pb.width.max(pb.height), pb.width * pb.height);
            kb.cmp(&ka)
        });
        let genes = sorted
            .into_iter()
            .map(|type_idx| {
                let ps = &spec.piece_types[type_idx];
                let count = ps.count as usize;
                Gene {
                    type_idx,
                    rotate: ps.height > ps.width,
                    selectors: std::iter::repeat_n(0u32, count).collect(),
                    inverses: std::iter::repeat_n(false, count).collect(),
                }
            })
            .collect::<Vec<Gene>>();
        genome.push(genes);
    }
    genome
}

fn make_genome<R: Rng>(spec: &ProblemSpec, long_dim_threshold: u32, large_area_threshold: u32, rng: &mut R) -> Genome {
    let n = spec.piece_types.len();
    let mut classes: [Vec<usize>; 3] = [vec![], vec![], vec![]];
    for i in 0..n {
        classes[piece_class(&spec.piece_types[i], spec, long_dim_threshold, large_area_threshold)].push(i);
    }
    let mut genome = Genome::with_capacity(3);
    for mut indices in classes {
        indices.sort_unstable_by(|&a, &b| {
            let area = |i: usize| {
                let ps = &spec.piece_types[i];
                (ps.width as u64) * (ps.height as u64) * (ps.count as u64)
            };
            area(b).cmp(&area(a))
        });
        let swaps = indices.len() / 4;
        for _ in 0..swaps {
            let i = (rng.next_u64() as usize) % indices.len().max(1);
            let j = (rng.next_u64() as usize) % indices.len().max(1);
            indices.swap(i, j);
        }
        let genes = indices
            .into_iter()
            .map(|type_idx| {
                let count = spec.piece_types[type_idx].count as usize;
                Gene {
                    type_idx,
                    rotate: rng.next_u64() & 1 != 0,
                    selectors: (0..count).map(|_| rng.next_u64() as u32).collect(),
                    inverses: (0..count).map(|_| rng.next_u64() & 1 != 0).collect(),
                }
            })
            .collect::<Vec<Gene>>();
        genome.push(genes);
    }
    genome
}

fn rng_01<R: Rng>(rng: &mut R) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256StarStar;

    use super::*;
    use crate::{expand::expand_problem, ga::GaConfig, parser::compact::parse_problem};

    fn gg(type_idx: usize, count: usize) -> Gene {
        Gene {
            type_idx,
            rotate: false,
            selectors: std::iter::repeat_n(0u32, count).collect(),
            inverses: std::iter::repeat_n(false, count).collect(),
        }
    }

    fn type_ids_flat(v: &[Gene]) -> Vec<usize> {
        v.iter().map(|g| g.type_idx).collect()
    }

    fn type_ids(genome: &Genome) -> Vec<usize> {
        genome.iter().flat_map(|c| c.iter()).map(|g| g.type_idx).collect()
    }

    fn sorted_type_ids(genome: &Genome) -> Vec<usize> {
        let mut v = type_ids(genome);
        v.sort_unstable();
        v
    }

    fn small_config() -> GaConfig {
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

    fn one_class(genes: Vec<Gene>) -> Genome {
        vec![genes]
    }

    fn ind(type_idx: usize, obj: crate::model::Objective) -> Individual {
        Individual {
            genome: one_class(vec![gg(type_idx, 1)]),
            objective: obj,
        }
    }

    // --- mutate ---

    #[test]
    fn mutate_preserves_permutation() {
        let mut genome = one_class((0..4).map(|i| gg(i, 2)).collect());
        mutate(
            &mut genome,
            1.0,
            0.0,
            0.0,
            (1, 3),
            0.0,
            &mut Xoshiro256StarStar::seed_from_u64(1),
        );
        assert_eq!(sorted_type_ids(&genome), vec![0, 1, 2, 3]);
    }

    #[test]
    fn mutate_no_op_at_zero_prob() {
        let orig = one_class((0..4).map(|i| gg(i, 2)).collect());
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
        let mut genome = one_class((0..4).map(|i| gg(i, 2)).collect());
        mutate(
            &mut genome,
            0.0,
            1.0,
            0.0,
            (1, 3),
            0.0,
            &mut Xoshiro256StarStar::seed_from_u64(3),
        );
        assert!(genome.iter().flat_map(|c| c.iter()).all(|g| g.rotate));
    }

    #[test]
    fn mutate_flips_all_inverses() {
        let orig = one_class((0..4).map(|i| gg(i, 2)).collect());
        let mut genome = orig.clone();
        mutate(
            &mut genome,
            0.0,
            0.0,
            0.0,
            (1, 3),
            1.0,
            &mut Xoshiro256StarStar::seed_from_u64(4),
        );
        let all_flipped = genome
            .iter()
            .flat_map(|c| c.iter())
            .flat_map(|g| g.inverses.iter())
            .all(|&v| v);
        assert!(
            all_flipped,
            "every inverse must have been flipped to true with inverse_p=1.0"
        );
    }

    #[test]
    fn mutate_nudges_all_selectors() {
        let orig = one_class((0..4).map(|i| gg(i, 3)).collect());
        let mut genome = orig.clone();
        mutate(
            &mut genome,
            0.0,
            0.0,
            1.0,
            (1, 1),
            0.0,
            &mut Xoshiro256StarStar::seed_from_u64(5),
        );
        let orig_sels = orig
            .iter()
            .flat_map(|c| c.iter())
            .flat_map(|g| g.selectors.iter().copied())
            .collect::<Vec<u32>>();
        let new_sels = genome
            .iter()
            .flat_map(|c| c.iter())
            .flat_map(|g| g.selectors.iter().copied())
            .collect::<Vec<u32>>();
        assert!(orig_sels.iter().zip(&new_sels).all(|(o, n)| o != n));
    }

    #[test]
    fn mutate_is_deterministic() {
        let orig = one_class((0..4).map(|i| gg(i, 2)).collect());
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
        let o = |dc| crate::model::Objective {
            sheets_used: 0.0,
            drop_consolidation_score: dc,
            layout_score: 0,
        };
        let pop = vec![ind(0, o(30)), ind(1, o(10)), ind(2, o(20))];
        let mut rng = Xoshiro256StarStar::seed_from_u64(1);
        let winner = tournament_select(&pop, 3, &mut rng);
        assert_eq!(
            (
                winner.objective.sheets_used_int(),
                winner.objective.drop_consolidation_score
            ),
            (0, 20)
        );
    }

    #[test]
    fn elite_returns_best() {
        let o = |dc| crate::model::Objective {
            sheets_used: 0.0,
            drop_consolidation_score: dc,
            layout_score: 0,
        };
        let pop = vec![ind(0, o(30)), ind(1, o(10)), ind(2, o(20))];
        let elite = select_elite(&pop, 1);
        assert_eq!(elite.len(), 1);
        assert_eq!(
            (
                elite[0].objective.sheets_used_int(),
                elite[0].objective.drop_consolidation_score
            ),
            (0, 30)
        );
    }

    // --- OX crossover ---

    #[test]
    fn ox_at_known() {
        let p1 = (0..5).map(|i| gg(i, 1)).collect::<Vec<_>>();
        let p2 = [3usize, 0, 4, 1, 2].into_iter().map(|i| gg(i, 1)).collect::<Vec<_>>();
        let (c1, c2) = ox_at(&p1, &p2, 1, 3, 5);
        assert_eq!(type_ids_flat(&c1), [4, 1, 2, 3, 0]);
        assert_eq!(type_ids_flat(&c2), [2, 0, 4, 3, 1]);
    }

    #[test]
    fn ox_produces_valid_permutations() {
        let n = 7;
        let p1 = one_class((0..n).map(|i| gg(i, 1)).collect());
        let p2 = one_class([6usize, 2, 4, 0, 3, 5, 1].into_iter().map(|i| gg(i, 1)).collect());
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        for _ in 0..50 {
            let (c1, c2) = ox_crossover(&p1, &p2, n, &mut rng);
            assert_eq!(sorted_type_ids(&c1), (0..n).collect::<Vec<_>>());
            assert_eq!(sorted_type_ids(&c2), (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn ox_gene_payload_travels_with_type_idx() {
        let mut p1 = one_class((0..5).map(|i| gg(i, 2)).collect::<Vec<_>>());
        let mut p2 = one_class((0..5).map(|i| gg(i, 2)).collect::<Vec<_>>());
        p1[0][2].selectors[0] = 999;
        p1[0][2].selectors[1] = 888;
        p2[0][2].selectors[0] = 111;
        p2[0][2].selectors[1] = 222;
        let (c1, _c2) = ox_at(&p1[0], &p2[0], 2, 3, 5);
        let gene2_in_c1 = c1.iter().find(|g| g.type_idx == 2).unwrap();
        assert_eq!(gene2_in_c1.selectors[0], 999, "child must carry donor payload");
        assert_eq!(gene2_in_c1.selectors[1], 888);
    }

    #[test]
    fn ox_is_deterministic() {
        let p1 = one_class((0..5).map(|i| gg(i, 1)).collect());
        let p2 = one_class([4usize, 3, 2, 1, 0].into_iter().map(|i| gg(i, 1)).collect());
        let (c1a, c2a) = ox_crossover(&p1, &p2, 5, &mut Xoshiro256StarStar::seed_from_u64(7));
        let (c1b, c2b) = ox_crossover(&p1, &p2, 5, &mut Xoshiro256StarStar::seed_from_u64(7));
        assert_eq!(c1a, c1b);
        assert_eq!(c2a, c2b);
    }

    // --- CX crossover ---

    #[test]
    fn cx_known() {
        let p1 = one_class((0..5).map(|i| gg(i, 1)).collect());
        let p2 = one_class([3usize, 0, 4, 1, 2].into_iter().map(|i| gg(i, 1)).collect());
        let (c1, c2) = cx_crossover(&p1, &p2, 5);
        assert_eq!(type_ids(&c1), [0, 1, 4, 3, 2]);
        assert_eq!(type_ids(&c2), [3, 0, 2, 1, 4]);
    }

    #[test]
    fn cx_produces_valid_permutations() {
        let n = 7;
        let p1 = one_class((0..n).map(|i| gg(i, 1)).collect());
        let p2 = one_class([6usize, 2, 4, 0, 3, 5, 1].into_iter().map(|i| gg(i, 1)).collect());
        let (c1, c2) = cx_crossover(&p1, &p2, n);
        assert_eq!(sorted_type_ids(&c1), (0..n).collect::<Vec<_>>());
        assert_eq!(sorted_type_ids(&c2), (0..n).collect::<Vec<_>>());
    }

    // --- genome generation ---

    #[test]
    fn random_genome_valid_permutation() {
        let spec = parse_problem("10x10R::3x2/3,4x3/2,5x1/4").unwrap();
        let problem = expand_problem(&spec);
        let n_types = spec.piece_types.len();
        let decoder = GlasDecoder {
            spec: Arc::new(spec),
            problem: Arc::new(problem),
        };
        let cfg = small_config();
        let mut rng = Xoshiro256StarStar::seed_from_u64(99);
        for _ in 0..20 {
            let genome = decoder.random_genome(&cfg, &mut rng);
            let total: usize = genome.iter().map(|c| c.len()).sum();
            assert_eq!(total, n_types);
            assert_eq!(sorted_type_ids(&genome), (0..n_types).collect::<Vec<_>>());
        }
    }

    #[test]
    fn random_genome_is_deterministic() {
        let spec = parse_problem("10x10R::3x2/3,4x3/2").unwrap();
        let problem = expand_problem(&spec);
        let decoder = GlasDecoder {
            spec: Arc::new(spec),
            problem: Arc::new(problem),
        };
        let cfg = small_config();
        let g1 = decoder.random_genome(&cfg, &mut Xoshiro256StarStar::seed_from_u64(7));
        let g2 = decoder.random_genome(&cfg, &mut Xoshiro256StarStar::seed_from_u64(7));
        assert!(g1.iter().zip(&g2).all(|(a, b)| a == b));
    }

    // --- run_ga ---

    #[test]
    fn run_ga_smoke() {
        let spec = parse_problem("10x10R::3x2/2,4x3/2").unwrap();
        let problem = expand_problem(&spec);
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let _best = run_ga(&spec, &problem, &small_config(), &mut rng);
    }

    #[test]
    fn run_ga_is_deterministic() {
        let spec = parse_problem("10x10R::3x2/2,4x3/2,2x2/3").unwrap();
        let problem = expand_problem(&spec);
        let b1 = run_ga(
            &spec,
            &problem,
            &small_config(),
            &mut Xoshiro256StarStar::seed_from_u64(123),
        );
        let b2 = run_ga(
            &spec,
            &problem,
            &small_config(),
            &mut Xoshiro256StarStar::seed_from_u64(123),
        );
        assert_eq!(b1.objective, b2.objective);
        assert_eq!(b1.genome, b2.genome);
    }

    #[test]
    fn run_ga_mt_is_deterministic() {
        let spec = Arc::new(parse_problem("10x10R::3x2/3,4x3/2,5x1/4").unwrap());
        let cfg = Arc::new(GaConfig {
            pop_size: 20,
            n_generations: 30,
            ..GaConfig::default()
        });
        let seeds = vec![0u64, 1, 2];

        let collect = || {
            let h = run_ga_mt(Arc::clone(&spec), Arc::clone(&cfg), seeds.clone(), 0, 10);
            h.blocking_wait()
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
}
