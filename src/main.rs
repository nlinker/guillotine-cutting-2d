use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use cutting::{
    ga,
    ga::GaConfig,
    glas::ga as glas_ga,
    glf::build_glf,
    model::{Objective, ProblemSpec, SolutionSpec},
    parse::parse_problem,
    parse_json::parse_problem_json,
    render::render_svg,
    slas::ga as slas_ga,
    transport::{ProgressMessage, ProgressSink},
};
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;
use tokio::sync::mpsc;

mod web;

/// Solver algorithm.
#[derive(clap::ValueEnum, serde::Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Algorithm {
    /// Group-SLAS genetic algorithm (one gene per piece type)
    #[default]
    Glas,
    /// Flat-SLAS genetic algorithm (one gene per physical piece)
    Slas,
}

impl std::fmt::Display for Algorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Algorithm::Glas => write!(f, "glas"),
            Algorithm::Slas => write!(f, "slas"),
        }
    }
}

#[cfg(windows)]
const PIPE_NAME: &str = r"\\.\pipe\cut_progress";
#[cfg(unix)]
const FIFO_PATH: &str = "/tmp/cut_progress";

#[derive(Parser)]
#[command(name = "cutting", about = "2D guillotine cutting optimizer")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the GA on a problem and print ranked results
    Calc {
        /// Compact problem string. Mutually exclusive with --json.
        #[arg(long)]
        compact: Option<String>,
        /// Path to a JSON problem file. Mutually exclusive with --compact.
        #[arg(long)]
        json: Option<String>,
        /// Base random seed
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Number of parallel threads (0 = auto-detect)
        #[arg(long, default_value_t = 0)]
        threads: usize,
        /// Generations per run
        #[arg(long, default_value_t = 2000)]
        gens: usize,
        /// Population size
        #[arg(long, default_value_t = 200)]
        pop: usize,
        /// Elite count
        #[arg(long, default_value_t = 5)]
        elite: usize,
        /// Tournament size
        #[arg(long, default_value_t = 5)]
        k: usize,
        /// Report global best every N generations; 0 = silent
        #[arg(long, default_value_t = 100)]
        progress: usize,
        /// Progress sink: "pipe" (default) or "stdout"
        #[arg(long, default_value = "pipe")]
        sink: String,
        /// Throttle sink: send at most one progress per N ms; 0 = no throttle
        #[arg(long, default_value_t = 1000)]
        sink_interval: u64,
        /// Render the best solution as SVG to stdout instead of JSON
        #[arg(long, default_value_t = false)]
        render: bool,
        /// Solver algorithm
        #[arg(long, default_value = "glas")]
        algorithm: Algorithm,
        /// Min side length (px) for a piece to be "long"; 0 = auto (sheet_max * 0.3)
        #[arg(long, default_value_t = 0)]
        long_dim_threshold: u32,
        /// Sqrt of min area (px) for a long piece to be "large"; 0 = auto (sqrt(sheet_area * 0.05))
        #[arg(long, default_value_t = 0)]
        large_area_threshold: u32,
    },
    /// Start a web server with an interactive UI
    Serve {
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
    /// Render a solution as SVG to stdout
    Render {
        /// Compact problem string. Mutually exclusive with --json.
        #[arg(long)]
        compact: Option<String>,
        /// Path to JSON problem file. Mutually exclusive with --compact.
        #[arg(long)]
        json: Option<String>,
        /// Path to solution JSON file (the `solution` field from a `done` event, or the object itself)
        #[arg(long)]
        solution: String,
    },
    /// Build the GLF (Guillotine Layout Function) table and print it
    Glf {
        /// Compact problem string, e.g. "10x8F:0:2x3/4,4x3,8x3,5x2/2"
        problem: String,
    },
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Calc {
            compact,
            json,
            seed,
            threads,
            gens,
            pop,
            elite,
            k,
            progress,
            sink,
            sink_interval,
            render,
            algorithm,
            long_dim_threshold,
            large_area_threshold,
        } => {
            let spec = load_problem(compact.as_deref(), json.as_deref())?;
            let cfg = ga_config(gens, pop, elite, k, &spec, large_area_threshold, long_dim_threshold);
            let n_threads = resolve_threads(threads);
            if render {
                run_calc_render(&spec, &cfg, seed, n_threads, progress, algorithm)?;
            } else {
                run_calc_with_sink(&spec, &cfg, seed, n_threads, progress, &sink, sink_interval, algorithm)?;
            }
        }
        Command::Serve { port } => web::run_serve(port)?,
        Command::Render {
            compact,
            json,
            solution,
        } => {
            let spec = load_problem(compact.as_deref(), json.as_deref())?;
            let sol_str = std::fs::read_to_string(&solution)?;
            let sol = parse_solution_json(&sol_str)?;
            print!("{}", render_svg(&spec, &sol)?);
        }
        Command::Glf { problem } => {
            let spec = parse_problem(&problem)?;
            let table = build_glf(&spec);
            let query_w = spec.sheet.width + spec.kerf;
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

/// Lazy genome decoder: calls `decode_spec` exactly once when `.decode()` is called.
struct LazyDecode(Box<dyn FnOnce(&ProblemSpec) -> SolutionSpec + Send>);

impl LazyDecode {
    fn decode(self, spec: &ProblemSpec) -> SolutionSpec {
        (self.0)(spec)
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
        results: Vec<(u64, Objective, LazyDecode)>,
    },
}

/// Decoder-agnostic handle. Dropping it signals the bridge thread (via the rx
/// side of the channel), which in turn drops the inner GA handle, stopping the GA.
struct AnyHandle {
    rx: mpsc::UnboundedReceiver<AnyEvent>,
}

/// Converts a `GaHandle<G>` into an `AnyHandle`. A bridge thread forwards events,
/// wrapping each genome in a lazy closure produced by `decode`.
fn ga_handle_to_any<G, F>(mut handle: ga::GaHandle<G>, decode: F) -> AnyHandle
where
    G: Clone + Send + 'static,
    F: Fn(&G, &ProblemSpec) -> SolutionSpec + Send + Clone + 'static,
{
    let (tx, rx) = mpsc::unbounded_channel::<AnyEvent>();
    std::thread::spawn(move || {
        while let Some(evt) = handle.rx.blocking_recv() {
            let any = match evt {
                ga::GaEvent::Progress(p) => {
                    let (genome, f) = (p.genome, decode.clone());
                    AnyEvent::Progress {
                        seed: p.seed,
                        generation: p.generation,
                        objective: p.objective,
                        lazy: LazyDecode(Box::new(move |spec| f(&genome, spec))),
                    }
                }
                ga::GaEvent::Done(results) => AnyEvent::Done {
                    results: results
                        .into_iter()
                        .map(|(seed, ind)| {
                            let (genome, f) = (ind.genome, decode.clone());
                            (seed, ind.objective, LazyDecode(Box::new(move |spec| f(&genome, spec))))
                        })
                        .collect(),
                },
            };
            if tx.send(any).is_err() {
                break;
            }
        }
        // `handle` dropped here -> handle.stop() -> GA threads halt
    });
    AnyHandle { rx }
}

fn make_any_handle(
    spec: Arc<ProblemSpec>,
    cfg: Arc<GaConfig>,
    seeds: Vec<u64>,
    progress_interval: usize,
    algorithm: Algorithm,
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
            ga_handle_to_any(handle, |g, spec| cutting::slas::decoder::decode_spec(spec, g))
        }
        Algorithm::Glas => {
            let handle = glas_ga::run_ga_mt(
                Arc::clone(&spec),
                Arc::clone(&cfg),
                seeds,
                progress_interval,
                progress_interval,
            );
            ga_handle_to_any(handle, |g, spec| cutting::glas::decoder::decode_spec(spec, g))
        }
    }
}

