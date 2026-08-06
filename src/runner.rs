use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use futures_util::future::BoxFuture;

use crate::{
    expand::{expand_problem, shrink_solution},
    ga,
    ga::{Decodable, GaConfig},
    glas::ga as glas_ga,
    model::{Objective, ProblemSpec, SolutionSpec},
    slas::ga as slas_ga,
    transport::{ProgressMessage, ProgressSink},
};

/// Lazy genome decoder: calls `decode_spec` exactly once when `LazyDecode::decode()` is called.
/// `decode` call is expensive, and Progress events are throttled or superseded before
/// ever being sent, so deferring the decode avoids wasting it on generations nobody sees.
pub struct LazyDecode(Box<dyn FnOnce(&ProblemSpec) -> SolutionSpec + Send>);

impl LazyDecode {
    pub fn decode(self, spec: &ProblemSpec) -> SolutionSpec {
        self.0(spec)
    }
}

/// Accumulator for the best not-yet-sent progress event, used by drain.
struct PendingProgress {
    seed: u64,
    generation: usize,
    objective: Objective,
    lazy: LazyDecode,
}

/// Unified progress event, decoder-agnostic.
pub enum AnyEvent {
    Progress {
        seed: u64,
        generation: usize,
        objective: Objective,
        lazy: LazyDecode,
    },
    Done {
        // seed, objective, decode, genome_json5
        results: Vec<(u64, Objective, LazyDecode, Option<serde_json::Value>)>,
    },
}

/// Erases genome type `G` so `GaHandle<G>` can be wrapped in `AnyHandle::Ga(Box<dyn Eraser>)`.
pub trait Eraser: Send {
    fn recv(&mut self) -> BoxFuture<'_, Option<AnyEvent>>;
    fn join(&mut self);
}

impl<G> Eraser for ga::GaHandle<G>
where
    G: Decodable + Clone + Send + serde::Serialize + 'static,
{
    fn recv(&mut self) -> BoxFuture<'_, Option<AnyEvent>> {
        Box::pin(async move {
            match self.rx.recv().await {
                None => None,
                Some(ga::GaEvent::Progress(p)) => {
                    let ga::ProgressEvent { seed, generation, genome, objective } = p;
                    Some(AnyEvent::Progress {
                        seed,
                        generation,
                        objective,
                        lazy: LazyDecode(Box::new(move |spec| genome.decode(spec))),
                    })
                }
                Some(ga::GaEvent::Done(done)) => Some(AnyEvent::Done {
                    results: done
                        .pairs
                        .into_iter()
                        .map(|(seed, ind)| {
                            let genome_json = serde_json::to_value(&ind.genome).ok();
                            let genome = ind.genome;
                            (
                                seed,
                                ind.objective,
                                LazyDecode(Box::new(move |spec| genome.decode(spec))),
                                genome_json,
                            )
                        })
                        .collect(),
                }),
            }
        })
    }

    fn join(&mut self) {
        ga::GaHandle::join(self);
    }
}

/// Algorithm plus the parameters it needs to run.
pub enum AlgConfig {
    Ga {
        kind: GaKind,
        cfg: Arc<GaConfig>,
        seeds: Vec<u64>,
        progress_interval: usize,
    },
    Heuristic {
        kind: HeuristicKind,
    },
}

pub enum GaKind {
    Slas,
    Glas,
}

pub enum HeuristicKind {
    Bfdh,
    Jylanki,
}

/// Decoder-agnostic handle, mirroring `AlgConfig`'s variants.
pub enum AnyHandle {
    Ga(Box<dyn Eraser>),
    Heuristic(Option<AnyEvent>),
}

impl AnyHandle {
    pub fn join(&mut self) {
        if let AnyHandle::Ga(inner) = self {
            inner.join();
        }
    }

    pub async fn recv(&mut self) -> Option<AnyEvent> {
        match self {
            // Dyn dispatches to `GaHandle<G>`'s `Eraser::recv` impl
            AnyHandle::Ga(inner) => inner.recv().await,
            AnyHandle::Heuristic(slot) => slot.take(),
        }
    }

    pub async fn blocking_wait(mut self) -> Vec<(u64, Objective, LazyDecode, Option<serde_json::Value>)> {
        let results = loop {
            match self.recv().await {
                None => break Vec::new(),
                Some(AnyEvent::Progress { .. }) => continue,
                Some(AnyEvent::Done { results }) => break results,
            }
        };
        self.join();
        results
    }
}

/// Starts the algorithm. Non-blocking for GA; blocking for heuristics - this
/// assumes heuristics are fast; if not, introduce a different handle type for them.
pub fn run_algorithm(spec: Arc<ProblemSpec>, alg_cfg: &AlgConfig) -> AnyHandle {
    match alg_cfg {
        AlgConfig::Ga { kind, cfg, seeds, progress_interval } => {
            let inner: Box<dyn Eraser> = match kind {
                GaKind::Slas => Box::new(slas_ga::run_ga_mt(
                    Arc::clone(&spec),
                    Arc::clone(cfg),
                    seeds.clone(),
                    *progress_interval,
                    *progress_interval,
                )),
                GaKind::Glas => Box::new(glas_ga::run_ga_mt(
                    Arc::clone(&spec),
                    Arc::clone(cfg),
                    seeds.clone(),
                    *progress_interval,
                    *progress_interval,
                )),
            };
            AnyHandle::Ga(inner)
        }
        AlgConfig::Heuristic { kind } => {
            let problem = expand_problem(&spec);
            let flat_sol = match kind {
                HeuristicKind::Jylanki => crate::heuristic::jylanki::jylanki_solve(&problem),
                HeuristicKind::Bfdh => crate::heuristic::bfdh::bfdh_solve(&problem),
            };
            let objective = flat_sol.eval(&problem);
            let sol_spec = shrink_solution(&flat_sol, &spec);
            let done_event = Some(AnyEvent::Done {
                results: vec![(
                    0,
                    objective,
                    LazyDecode(Box::new(move |_spec: &ProblemSpec| sol_spec)),
                    None,
                )],
            });
            AnyHandle::Heuristic(done_event)
        }
    }
}

