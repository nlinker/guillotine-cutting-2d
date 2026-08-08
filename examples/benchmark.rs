use cut::{ga::GaConfig, generator::{GeneratorConfig, generate}, glas, model::{Objective, Sheet}};
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;
use cut::expand::shrink_problem;

const N_INSTANCES: usize = 10;
const GEN_BASE_SEED: u64 = 777;

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

fn main() {
    let suites = [Suite {
        name: "baseline",
        gen_cfg: GeneratorConfig {
            sheet: Sheet { width: 100, height: 100 },
            sheets_count: 2,
            min_size: 5,
            kerf: 0,
            weights: vec![3., 3., 3., 2., 2., 2., 1., 1., 1.],
            stage_count: 4,
        },
        ga_cfg: GaConfig { pop_size: 200, iteration_count: 2000, ..GaConfig::default() },
    }];

    for s in &suites {
        run_suite(s);
    }
}

fn run_suite(s: &Suite) {
    let mut results = Vec::with_capacity(N_INSTANCES);
    for i in 0..N_INSTANCES {
        let gen_seed = GEN_BASE_SEED + i as u64;
        let out = generate(&s.gen_cfg, &mut Xoshiro256StarStar::seed_from_u64(gen_seed));
        let spec = shrink_problem(&out.problem);
        let ref_obj = out.optimal_solution.eval(&out.problem);

        let best = glas::ga::run_ga(
            &spec,
            &s.ga_cfg,
            &mut Xoshiro256StarStar::seed_from_u64(ga_seed(gen_seed)),
        );
        let r = InstanceResult { gen_seed, ref_obj, ga_obj: best.objective };
        results.push(r);
    }
    print_report(s, &results);
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

    println!();
}
