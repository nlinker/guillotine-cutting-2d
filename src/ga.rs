use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::{
    decoder::{Gene, Genome, decode},
    expand::expand_problem,
    model::{Objective, Problem, ProblemSpec},
};

/// GA hyperparameters.
#[derive(Debug, Clone)]
pub struct GaConfig {
    /// Number of individuals in the population.
    pub pop_size: usize,

    /// Number of generations to run before returning the best individual found.
    pub n_generations: usize,

    /// Number of top individuals copied unchanged into the next generation (elitism).
    /// Keeps the best solution from being lost to crossover or mutation.
    /// Typical value: 1-2. Set to 0 to disable.
    pub n_elite: usize,

    /// Tournament size: how many individuals compete for each parent slot.
    /// Higher values increase selection pressure (best wins more often).
    /// Typical value: 2-5.
    pub tournament_k: usize,

    /// Probability that two parents produce children via OX crossover.
    /// With probability `1 - crossover_p` children are clones of their parents.
    /// Typical value: 0.7-0.9.
    pub crossover_p: f64,

    /// Per-gene probability of a swap mutation (exchanges this gene with a random other).
    /// Preserves the permutation invariant.
    /// Typical value: 0.05-0.2.
    pub swap_p: f64,

    /// Per-gene probability of flipping the `rotate` flag.
    /// Only has effect when the piece allows rotation.
    /// Typical value: 0.02-0.1.
    pub flip_p: f64,

    /// Per-gene probability of nudging `point_selector` by a random amount.
    /// Controls which free rectangle the decoder tries first for this piece.
    /// Small steps let the GA explore rect choices smoothly rather than jumping.
    /// Typical value: 0.05-0.15.
    pub point_p: f64,

    /// Inclusive range `(lo, hi)` for the nudge magnitude applied to `point_selector`.
    /// A value is drawn uniformly from `lo..=hi` and added or subtracted (wrapping).
    /// Default: `(1, 3)`.
    pub point_delta: (u32, u32),
}

impl fmt::Display for GaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pop={} gens={} elite={} k={} crossover_p={:.2} swap_p={:.2} flip_p={:.2} point_p={:.2} delta={}..={}",
            self.pop_size,
            self.n_generations,
            self.n_elite,
            self.tournament_k,
            self.crossover_p,
            self.swap_p,
            self.flip_p,
            self.point_p,
            self.point_delta.0,
            self.point_delta.1,
        )
    }
}

/// A genome paired with its cached fitness value to avoid re-decoding during selection.
#[derive(Debug, Clone)]
pub struct Individual {
    pub genome: Genome,
    pub objective: Objective,
}

/// Progress snapshot emitted every `GaContext::progress_interval` generations.
/// Contains the current global best across all islands.
/// `objective` is pre-computed from genome to avoid re-decoding.
#[derive(Debug, Clone)]
pub struct ProgressEvent {
    pub seed: u64,
    pub generation: usize,
    pub genome: Genome,
    pub objective: Objective,
}

/// Event delivered from the GA to the caller via `GaHandle`.
#[derive(Debug)]
pub enum GaEvent {
    /// Emitted every `progress_interval` generations with the current global best.
    Progress(ProgressEvent),
    /// Emitted once when all islands finish; carries results sorted by objective.
    Done(Vec<(u64, Individual)>),
}

/// Caller-facing handle for observing and stopping a running GA.
///
/// Dropping the handle requests early termination - useful when an SSE client disconnects.
pub struct GaHandle {
    pub rx: UnboundedReceiver<GaEvent>,
    stop: Arc<AtomicBool>,
}

impl GaHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for GaHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Internal per-thread GA context. Obtain via `ga_channel`.
pub struct GaContext {
    tx: UnboundedSender<GaEvent>,
    stop: Arc<AtomicBool>,
    pub progress_interval: usize,
    pub seed: u64,
}

impl Clone for GaContext {
    fn clone(&self) -> Self {
        GaContext {
            tx: self.tx.clone(),
            stop: Arc::clone(&self.stop),
            progress_interval: self.progress_interval,
            seed: self.seed,
        }
    }
}

