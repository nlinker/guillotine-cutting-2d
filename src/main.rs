use std::{error::Error, sync::Arc, time::Instant};

use clap::Parser;
use cut::{
    exact::glf::build_glf,
    expand::expand_problem,
    ga::GaConfig,
    model::ProblemSpec,
    parser,
    render::render_svg,
    runner::{AlgConfig, GaKind, HeuristicKind},
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

#[rustfmt::skip]
fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::Calc {
            compact, json, seed, threads, iterations, pop, progress, sink,
            sink_interval,render, algorithm, long_dim_threshold, large_area_threshold
        } => {
            let spec = load_problem(compact.as_deref(), json.as_deref())?;
            let cfg = GaConfig::new(&spec, iterations, pop, large_area_threshold, long_dim_threshold);
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

/// Converts the CLI-facing `Algorithm` into the library's `AlgConfig`.
pub(crate) fn make_algorithm_config(
    algorithm: Algorithm,
    cfg: Arc<GaConfig>,
    seeds: Vec<u64>,
    progress_interval: usize,
) -> AlgConfig {
    match algorithm {
        Algorithm::Slas => AlgConfig::Ga { kind: GaKind::Slas, cfg, seeds, progress_interval },
        Algorithm::Glas => AlgConfig::Ga { kind: GaKind::Glas, cfg, seeds, progress_interval },
        Algorithm::Bfdh => AlgConfig::Heuristic { kind: HeuristicKind::Bfdh },
        Algorithm::Jylanki => AlgConfig::Heuristic { kind: HeuristicKind::Jylanki },
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
    let alg_cfg = make_algorithm_config(algorithm, cfg, seeds, progress_interval);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;

    match sink_mode {
        "stdout" => {
            let mut sink = cut::transport::stdout::StdoutSink;
            let handle = cut::runner::run_algorithm(Arc::clone(&spec), &alg_cfg);
            Ok(rt.block_on(cut::runner::drain(handle, spec, &mut sink, sink_interval_ms))?)
        }
        _ => {
            #[cfg(windows)]
            {
                eprintln!("Waiting for client on {PIPE_NAME} ...");
                let mut sink = cut::transport::windows::WindowsPipeSink::create_and_wait(PIPE_NAME)?;
                let handle = cut::runner::run_algorithm(Arc::clone(&spec), &alg_cfg);
                Ok(rt.block_on(cut::runner::drain(handle, spec, &mut sink, sink_interval_ms))?)
            }
            #[cfg(unix)]
            {
                eprintln!("Waiting for reader on {FIFO_PATH} ...");
                let mut sink = cut::transport::unix::FifoSink::new(FIFO_PATH)?;
                let handle = cut::runner::run_algorithm(Arc::clone(&spec), &alg_cfg);
                Ok(rt.block_on(cut::runner::drain(handle, spec, &mut sink, sink_interval_ms))?)
            }
            #[cfg(not(any(windows, unix)))]
            {
                eprintln!("Named pipe not supported on this platform, falling back to stdout");
                let mut sink = cut::transport::stdout::StdoutSink;
                let handle = cut::runner::run_algorithm(Arc::clone(&spec), &alg_cfg);
                Ok(rt.block_on(cut::runner::drain(handle, spec, &mut sink, sink_interval_ms))?)
            }
        }
    }
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

    let spec_arc = Arc::new(spec.clone());
    let cfg_arc = Arc::new(cfg.clone());
    let alg_cfg = make_algorithm_config(algorithm, cfg_arc, seeds, progress_interval);
    let handle = cut::runner::run_algorithm(Arc::clone(&spec_arc), &alg_cfg);
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let t0 = Instant::now();
    let mut results = rt.block_on(handle.blocking_wait());
    eprintln!("Done in {:.1}s", t0.elapsed().as_secs_f64());
    let (_, _, lazy, _) = results.remove(0);
    let sol = lazy.decode(&spec_arc);
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
