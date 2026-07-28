use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::Parser;
use cut::{
    exact::glf::build_glf,
    expand::{expand_problem, shrink_solution},
    ga,
    ga::GaConfig,
    glas::ga as glas_ga,
    model::{Objective, ProblemSpec, SolutionSpec},
    parser,
    render::render_svg,
    slas::ga as slas_ga,
    transport::{CapturingSink, ProgressMessage, ProgressSink},
};
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;

use crate::cli::{Algorithm, Cli, Command};

mod cli;
mod web;

#[cfg(windows)]
const PIPE_NAME: &str = r"\\.\pipe\cut_progress";
#[cfg(unix)]
const FIFO_PATH: &str = "/tmp/cut_progress";

/// Lazy genome decoder: calls `decode_spec` exactly once when `.decode()` is called.
/// Most progress events are throttled or superseded before ever being sent, so
/// deferring the decode avoids wasting it on generations nobody sees.
struct LazyDecode(Box<dyn FnOnce(&ProblemSpec) -> SolutionSpec + Send>);

impl LazyDecode {
    fn decode(self, spec: &ProblemSpec) -> SolutionSpec {
        self.0(spec)
    }
}

/// Unified progress event, decoder-agnostic.
struct PendingProgress {
    seed: u64,
    generation: usize,
    objective: Objective,
    lazy: LazyDecode,
}

enum AnyEvent {
    Progress {
        seed: u64,
        generation: usize,
        objective: Objective,
        lazy: LazyDecode,
    },
    Done {
        results: Vec<(u64, Objective, LazyDecode, Option<serde_json::Value>)>,
    },
}

/// Type-erases `ga::GaHandle<G>` behind dynamic dispatch, so callers that don't
/// care about the genome type can hold a single concrete `AnyHandle`.
trait Eraser: Send {
    fn recv(&mut self) -> Option<AnyEvent>;
    fn join(&mut self);
}

struct TypedGaHandle<G: Clone + Send + 'static, F> {
    handle: ga::GaHandle<G>,
    decode: F,
}

impl<G, F> Eraser for TypedGaHandle<G, F>
where
    G: Clone + Send + serde::Serialize + 'static,
    F: Fn(&G, &ProblemSpec) -> SolutionSpec + Send + Clone + 'static,
{
    fn recv(&mut self) -> Option<AnyEvent> {
        match self.handle.rx.blocking_recv() {
            None => None,
            Some(ga::GaEvent::Progress(p)) => {
                let (genome, f) = (p.genome, self.decode.clone());
                Some(AnyEvent::Progress {
                    seed: p.seed,
                    generation: p.generation,
                    objective: p.objective,
                    lazy: LazyDecode(Box::new(move |spec| f(&genome, spec))),
                })
            }
            Some(ga::GaEvent::Done(results)) => Some(AnyEvent::Done {
                results: results
                    .into_iter()
                    .map(|(seed, ind)| {
                        let (genome, f) = (ind.genome, self.decode.clone());
                        let genome_json = serde_json::to_value(&genome).ok();
                        (
                            seed,
                            ind.objective,
                            LazyDecode(Box::new(move |spec| f(&genome, spec))),
                            genome_json,
                        )
                    })
                    .collect(),
            }),
        }
    }

    fn join(&mut self) {
        self.handle.join();
    }
}

/// Decoder-agnostic handle. Dropping it drops the inner `GaHandle`, which
/// signals it to stop (see `GaHandle`'s `Drop` impl).
struct AnyHandle {
    inner: Box<dyn Eraser>,
}

impl AnyHandle {
    fn recv(&mut self) -> Option<AnyEvent> {
        self.inner.recv()
    }

    fn join(&mut self) {
        self.inner.join();
    }
}