/// Creates a linked `(GaHandle, GaContext)` pair.
///
/// Pass `ctx` to `run_ga_mt`; use `handle` to read events and
/// call `handle.stop()` for early termination.
pub fn ga_channel(progress_interval: usize) -> (GaHandle, GaContext) {
    let (tx, rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    (
        GaHandle {
            rx,
            stop: Arc::clone(&stop),
        },
        GaContext {
            tx,
            stop,
            progress_interval,
            seed: 0,
        },
    )
}

/// Runs the GA for `config.n_generations` and returns the best `Individual` found.
///
/// Each generation: elite individuals are carried over unchanged; the remainder is
/// filled by tournament selection -> OX crossover (with probability `crossover_p`) ->
/// mutation -> decode. The running best is tracked independently of elitism so that
/// `n_elite = 0` still returns a valid result.
///
/// The returned `Individual.genome` contains flat piece indices (0..total_pieces).
/// Use `cutting::decode` to convert to a type-indexed `SolutionSpec`.
pub fn run_ga<R: Rng>(problem: &Problem, config: &GaConfig, rng: &mut R) -> Individual {
    let pool = Mutex::new(None);
    run_ga_inner(problem, config, rng, &pool, None)
}

/// Inner GA loop shared by `run_ga` and `run_ga_mt`.
///
/// When `ctx` is `Some`, every `progress_interval` generations:
/// - checks the stop flag and exits early if set
/// - updates the shared migration pool; injects global best into local population
/// - sends `GaEvent::Progress` with the current global best
fn run_ga_inner<R: Rng>(
    problem: &Problem,
    config: &GaConfig,
    rng: &mut R,
    migration_pool: &Mutex<Option<Individual>>,
    ctx: Option<&GaContext>,
) -> Individual {
    let mut pop = init_population(problem, config.pop_size, rng);
    let mut best = select_elite(&pop, 1).into_iter().next().unwrap();

    for step in 0..config.n_generations {
        let elite = select_elite(&pop, config.n_elite);
        let mut next_pop = elite;

        while next_pop.len() < config.pop_size {
            let p1 = tournament_select(&pop, config.tournament_k, rng).genome.clone();
            let p2 = tournament_select(&pop, config.tournament_k, rng).genome.clone();

            let (mut g1, mut g2) = if rng_01(rng) < config.crossover_p {
                ox_crossover(&p1, &p2, rng)
            } else {
                (p1, p2)
            };

            mutate(
                &mut g1,
                rng,
                config.swap_p,
                config.flip_p,
                config.point_p,
                config.point_delta,
            );
            let sol1 = decode(problem, &g1);
            next_pop.push(Individual {
                genome: g1,
                objective: sol1.objective(problem),
            });

            if next_pop.len() < config.pop_size {
                mutate(
                    &mut g2,
                    rng,
                    config.swap_p,
                    config.flip_p,
                    config.point_p,
                    config.point_delta,
                );
                let sol2 = decode(problem, &g2);
                next_pop.push(Individual {
                    genome: g2,
                    objective: sol2.objective(problem),
                });
            }
        }

        pop = next_pop;
        let gen_best = select_elite(&pop, 1).into_iter().next().unwrap();
        if gen_best.objective < best.objective {
            best = gen_best;
        }

        if let Some(ctx) = ctx
            && (step + 1) % ctx.progress_interval == 0
        {
            if ctx.stop.load(Ordering::Relaxed) {
                break;
            }
            let event = {
                let mut pool = migration_pool.lock().unwrap();
                if pool.as_ref().is_none_or(|g| best.objective < g.objective) {
                    *pool = Some(best.clone());
                }
                let worst_idx = pop
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, i)| i.objective)
                    .map(|(i, _)| i)
                    .unwrap();
                if let Some(global) = pool.as_ref()
                    && global.objective < pop[worst_idx].objective
                {
                    pop[worst_idx] = global.clone();
                }
                pool.as_ref().map(|g| {
                    GaEvent::Progress(ProgressEvent {
                        seed: ctx.seed,
                        generation: step + 1,
                        objective: g.objective,
                        genome: g.genome.clone(),
                    })
                })
            };
            if let Some(evt) = event {
                ctx.tx.send(evt).ok();
            }
        }
    }

    best
}

