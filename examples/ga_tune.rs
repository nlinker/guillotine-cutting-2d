/// GA hyperparameter tuning for the real furniture cutting problem.
///
/// Runs each GaConfig variant over N_SEEDS seeds and reports:
/// - how many times the ideal solution was found (1×400×400 on last sheet)
/// - how many times any 1-piece solution was found
/// - best objective, avg objective, wall time
///
/// Run with:  cargo run --example ga_tune --release
use std::time::Instant;

use cutting::{
    ga::{GaConfig, run_ga},
    parse::parse_problem,
};
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;

const PROBLEM: &str = "2600x1800F:3:400x400-6,495x495-6,270x320-10,150x450-17r";
const N_SEEDS: usize = 100;

/// Ideal: 1×400×400 on last sheet. obj = 2*(2600*1800+1) + 400*400 = 9_520_002
const IDEAL_OBJ: i64 = 9_520_002;
/// 1-piece threshold: any single piece on last sheet (≤ 1×495×495 area = 245_025)
const ONE_PIECE_OBJ: i64 = 2 * (2600 * 1800 + 1) + 495 * 495; // 9_605_027

struct Variant {
    name: &'static str,
    cfg: GaConfig,
}

fn run_variant(v: &Variant, problem: &cutting::model::Problem) {
    let t0 = Instant::now();
    let mut best_obj = i64::MAX;
    let mut sum_obj: i64 = 0;
    let mut ideal_count = 0usize;
    let mut one_piece_count = 0usize;

    for seed in 0..N_SEEDS as u64 {
        let mut rng = Xoshiro256StarStar::seed_from_u64(seed);
        let ind = run_ga(problem, &v.cfg, &mut rng);
        let obj = ind.objective;
        sum_obj += obj;
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
    let avg_obj = sum_obj / N_SEEDS as i64;

    println!(
        "{:<40}  ideal={:3}/{}  1-piece={:3}/{}  best={:10}  avg={:10}  t={:.1}s",
        v.name,
        ideal_count,
        N_SEEDS,
        one_piece_count,
        N_SEEDS,
        best_obj,
        avg_obj,
        elapsed.as_secs_f64(),
    );
}

fn cfg(
    pop_size: usize,
    n_generations: usize,
    n_elite: usize,
    tournament_k: usize,
    p_crossover: f64,
    p_swap: f64,
    p_flip: f64,
    point_p: f64,
) -> GaConfig {
    GaConfig {
        pop_size,
        n_generations,
        n_elite,
        tournament_k,
        p_crossover,
        p_swap,
        p_flip,
        point_p,
        point_delta: (1, 3),
    }
}

fn main() {
    let problem = parse_problem(PROBLEM).expect("parse error");
    println!("Problem : {PROBLEM}");
    println!(
        "Pieces  : {}   Sheet: {}×{}",
        problem.pieces.len(),
        problem.sheet.width,
        problem.sheet.height
    );
    println!("Seeds   : 0..{N_SEEDS}   ideal_obj={IDEAL_OBJ}   1-piece_obj≤{ONE_PIECE_OBJ}");
    println!();
    println!(
        "{:<40}  {:>9}  {:>9}  {:>10}  {:>10}  {:>6}",
        "config", "ideal", "1-piece", "best_obj", "avg_obj", "time"
    );
    println!("{}", "-".repeat(100));

    let variants = vec![
        // == baseline ==========================================
        Variant {
            name: "baseline  pop=100 gen=500",
            cfg: cfg(100, 500, 2, 3, 0.80, 0.15, 0.05, 0.10),
        },
        // == vary population size ==============================
        Variant {
            name: "pop=200 gen=500",
            cfg: cfg(200, 500, 2, 3, 0.80, 0.15, 0.05, 0.10),
        },
        Variant {
            name: "pop=300 gen=500",
            cfg: cfg(300, 500, 2, 3, 0.80, 0.15, 0.05, 0.10),
        },
        // == vary generations ==================================
        Variant {
            name: "pop=100 gen=1000",
            cfg: cfg(100, 1000, 2, 3, 0.80, 0.15, 0.05, 0.10),
        },
        Variant {
            name: "pop=100 gen=2000",
            cfg: cfg(100, 2000, 2, 3, 0.80, 0.15, 0.05, 0.10),
        },
        // == both =============================================
        Variant {
            name: "pop=200 gen=1000",
            cfg: cfg(200, 1000, 2, 3, 0.80, 0.15, 0.05, 0.10),
        },
        Variant {
            name: "pop=200 gen=2000",
            cfg: cfg(200, 2000, 2, 3, 0.80, 0.15, 0.05, 0.10),
        },
        // == vary elitism ======================================
        Variant {
            name: "pop=200 gen=1000 elite=5",
            cfg: cfg(200, 1000, 5, 3, 0.80, 0.15, 0.05, 0.10),
        },
        // == vary tournament pressure ==========================
        Variant {
            name: "pop=200 gen=1000 k=2",
            cfg: cfg(200, 1000, 2, 2, 0.80, 0.15, 0.05, 0.10),
        },
        Variant {
            name: "pop=200 gen=1000 k=5",
            cfg: cfg(200, 1000, 2, 5, 0.80, 0.15, 0.05, 0.10),
        },
        // == vary mutation rates ===============================
        Variant {
            name: "pop=200 gen=1000 p_swap=0.25",
            cfg: cfg(200, 1000, 2, 3, 0.80, 0.25, 0.05, 0.10),
        },
        Variant {
            name: "pop=200 gen=1000 p_swap=0.05",
            cfg: cfg(200, 1000, 2, 3, 0.80, 0.05, 0.05, 0.10),
        },
        Variant {
            name: "pop=200 gen=1000 p_point=0.20",
            cfg: cfg(200, 1000, 2, 3, 0.80, 0.15, 0.05, 0.20),
        },
        // == vary crossover probability ========================
        Variant {
            name: "pop=200 gen=1000 p_cx=0.60",
            cfg: cfg(200, 1000, 2, 3, 0.60, 0.15, 0.05, 0.10),
        },
        Variant {
            name: "pop=200 gen=1000 p_cx=0.95",
            cfg: cfg(200, 1000, 2, 3, 0.95, 0.15, 0.05, 0.10),
        },
    ];

    for v in &variants {
        run_variant(v, &problem);
    }
}
