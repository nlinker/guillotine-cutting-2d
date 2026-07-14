/// GA + BPC quality benchmark — not a correctness test.
///
/// For each `Suite`, generates `N_INSTANCES` problem instances (each with its own
/// deterministic RNG seed) and runs both the GA and the exact BPC solver on each
/// one. Both results are compared against the generator's known reference
/// objective (GA on the full objective, BPC on sheet count only — BPC doesn't
/// optimize `layout_score`/`drop_consolidation_score`).
///
/// Reproducibility: every instance is identified by its `gen_seed`. To reproduce
/// a specific instance run with that seed in isolation:
///
///   let out = generate(&suite.gen, &mut Xoshiro256StarStar::seed_from_u64(gen_seed));
///   let best = run_ga(&out.problem, &suite.ga, &mut Xoshiro256StarStar::seed_from_u64(ga_seed(gen_seed)));
///
/// Run with:  cargo run --example benchmark --release
use std::sync::Arc;

use cut::{
    exact::{BpcConfig, drain_bpc, run_bpc_bg},
    expand::shrink_problem,
    ga::GaConfig,
    generator::{GeneratorConfig, generate},
    model::{Objective, Sheet},
    slas::ga::run_ga,
    transport::{ProgressMessage, ProgressSink},
};
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;

const N_INSTANCES: usize = 100;
const GEN_BASE_SEED: u64 = 777;
/// BPC is exact and much slower than the GA; flip off for a quick GA-only run.
const RUN_BPC: bool = false;

/// Derive a GA seed from the generator seed so the two streams are independent.
fn ga_seed(gen_seed: u64) -> u64 {
    gen_seed ^ 0x9e37_79b9_7f4a_7c15
}

struct Suite {
    name: &'static str,
    gen_cfg: GeneratorConfig,
    ga_cfg: GaConfig,
}

struct InstanceResult {
    gen_seed: u64,
    ref_obj: Objective,
    ga_obj: Objective,
    bpc_sheets: usize,
    bpc_proven: bool,
}

/// Run BPC to completion on `problem`, blocking the calling thread.
fn run_bpc_sync(problem: &cut::model::Problem) -> (usize, bool) {
    let spec = Arc::new(shrink_problem(problem));
    let cfg = Arc::new(BpcConfig::default());
    let handle = run_bpc_bg(Arc::clone(&spec), cfg);

    struct Capture {
        sheets: usize,
        proven: bool,
    }
    impl ProgressSink for Capture {
        fn send(&mut self, msg: &ProgressMessage) -> Result<(), std::io::Error> {
            if let ProgressMessage::Done {
                sheets_used,
                proven_optimal,
                ..
            } = msg
            {
                self.sheets = *sheets_used;
                self.proven = proven_optimal.unwrap_or(false);
            }
            Ok(())
        }
    }

    let mut cap = Capture {
        sheets: 0,
        proven: false,
    };
    drain_bpc(handle, &spec, &mut cap, 0).expect("drain_bpc failed");
    (cap.sheets, cap.proven)
}

fn run_suite(s: &Suite) -> Vec<InstanceResult> {
    (0..N_INSTANCES)
        .map(|i| {
            let gen_seed = GEN_BASE_SEED + i as u64;
            let out = generate(&s.gen_cfg, &mut Xoshiro256StarStar::seed_from_u64(gen_seed));
            let ref_obj = out.optimal_solution.objective(&out.problem);
            let best = run_ga(
                &out.problem,
                &s.ga_cfg,
                &mut Xoshiro256StarStar::seed_from_u64(ga_seed(gen_seed)),
            );
            let (bpc_sheets, bpc_proven) = if RUN_BPC {
                run_bpc_sync(&out.problem)
            } else {
                (0, false)
            };
            InstanceResult {
                gen_seed,
                ref_obj,
                ga_obj: best.objective,
                bpc_sheets,
                bpc_proven,
            }
        })
        .collect()
}