/// Streams `handle`'s events into `sink`, throttled by `sink_interval_ms`.
pub async fn drain(
    mut handle: AnyHandle,
    spec: Arc<ProblemSpec>,
    sink: &mut dyn ProgressSink,
    sink_interval_ms: u64,
) -> Result<(), std::io::Error> {
    let throttled = sink_interval_ms > 0;
    let throttle = Duration::from_millis(sink_interval_ms);
    let mut last_sent: Option<Instant> = None;
    let mut best_pending: Option<PendingProgress> = None;
    let t0 = Instant::now();

    loop {
        match handle.recv().await {
            None => break,
            Some(AnyEvent::Progress { seed, generation, objective, lazy }) => {
                if !throttled {
                    // Raw progress: no decode, no solution payload
                    drop(lazy);
                    let msg = ProgressMessage::Progress {
                        generation,
                        sheets_used: objective.sheets_used_int(),
                        secondary_objective: objective.secondary(),
                        seed,
                        solution: None,
                        pieces: None,
                    };
                    if sink.send(msg).is_err() {
                        break;
                    }
                } else {
                    let better = best_pending.as_ref().is_none_or(|b| objective < b.objective);
                    if better {
                        best_pending = Some(PendingProgress { seed, generation, objective, lazy });
                    }
                    // else: lazy (and the genome captured inside) is dropped here
                    let should_flush = last_sent.is_none_or(|t| t.elapsed() >= throttle);
                    if should_flush && let Some(pending) = best_pending.take() {
                        let sol = pending.lazy.decode(&spec);
                        let msg = ProgressMessage::Progress {
                            generation: pending.generation,
                            sheets_used: pending.objective.sheets_used_int(),
                            secondary_objective: pending.objective.secondary(),
                            seed: pending.seed,
                            solution: Some(sol),
                            pieces: Some(spec.piece_types.clone()),
                        };
                        if sink.send(msg).is_err() {
                            break;
                        }
                        last_sent = Some(Instant::now());
                    }
                }
            }
            Some(AnyEvent::Done { mut results }) => {
                // Flush any throttled pending event first
                if let Some(pending) = best_pending.take() {
                    let sol = pending.lazy.decode(&spec);
                    sink.send(ProgressMessage::Progress {
                        generation: pending.generation,
                        sheets_used: pending.objective.sheets_used_int(),
                        secondary_objective: pending.objective.secondary(),
                        seed: pending.seed,
                        solution: Some(sol),
                        pieces: Some(spec.piece_types.clone()),
                    })
                    .ok();
                }
                eprintln!("Done in {:.1}s", t0.elapsed().as_secs_f64());
                let (best_seed, best_obj, lazy, genome_json) = results.remove(0);
                let sol = lazy.decode(&spec);
                let cut_lengths = sol.cut_lengths(&spec);
                sink.send(ProgressMessage::Done {
                    seed: best_seed,
                    sheets_used: best_obj.sheets_used_int(),
                    cut_lengths,
                    solution: sol,
                    pieces: spec.piece_types.clone(),
                    genome: genome_json,
                })
                .ok();
                break;
            }
        }
    }
    handle.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use rand::{Rng, SeedableRng};
    use rand_xoshiro::Xoshiro256StarStar;

    use super::*;
    use crate::parser::compact::parse_problem;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn run_ga_produces_solution() {
        let spec = Arc::new(parse_problem("600x400R:0,0:200x200/4").unwrap());
        let cfg = Arc::new(GaConfig { pop_size: 20, iteration_count: 10, ..GaConfig::default() });
        let mut rng = Xoshiro256StarStar::seed_from_u64(1);
        let seeds: Vec<u64> = (0..2).map(|_| rng.next_u64()).collect();
        let alg_cfg = AlgConfig::Ga { kind: GaKind::Glas, cfg, seeds, progress_interval: 0 };

        let handle = run_algorithm(Arc::clone(&spec), &alg_cfg);
        let mut results = rt().block_on(handle.blocking_wait());
        assert!(!results.is_empty());

        let (_, _, lazy, _) = results.remove(0);
        let sol = lazy.decode(&spec);
        assert_eq!(sol.sheets_used(), 1);
    }

    #[test]
    fn run_heuristic_sends_done_only() {
        let spec = Arc::new(parse_problem("600x400R:0,0:200x200/4").unwrap());
        let alg_cfg = AlgConfig::Heuristic { kind: HeuristicKind::Bfdh };

        let mut handle = run_algorithm(Arc::clone(&spec), &alg_cfg);
        let first = rt().block_on(handle.recv());
        assert!(matches!(first, Some(AnyEvent::Done { .. })));
        let second = rt().block_on(handle.recv());
        assert!(second.is_none());
    }
}
