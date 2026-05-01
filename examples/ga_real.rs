/// Fixed-problem GA benchmark: one real furniture cutting problem, many seeds.
///
/// Runs the GA with seeds 0..N_SEEDS, collects objectives, then reprints
/// selected solutions in detail.
///
/// Run with:  cargo run --example ga_real --release
use std::collections::BTreeMap;

use cutting::decoder::decode;
use cutting::ga::{GaConfig, run_ga};
use cutting::model::{Piece, Placement, Problem, Solution};
use cutting::parse::parse_problem;
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;

const PROBLEM: &str = "2600x1800:400x400x6n,495x495x6n,270x320x10n,150x450x17";
const KERF: u32 = 3;
const N_SEEDS: usize = 100;

fn ga_cfg() -> GaConfig {
    GaConfig {
        pop_size: 100,
        n_generations: 500,
        n_elite: 2,
        tournament_k: 3,
        p_crossover: 0.80,
        p_swap: 0.15,
        p_flip: 0.05,
        p_point: 0.10,
    }
}

struct Run {
    seed: u64,
    obj: i64,
    sheets: usize,
    /// Number of pieces placed on the last (highest-index) sheet.
    pieces_on_last: usize,
    /// Summary of what's on the last sheet: "NxWxH, ..."
    last_sheet_summary: String,
}

fn summarize_last_sheet(problem: &Problem, sol: &Solution) -> (usize, String) {
    let last = sol.sheets_used().saturating_sub(1);
    let on_last: Vec<&Piece> = sol.placements.iter()
        .filter(|pl| pl.sheet_idx == last)
        .map(|pl| &problem.pieces[pl.piece_idx])
        .collect();
    let count = on_last.len();
    // group by dimensions (canonical: min×max for display)
    let mut groups: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    for p in &on_last {
        let key = (p.width.min(p.height), p.width.max(p.height));
        *groups.entry(key).or_default() += 1;
    }
    let summary = groups.iter()
        .map(|((w, h), n)| format!("{n}×{w}×{h}"))
        .collect::<Vec<_>>()
        .join(", ");
    (count, summary)
}

fn main() {
    let problem = parse_problem(PROBLEM, KERF).expect("parse error");
    let cfg = ga_cfg();

    println!("Problem : {PROBLEM}");
    println!("Kerf    : {KERF} mm");
    println!("Pieces  : {}", problem.pieces.len());
    println!("Sheet   : {}×{}", problem.sheet.width, problem.sheet.height);
    println!(
        "GA      : pop={}  gens={}  elite={}  k={}  \
         p_cx={:.2}  p_sw={:.2}  p_fl={:.2}  p_pt={:.2}",
        cfg.pop_size, cfg.n_generations, cfg.n_elite, cfg.tournament_k,
        cfg.p_crossover, cfg.p_swap, cfg.p_flip, cfg.p_point,
    );
    println!("Seeds   : 0..{N_SEEDS}");
    println!();

    let mut runs: Vec<Run> = (0..N_SEEDS as u64)
        .map(|seed| {
            let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
            let best = run_ga(&problem, &cfg, &mut rng);
            let sol = decode(&problem, &best.genome);
            let sheets = sol.sheets_used();
            let (pieces_on_last, last_sheet_summary) = summarize_last_sheet(&problem, &sol);
            Run { seed, obj: best.objective, sheets, pieces_on_last, last_sheet_summary }
        })
        .collect();

    // primary sort: sheets used, then pieces on last sheet, then objective
    runs.sort_by_key(|r| (r.sheets, r.pieces_on_last, r.obj));

    println!("{:>6}  {:>6}  {:>8}  {:>10}  last sheet contents",
        "seed", "sheets", "last_n", "objective");
    println!("{:->6}  {:->6}  {:->8}  {:->10}  {:->30}", "", "", "", "", "");
    for r in &runs {
        println!("{:6}  {:6}  {:8}  {:10}  {}",
            r.seed, r.sheets, r.pieces_on_last, r.obj, r.last_sheet_summary);
    }
    println!();

    let min_last = runs[0].pieces_on_last;
    let ideal_found = runs.iter().any(|r| r.pieces_on_last == 1
        && r.last_sheet_summary.contains("400×400"));

    println!("Fewest pieces on last sheet : {min_last}");
    println!("Ideal (1×400×400 on sheet 1): {}", if ideal_found { "YES ✓" } else { "not found" });
    println!();

    // show details for: fewest-on-last, best obj, worst obj
    let seeds_to_show: Vec<(&str, u64)> = {
        let fewest_seed = runs[0].seed;
        let best_seed = *runs.iter().min_by_key(|r| r.obj).map(|r| &r.seed).unwrap();
        let worst_seed = *runs.iter().max_by_key(|r| r.obj).map(|r| &r.seed).unwrap();
        let mut v = vec![("FEWEST ON LAST", fewest_seed), ("BEST OBJ", best_seed)];
        if worst_seed != best_seed {
            v.push(("WORST OBJ", worst_seed));
        }
        v.dedup_by_key(|x| x.1);
        v
    };

    for (label, seed) in seeds_to_show {
        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
        let best = run_ga(&problem, &cfg, &mut rng);
        let sol = decode(&problem, &best.genome);
        let (n, summary) = summarize_last_sheet(&problem, &sol);
        println!("─── {label} (seed={seed}  obj={}  sheets={}  last={n}: {summary}) ───",
            best.objective, sol.sheets_used());
        print_solution(&problem, &sol);
        println!();
    }
}

fn print_solution(problem: &Problem, sol: &Solution) {
    let mut by_sheet: BTreeMap<usize, Vec<&Placement>> = BTreeMap::new();
    for pl in &sol.placements {
        by_sheet.entry(pl.sheet_idx).or_default().push(pl);
    }
    for (sheet_idx, mut pls) in by_sheet {
        println!("  Sheet {} ({}×{}):", sheet_idx, problem.sheet.width, problem.sheet.height);
        pls.sort_by_key(|p| (p.y, p.x));
        for pl in pls {
            let p = &problem.pieces[pl.piece_idx];
            let (pw, ph) = if pl.rotated { (p.height, p.width) } else { (p.width, p.height) };
            println!("    idx={:2}  {}×{}  at ({:4},{:4}){}",
                pl.piece_idx, pw, ph, pl.x, pl.y,
                if pl.rotated { "  [rot]" } else { "" });
        }
    }
}