fn parse_solution_json(s: &str) -> Result<SolutionSpec, Box<dyn Error>> {
    let v: serde_json::Value = serde_json::from_str(s)?;
    let sol_val = if v.get("solution").is_some() {
        &v["solution"]
    } else {
        &v
    };
    Ok(serde_json::from_value(sol_val.clone())?)
}

fn load_problem(compact: Option<&str>, json: Option<&str>) -> Result<ProblemSpec, Box<dyn Error>> {
    match (compact, json) {
        (Some(_), Some(_)) => Err("--compact and --json are mutually exclusive".into()),
        (None, None) => Err("provide exactly one of --compact <string> or --json <path>".into()),
        (Some(s), None) => Ok(parse_problem(s)?),
        (None, Some(path)) => {
            let s = std::fs::read_to_string(path)?;
            Ok(parse_problem_json(&s)?)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_calc_with_sink(
    spec: &ProblemSpec,
    cfg: &GaConfig,
    base_seed: u64,
    n_threads: usize,
    progress_interval: usize,
    sink_mode: &str,
    sink_interval_ms: u64,
    algorithm: Algorithm,
) -> Result<(), Box<dyn Error>> {
    let mut rng = Xoshiro256StarStar::seed_from_u64(base_seed);
    let seeds = (0..n_threads).map(|_| rng.next_u64()).collect::<Vec<_>>();

    let total: u32 = spec.piespecs.iter().map(|p| p.count).sum();
    eprintln!(
        "Pieces  : {} ({} types)   Sheet: {}×{}   Algorithm: {}",
        total,
        spec.piespecs.len(),
        spec.sheet.width,
        spec.sheet.height,
        algorithm,
    );
    eprintln!("GA cfg  : {cfg}");
    eprintln!("Sink    : {sink_mode}  interval={sink_interval_ms}ms");

    let spec = Arc::new(spec.clone());
    let cfg = Arc::new(cfg.clone());
    let any_handle = make_any_handle(Arc::clone(&spec), Arc::clone(&cfg), seeds, progress_interval, algorithm);

    match sink_mode {
        "stdout" => {
            let mut sink = cutting::transport::stdout::StdoutSink;
            run_with_any_handle(any_handle, spec, &mut sink, sink_interval_ms)
        }
        _ => {
            #[cfg(windows)]
            {
                eprintln!("Waiting for client on {PIPE_NAME} …");
                let mut sink = cutting::transport::windows::WindowsPipeSink::create_and_wait(PIPE_NAME)?;
                run_with_any_handle(any_handle, spec, &mut sink, sink_interval_ms)
            }
            #[cfg(unix)]
            {
                eprintln!("Waiting for reader on {FIFO_PATH} …");
                let mut sink = cutting::transport::unix::FifoSink::new(FIFO_PATH)?;
                run_with_any_handle(any_handle, spec, &mut sink, sink_interval_ms)
            }
            #[cfg(not(any(windows, unix)))]
            {
                eprintln!("Named pipe not supported on this platform, falling back to stdout");
                let mut sink = cutting::transport::stdout::StdoutSink;
                run_with_any_handle(any_handle, spec, &mut sink, sink_interval_ms)
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
        match handle.rx.blocking_recv() {
            None => break,
            Some(AnyEvent::Progress {
                seed,
                generation,
                objective,
                lazy,
            }) => {
                if !throttled {
                    // Raw progress: no decode, no solution payload
                    drop(lazy);
                    let msg = ProgressMessage::Progress {
                        generation,
                        sheets_used: objective.sheets_used,
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
                        best_pending = Some(PendingProgress {
                            seed,
                            generation,
                            objective,
                            lazy,
                        });
                    }
                    // else: lazy (and the genome captured inside) is dropped here
                    let should_flush = last_sent.is_none_or(|t| t.elapsed() >= throttle);
                    if should_flush && let Some(pending) = best_pending.take() {
                        let sol = pending.lazy.decode(&spec);
                        let msg = ProgressMessage::Progress {
                            generation: pending.generation,
                            sheets_used: pending.objective.sheets_used,
                            secondary_objective: pending.objective.secondary(),
                            seed: pending.seed,
                            solution: Some(sol),
                            pieces: Some(spec.piespecs.clone()),
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
                        sheets_used: pending.objective.sheets_used,
                        secondary_objective: pending.objective.secondary(),
                        seed: pending.seed,
                        solution: Some(sol),
                        pieces: Some(spec.piespecs.clone()),
                    })
                    .ok();
                }
                eprintln!("Done in {:.1}s", t0.elapsed().as_secs_f64());
                let (best_seed, best_obj, lazy) = results.remove(0);
                let sol = lazy.decode(&spec);
                let cut_lengths = sol.cut_lengths(&spec);
                sink.send(&ProgressMessage::Done {
                    seed: best_seed,
                    sheets_used: best_obj.sheets_used,
                    cut_lengths,
                    solution: sol,
                    pieces: spec.piespecs.clone(),
                })
                .ok();
                break;
            }
        }
    }
    Ok(())
}

pub(crate) fn run_with_sink_any(
    spec: Arc<ProblemSpec>,
    cfg: Arc<GaConfig>,
    seeds: &[u64],
    progress_interval: usize,
    algorithm: Algorithm,
    sink: &mut dyn ProgressSink,
    sink_interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let any = make_any_handle(
        Arc::clone(&spec),
        Arc::clone(&cfg),
        seeds.to_vec(),
        progress_interval,
        algorithm,
    );
    run_with_any_handle(any, spec, sink, sink_interval_ms)
}

fn run_calc_render(
    spec: &ProblemSpec,
    cfg: &GaConfig,
    base_seed: u64,
    n_threads: usize,
    progress_interval: usize,
    algorithm: Algorithm,
) -> Result<(), Box<dyn Error>> {
    let mut rng = Xoshiro256StarStar::seed_from_u64(base_seed);
    let seeds = (0..n_threads).map(|_| rng.next_u64()).collect::<Vec<_>>();
    let spec = Arc::new(spec.clone());
    let cfg = Arc::new(cfg.clone());
    let t0 = Instant::now();
    let mut handle = make_any_handle(Arc::clone(&spec), Arc::clone(&cfg), seeds, progress_interval, algorithm);
    loop {
        match handle.rx.blocking_recv() {
            None => break,
            Some(AnyEvent::Progress { .. }) => {}
            Some(AnyEvent::Done { mut results }) => {
                eprintln!("Done in {:.1}s", t0.elapsed().as_secs_f64());
                let (_, _, lazy) = results.remove(0);
                let sol = lazy.decode(&spec);
                print!("{}", render_svg(&spec, &sol)?);
                break;
            }
        }
    }
    Ok(())
}

fn resolve_threads(n: usize) -> usize {
    if n == 0 {
        std::thread::available_parallelism().map_or(8, |p| p.get())
    } else {
        n
    }
}

pub(crate) fn ga_config(
    gens: usize,
    pop: usize,
    elite: usize,
    k: usize,
    spec: &ProblemSpec,
    large_area_threshold: u32,
    long_dim_threshold: u32,
) -> GaConfig {
    let sh = spec.sheet;
    GaConfig {
        pop_size: pop,
        n_generations: gens,
        n_elite: elite,
        tournament_k: k,
        long_dim_threshold: if long_dim_threshold == 0 {
            (sh.width.max(sh.height) as f64 * 0.3) as u32
        } else {
            long_dim_threshold
        },
        large_area_threshold: if large_area_threshold == 0 {
            ((sh.width as f64 * sh.height as f64 * 0.05).sqrt()) as u32
        } else {
            large_area_threshold
        },
        ..GaConfig::default()
    }
}
