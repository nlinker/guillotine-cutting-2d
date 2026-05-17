/// GA quality benchmark — not a correctness test.
///
/// For each `Suite`, generates `N_INSTANCES` problem instances (each with its own
/// deterministic RNG seed) and runs the GA on each one.  The GA result is compared
/// against the generator's known reference objective.
///
/// Reproducibility: every instance is identified by its `gen_seed`.  To reproduce
/// a specific instance run with that seed in isolation:
///
///   let out = generate(&suite.gen, &mut Xoshiro256StarStar::seed_from_u64(gen_seed));
///   let best = run_ga(&out.problem, &suite.ga, &mut Xoshiro256StarStar::seed_from_u64(ga_seed(gen_seed)));
///
/// Run with:  cargo run --example ga_benchmark --release
use cutting::ga::{GaConfig, run_ga};
use cutting::{
    generator::{GeneratorConfig, generate},
    model::{Objective, Sheet},
};
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;

const N_INSTANCES: usize = 1000;
const GEN_BASE_SEED: u64 = 0;

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
            InstanceResult {
                gen_seed,
                ref_obj,
                ga_obj: best.objective,
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
        "Better    : {:4} ({:.1}%)  [GA found fewer sheets or better leftovers]",
        better,
        better as f64 / n as f64 * 100.0
    );
    println!("Worse     : {:4} ({:.1}%)", worse, worse as f64 / n as f64 * 100.0);

    if worse > 0 {
        let sheet_area = s.gen_cfg.sheet.width as i64 * s.gen_cfg.sheet.height as i64;
        let encode = |obj: Objective| obj.0 as i64 * (sheet_area + 1) + obj.1;
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

    println!();
}

fn main() {
    let suites = [Suite {
        name: "baseline",
        gen_cfg: GeneratorConfig {
            sheet: Sheet {
                width: 200,
                height: 160,
            },
            sheets_count: 4,
            min_size: 20,
            kerf: 1,
            weights: vec![1.0, 2.0, 2.0, 2.0],
            stage_count: 2,
        },
        ga_cfg: GaConfig {
            pop_size: 50,
            n_generations: 200,
            n_elite: 2,
            tournament_k: 3,
            crossover_p: 0.80,
            swap_p: 0.15,
            flip_p: 0.05,
            point_p: 0.10,
            point_delta: (1, 3),
            inverse_p: 0.05,
        },
    }];

    for s in &suites {
        let results = run_suite(s);
        print_report(s, &results);
    }
}
