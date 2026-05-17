/// Fixed-problem GA benchmark: runs N_PARALLEL GA instances simultaneously (one per seed),
/// picks the best result, and prints detailed placement info.
///
/// Run with:  cargo run --example ga_real --release
use std::collections::BTreeMap;
use std::{sync::Arc, time::Instant};

use cutting::{
    slas::decoder::decode_spec,
    ga::{GaConfig, GaEvent, run_ga_mt},
    model::{Objective, PieceSpec, ProblemSpec, SolutionSpec},
    parse::parse_problem,
};

const PROBLEM: &str = "200x160F:1:22x26/4,32x20/7,35x20/2,42x21/5,46x26r,67x34/3,75x42/2,76x22/4,83x32/4r,83x82,93x31,106x31,124x26/5,130x22/6,157x31/3,164x21/2,177x31";
const N_PARALLEL: usize = 12;

fn ga_cfg() -> GaConfig {
    GaConfig {
        pop_size: 200,
        n_generations: 2000,
        n_elite: 5,
        tournament_k: 5,
        crossover_p: 0.80,
        swap_p: 0.15,
        flip_p: 0.05,
        point_p: 0.10,
        point_delta: (1, 3),
    }
}

fn summarize_last_sheet(spec: &ProblemSpec, sol: &SolutionSpec) -> (usize, String) {
    let last = sol.sheets_used().saturating_sub(1);
    let on_last: Vec<&PieceSpec> = sol
        .placements
        .iter()
        .filter(|pl| pl.sheet_idx == last)
        .map(|pl| &spec.piespecs[pl.piespec_idx])
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
    let spec = parse_problem(PROBLEM).expect("parse error");
    let total: u32 = spec.piespecs.iter().map(|p| p.count).sum();
    let cfg = ga_cfg();
    let seeds: Vec<u64> = (0..N_PARALLEL as u64).collect();

    println!("Problem  : {PROBLEM}");
    println!(
        "Pieces   : {} ({} types)   Sheet: {}×{}",
        total,
        spec.piespecs.len(),
        spec.sheet.width,
        spec.sheet.height
    );
    println!("GA cfg   : {cfg}");
    println!("Parallel : {} threads  seeds={:?}", N_PARALLEL, seeds);
    println!();

    let t0 = Instant::now();
    let mut handle = run_ga_mt(Arc::new(spec.clone()), Arc::new(cfg.clone()), seeds.clone(), 0);
    let results = loop {
        match handle.rx.blocking_recv() {
            Some(GaEvent::Done(r)) => break r,
            _ => {}
        }
    };
    println!("Done in {:.1}s\n", t0.elapsed().as_secs_f64());

    let decoded: Vec<(u64, Objective, SolutionSpec, usize, String)> = results
        .iter()
        .map(|(seed, ind)| {
            let sol = decode_spec(&spec, &ind.genome);
            let (n, s) = summarize_last_sheet(&spec, &sol);
            (*seed, ind.objective, sol, n, s)
        })
        .collect();

    println!(
        "{:>6}  {:>6}  {:>8}  {:>12}  last sheet",
        "seed", "sheets", "last_n", "last_area"
    );
    println!("{}", "-".repeat(65));
    for (seed, obj, _sol, n, summary) in &decoded {
        println!("{:6}  {:6}  {:8}  {:12}  {}", seed, obj.0, n, obj.1, summary);
    }
    println!();

    let (best_seed, best_obj, best_sol, best_n, best_summary) = &decoded[0];
    println!(
        "BEST (seed={best_seed}  sheets={}  last_area={}  last={best_n}: {best_summary})",
        best_obj.0, best_obj.1
    );
    print_solution(&spec, best_sol);
}

fn print_solution(spec: &ProblemSpec, sol: &SolutionSpec) {
    let mut by_sheet: BTreeMap<usize, Vec<_>> = BTreeMap::new();
    for pl in &sol.placements {
        by_sheet.entry(pl.sheet_idx).or_default().push(pl);
    }
    for (sheet_idx, mut pls) in by_sheet {
        println!("  Sheet {} ({}×{}):", sheet_idx, spec.sheet.width, spec.sheet.height);
        pls.sort_by_key(|p| (p.y, p.x));
        for pl in pls {
            let p = &spec.piespecs[pl.piespec_idx];
            let (pw, ph) = if pl.rotated {
                (p.height, p.width)
            } else {
                (p.width, p.height)
            };
            println!(
                "    idx={:2}  {}×{}  at ({:4},{:4}){}",
                pl.piespec_idx,
                pw,
                ph,
                pl.x,
                pl.y,
                if pl.rotated { "  [rot]" } else { "" }
            );
        }
    }
}
