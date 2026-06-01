use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::{
    expand::expand_problem,
    ga::GaConfig,
    glas::decoder::{Gene, Genome, decode},
    model::{Objective, PieceSpec, Problem, ProblemSpec},
};

/// A glas genome paired with its cached fitness value to avoid re-decoding during selection.
#[derive(Debug, Clone)]
pub struct Individual {
    pub genome: Genome,
    pub objective: Objective,
}

/// Progress snapshot emitted every `progress_interval` generations.
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
/// Dropping the handle requests early termination.
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

/// Internal per-thread GA context.
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

/// Runs the GA for `config.n_generations` and returns the best `Individual` found.
pub fn run_ga<R: Rng>(spec: &ProblemSpec, problem: &Problem, config: &GaConfig, rng: &mut R) -> Individual {
    let pool = Mutex::new(None);
    run_ga_inner(spec, problem, config, &pool, None, rng)
}

/// Spawns the GA on multiple threads (one per seed) and returns a `GaHandle`.
///
/// Events arrive through `handle.rx`: `GaEvent::Progress` every `progress_interval`
/// generations and `GaEvent::Done` when all islands finish.
/// Dropping the handle requests early termination.
pub fn run_ga_mt(spec: Arc<ProblemSpec>, config: Arc<GaConfig>, seeds: Vec<u64>, progress_interval: usize) -> GaHandle {
    let (handle, ctx) = ga_channel(progress_interval);
    std::thread::spawn(move || {
        let flat = Arc::new(expand_problem(&spec));
        let migration_pool: Mutex<Option<Individual>> = Mutex::new(None);
        let pool_ref = &migration_pool;
        let s = &*spec;
        let p = &*flat;
        let c = &*config;
        let mut results: Vec<(u64, Individual)> = std::thread::scope(|scope| {
            seeds
                .iter()
                .map(|&seed| {
                    let thread_ctx = GaContext { seed, ..ctx.clone() };
                    scope.spawn(move || {
                        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
                        let individual = run_ga_inner(s, p, c, pool_ref, Some(&thread_ctx), &mut rng);
                        (seed, individual)
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|h| h.join().expect("GA thread panicked"))
                .collect()
        });
        results.sort_by_key(|(_, ind)| ind.objective);
        ctx.tx.send(GaEvent::Done(results)).ok();
    });
    handle
}

/// OX (Ordered Crossover) for two glas genomes.
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

/// CX (Cycle Crossover) for two glas genomes.
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

/// Mutate a glas genome in-place. Applied independently per class.
/// For each gene within a class (when class has ≥2 genes):
/// - with probability `swap_p`: swap it with a random other gene within the same class
/// - with probability `flip_p`: flip `rotate`
/// - for each `selectors[k]` with probability `point_p`: nudge by ±`point_delta` (wrapping)
/// - for each `inverses[k]` with probability `inverse_p`: flip the boolean (TlH↔TlV)
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

/// Generates `size` random individuals for the given spec.
pub fn init_population<R: Rng>(
    spec: &ProblemSpec,
    problem: &Problem,
    size: usize,
    config: &GaConfig,
    rng: &mut R,
) -> Vec<Individual> {
    (0..size)
        .map(|_| {
            let genome = random_genome(spec, config.long_dim_ratio, config.large_area_ratio, rng);
            let sol = decode(problem, spec, &genome);
            Individual {
                genome,
                objective: sol.eval(problem),
            }
        })
        .collect()
}

/// Returns the `n_elite` individuals with the lowest objective, sorted ascending.
pub fn select_elite(individuals: &[Individual], n_elite: usize) -> Vec<Individual> {
    let mut ranked = individuals.iter().collect::<Vec<_>>();
    ranked.sort_unstable_by_key(|ind| ind.objective);
    ranked.into_iter().take(n_elite).cloned().collect()
}

/// Picks `k` individuals at random and returns the one with the lowest objective.
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

fn run_ga_inner<R: Rng>(
    spec: &ProblemSpec,
    problem: &Problem,
    config: &GaConfig,
    migration_pool: &Mutex<Option<Individual>>,
    ctx: Option<&GaContext>,
    rng: &mut R,
) -> Individual {
    let mut pop = init_population(spec, problem, config.pop_size, config, rng);
    let mut best = select_elite(&pop, 1).into_iter().next().expect("pop is non-empty");

    for step in 0..config.n_generations {
        let elite = select_elite(&pop, config.n_elite);
        let mut next_pop = elite;

        while next_pop.len() < config.pop_size {
            let p1 = tournament_select(&pop, config.tournament_k, rng).genome.clone();
            let p2 = tournament_select(&pop, config.tournament_k, rng).genome.clone();

            let (mut g1, mut g2) = if rng_01(rng) < config.crossover_p {
                ox_crossover(&p1, &p2, spec.piespecs.len(), rng)
            } else {
                (p1, p2)
            };

            mutate(
                &mut g1,
                config.swap_p,
                config.flip_p,
                config.point_p,
                config.point_delta,
                config.inverse_p,
                rng,
            );
            let sol1 = decode(problem, spec, &g1);
            next_pop.push(Individual {
                genome: g1,
                objective: sol1.eval(problem),
            });

            if next_pop.len() < config.pop_size {
                mutate(
                    &mut g2,
                    config.swap_p,
                    config.flip_p,
                    config.point_p,
                    config.point_delta,
                    config.inverse_p,
                    rng,
                );
                let sol2 = decode(problem, spec, &g2);
                next_pop.push(Individual {
                    genome: g2,
                    objective: sol2.eval(problem),
                });
            }
        }

        pop = next_pop;
        let gen_best = select_elite(&pop, 1).into_iter().next().expect("pop is non-empty");
        if gen_best.objective < best.objective {
            best = gen_best;
        }

        if let Some(ctx) = ctx
            && ctx.progress_interval > 0
            && (step + 1) % ctx.progress_interval == 0
        {
            if ctx.stop.load(Ordering::Relaxed) {
                break;
            }
            let event = {
                let mut pool = migration_pool.lock().expect("migration pool lock poisoned");
                if pool.as_ref().is_none_or(|g| best.objective < g.objective) {
                    *pool = Some(best.clone());
                }
                let worst_idx = pop
                    .iter()
                    .enumerate()
                    .max_by_key(|(_, i)| i.objective)
                    .map(|(i, _)| i)
                    .expect("pop is non-empty");
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

fn ox_at(p1: &[Gene], p2: &[Gene], lo: usize, hi: usize, n_types: usize) -> (Vec<Gene>, Vec<Gene>) {
    fn build_child(donor: &[Gene], filler: &[Gene], lo: usize, hi: usize, n_types: usize) -> Vec<Gene> {
        let n = donor.len();
        let mut in_segment = vec![false; n_types];
        for gene in &donor[lo..hi] {
            in_segment[gene.type_idx] = true;
        }
        // Filler genes in OX order (starting from hi, wrapping), skipping segment types.
        // Processed lazily — no intermediate Vec allocation.
        let mut fill_iter = (0..n)
            .map(|i| &filler[(hi + i) % n])
            .filter(|g| !in_segment[g.type_idx]);
        // Fill positions in OX order: (hi..n) then (0..lo) — no intermediate Vec.
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

fn piece_class(ps: &PieceSpec, spec: &ProblemSpec, long_dim_ratio: f64, large_area_ratio: f64) -> usize {
    let max_dim = ps.width.max(ps.height) as f64;
    let sheet_max = spec.sheet.width.max(spec.sheet.height) as f64;
    let area = ps.width as f64 * ps.height as f64;
    let sheet_area = spec.sheet.width as f64 * spec.sheet.height as f64;
    if max_dim >= sheet_max * long_dim_ratio {
        if area >= sheet_area * large_area_ratio { 0 } else { 1 }
    } else {
        2
    }
}

fn random_genome<R: Rng>(spec: &ProblemSpec, long_dim_ratio: f64, large_area_ratio: f64, rng: &mut R) -> Genome {
    let n = spec.piespecs.len();
    let mut classes: [Vec<usize>; 3] = [vec![], vec![], vec![]];
    for i in 0..n {
        classes[piece_class(&spec.piespecs[i], spec, long_dim_ratio, large_area_ratio)].push(i);
    }
    let mut genome = Genome::with_capacity(3);
    for mut indices in classes {
        // Sort by total area descending within class, then apply a small random perturbation.
        indices.sort_unstable_by(|&a, &b| {
            let area = |i: usize| {
                let ps = &spec.piespecs[i];
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
        let genes: Vec<Gene> = indices
            .into_iter()
            .map(|type_idx| {
                let count = spec.piespecs[type_idx].count as usize;
                Gene {
                    type_idx,
                    rotate: rng.next_u64() & 1 != 0,
                    selectors: (0..count).map(|_| rng.next_u64() as u32).collect(),
                    inverses: (0..count).map(|_| rng.next_u64() & 1 != 0).collect(),
                }
            })
            .collect();
        genome.push(genes);
    }
    genome
}

fn ga_channel(progress_interval: usize) -> (GaHandle, GaContext) {
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

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256StarStar;

    use super::*;
    use crate::{expand::expand_problem, ga::GaConfig, parse::parse_problem};

    /// Build a gene for `type_idx` with `count` selectors/inverses all zeroed/false.
    fn gg(type_idx: usize, count: usize) -> Gene {
        Gene {
            type_idx,
            rotate: false,
            selectors: std::iter::repeat(0u32).take(count).collect(),
            inverses: std::iter::repeat(false).take(count).collect(),
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
            flip_p: 0.05,
            point_p: 0.05,
            point_delta: (1, 3),
            inverse_p: 0.05,
            long_dim_ratio: 0.29,
            large_area_ratio: 0.034,
        }
    }

    fn one_class(genes: Vec<Gene>) -> Genome {
        vec![genes]
    }

    #[test]
    fn mutate_preserves_permutation() {
        // 4 types, count=2 each, all in one class
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
        // With inverse_p=1.0, every inverse value (initially false) must be flipped to true.
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
        // With point_p=1.0 every selector must change (delta >= 1).
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
        let orig_sels: Vec<u32> = orig
            .iter()
            .flat_map(|c| c.iter())
            .flat_map(|g| g.selectors.iter().copied())
            .collect();
        let new_sels: Vec<u32> = genome
            .iter()
            .flat_map(|c| c.iter())
            .flat_map(|g| g.selectors.iter().copied())
            .collect();
        // Every selector shifted by exactly 1 (wrapping) — none are equal to the original.
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

    #[test]
    fn ox_at_known() {
        // p1=[0,1,2,3,4], p2=[3,0,4,1,2], segment [lo=1, hi=3)
        // child1 segment from p1: [_,1,2,_,_]; filler from p2 at 3: 1(skip),2(skip),3,0,4 → [4,1,2,3,0]
        // child2 segment from p2: [_,0,4,_,_]; filler from p1 at 3: 3,4(skip),0(skip),1,2 → [2,0,4,3,1]
        let p1: Vec<Gene> = (0..5).map(|i| gg(i, 1)).collect();
        let p2: Vec<Gene> = [3usize, 0, 4, 1, 2].into_iter().map(|i| gg(i, 1)).collect();
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
        // Verify that when gene type_idx=2 lands in the child, its selectors/inverses are preserved.
        let mut p1 = one_class((0..5).map(|i| gg(i, 2)).collect::<Vec<_>>());
        let mut p2 = one_class((0..5).map(|i| gg(i, 2)).collect::<Vec<_>>());
        // Give type 2 in p1 a distinctive selector value.
        p1[0][2].selectors[0] = 999;
        p1[0][2].selectors[1] = 888;
        // Give type 2 in p2 a different value (so we can tell which parent it came from).
        p2[0][2].selectors[0] = 111;
        p2[0][2].selectors[1] = 222;
        // Segment [lo=2, hi=3) — child1 takes type_idx=2 from p1.
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

    #[test]
    fn cx_known() {
        // P1=[0,1,2,3,4], P2=[3,0,4,1,2]
        // Same cycles as flat version keyed on type_idx (which equals piece_idx here).
        // C1=[0,1,4,3,2], C2=[3,0,2,1,4]
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

    #[test]
    fn init_population_size_and_valid_permutations() {
        let spec = parse_problem("10x10R:0:3x2/3,4x3/2,5x1/4").unwrap();
        let problem = expand_problem(&spec);
        let n_types = spec.piespecs.len();
        let mut rng = Xoshiro256StarStar::seed_from_u64(99);
        let pop = init_population(&spec, &problem, 20, &small_config(), &mut rng);
        assert_eq!(pop.len(), 20);
        for ind in &pop {
            let total: usize = ind.genome.iter().map(|c| c.len()).sum();
            assert_eq!(total, n_types);
            assert_eq!(sorted_type_ids(&ind.genome), (0..n_types).collect::<Vec<_>>());
        }
    }

    #[test]
    fn init_population_is_deterministic() {
        let spec = parse_problem("10x10R:0:3x2/3,4x3/2").unwrap();
        let problem = expand_problem(&spec);
        let cfg = small_config();
        let pop1 = init_population(&spec, &problem, 5, &cfg, &mut Xoshiro256StarStar::seed_from_u64(7));
        let pop2 = init_population(&spec, &problem, 5, &cfg, &mut Xoshiro256StarStar::seed_from_u64(7));
        assert!(pop1.iter().zip(&pop2).all(|(a, b)| a.genome == b.genome));
    }

    #[test]
    fn run_ga_smoke() {
        let spec = parse_problem("10x10R:0:3x2/2,4x3/2").unwrap();
        let problem = expand_problem(&spec);
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let _best = run_ga(&spec, &problem, &small_config(), &mut rng);
    }

    #[test]
    fn run_ga_is_deterministic() {
        let spec = parse_problem("10x10R:0:3x2/2,4x3/2,2x2/3").unwrap();
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
    fn tournament_full_k_returns_best() {
        let ind = |type_idx: usize, obj: Objective| Individual {
            genome: one_class(vec![gg(type_idx, 1)]),
            objective: obj,
        };
        let pop = vec![ind(0, (0, 30, 0)), ind(1, (0, 10, 0)), ind(2, (0, 20, 0))];
        let mut rng = Xoshiro256StarStar::seed_from_u64(1);
        let winner = tournament_select(&pop, 3, &mut rng);
        assert_eq!((winner.objective.0, winner.objective.1), (0, 10));
    }

    #[test]
    fn elite_returns_best() {
        let ind = |type_idx: usize, obj: Objective| Individual {
            genome: one_class(vec![gg(type_idx, 1)]),
            objective: obj,
        };
        let pop = vec![ind(0, (0, 30, 0)), ind(1, (0, 10, 0)), ind(2, (0, 20, 0))];
        let elite = select_elite(&pop, 1);
        assert_eq!(elite.len(), 1);
        assert_eq!((elite[0].objective.0, elite[0].objective.1), (0, 10));
    }
}
