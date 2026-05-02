use std::collections::BTreeMap;
use std::error::Error;
use std::time::Instant;

use clap::{Parser, Subcommand};
use cutting::{
    decoder::decode,
    ga::{GaConfig, Individual, run_ga_mt},
    model::{Placement, Problem, Solution},
    parse::parse_problem,
};

mod web;

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
        /// Problem string e.g. "2600x1800F:3:400x400-6,495x495-6,270x320-10,150x450-17r"
        problem: String,
        /// Number of parallel GA runs
        #[arg(long, default_value_t = 8)]
        seeds: usize,
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
        Command::Calc { problem, seeds, gens, pop, elite, k } => {
            run_calc(&problem, &ga_config(gens, pop, elite, k), seeds)?;
        }
        Command::Serve { port } => web::run_serve(port)?,
    }
    Ok(())
}

fn run_calc(problem_str: &str, cfg: &GaConfig, n_seeds: usize) -> Result<(), Box<dyn Error>> {
    let problem = parse_problem(problem_str)?;
    let seeds: Vec<u64> = (0..n_seeds as u64).collect();

    println!("Problem : {problem_str}");
    println!("Pieces  : {}   Sheet: {}×{}", problem.pieces.len(), problem.sheet.width, problem.sheet.height);
    println!("GA cfg  : {cfg}");
    println!("Seeds   : {seeds:?}");
    println!();

    let t0 = Instant::now();
    let results = run_ga_mt(&problem, cfg, &seeds);
    println!("Done in {:.1}s\n", t0.elapsed().as_secs_f64());

    let decoded = decode_results(&problem, &results);

    println!("{:>6}  {:>6}  {:>10}  {:>8}  last sheet", "seed", "sheets", "objective", "last_n");
    println!("{}", "-".repeat(55));
    for (seed, obj, sol, n, summary) in &decoded {
        println!("{seed:6}  {:6}  {obj:10}  {n:8}  {summary}", sol.sheets_used());
    }
    println!();

    let (best_seed, best_obj, best_sol, best_n, best_summary) = &decoded[0];
    println!(
        "BEST (seed={best_seed}  obj={best_obj}  sheets={}  last={best_n}: {best_summary})",
        best_sol.sheets_used()
    );
    print_solution(&problem, best_sol);
    Ok(())
}

pub(crate) fn decode_results(problem: &Problem, results: &[(u64, Individual)]) -> Vec<(u64, i64, Solution, usize, String)> {
    results.iter().map(|(seed, ind)| {
        let sol = decode(problem, &ind.genome);
        let (n, s) = summarize_last_sheet(problem, &sol);
        (*seed, ind.objective, sol, n, s)
    }).collect()
}

pub(crate) fn summarize_last_sheet(problem: &Problem, sol: &Solution) -> (usize, String) {
    let last = sol.sheets_used().saturating_sub(1);
    let mut groups: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    let mut count = 0usize;
    for pl in sol.placements.iter().filter(|pl| pl.sheet_idx == last) {
        let p = &problem.pieces[pl.piece_idx];
        *groups.entry((p.width.min(p.height), p.width.max(p.height))).or_default() += 1;
        count += 1;
    }
    let summary = groups.iter()
        .map(|((w, h), n)| format!("{n}×{w}×{h}"))
        .collect::<Vec<_>>()
        .join(", ");
    (count, summary)
}

pub(crate) fn ga_config(gens: usize, pop: usize, elite: usize, k: usize) -> GaConfig {
    GaConfig {
        pop_size: pop,
        n_generations: gens,
        n_elite: elite,
        tournament_k: k,
        p_crossover: 0.80,
        p_swap: 0.15,
        p_flip: 0.05,
        p_point: 0.10,
    }
}

fn print_solution(problem: &Problem, sol: &Solution) {
    let mut by_sheet: BTreeMap<usize, Vec<&Placement>> = BTreeMap::new();
    for pl in &sol.placements {
        by_sheet.entry(pl.sheet_idx).or_default().push(pl);
    }
    for (sheet_idx, mut pls) in by_sheet {
        println!("  Sheet {sheet_idx} ({}×{}):", problem.sheet.width, problem.sheet.height);
        pls.sort_by_key(|p| (p.y, p.x));
        for pl in pls {
            let p = &problem.pieces[pl.piece_idx];
            let (pw, ph) = if pl.rotated { (p.height, p.width) } else { (p.width, p.height) };
            println!("    idx={:2}  {pw}×{ph}  at ({:4},{:4}){}",
                pl.piece_idx, pl.x, pl.y,
                if pl.rotated { "  [rot]" } else { "" });
        }
    }
}
