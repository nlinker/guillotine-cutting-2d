/// Fixed-problem GA benchmark: runs N_PARALLEL GA instances simultaneously (one per seed),
/// picks the best result, and prints detailed placement info.
///
/// Run with:  cargo run --example ga_real --release
use std::collections::BTreeMap;
use std::time::Instant;

use std::sync::Arc;

use cutting::{
    decoder::decode,
    ga::{GaConfig, GaEvent, ga_channel, run_ga_mt},
    model::{Piece, Placement, Problem, Solution},
    parse::parse_problem,
};

const PROBLEM: &str = "200x160F:1:22x26-4,32x20-7,35x20-2,42x21-5,46x26r,67x34-3,75x42-2,76x22-4,83x32-4r,83x82,93x31,106x31,124x26-5,130x22-6,157x31-3,164x21-2,177x31";
const N_PARALLEL: usize = 12;

fn ga_cfg() -> GaConfig {
    GaConfig {
        pop_size: 200,
        n_generations: 2000,
        n_elite: 5,
        tournament_k: 5,
        p_crossover: 0.80,
        p_swap: 0.15,
        p_flip: 0.05,
        p_point: 0.10,
    }
}

fn summarize_last_sheet(problem: &Problem, sol: &Solution) -> (usize, String) {
    let last = sol.sheets_used().saturating_sub(1);
    let on_last: Vec<&Piece> = sol
        .placements
        .iter()
        .filter(|pl| pl.sheet_idx == last)
        .map(|pl| &problem.pieces[pl.piece_idx])
        .collect();
    let count = on_last.len();
    let mut groups: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for p in &on_last {
        let key = (p.width.min(p.height), p.width.max(p.height));
        *groups.entry(key).or_default() += 1;
    }
    let summary = groups
        .iter()
        .map(|((w, h), n)| format!("{n}×{w}×{h}"))
        .collect::<Vec<_>>()
        .join(", ");
    (count, summary)
}

fn main() {
    let problem = parse_problem(PROBLEM).expect("parse error");
    let cfg = ga_cfg();
    let seeds: Vec<u64> = (0..N_PARALLEL as u64).collect();

    println!("Problem  : {PROBLEM}");
    println!(
        "Pieces   : {}   Sheet: {}×{}",
        problem.pieces.len(),
        problem.sheet.width,
        problem.sheet.height
    );
    println!("GA cfg   : {cfg}");
    println!("Parallel : {} threads  seeds={:?}", N_PARALLEL, seeds);
    println!();

    let t0 = Instant::now();
    let (mut handle, ctx) = ga_channel(0);
    run_ga_mt(Arc::new(problem.clone()), Arc::new(cfg.clone()), seeds.clone(), ctx);
    let results = loop {
        match handle.rx.blocking_recv() {
            Some(GaEvent::Done(r)) => break r,
            _ => {}
        }
    };
    println!("Done in {:.1}s\n", t0.elapsed().as_secs_f64());

    // compute per-result summaries (decode once each)
    let decoded: Vec<(u64, i64, Solution, usize, String)> = results
        .iter()
        .map(|(seed, ind)| {
            let sol = decode(&problem, &ind.genome);
            let (n, s) = summarize_last_sheet(&problem, &sol);
            (*seed, ind.objective, sol, n, s)
        })
        .collect();

    println!(
        "{:>6}  {:>6}  {:>8}  {:>10}  last sheet",
        "seed", "sheets", "last_n", "objective"
    );
    println!("{}", "-".repeat(65));
    for (seed, obj, sol, n, summary) in &decoded {
        println!("{:6}  {:6}  {:8}  {:10}  {}", seed, sol.sheets_used(), n, obj, summary);
    }
    println!();

    let (best_seed, best_obj, best_sol, best_n, best_summary) = &decoded[0];
    println!(
        "BEST (seed={best_seed}  obj={best_obj}  sheets={}  last={best_n}: {best_summary})",
        best_sol.sheets_used()
    );
    print_solution(&problem, best_sol);
}

fn print_solution(problem: &Problem, sol: &Solution) {
    let mut by_sheet: BTreeMap<usize, Vec<&Placement>> = BTreeMap::new();
    for pl in &sol.placements {
        by_sheet.entry(pl.sheet_idx).or_default().push(pl);
    }
    for (sheet_idx, mut pls) in by_sheet {
        println!(
            "  Sheet {} ({}×{}):",
            sheet_idx, problem.sheet.width, problem.sheet.height
        );
        pls.sort_by_key(|p| (p.y, p.x));
        for pl in pls {
            let p = &problem.pieces[pl.piece_idx];
            let (pw, ph) = if pl.rotated {
                (p.height, p.width)
            } else {
                (p.width, p.height)
            };
            println!(
                "    idx={:2}  {}×{}  at ({:4},{:4}){}",
                pl.piece_idx,
                pw,
                ph,
                pl.x,
                pl.y,
                if pl.rotated { "  [rot]" } else { "" }
            );
        }
    }
}
