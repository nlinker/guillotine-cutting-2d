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

use crate::model::{Objective, ProblemSpec, SolutionSpec};

/// GA hyperparameters.
#[derive(Debug, Clone)]
pub struct GaConfig {
    /// Number of individuals in the population.
    pub pop_size: usize,

    /// Number of generations (=GA iterations) to run
    pub iteration_count: usize,

    /// Number of top individuals copied unchanged into the next generation (elitism).
    /// Typical value: 1-2. Set to 0 to disable.
    pub elite_count: usize,

    /// Number of individuals competing for each parent slot (tournament selection).
    /// Typical value: 2-5.
    pub tournament_size: usize,

    /// Probability of crossover; otherwise children are clones of their parents.
    /// Typical value: 0.7-0.9.
    pub crossover_p: f64,

    /// Per-gene swap-mutation probability (exchanges with a random other gene, permutation-safe).
    /// Typical value: 0.05-0.2.
    pub swap_p: f64,

    /// Per-gene probability of flipping the `rotate` flag (no-op if the piece can't rotate).
    /// Typical value: 0.02-0.1.
    pub flip_p: f64,

    /// Per-gene probability of shifting `point_selector`, which picks the free rect the decoder
    /// tries first for this piece.
    /// Typical value: 0.05-0.15.
    pub point_p: f64,

    /// Inclusive range `(lo, hi)` for the `point_selector` shift (drawn uniformly, added or
    /// subtracted with wrapping).
    /// Default: `(1, 3)`.
    pub point_delta: (u32, u32),

    /// Per-gene probability of flipping the split-direction flag.
    /// Typical value: 0.02-0.05.
    pub inverse_p: f64,

    /// Minimum dominant side (px) for a piece type to count as "long"; smaller types are placed
    /// last by the glas decoder.
    /// 0 = auto-derive: max(sheet.width, sheet.height) * 0.3.
    pub long_dim_threshold: u32,

    /// Sqrt of the minimum area (px) for a long piece to count as "large" vs "medium".
    /// 0 = auto-derive: sqrt(sheet.width * sheet.height * 0.05).
    pub large_area_threshold: u32,
}

impl fmt::Display for GaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pop={} gens={} elite={} k={} crossover_p={:.2} swap_p={:.2} flip_p={:.2} point_p={:.2} delta={}..={} inverse_p={:.2} long_dim_threshold={} large_area_threshold={}",
            self.pop_size,
            self.iteration_count,
            self.elite_count,
            self.tournament_size,
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
            iteration_count: 1000,
            elite_count: 2,
            tournament_size: 3,
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

impl GaConfig {
    pub fn new(
        spec: &ProblemSpec,
        iteration_count: usize,
        pop_size: usize,
        large_area_threshold: u32,
        long_dim_threshold: u32,
    ) -> Self {
        let sh = spec.sheet;
        let default_long = (sh.width.max(sh.height) as f64 * 0.3) as u32;
        let default_large = (sh.width as f64 * sh.height as f64 * 0.05).sqrt() as u32;
        let long_dim_threshold = if long_dim_threshold == 0 {
            default_long
        } else {
            long_dim_threshold
        };
        let large_area_threshold = if large_area_threshold == 0 {
            default_large
        } else {
            large_area_threshold
        };
        Self {
            pop_size,
            iteration_count,
            long_dim_threshold,
            large_area_threshold,
            ..Self::default()
        }
    }
}

/// Genome representation and genetic operators for one decoder variant (SlasDecoder,
/// GlasDecoder, ...). Has no knowledge of ProblemSpec/SolutionSpec -- those are wire types
/// handled by the caller.
pub trait GaDecoder {
    type Genome: Clone + Send + 'static;

    fn random_genome<R: Rng>(&self, config: &GaConfig, rng: &mut R) -> Self::Genome;
    fn eval(&self, genome: &Self::Genome) -> Objective;
    fn crossover<R: Rng>(&self, p1: &Self::Genome, p2: &Self::Genome, rng: &mut R) -> (Self::Genome, Self::Genome);
    fn mutate<R: Rng>(&self, genome: &mut Self::Genome, config: &GaConfig, rng: &mut R);

    /// Deterministic seed genomes injected into the initial population. Default: empty.
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

#[derive(Debug, Clone)]
pub struct DoneEvent<G> {
    pub pairs: Vec<(u64, Individual<G>)>,
}

#[derive(Debug)]
pub enum GaEvent<G: Clone + Send + 'static> {
    Progress(ProgressEvent<G>),
    Done(DoneEvent<G>),
}

