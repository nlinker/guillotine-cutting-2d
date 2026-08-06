use std::time::Instant;

use cut::{expand::expand_problem, ga::GaConfig, model::Objective, parser::compact::parse_problem, slas::ga::run_ga};
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;

const PROBLEM: &str = "2600x1800F:3,0: 400x400/6, 495x495/6, 270x320/10, 150x450/17r";
const N_SEEDS: usize = 100;

/// Ideal sentinel: any 2-sheet solution beats this (sheets_used = 2.0 is above all
/// 2-sheet solutions whose float encoding lies in [1.0, 2.0)).
const IDEAL_OBJ: Objective = Objective { sheets_used: 2.0, drop_consolidation_score: 0, layout_score: 0 };

/// 1-piece threshold: same sentinel — any 2-sheet solution qualifies.
const ONE_PIECE_OBJ: Objective = Objective { sheets_used: 2.0, drop_consolidation_score: 0, layout_score: 0 };

struct Variant {
    name: &'static str,
    cfg: GaConfig,
}

fn run_variant(v: &Variant, problem: &cut::model::Problem) {
    let t0 = Instant::now();
    let mut best_obj: Objective = Objective { sheets_used: f64::MAX, drop_consolidation_score: 0, layout_score: 0 };
    let mut sum_sheets: usize = 0;
    let mut sum_consolidation: u64 = 0;
    let mut ideal_count = 0usize;
    let mut one_piece_count = 0usize;

    for seed in 0..N_SEEDS as u64 {
        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
        let ind = run_ga(problem, &v.cfg, &mut rng);
        let obj = ind.objective;
        sum_sheets += obj.sheets_used_int();
        sum_consolidation += obj.drop_consolidation_score;
        if obj < best_obj {
            best_obj = obj;
        }
        if obj <= IDEAL_OBJ {
            ideal_count += 1;
        }
        if obj <= ONE_PIECE_OBJ {
            one_piece_count += 1;
        }
    }

    let elapsed = t0.elapsed();
    let avg_sheets = sum_sheets / N_SEEDS;
    let avg_consolidation = sum_consolidation / N_SEEDS as u64;

    println!(
        "{:<40}  ideal={:3}/{}  1-piece={:3}/{}  best=({},{})\tavg=({},{})  t={:.1}s",
        v.name,
        ideal_count,
        N_SEEDS,
        one_piece_count,
        N_SEEDS,
        best_obj.sheets_used_int(),
        best_obj.drop_consolidation_score,
        avg_sheets,
        avg_consolidation,
        elapsed.as_secs_f64(),
    );
}

#[allow(clippy::too_many_arguments)]
fn cfg(
    pop_size: usize,
    n_generations: usize,
    n_elite: usize,
    tournament_k: usize,
    crossover_p: f64,
    swap_p: f64,
    flip_p: f64,
    point_p: f64,
    inverse_p: f64,
) -> GaConfig {
    GaConfig {
        pop_size,
        iteration_count: n_generations,
        elite_count: n_elite,
        tournament_size: tournament_k,
        crossover_p,
        swap_p,
        flip_p,
        point_p,
        inverse_p,
        ..GaConfig::default()
    }
}

fn main() {
    let spec = parse_problem(PROBLEM).expect("parse error");
    let problem = expand_problem(&spec);
    println!("Problem : {PROBLEM}");
    println!(
        "Pieces  : {}   Sheet: {}×{}",
        problem.pieces.len(),
        problem.sheet.width,
        problem.sheet.height
    );
    println!("Seeds   : 0..{N_SEEDS}   ideal_obj={IDEAL_OBJ:?}   1-piece_obj≤{ONE_PIECE_OBJ:?}");
    println!();
    println!(
        "{:<40}  {:>9}  {:>9}  {:>10}  {:>10}  {:>6}",
        "config", "ideal", "1-piece", "best_obj", "avg_obj", "time"
    );
    println!("{}", "-".repeat(100));

    let variants = vec![
        // == inverse flag effect ===============================
        Variant { name: "inverse_p=0.00  pop=200 gen=1000", cfg: cfg(200, 1000, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.00) },
        Variant { name: "inverse_p=0.02  pop=200 gen=1000", cfg: cfg(200, 1000, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.02) },
        Variant { name: "inverse_p=0.05  pop=200 gen=1000", cfg: cfg(200, 1000, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        Variant { name: "inverse_p=0.10  pop=200 gen=1000", cfg: cfg(200, 1000, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.10) },
        // == baseline ==========================================
        Variant { name: "baseline  pop=100 gen=500", cfg: cfg(100, 500, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        // == vary population size ==============================
        Variant { name: "pop=200 gen=500", cfg: cfg(200, 500, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        Variant { name: "pop=300 gen=500", cfg: cfg(300, 500, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        // == vary generations ==================================
        Variant { name: "pop=100 gen=1000", cfg: cfg(100, 1000, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        Variant { name: "pop=100 gen=2000", cfg: cfg(100, 2000, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        // == both =============================================
        Variant { name: "pop=200 gen=1000", cfg: cfg(200, 1000, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        Variant { name: "pop=200 gen=2000", cfg: cfg(200, 2000, 2, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        // == vary elitism ======================================
        Variant { name: "pop=200 gen=1000 elite=5", cfg: cfg(200, 1000, 5, 3, 0.80, 0.15, 0.05, 0.10, 0.05) },
        // == vary tournament pressure ==========================
        Variant { name: "pop=200 gen=1000 k=2", cfg: cfg(200, 1000, 2, 2, 0.80, 0.15, 0.05, 0.10, 0.05) },
        Variant { name: "pop=200 gen=1000 k=5", cfg: cfg(200, 1000, 2, 5, 0.80, 0.15, 0.05, 0.10, 0.05) },
        // == vary mutation rates ===============================
        Variant { name: "pop=200 gen=1000 swap_p=0.25", cfg: cfg(200, 1000, 2, 3, 0.80, 0.25, 0.05, 0.10, 0.05) },
        Variant { name: "pop=200 gen=1000 swap_p=0.05", cfg: cfg(200, 1000, 2, 3, 0.80, 0.05, 0.05, 0.10, 0.05) },
        Variant { name: "pop=200 gen=1000 p_point=0.20", cfg: cfg(200, 1000, 2, 3, 0.80, 0.15, 0.05, 0.20, 0.05) },
        // == vary crossover probability ========================
        Variant { name: "pop=200 gen=1000 p_cx=0.60", cfg: cfg(200, 1000, 2, 3, 0.60, 0.15, 0.05, 0.10, 0.05) },
        Variant { name: "pop=200 gen=1000 p_cx=0.95", cfg: cfg(200, 1000, 2, 3, 0.95, 0.15, 0.05, 0.10, 0.05) },
    ];

    for v in &variants {
        run_variant(v, &problem);
    }
}
