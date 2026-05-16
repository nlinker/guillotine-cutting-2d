use std::{
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use cutting::{
    decoder::decode_spec,
    ga::{GaConfig, GaEvent, ProgressEvent, run_ga_mt},
    glf::build_glf,
    model::{ProblemSpec, SolutionSpec},
    parse::parse_problem,
    parse_json::parse_problem_json,
    render::render_svg,
    transport::{ProgressMessage, ProgressSink},
};
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;

mod web;

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
        } => {
            let cfg = ga_config(gens, pop, elite, k);
            let spec = load_problem(compact.as_deref(), json.as_deref())?;
            let n_threads = resolve_threads(threads);
            if render {
                run_calc_render(&spec, &cfg, seed, n_threads, progress)?;
            } else {
                run_calc_with_sink(&spec, &cfg, seed, n_threads, progress, &sink, sink_interval)?;
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
            println!("{}", table.display_table(query_w));
            if let Some(h) = table.eval_full_set(query_w) {
                println!("\nMinimum height for width={}: {}", spec.sheet.width, h - spec.kerf);
            } else {
                println!("\nPieces do not fit in width={}", spec.sheet.width);
            }
        }
    }
    Ok(())
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

fn run_calc_with_sink(
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

    let total: u32 = spec.piespecs.iter().map(|p| p.count).sum();
    eprintln!(
        "Pieces  : {} ({} types)   Sheet: {}×{}",
        total,
        spec.piespecs.len(),
        spec.sheet.width,
        spec.sheet.height
    );
    eprintln!("GA cfg  : {cfg}");
    eprintln!("Sink    : {sink_mode}  interval={sink_interval_ms}ms");

    let spec = Arc::new(spec.clone());
    let cfg = Arc::new(cfg.clone());
    match sink_mode {
        "stdout" => {
            let mut sink = cutting::transport::stdout::StdoutSink;
            run_with_sink(
                Arc::clone(&spec),
                Arc::clone(&cfg),
                &seeds,
                progress_interval,
                &mut sink,
                sink_interval_ms,
            )
        }
        _ => {
            #[cfg(windows)]
            {
                eprintln!("Waiting for client on {PIPE_NAME} …");
                let mut sink = cutting::transport::windows::WindowsPipeSink::create_and_wait(PIPE_NAME)?;
                run_with_sink(
                    Arc::clone(&spec),
                    Arc::clone(&cfg),
                    &seeds,
                    progress_interval,
                    &mut sink,
                    sink_interval_ms,
                )
            }
            #[cfg(unix)]
            {
                eprintln!("Waiting for reader on {FIFO_PATH} …");
                let mut sink = cutting::transport::unix::FifoSink::new(FIFO_PATH)?;
                run_with_sink(
                    Arc::clone(&spec),
                    Arc::clone(&cfg),
                    &seeds,
                    progress_interval,
                    &mut sink,
                    sink_interval_ms,
                )
            }
            #[cfg(not(any(windows, unix)))]
            {
                eprintln!("Named pipe not supported on this platform, falling back to stdout");
                let mut sink = cutting::transport::stdout::StdoutSink;
                run_with_sink(
                    Arc::clone(&spec),
                    Arc::clone(&cfg),
                    &seeds,
                    progress_interval,
                    &mut sink,
                    sink_interval_ms,
                )
            }
        }
    }
}

pub(crate) fn run_with_sink(
    spec: Arc<ProblemSpec>,
    cfg: Arc<GaConfig>,
    seeds: &[u64],
    progress_interval: usize,
    sink: &mut dyn ProgressSink,
    sink_interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let mut handle = run_ga_mt(Arc::clone(&spec), Arc::clone(&cfg), seeds.to_vec(), progress_interval);

    let throttled = sink_interval_ms > 0;
    let throttle = Duration::from_millis(sink_interval_ms);
    let mut last_sent: Option<Instant> = None;
    let mut best_pending: Option<ProgressEvent> = None;

    let t0 = Instant::now();
    loop {
        match handle.rx.blocking_recv() {
            None => break,
            Some(GaEvent::Progress(p)) => {
                if !throttled {
                    let msg = ProgressMessage::Progress {
                        generation: p.generation,
                        sheets_used: p.objective.0,
                        last_sheet_area: p.objective.1,
                        seed: p.seed,
                        solution: None,
                        pieces: None,
                    };
                    if sink.send(&msg).is_err() {
                        handle.stop();
                        break;
                    }
                } else {
                    let better = best_pending.as_ref().is_none_or(|b| p.objective < b.objective);
                    if better {
                        best_pending = Some(p);
                    }
                    let should_flush = last_sent.is_none_or(|t| t.elapsed() >= throttle);
                    if should_flush && let Some(evt) = best_pending.take() {
                        let sol = decode_spec(&spec, &evt.genome);
                        let msg = ProgressMessage::Progress {
                            generation: evt.generation,
                            sheets_used: evt.objective.0,
                            last_sheet_area: evt.objective.1,
                            seed: evt.seed,
                            solution: Some(sol),
                            pieces: Some(spec.piespecs.clone()),
                        };
                        if sink.send(&msg).is_err() {
                            handle.stop();
                            break;
                        }
                        last_sent = Some(Instant::now());
                    }
                }
            }
            Some(GaEvent::Done(results)) => {
                if let Some(evt) = best_pending.take() {
                    let sol = decode_spec(&spec, &evt.genome);
                    sink.send(&ProgressMessage::Progress {
                        generation: evt.generation,
                        sheets_used: evt.objective.0,
                        last_sheet_area: evt.objective.1,
                        seed: evt.seed,
                        solution: Some(sol),
                        pieces: Some(spec.piespecs.clone()),
                    })
                    .ok();
                }
                eprintln!("Done in {:.1}s", t0.elapsed().as_secs_f64());
                let (best_seed, best) = &results[0];
                let sol = decode_spec(&spec, &best.genome);
                sink.send(&ProgressMessage::Done {
                    seed: *best_seed,
                    sheets_used: best.objective.0,
                    last_sheet_area: best.objective.1,
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

fn run_calc_render(
    spec: &ProblemSpec,
    cfg: &GaConfig,
    base_seed: u64,
    n_threads: usize,
    progress_interval: usize,
) -> Result<(), Box<dyn Error>> {
    let mut rng = Xoshiro256StarStar::seed_from_u64(base_seed);
    let seeds = (0..n_threads).map(|_| rng.next_u64()).collect::<Vec<_>>();
    let spec = Arc::new(spec.clone());
    let cfg = Arc::new(cfg.clone());
    let t0 = Instant::now();
    let mut handle = run_ga_mt(Arc::clone(&spec), Arc::clone(&cfg), seeds, progress_interval);
    loop {
        match handle.rx.blocking_recv() {
            None => break,
            Some(GaEvent::Progress(_)) => {}
            Some(GaEvent::Done(results)) => {
                eprintln!("Done in {:.1}s", t0.elapsed().as_secs_f64());
                let (_, best) = &results[0];
                let sol = decode_spec(&spec, &best.genome);
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

pub(crate) fn ga_config(gens: usize, pop: usize, elite: usize, k: usize) -> GaConfig {
    GaConfig {
        pop_size: pop,
        n_generations: gens,
        n_elite: elite,
        tournament_k: k,
        crossover_p: 0.80,
        swap_p: 0.15,
        flip_p: 0.05,
        point_p: 0.10,
        point_delta: (1, 3),
    }
}