fn print_report(s: &Suite, results: &[InstanceResult]) {
    let n = results.len();
    let matched = results.iter().filter(|r| r.ga_obj == r.ref_obj).count();
    let better = results.iter().filter(|r| r.ga_obj < r.ref_obj).count();
    let worse = results.iter().filter(|r| r.ga_obj > r.ref_obj).count();

    let g = &s.gen_cfg;
    println!(
        "=== {} | {}×{}  k={}  min={}  kerf={}  stages={} ===",
        s.name, g.sheet.width, g.sheet.height, g.sheets_count, g.min_size, g.kerf, g.stage_count,
    );
    println!("GA cfg: {}", s.ga_cfg);
    println!("Instances : {n}");
    println!("Matched   : {:4} ({:.1}%)", matched, matched as f64 / n as f64 * 100.0);
    println!(
        "Better    : {:4} ({:.1}%)  [GA found fewer sheets or better-consolidated drops]",
        better,
        better as f64 / n as f64 * 100.0
    );
    println!("Worse     : {:4} ({:.1}%)", worse, worse as f64 / n as f64 * 100.0);

    if worse > 0 {
        let sheet_area = s.gen_cfg.sheet.width as i64 * s.gen_cfg.sheet.height as i64;
        // drop_consolidation_score is area^2-scaled and higher-is-better, unlike the old
        // leftover_area term it replaces; divide back down to ~area scale and subtract
        // so that, as before, a larger encoded value still means a worse solution.
        let encode = |obj: Objective| {
            obj.sheets_used_int() as i64 * (sheet_area + 1)
                - (obj.drop_consolidation_score / sheet_area.max(1) as u64) as i64
        };
        let mut gaps: Vec<i64> = results
            .iter()
            .filter(|r| r.ga_obj > r.ref_obj)
            .map(|r| encode(r.ga_obj) - encode(r.ref_obj))
            .collect();
        gaps.sort_unstable();
        let sum: i64 = gaps.iter().sum();
        let mean = sum as f64 / gaps.len() as f64;
        let p50 = gaps[gaps.len() / 2];
        let p95 = gaps[gaps.len() * 95 / 100];
        println!(
            "  gap  min={} max={} mean={:.1} p50={} p95={}",
            gaps[0],
            gaps[gaps.len() - 1],
            mean,
            p50,
            p95,
        );

        let mut by_gap: Vec<&InstanceResult> = results.iter().filter(|r| r.ga_obj > r.ref_obj).collect();
        by_gap.sort_unstable_by_key(|r| -(encode(r.ga_obj) - encode(r.ref_obj)));
        println!("  worst instances (gen_seed / ref / ga / gap):");
        for r in by_gap.iter().take(5) {
            let gap = encode(r.ga_obj) - encode(r.ref_obj);
            println!(
                "    gen_seed={:6}  ref={:?}  ga={:?}  gap={}",
                r.gen_seed, r.ref_obj, r.ga_obj, gap,
            );
        }
    }

    if RUN_BPC {
        let ref_sheets = results.iter().map(|r| r.ref_obj.sheets_used_int()).collect::<Vec<_>>();
        let bpc_matched = results
            .iter()
            .zip(&ref_sheets)
            .filter(|(r, rs)| r.bpc_sheets == **rs)
            .count();
        let bpc_better = results
            .iter()
            .zip(&ref_sheets)
            .filter(|(r, rs)| r.bpc_sheets < **rs)
            .count();
        let bpc_worse = results
            .iter()
            .zip(&ref_sheets)
            .filter(|(r, rs)| r.bpc_sheets > **rs)
            .count();
        let bpc_proven = results.iter().filter(|r| r.bpc_proven).count();
        println!(
            "BPC matched : {:4} ({:.1}%)",
            bpc_matched,
            bpc_matched as f64 / n as f64 * 100.0
        );
        println!(
            "BPC better  : {:4} ({:.1}%)",
            bpc_better,
            bpc_better as f64 / n as f64 * 100.0
        );
        println!(
            "BPC worse   : {:4} ({:.1}%)",
            bpc_worse,
            bpc_worse as f64 / n as f64 * 100.0
        );
        println!(
            "BPC proven optimal : {:4} ({:.1}%)",
            bpc_proven,
            bpc_proven as f64 / n as f64 * 100.0
        );
    }

    println!();
}

fn main() {
    let suites = [Suite {
        name: "baseline",
        gen_cfg: GeneratorConfig {
            sheet: Sheet { width: 60, height: 50 },
            sheets_count: 4,
            min_size: 15,
            kerf: 1,
            weights: vec![1.0, 1.0],
            stage_count: 2,
        },
        ga_cfg: GaConfig {
            pop_size: 50,
            n_generations: 200,
            ..GaConfig::default()
        },
    }];

    for s in &suites {
        let results = run_suite(s);
        print_report(s, &results);
    }
}