#[rustfmt::skip]
fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Calc {
            compact, json, seed, threads, gens, pop, elite, k, progress, sink,
            sink_interval,render, algorithm, long_dim_threshold, large_area_threshold
        } => {
            let spec = load_problem(compact.as_deref(), json.as_deref())?;
            let cfg = GaConfig::new(&spec, gens, pop, elite, k, large_area_threshold, long_dim_threshold);
            let n_threads = resolve_threads(threads);
            if render {
                run_calc_render(algorithm, &spec, &cfg, seed, n_threads, progress)?;
            } else {
                run_calc_with_sink(algorithm, &spec, &cfg, seed, n_threads, progress, &sink, sink_interval)?;
            }
        }
        Command::Serve { port } => {
            web::run_serve(port)?;
        }
        Command::Render { compact, json, solution } => {
            let spec = load_problem(compact.as_deref(), json.as_deref())?;
            let sol_str = std::fs::read_to_string(&solution)?;
            let sol = parser::json::parse_solution_json(&sol_str)?;
            print!("{}", render_svg(&spec, &sol)?);
        }
        Command::Glf { problem } => {
            let spec = parser::compact::parse_problem(&problem)?;
            let expanded = expand_problem(&spec);
            let table = build_glf(&expanded);
            let query_w = expanded.sheet.width;
            println!("{}", table.render(query_w));
            if let Some(h) = table.eval_full_set(query_w) {
                println!("\nMinimum height for width={}: {}", spec.sheet.width, h - spec.kerf);
            } else {
                println!("\nPieces do not fit in width={}", spec.sheet.width);
            }
        }
    }
    Ok(())
}

/// Converts a `GaHandle<G>` into an `AnyHandle`, wrapping each genome in a
/// lazy closure produced by `decode`.
fn ga_handle_to_any<G, F>(handle: ga::GaHandle<G>, decode: F) -> AnyHandle
where
    G: Clone + Send + serde::Serialize + 'static,
    F: Fn(&G, &ProblemSpec) -> SolutionSpec + Send + Clone + 'static,
{
    AnyHandle { inner: Box::new(TypedGaHandle { handle, decode }) }
}

fn make_any_handle(
    algorithm: Algorithm,
    spec: Arc<ProblemSpec>,
    cfg: Arc<GaConfig>,
    seeds: Vec<u64>,
    progress_interval: usize,
) -> AnyHandle {
    match algorithm {
        Algorithm::Slas => {
            let handle = slas_ga::run_ga_mt(
                Arc::clone(&spec),
                Arc::clone(&cfg),
                seeds,
                progress_interval,
                progress_interval,
            );
            ga_handle_to_any(handle, |g, spec| cut::slas::decoder::decode_spec(spec, g))
        }
        Algorithm::Glas => {
            let handle = glas_ga::run_ga_mt(
                Arc::clone(&spec),
                Arc::clone(&cfg),
                seeds,
                progress_interval,
                progress_interval,
            );
            ga_handle_to_any(handle, |g, spec| cut::glas::decoder::decode_spec(spec, g))
        }
        Algorithm::Bfdh => unreachable!("Bfdh is handled before make_any_handle"),
        Algorithm::Jylanki => unreachable!("Jylanki is handled before make_any_handle"),
    }
}

