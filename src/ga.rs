use std::{
    fmt,
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::ScopedJoinHandle,
};

use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::model::Objective;

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

    /// Probability that two parents produce children via crossover.
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

    /// Per-gene probability of flipping the `inverse` flag.
    /// When flipped, the SLAS split direction is reversed for that piece, letting the GA
    /// represent cut trees that the default `lw <= lh` heuristic cannot.
    /// Typical value: 0.02-0.05.
    pub inverse_p: f64,

    /// Minimum dominant side length (px) for a piece type to be considered "long".
    /// Piece types with max(w,h) < long_dim_threshold go into the "small" class and are placed
    /// last by the glas decoder.
    /// 0 = auto-derive: max(sheet.width, sheet.height) * 0.3.
    pub long_dim_threshold: u32,

    /// Sqrt of the minimum area (px) for a long piece to be "large".
    /// A long piece is "large" if width*height >= large_area_threshold^2; otherwise "medium".
    /// 0 = auto-derive: sqrt(sheet.width * sheet.height * 0.05).
    pub large_area_threshold: u32,
}

impl fmt::Display for GaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pop={} gens={} elite={} k={} crossover_p={:.2} swap_p={:.2} flip_p={:.2} point_p={:.2} delta={}..={} inverse_p={:.2} long_dim_threshold={} large_area_threshold={}",
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
            self.inverse_p,
            self.long_dim_threshold,
            self.large_area_threshold,
        )
    }
}

impl Default for GaConfig {
    fn default() -> Self {
        Self {
            pop_size: 200,
            n_generations: 1000,
            n_elite: 2,
            tournament_k: 3,
            crossover_p: 0.80,
            swap_p: 0.15,
            flip_p: 0.05,
            point_p: 0.10,
            point_delta: (1, 3),
            inverse_p: 0.05,
            long_dim_threshold: 0,
            large_area_threshold: 0,
        }
    }
}

/// Encapsulates genome representation and genetic operators for one decoder variant.
///
/// Implementors (SlasDecoder, GlasDecoder, ...) own the problem data they need and
/// expose a uniform interface to the generic GA loop. The trait has no knowledge of
/// ProblemSpec / SolutionSpec -- those are wire types handled by the caller.
pub trait GaDecoder {
    type Genome: Clone + Send + 'static;

    fn random_genome<R: Rng>(&self, config: &GaConfig, rng: &mut R) -> Self::Genome;
    fn eval(&self, genome: &Self::Genome) -> Objective;
    fn crossover<R: Rng>(&self, p1: &Self::Genome, p2: &Self::Genome, rng: &mut R) -> (Self::Genome, Self::Genome);
    fn mutate<R: Rng>(&self, genome: &mut Self::Genome, config: &GaConfig, rng: &mut R);

    /// Deterministic seed genomes injected at position 0 in the initial population.
    /// Default: empty (all individuals are random).
    fn seed_genomes(&self, _config: &GaConfig) -> Vec<Self::Genome> {
        vec![]
    }
}

#[derive(Debug, Clone)]
pub struct Individual<G> {
    pub genome: G,
    pub objective: Objective,
}

#[derive(Debug, Clone)]
pub struct ProgressEvent<G> {
    pub seed: u64,
    pub generation: usize,
    pub genome: G,
    pub objective: Objective,
}

#[derive(Debug)]
pub enum GaEvent<G: Clone + Send + 'static> {
    Progress(ProgressEvent<G>),
    Done(Vec<(u64, Individual<G>)>),
}

/// Caller-facing handle for observing and stopping a running GA.
///
/// Dropping the handle requests early termination.
pub struct GaHandle<G: Clone + Send + 'static> {
    pub rx: UnboundedReceiver<GaEvent<G>>,
    stop: Arc<AtomicBool>,
}

impl<G: Clone + Send + 'static> GaHandle<G> {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Blocks until the GA run finishes, discarding intermediate `Progress` events.
    /// Returns results sorted best-first (see `GaEvent::Done`).
    pub fn blocking_wait(mut self) -> Vec<(u64, Individual<G>)> {
        loop {
            match self.rx.blocking_recv() {
                Some(GaEvent::Done(results)) => return results,
                Some(GaEvent::Progress(_)) => continue,
                None => return Vec::new(),
            }
        }
    }
}

impl<G: Clone + Send + 'static> Drop for GaHandle<G> {
    fn drop(&mut self) {
        self.stop();
    }
}

struct GaContext<G: Clone + Send + 'static> {
    tx: UnboundedSender<GaEvent<G>>,
    stop: Arc<AtomicBool>,
    progress_interval: usize,
    seed: u64,
}

