use std::{
    collections::BTreeMap,
    error::Error,
    sync::Arc,
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use cutting::{
    decoder::decode,
    ga::{GaConfig, GaEvent, ProgressEvent, ga_channel, run_ga_mt},
    model::{Placement, Problem, Solution},
    parse::parse_problem,
    parse_json::parse_problem_json,
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
        /// Problem string e.g. "2600x1800F:3:400x400/6,495x495/6" (mutually exclusive with --json)
        problem: Option<String>,
        /// Path to a JSON problem file (mutually exclusive with positional problem string)
        #[arg(long)]
        json: Option<String>,
        /// Base random seed; different values produce different layouts
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Number of parallel threads (independent GA runs)
        #[arg(long, default_value_t = 8)]
        threads: usize,
        /// Generations per run
        #[arg(long, default_value_t = 2000)]
        gens: usize,
        /// Population size
        #[arg(long, default_value_t = 200)]
        pop: usize,
        /// Elite count (top individuals carried unchanged to next generation)
        #[arg(long, default_value_t = 5)]
        elite: usize,
        /// Tournament size
        #[arg(long, default_value_t = 5)]
        k: usize,
        /// Report global best every N generations; 0 = silent
        #[arg(long, default_value_t = 100)]
        progress: usize,
        /// Progress sink: "pipe" (default, named pipe / FIFO) or "stdout"
        #[arg(long, default_value = "pipe")]
        sink: String,
        /// Throttle sink: send at most one progress per N ms, includes solution; 0 = no throttle
        #[arg(long, default_value_t = 1000)]
        sink_interval: u64,
    },
    /// Start a web server with an interactive UI
    Serve {
        /// Port to listen on
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Calc {
            problem,
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
        } => {
            let cfg = ga_config(gens, pop, elite, k);
            let parsed = load_problem(problem.as_deref(), json.as_deref())?;
            run_calc_with_sink(&parsed, &cfg, seed, threads, progress, &sink, sink_interval)?;
        }
        Command::Serve { port } => web::run_serve(port)?,
    }
    Ok(())
}

fn load_problem(problem: Option<&str>, json: Option<&str>) -> Result<Problem, Box<dyn Error>> {
    match (problem, json) {
        (Some(_), Some(_)) => Err("specify either a problem string or --json, not both".into()),
        (None, None) => Err("provide a problem string or --json <path>".into()),
        (Some(s), None) => Ok(parse_problem(s)?),
        (None, Some(path)) => {
            let s = std::fs::read_to_string(path)?;
            Ok(parse_problem_json(&s)?)
        }
    }
}

fn run_calc_with_sink(
    problem: &Problem,
    cfg: &GaConfig,
    base_seed: u64,
    n_threads: usize,
    progress_interval: usize,
    sink_mode: &str,
    sink_interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let mut rng = Xoshiro256StarStar::seed_from_u64(base_seed);
    let seeds: Vec<u64> = (0..n_threads).map(|_| rng.next_u64()).collect();

    eprintln!(
        "Pieces  : {}   Sheet: {}×{}",
        problem.pieces.len(),
        problem.sheet.width,
        problem.sheet.height
    );
    eprintln!("GA cfg  : {cfg}");
    eprintln!("Sink    : {sink_mode}  interval={sink_interval_ms}ms");

    let problem = Arc::new(problem.clone());
    let cfg = Arc::new(cfg.clone());
    match sink_mode {
        "stdout" => {
            let mut sink = cutting::transport::stdout::StdoutSink;
            run_with_sink(
                Arc::clone(&problem),
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
                    Arc::clone(&problem),
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
                    Arc::clone(&problem),
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
                    Arc::clone(&problem),
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
    problem: Arc<Problem>,
    cfg: Arc<GaConfig>,
    seeds: &[u64],
    progress_interval: usize,
    sink: &mut dyn ProgressSink,
    sink_interval_ms: u64,
) -> Result<(), Box<dyn Error>> {
    let (mut handle, ctx) = ga_channel(progress_interval);
    run_ga_mt(Arc::clone(&problem), Arc::clone(&cfg), seeds.to_vec(), ctx);

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
                        let sol = decode(&*problem, &evt.genome);
                        let msg = ProgressMessage::Progress {
                            generation: evt.generation,
                            sheets_used: evt.objective.0,
                            last_sheet_area: evt.objective.1,
                            seed: evt.seed,
                            solution: Some(sol),
                            pieces: Some(problem.pieces.clone()),
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
                    let sol = decode(&*problem, &evt.genome);
                    sink.send(&ProgressMessage::Progress {
                        generation: evt.generation,
                        sheets_used: evt.objective.0,
                        last_sheet_area: evt.objective.1,
                        seed: evt.seed,
                        solution: Some(sol),
                        pieces: Some(problem.pieces.clone()),
                    })
                    .ok();
                }
                eprintln!("Done in {:.1}s", t0.elapsed().as_secs_f64());
                let (best_seed, best) = &results[0];
                let sol = decode(&*problem, &best.genome);
                let msg = ProgressMessage::Done {
                    seed: *best_seed,
                    sheets_used: best.objective.0,
                    last_sheet_area: best.objective.1,
                    solution: sol,
                    pieces: problem.pieces.clone(),
                };
                sink.send(&msg).ok();
                break;
            }
        }
    }
    Ok(())
}

// == Legacy helpers used by web.rs =========================================

pub(crate) fn ga_config(gens: usize, pop: usize, elite: usize, k: usize) -> GaConfig {
    GaConfig {
        pop_size: pop,
        n_generations: gens,
        n_elite: elite,
        tournament_k: k,
        p_crossover: 0.80,
        p_swap: 0.15,
        p_flip: 0.05,
        point_p: 0.10,
        point_delta: (1, 3),
    }
}

#[allow(dead_code)]
fn print_solution(problem: &Problem, sol: &Solution) {
    let mut by_sheet: BTreeMap<usize, Vec<&Placement>> = BTreeMap::new();
    for pl in &sol.placements {
        by_sheet.entry(pl.sheet_idx).or_default().push(pl);
    }
    for (sheet_idx, mut pls) in by_sheet {
        println!(
            "  Sheet {sheet_idx} ({}×{}):",
            problem.sheet.width, problem.sheet.height
        );
        pls.sort_by_key(|p| (p.y, p.x));
        for pl in pls {
            let p = &problem.pieces[pl.piece_idx];
            let (pw, ph) = if pl.rotated {
                (p.height, p.width)
            } else {
                (p.width, p.height)
            };
            let name = if p.name.is_empty() {
                String::new()
            } else {
                format!("  \"{}\"", p.name)
            };
            println!(
                "    idx={:2}  {pw}×{ph}  at ({:4},{:4}){}{}",
                pl.piece_idx,
                pl.x,
                pl.y,
                if pl.rotated { "  [rot]" } else { "" },
                name
            );
        }
    }
    if let Ok(x) = serde_json::to_string(sol) {
        println!("json = {x}");
    }
}