/// A genome that can decode itself into a solution. Lets `runner::Eraser` be implemented
/// directly on `GaHandle<G>`, no separate decode closure needed.
pub trait Decodable {
    fn decode(&self, spec: &ProblemSpec) -> SolutionSpec;
}

/// Caller-side handle for observing and stopping a running GA. Drop to terminate early.
pub struct GaHandle<G: Clone + Send + 'static> {
    pub rx: UnboundedReceiver<GaEvent<G>>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl<G: Clone + Send + 'static> GaHandle<G> {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Joins the supervisor thread, if not already joined.
    /// Called from `blocking_wait` below, and from
    /// `AnyHandle::blocking_wait`/`drain` via `Eraser::join`.
    pub fn join(&mut self) {
        if let Some(h) = self.join.take()
            && let Err(payload) = h.join()
        {
            eprintln!("GA supervisor thread panicked: {}", panic_message(&*payload));
        }
    }

    /// Blocks until the GA finishes, discarding `Progress` events; returns results sorted
    /// best-first (see `GaEvent::Done`).
    pub fn blocking_wait(mut self) -> Vec<(u64, Individual<G>)> {
        let result = loop {
            match self.rx.blocking_recv() {
                Some(GaEvent::Done(done)) => break done.pairs,
                Some(GaEvent::Progress(_)) => continue,
                None => break Vec::new(),
            }
        };
        self.join();
        result
    }
}

/// Extracts a readable message from a thread panic payload, with a fallback for non-string
/// payloads. Called from `GaHandle::join` when the supervisor thread panicked.
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panicked with a non-string payload".to_string())
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

/// Shared state for barrier-based _migration_ (GA threads exchanging their current best
/// individual) in `run_ga_mt`. All N threads synchronize every `interval` generations via
/// two barriers:
/// `barrier1` once each has written its best individual to its slot, `barrier2` once each has
/// read the global best and injected it. Stop flag is checked only after `barrier2`, to avoid
/// deadlock.
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
    let handle = GaHandle { rx, stop: Arc::clone(&stop), join: None };
    let context = GaContext { tx, stop, progress_interval, seed: 0 };
    (handle, context)
}