impl<G: Clone + Send + 'static> Clone for GaContext<G> {
    fn clone(&self) -> Self {
        GaContext {
            tx: self.tx.clone(),
            stop: Arc::clone(&self.stop),
            progress_interval: self.progress_interval,
            seed: self.seed,
        }
    }
}

/// Shared state for barrier-based migration in `run_ga_mt`.
///
/// All N threads synchronize every `interval` generations via two barriers:
/// - `barrier1`: all threads have written their best individual to their slot
/// - `barrier2`: all threads have read the global best and injected it
///
/// Stop flag is checked only after `barrier2` to avoid deadlocks.
struct SyncMigration<'a, G: Clone + Send + 'static> {
    bests: &'a [Mutex<Option<Individual<G>>>],
    barrier1: &'a Barrier,
    barrier2: &'a Barrier,
    idx: usize,
    interval: usize,
}

fn ga_channel<G: Clone + Send + 'static>(progress_interval: usize) -> (GaHandle<G>, GaContext<G>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let stop = Arc::new(AtomicBool::new(false));
    let handle = GaHandle {
        rx,
        stop: Arc::clone(&stop),
    };
    let context = GaContext {
        tx,
        stop,
        progress_interval,
        seed: 0,
    };
    (handle, context)
}

fn rng_01<R: Rng>(rng: &mut R) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Returns the `n_elite` individuals with the lowest objective (lower is better),
/// sorted ascending. If `n_elite >= individuals.len()`, all are returned sorted.
pub fn select_elite<G: Clone>(individuals: &[Individual<G>], n_elite: usize) -> Vec<Individual<G>> {
    let mut ranked = individuals.iter().collect::<Vec<&Individual<G>>>();
    ranked.sort_unstable_by_key(|ind| ind.objective);
    ranked.into_iter().take(n_elite).cloned().collect()
}

/// Picks `k` individuals at random and returns the one with the lowest objective.
pub fn tournament_select<'a, G, R: Rng>(individuals: &'a [Individual<G>], k: usize, rng: &mut R) -> &'a Individual<G> {
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

/// Runs the GA for `config.n_generations` and returns the best individual found.
pub fn run_ga<D: GaDecoder, R: Rng>(decoder: &D, config: &GaConfig, rng: &mut R) -> Individual<D::Genome> {
    run_ga_inner(decoder, config, None, None, rng)
}

/// Spawns the GA on multiple threads (one per seed) and returns a `GaHandle`.
///
/// Events arrive through `handle.rx`: `GaEvent::Progress` every `migration_interval`
/// generations (when `migration_interval > 0`) and `GaEvent::Done` when all islands finish.
/// Dropping the handle requests early termination.
///
/// `migration_interval = 0` disables migration entirely (independent islands).
/// With `migration_interval > 0` all islands synchronize at a global barrier every N
/// generations and share the best individual, which makes results fully deterministic:
/// identical seeds + identical config always produce identical output.
pub fn run_ga_mt<D: GaDecoder + Send + Sync + 'static>(
    decoder: Arc<D>,
    config: Arc<GaConfig>,
    seeds: Vec<u64>,
    progress_interval: usize,
    migration_interval: usize,
) -> GaHandle<D::Genome> {
    let (handle, ctx) = ga_channel::<D::Genome>(progress_interval);
    std::thread::spawn(move || {
        let n = seeds.len();
        let bests = (0..n)
            .map(|_| Mutex::new(None))
            .collect::<Vec<Mutex<Option<Individual<D::Genome>>>>>();
        let barrier1 = Barrier::new(n);
        let barrier2 = Barrier::new(n);
        let d = &*decoder;
        let c = &*config;
        let mut results = std::thread::scope(|s| {
            seeds
                .iter()
                .enumerate()
                .map(|(idx, &seed)| {
                    let mig = if migration_interval > 0 && n > 1 {
                        Some(SyncMigration {
                            bests: &bests,
                            barrier1: &barrier1,
                            barrier2: &barrier2,
                            idx,
                            interval: migration_interval,
                        })
                    } else {
                        None
                    };
                    let thread_ctx = GaContext { seed, ..ctx.clone() };
                    s.spawn(move || {
                        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
                        let individual = run_ga_inner(d, c, mig, Some(&thread_ctx), &mut rng);
                        (seed, individual)
                    })
                })
                .collect::<Vec<ScopedJoinHandle<_>>>()
                .into_iter()
                .map(|h| h.join().expect("GA thread panicked"))
                .collect::<Vec<(u64, Individual<D::Genome>)>>()
        });
        results.sort_by_key(|(_, ind)| ind.objective);
        ctx.tx.send(GaEvent::Done(results)).ok();
    });
    handle
}