fn load_problem(compact: Option<&str>, json: Option<&str>) -> Result<ProblemSpec, Box<dyn Error>> {
    match (compact, json) {
        (Some(_), Some(_)) => Err("--compact and --json are mutually exclusive".into()),
        (None, None) => Err("provide exactly one of --compact <string> or --json <path>".into()),
        (Some(s), None) => Ok(parser::compact::parse_problem(s)?),
        (None, Some(path)) => {
            let s = std::fs::read_to_string(path)?;
            Ok(parser::json::parse_problem(&s)?)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_calc_with_sink(
    algorithm: Algorithm,
    spec: &ProblemSpec,
    cfg: &GaConfig,
    base_seed: u64,
    n_threads: usize,
    progress_interval: usize,
    sink_mode: &str,
    sink_interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let mut rng = Xoshiro256StarStar::seed_from_u64(base_seed);
    let seeds = (0..n_threads).map(|_| rng.next_u64()).collect::<Vec<_>>();

    let total: u32 = spec.piece_types.iter().map(|p| p.count).sum();
    eprintln!(
        "Pieces  : {} ({} types)   Sheet: {}×{}   Algorithm: {}",
        total,
        spec.piece_types.len(),
        spec.sheet.width,
        spec.sheet.height,
        algorithm,
    );
    eprintln!("GA cfg  : {cfg}");
    eprintln!("Sink    : {sink_mode}  interval={sink_interval_ms}ms");

    let spec = Arc::new(spec.clone());
    let cfg = Arc::new(cfg.clone());

    match sink_mode {
        "stdout" => {
            let mut sink = cut::transport::stdout::StdoutSink;
            run_with_sink_any(
                algorithm,
                spec,
                cfg,
                &seeds,
                progress_interval,
                &mut sink,
                sink_interval_ms,
            )
        }
        _ => {
            #[cfg(windows)]
            {
                eprintln!("Waiting for client on {PIPE_NAME} ...");
                let mut sink = cut::transport::windows::WindowsPipeSink::create_and_wait(PIPE_NAME)?;
                run_with_sink_any(
                    algorithm,
                    spec,
                    cfg,
                    &seeds,
                    progress_interval,
                    &mut sink,
                    sink_interval_ms,
                )
            }
            #[cfg(unix)]
            {
                eprintln!("Waiting for reader on {FIFO_PATH} ...");
                let mut sink = cut::transport::unix::FifoSink::new(FIFO_PATH)?;
                run_with_sink_any(
                    algorithm,
                    spec,
                    cfg,
                    &seeds,
                    progress_interval,
                    &mut sink,
                    sink_interval_ms,
                )
            }
            #[cfg(not(any(windows, unix)))]
            {
                eprintln!("Named pipe not supported on this platform, falling back to stdout");
                let mut sink = cut::transport::stdout::StdoutSink;
                run_with_sink_any(
                    algorithm,
                    spec,
                    cfg,
                    &seeds,
                    progress_interval,
                    &mut sink,
                    sink_interval_ms,
                )
            }
        }
    }
}

/// Decoder-agnostic event loop over an `AnyHandle`. Called by both the CLI and (via
/// `run_with_sink`) the web server. Handles throttling and forwarding to `sink`.
fn run_with_any_handle(
    mut handle: AnyHandle,
    spec: Arc<ProblemSpec>,
    sink: &mut dyn ProgressSink,
    sink_interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let throttled = sink_interval_ms > 0;
    let throttle = Duration::from_millis(sink_interval_ms);
    let mut last_sent: Option<Instant> = None;
    let mut best_pending: Option<PendingProgress> = None;
    let t0 = Instant::now();

    loop {
        match handle.recv() {
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
                    if sink.send(&msg).is_err() {
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
                        if sink.send(&msg).is_err() {
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
                    sink.send(&ProgressMessage::Progress {
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
                sink.send(&ProgressMessage::Done {
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

pub(crate) fn run_with_sink_any(
    algorithm: Algorithm,
    spec: Arc<ProblemSpec>,
    cfg: Arc<GaConfig>,
    seeds: &[u64],
    progress_interval: usize,
    sink: &mut dyn ProgressSink,
    sink_interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    if matches!(algorithm, Algorithm::Bfdh | Algorithm::Jylanki) {
        let problem = expand_problem(&spec);
        let flat_sol = match algorithm {
            Algorithm::Jylanki => cut::heuristic::jylanki_solve(&problem),
            _ => cut::heuristic::bfdh_solve(&problem),
        };
        let objective = flat_sol.eval(&problem);
        let sol_spec = shrink_solution(&flat_sol, &spec);
        let cut_lengths = sol_spec.cut_lengths(&spec);
        sink.send(&ProgressMessage::Done {
            seed: 0,
            sheets_used: objective.sheets_used_int(),
            cut_lengths,
            solution: sol_spec,
            pieces: spec.piece_types.clone(),
            genome: None,
        })?;
        return Ok(());
    }
    let any = make_any_handle(
        algorithm,
        Arc::clone(&spec),
        Arc::clone(&cfg),
        seeds.to_vec(),
        progress_interval,
    );
    run_with_any_handle(any, spec, sink, sink_interval_ms)
}

fn run_calc_render(
    algorithm: Algorithm,
    spec: &ProblemSpec,
    cfg: &GaConfig,
    base_seed: u64,
    n_threads: usize,
    progress_interval: usize,
) -> Result<(), Box<dyn Error>> {
    let mut rng = Xoshiro256StarStar::seed_from_u64(base_seed);
    let seeds = (0..n_threads).map(|_| rng.next_u64()).collect::<Vec<_>>();
    let spec_arc = Arc::new(spec.clone());
    let cfg_arc = Arc::new(cfg.clone());
    let mut sink = CapturingSink::default();
    run_with_sink_any(algorithm, spec_arc, cfg_arc, &seeds, progress_interval, &mut sink, 0)?;
    let sol = sink
        .solution
        .expect("run_with_sink_any always sends Done before returning");
    print!("{}", render_svg(spec, &sol)?);
    Ok(())
}

fn resolve_threads(n: usize) -> usize {
    if n == 0 {
        std::thread::available_parallelism().map_or(8, |p| p.get())
    } else {
        n
    }
}
