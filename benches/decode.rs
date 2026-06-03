use criterion::{Criterion, black_box, criterion_group, criterion_main};
use cutting::{
    expand::expand_problem,
    glas::decoder::{Gene as GlasGene, Genome as GlasGenome, decode as glas_decode},
    model::ProblemSpec,
    parse::parse_problem,
    parse_json::parse_problem_json,
    slas::decoder::{Gene as SlasGene, Genome as SlasGenome, decode as slas_decode},
};

fn slas_genome(spec: &ProblemSpec) -> SlasGenome {
    let problem = expand_problem(spec);
    (0..problem.pieces.len())
        .map(|i| SlasGene {
            piece_idx: i,
            rotate: false,
            point_selector: 0,
            inverse: false,
        })
        .collect()
}

fn glas_genome(spec: &ProblemSpec) -> GlasGenome {
    let mut indices: Vec<usize> = (0..spec.piespecs.len()).collect();
    indices.sort_unstable_by(|&a, &b| {
        let area = |i: usize| {
            let ps = &spec.piespecs[i];
            (ps.width as u64) * (ps.height as u64) * (ps.count as u64)
        };
        area(b).cmp(&area(a))
    });
    let genes: Vec<GlasGene> = indices
        .into_iter()
        .map(|type_idx| {
            let count = spec.piespecs[type_idx].count as usize;
            GlasGene {
                type_idx,
                rotate: false,
                selectors: std::iter::repeat(0u32).take(count).collect(),
                inverses: std::iter::repeat(false).take(count).collect(),
            }
        })
        .collect();
    vec![genes, vec![], vec![]]
}

fn bench_decode(c: &mut Criterion) {
    let real_json = include_str!("../src/web/real1.json");
    let real_spec = parse_problem_json(real_json).expect("parse real1.json");
    let real_problem = expand_problem(&real_spec);
    let real_slas = slas_genome(&real_spec);
    let real_glas = glas_genome(&real_spec);

    let synth_spec = parse_problem("2600x1800R:3:400x400/6,495x495/6,270x320/10,150x450/17r").expect("parse");
    let synth_problem = expand_problem(&synth_spec);
    let synth_slas = slas_genome(&synth_spec);
    let synth_glas = glas_genome(&synth_spec);

    let mut g = c.benchmark_group("decode/synthetic");
    g.bench_function("slas", |b| {
        b.iter(|| slas_decode(black_box(&synth_problem), black_box(&synth_slas)))
    });
    g.bench_function("glas", |b| {
        b.iter(|| {
            glas_decode(black_box(&synth_problem), black_box(&synth_spec), black_box(&synth_glas))
        })
    });
    g.finish();

    let mut g = c.benchmark_group("decode/real");
    g.bench_function("slas", |b| {
        b.iter(|| slas_decode(black_box(&real_problem), black_box(&real_slas)))
    });
    g.bench_function("glas", |b| {
        b.iter(|| glas_decode(black_box(&real_problem), black_box(&real_spec), black_box(&real_glas)))
    });
    g.finish();
}

fn bench_staircase_area(c: &mut Criterion) {
    let real_json = include_str!("../src/web/real1.json");
    let real_spec = parse_problem_json(real_json).expect("parse real1.json");
    let real_problem = expand_problem(&real_spec);
    let real_sol = slas_decode(&real_problem, &slas_genome(&real_spec));

    let synth_spec = parse_problem("2600x1800R:3:400x400/6,495x495/6,270x320/10,150x450/17r").expect("parse");
    let synth_problem = expand_problem(&synth_spec);
    let synth_sol = slas_decode(&synth_problem, &slas_genome(&synth_spec));

    let mut g = c.benchmark_group("staircase_area");
    g.bench_function("synthetic", |b| {
        b.iter(|| black_box(&synth_sol).staircase_area(black_box(&synth_problem)))
    });
    g.bench_function("real", |b| {
        b.iter(|| black_box(&real_sol).staircase_area(black_box(&real_problem)))
    });
    g.finish();
}

criterion_group!(benches, bench_decode, bench_staircase_area);
criterion_main!(benches);