fn init_population<D: GaDecoder, R: Rng>(
    decoder: &D,
    config: &GaConfig,
    size: usize,
    rng: &mut R,
) -> Vec<Individual<D::Genome>> {
    let seeds = decoder.seed_genomes(config);
    let n_seeds = seeds.len().min(size);
    let mut pop = seeds
        .into_iter()
        .take(n_seeds)
        .map(|genome| {
            let objective = decoder.eval(&genome);
            Individual { genome, objective }
        })
        .collect::<Vec<_>>();
    pop.extend((n_seeds..size).map(|_| {
        let genome = decoder.random_genome(config, rng);
        let objective = decoder.eval(&genome);
        Individual { genome, objective }
    }));
    pop
}

fn run_ga_inner<D: GaDecoder, R: Rng>(
    decoder: &D,
    config: &GaConfig,
    migration: Option<SyncMigration<'_, D::Genome>>,
    ctx: Option<&GaContext<D::Genome>>,
    rng: &mut R,
) -> Individual<D::Genome> {
    let mut pop = init_population(decoder, config, config.pop_size, rng);
    let mut best = select_elite(&pop, 1).into_iter().next().expect("pop is non-empty");

    for step in 0..config.n_generations {
        let elite = select_elite(&pop, config.n_elite);
        let mut next_pop = elite;

        while next_pop.len() < config.pop_size {
            let p1 = tournament_select(&pop, config.tournament_k, rng).genome.clone();
            let p2 = tournament_select(&pop, config.tournament_k, rng).genome.clone();

            let (mut g1, mut g2) = if rng_01(rng) < config.crossover_p {
                decoder.crossover(&p1, &p2, rng)
            } else {
                (p1, p2)
            };

            decoder.mutate(&mut g1, config, rng);
            let obj1 = decoder.eval(&g1);
            next_pop.push(Individual {
                genome: g1,
                objective: obj1,
            });

            if next_pop.len() < config.pop_size {
                decoder.mutate(&mut g2, config, rng);
                let obj2 = decoder.eval(&g2);
                next_pop.push(Individual {
                    genome: g2,
                    objective: obj2,
                });
            }
        }

        pop = next_pop;
        let gen_best = select_elite(&pop, 1).into_iter().next().expect("pop is non-empty");
        if gen_best.objective < best.objective {
            best = gen_best;
        }

        // Progress when migration is disabled: each island reports its local best.
        // Stop check is safe here because there is no barrier waiting.
        if migration.is_none()
            && let Some(ctx) = ctx
            && ctx.progress_interval > 0
            && (step + 1) % ctx.progress_interval == 0
        {
            ctx.tx
                .send(GaEvent::Progress(ProgressEvent {
                    seed: ctx.seed,
                    generation: step + 1,
                    objective: best.objective,
                    genome: best.genome.clone(),
                }))
                .ok();
            if ctx.stop.load(Ordering::Relaxed) {
                break;
            }
        }

        if let Some(ref mig) = migration
            && mig.interval > 0
            && (step + 1) % mig.interval == 0
        {
            // Phase 1: write current best into own slot
            {
                let mut slot = mig.bests[mig.idx].lock().expect("migration slot poisoned");
                if slot.as_ref().is_none_or(|g| best.objective < g.objective) {
                    *slot = Some(best.clone());
                }
            }
            mig.barrier1.wait(); // all slots written

            // Phase 2: read global best, inject into worst; island 0 sends one progress event.
            {
                let global = mig
                    .bests
                    .iter()
                    .filter_map(|s| s.lock().expect("migration slot poisoned").clone())
                    .min_by_key(|i| i.objective);
                if let Some(gb) = global {
                    let worst_idx = pop
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, i)| i.objective)
                        .map(|(i, _)| i)
                        .expect("pop is non-empty");
                    if gb.objective < pop[worst_idx].objective {
                        pop[worst_idx] = gb.clone();
                    }
                    if mig.idx == 0
                        && let Some(ctx) = ctx
                    {
                        ctx.tx
                            .send(GaEvent::Progress(ProgressEvent {
                                seed: ctx.seed,
                                generation: step + 1,
                                objective: gb.objective,
                                genome: gb.genome.clone(),
                            }))
                            .ok();
                    }
                }
            }
            mig.barrier2.wait(); // all threads done reading; safe to start next epoch

            // Check stop only after both barriers to avoid deadlock
            if ctx.is_some_and(|c| c.stop.load(Ordering::Relaxed)) {
                break;
            }
        }
    }

    best
}