pub fn rng_01<R: Rng>(rng: &mut R) -> f64 {
    (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Returns the `n_elite` best individuals by objective, sorted ascending; if
/// `n_elite >= individuals.len()`, returns all, sorted.
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

/// Runs the GA for `config.n_iterations` and returns the best individual found.
pub fn run_ga<D: GaDecoder, R: Rng>(decoder: &D, config: &GaConfig, rng: &mut R) -> Individual<D::Genome> {
    run_ga_inner(decoder, config, None, None, rng)
}

/// Spawns the GA on multiple threads (one per seed) and returns a `GaHandle`. Dropping the
/// handle requests early termination.
///
/// `handle.rx` receives `GaEvent::Progress` and `GaEvent::Done` messages. With
/// migration on, islands share their best individual via a barrier each interval, which makes
/// output _fully deterministic_ for a given seed set and config.
pub fn run_ga_mt<D: GaDecoder + Send + Sync + 'static>(
    decoder: Arc<D>,
    config: Arc<GaConfig>,
    seeds: Vec<u64>,
    progress_interval: usize,
    migration_interval: usize,
) -> GaHandle<D::Genome> {
    let (mut handle, ctx) = ga_channel::<D::Genome>(progress_interval);
    let join = std::thread::spawn(move || {
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
        ctx.tx.send(GaEvent::Done(DoneEvent { pairs: results })).ok();
    });
    handle.join = Some(join);
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

    for step in 0..config.iteration_count {
        let elite = select_elite(&pop, config.elite_count);
        let mut next_pop = elite;

        while next_pop.len() < config.pop_size {
            let p1 = tournament_select(&pop, config.tournament_size, rng).genome.clone();
            let p2 = tournament_select(&pop, config.tournament_size, rng).genome.clone();

            let (mut g1, mut g2) = if rng_01(rng) < config.crossover_p {
                decoder.crossover(&p1, &p2, rng)
            } else {
                (p1, p2)
            };

            decoder.mutate(&mut g1, config, rng);
            let obj1 = decoder.eval(&g1);
            next_pop.push(Individual { genome: g1, objective: obj1 });

            if next_pop.len() < config.pop_size {
                decoder.mutate(&mut g2, config, rng);
                let obj2 = decoder.eval(&g2);
                next_pop.push(Individual { genome: g2, objective: obj2 });
            }
        }

        pop = next_pop;
        let gen_best = select_elite(&pop, 1).into_iter().next().expect("pop is non-empty");
        if gen_best.objective < best.objective {
            best = gen_best;
        }

        // Migration disabled. Each island reports its own local best.
        // To check stop is safe here (no barrier waiting).
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
            // Step 1 - write current best into own slot
            {
                let mut slot = mig.bests[mig.idx].lock().expect("migration slot poisoned");
                if slot.as_ref().is_none_or(|g| best.objective < g.objective) {
                    *slot = Some(best.clone());
                }
            }
            mig.barrier1.wait(); // all slots written

            // Step 2 - read global best, inject into worst; island 0 sends one progress event.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ind(objective: Objective) -> Individual<()> {
        Individual { genome: (), objective }
    }

    #[derive(Clone)]
    struct PanicGenome;

    /// The decoder that panics on `eval`
    struct PanicDecoder;

    impl GaDecoder for PanicDecoder {
        type Genome = PanicGenome;

        fn random_genome<R: Rng>(&self, _config: &GaConfig, _rng: &mut R) -> Self::Genome {
            PanicGenome
        }

        fn eval(&self, _genome: &Self::Genome) -> Objective {
            panic!("PanicDecoder::eval always panics");
        }

        fn crossover<R: Rng>(
            &self,
            _p1: &Self::Genome,
            _p2: &Self::Genome,
            _rng: &mut R,
        ) -> (Self::Genome, Self::Genome) {
            (PanicGenome, PanicGenome)
        }

        fn mutate<R: Rng>(&self, _genome: &mut Self::Genome, _config: &GaConfig, _rng: &mut R) {}
    }

    // Checks the panic is joined (not a zombie thread) and `blocking_wait` still returns.
    #[test]
    fn run_ga_mt_panic_is_joined_not_lost() {
        let prev_hook = std::panic::take_hook();
        // suppress the panic-hook print; the unwind (and our catch via join) still happens
        std::panic::set_hook(Box::new(|_| {}));

        let decoder = Arc::new(PanicDecoder);
        let config = Arc::new(GaConfig { pop_size: 4, iteration_count: 5, ..GaConfig::default() });
        let handle = run_ga_mt(decoder, config, vec![1], 0, 0);
        let results = handle.blocking_wait();

        std::panic::set_hook(prev_hook);

        assert!(results.is_empty());
    }

    #[test]
    fn tournament_full_k_returns_best() {
        let o = |dc| Objective { sheets_used: 0.0, drop_consolidation_score: dc, layout_score: 0 };
        let pop = vec![ind(o(30)), ind(o(10)), ind(o(20))];
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
    fn tournament_is_deterministic() {
        let o = |dc| Objective { sheets_used: 0.0, drop_consolidation_score: dc, layout_score: 0 };
        let pop = vec![ind(o(5)), ind(o(3)), ind(o(8)), ind(o(1))];
        let w1 = tournament_select(&pop, 2, &mut Xoshiro256StarStar::seed_from_u64(42));
        let w2 = tournament_select(&pop, 2, &mut Xoshiro256StarStar::seed_from_u64(42));
        assert_eq!(w1.objective, w2.objective);
    }

    #[test]
    fn elite_top_k_sorted() {
        let o = |dc| Objective { sheets_used: 0.0, drop_consolidation_score: dc, layout_score: 0 };
        let pop = vec![ind(o(50)), ind(o(10)), ind(o(30)), ind(o(20))];
        let elite = select_elite(&pop, 2);
        assert_eq!(
            elite
                .iter()
                .map(|e| (e.objective.sheets_used_int(), e.objective.drop_consolidation_score))
                .collect::<Vec<_>>(),
            [(0, 50), (0, 30)]
        );
    }

    #[test]
    fn elite_n_exceeds_pop() {
        let o = |dc| Objective { sheets_used: 0.0, drop_consolidation_score: dc, layout_score: 0 };
        let pop = vec![ind(o(5)), ind(o(3))];
        let elite = select_elite(&pop, 10);
        assert_eq!(elite.len(), 2);
        assert_eq!(
            (
                elite[0].objective.sheets_used_int(),
                elite[0].objective.drop_consolidation_score
            ),
            (0, 5)
        );
    }
}