/// Spawns the GA in a background thread and returns immediately.
///
/// Progress and final results arrive through the `GaHandle` from `ga_channel`.
/// Sends `GaEvent::Progress` during the run and `GaEvent::Done` when finished.
/// Dropping `GaHandle` requests early termination via the stop flag.
pub fn run_ga_mt(spec: Arc<ProblemSpec>, config: Arc<GaConfig>, seeds: Vec<u64>, ctx: GaContext) {
    std::thread::spawn(move || {
        let flat = Arc::new(expand_problem(&spec));
        let migration_pool: Mutex<Option<Individual>> = Mutex::new(None);
        let pool_ref = &migration_pool;
        let p = &*flat;
        let c = &*config;
        let mut results: Vec<(u64, Individual)> = std::thread::scope(|s| {
            seeds
                .iter()
                .map(|&seed| {
                    let thread_ctx = GaContext { seed, ..ctx.clone() };
                    s.spawn(move || {
                        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
                        (seed, run_ga_inner(p, c, &mut rng, pool_ref, Some(&thread_ctx)))
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect()
        });
        results.sort_by_key(|(_, ind)| ind.objective);
        ctx.tx.send(GaEvent::Done(results)).ok();
    });
}

/// OX (Ordered Crossover) for two genomes.
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
///           ↓     ↓
/// P1: [ 0 │ 1  2 │ 3  4 ]  ──->  C1: [ 4 │ 1  2 │ 3  0 ]
/// P2: [ 3 │ 0  4 │ 1  2 ]  ──->  C2: [ 2 │ 0  4 │ 3  1 ]
///
///   C1 segment ← P1;  remaining ← P2 from hi, wrapping, skipping dupes
///   C2 segment ← P2;  remaining ← P1 from hi, wrapping, skipping dupes
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

/// CX (Cycle Crossover) for two genomes. No RNG required - cycle structure is
/// fully determined by the two parents.
///
/// **Note**: each `piece_idx` value appears exactly once in the genome (bijection).
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

/// Mutate a genome in-place. For each gene, independently:
/// - with probability `swap_p`: swap it with a random other gene (preserves permutation)
/// - with probability `flip_p`: flip `rotate`
/// - with probability `point_p`: nudge `point_selector` by ±`point_delta` wrapping
pub fn mutate<R: Rng>(
    genome: &mut Genome,
    rng: &mut R,
    swap_p: f64,
    flip_p: f64,
    point_p: f64,
    point_delta: (u32, u32),
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
    }
}

/// Returns the `n_elite` individuals with the lowest objective (lower is better),
/// sorted ascending. If `n_elite >= individuals.len()`, all are returned sorted.
/// Typical value: `n_elite = 1`.
pub fn select_elite(individuals: &[Individual], n_elite: usize) -> Vec<Individual> {
    let mut ranked = individuals.iter().collect::<Vec<_>>();
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

/// Generates `size` random individuals, each with a shuffled genome and a freshly computed
pub fn init_population<R: Rng>(problem: &Problem, size: usize, rng: &mut R) -> Vec<Individual> {
    let n = problem.pieces.len();
    (0..size)
        .map(|_| {
            let genome = random_genome(n, rng);
            let sol = decode(problem, &genome);
            Individual {
                genome,
                objective: sol.objective(problem),
            }
        })
        .collect()
}

fn random_genome<R: Rng>(n: usize, rng: &mut R) -> Genome {
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
        })
        .collect()
}

/// get random float in (0, 1)
fn rng_01<R: Rng>(rng: &mut R) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256StarStar;
    use crate::{expand::expand_problem, parse::parse_problem};

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
        mutate(
            &mut genome,
            &mut Xoshiro256StarStar::seed_from_u64(1),
            1.0,
            1.0,
            1.0,
            (1, 3),
        );
        assert_eq!(sorted_ids(&genome), (0..n).collect::<Vec<_>>());
    }

    #[test]
    fn mutate_no_op_at_zero_prob() {
        let orig: Genome = (0..5usize).map(g).collect();
        let mut genome = orig.clone();
        mutate(
            &mut genome,
            &mut Xoshiro256StarStar::seed_from_u64(2),
            0.0,
            0.0,
            0.0,
            (1, 3),
        );
        assert_eq!(genome, orig);
    }

    #[test]
    fn mutate_flips_all_rotate() {
        let n = 4;
        let mut genome: Genome = (0..n).map(g).collect();
        mutate(
            &mut genome,
            &mut Xoshiro256StarStar::seed_from_u64(3),
            0.0,
            1.0,
            0.0,
            (1, 3),
        );
        assert!(genome.iter().all(|g| g.rotate));
    }

    #[test]
    fn mutate_is_deterministic() {
        let orig: Genome = (0..6usize).map(g).collect();
        let mut g1 = orig.clone();
        let mut g2 = orig.clone();
        mutate(
            &mut g1,
            &mut Xoshiro256StarStar::seed_from_u64(42),
            0.3,
            0.2,
            0.2,
            (1, 3),
        );
        mutate(
            &mut g2,
            &mut Xoshiro256StarStar::seed_from_u64(42),
            0.3,
            0.2,
            0.2,
            (1, 3),
        );
        assert_eq!(g1, g2);
    }

    #[test]
    fn tournament_full_k_returns_best() {
        let pop = vec![ind(0, (0, 30)), ind(1, (0, 10)), ind(2, (0, 20))];
        let mut rng = Xoshiro256StarStar::seed_from_u64(1);
        let winner = tournament_select(&pop, 3, &mut rng);
        assert_eq!(winner.objective, (0, 10));
    }

    #[test]
    fn tournament_is_deterministic() {
        let pop = vec![ind(0, (0, 5)), ind(1, (0, 3)), ind(2, (0, 8)), ind(3, (0, 1))];
        let w1 = tournament_select(&pop, 2, &mut Xoshiro256StarStar::seed_from_u64(42));
        let w2 = tournament_select(&pop, 2, &mut Xoshiro256StarStar::seed_from_u64(42));
        assert_eq!(w1.objective, w2.objective);
    }

    #[test]
    fn init_population_size_and_valid_permutations() {
        let spec = parse_problem("10x10R:0:3x2,4x3,2x2f,5x1").unwrap();
        let flat = expand_problem(&spec);
        let n = flat.pieces.len();
        let mut rng = Xoshiro256StarStar::seed_from_u64(99);
        let pop = init_population(&flat, 20, &mut rng);
        assert_eq!(pop.len(), 20);
        for ind in &pop {
            assert_eq!(sorted_ids(&ind.genome), (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn init_population_is_deterministic() {
        let spec = parse_problem("10x10R:0:3x2,4x3,2x2f").unwrap();
        let flat = expand_problem(&spec);
        let pop1 = init_population(&flat, 5, &mut Xoshiro256StarStar::seed_from_u64(7));
        let pop2 = init_population(&flat, 5, &mut Xoshiro256StarStar::seed_from_u64(7));
        assert!(pop1.iter().zip(&pop2).all(|(a, b)| a.genome == b.genome));
    }

    fn default_config() -> GaConfig {
        GaConfig {
            pop_size: 20,
            n_generations: 10,
            n_elite: 1,
            tournament_k: 2,
            crossover_p: 0.8,
            swap_p: 0.1,
            flip_p: 0.05,
            point_p: 0.05,
            point_delta: (1, 3),
        }
    }

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

    fn ind(piece_idx: usize, objective: Objective) -> Individual {
        Individual {
            genome: vec![g(piece_idx)],
            objective,
        }
    }

    #[test]
    fn elite_returns_best() {
        let pop = vec![ind(0, (0, 30)), ind(1, (0, 10)), ind(2, (0, 20))];
        let elite = select_elite(&pop, 1);
        assert_eq!(elite.len(), 1);
        assert_eq!(elite[0].objective, (0, 10));
    }

    #[test]
    fn elite_top_k_sorted() {
        let pop = vec![ind(0, (0, 50)), ind(1, (0, 10)), ind(2, (0, 30)), ind(3, (0, 20))];
        let elite = select_elite(&pop, 2);
        assert_eq!(
            elite.iter().map(|e| e.objective).collect::<Vec<_>>(),
            [(0, 10), (0, 20)]
        );
    }

    #[test]
    fn elite_n_exceeds_pop() {
        let pop = vec![ind(0, (0, 5)), ind(1, (0, 3))];
        let elite = select_elite(&pop, 10);
        assert_eq!(elite.len(), 2);
        assert_eq!(elite[0].objective, (0, 3));
    }

    #[test]
    fn cx_known() {
        // P1=[0,1,2,3,4], P2=[3,0,4,1,2]
        // cycle 0 (even): positions {0,3,1} - trace: 0->pos_of(3)=3->pos_of(1)=1->pos_of(0)=0
        // cycle 1 (odd):  positions {2,4}   - trace: 2->pos_of(4)=4->pos_of(2)=2
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
        // P1 = identity -> cycles are all singletons, alternating parity
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
        // child1: segment from p1 -> [_,1,2,_,_]
        //   filler p2 from pos 3: 1(skip),2(skip),3,0,4 -> fill [3,4,0] with [3,0,4]
        //   -> [4,1,2,3,0]
        //
        // child2: segment from p2 -> [_,0,4,_,_]
        //   filler p1 from pos 3: 3,4(skip),0(skip),1,2 -> fill [3,4,0] with [3,1,2]
        //   -> [2,0,4,3,1]
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
