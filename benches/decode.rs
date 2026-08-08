use std::collections::HashMap;

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use cut::{
    expand::expand_problem,
    generator::{GeneratorConfig, generate},
    glas::decoder::{Gene as GlasGene, Genome as GlasGenome, decode as glas_decode},
    model::{PieceType, Problem, ProblemSpec, Sheet},
};
use rand::SeedableRng;
use rand_xoshiro::Xoshiro256StarStar;

fn glas_genome(spec: &ProblemSpec) -> GlasGenome {
    let mut indices: Vec<usize> = (0..spec.piece_types.len()).collect();
    indices.sort_unstable_by(|&a, &b| {
        let area = |i: usize| {
            let ps = &spec.piece_types[i];
            (ps.width as u64) * (ps.height as u64) * (ps.count as u64)
        };
        area(b).cmp(&area(a))
    });
    let genes: Vec<GlasGene> = indices
        .into_iter()
        .map(|type_idx| {
            let count = spec.piece_types[type_idx].count as usize;
            GlasGene {
                type_idx,
                rotate: false,
                selectors: std::iter::repeat_n(0u32, count).collect(),
                inverses: std::iter::repeat_n(false, count).collect(),
            }
        })
        .collect();
    vec![genes, vec![], vec![]]
}

fn problem_to_spec(problem: &Problem) -> ProblemSpec {
    let mut counts: HashMap<(u32, u32, bool), u32> = HashMap::new();
    for p in &problem.pieces {
        *counts.entry((p.width, p.height, p.can_rotate)).or_insert(0) += 1;
    }
    let piece_types = counts
        .into_iter()
        .map(|((width, height, can_rotate), count)| PieceType { name: String::new(), width, height, count, can_rotate })
        .collect();
    ProblemSpec { sheet: problem.sheet, kerf: 0, margin: 0, piece_types }
}

fn heavy_spec() -> ProblemSpec {
    let cfg = GeneratorConfig {
        sheet: Sheet { width: 100, height: 100 },
        sheets_count: 2,
        min_size: 5,
        kerf: 0,
        weights: vec![3., 3., 3., 2., 2., 2., 1., 1., 1.],
        stage_count: 4,
    };
    let mut rng = Xoshiro256StarStar::seed_from_u64(42);
    let out = generate(&cfg, &mut rng);
    problem_to_spec(&out.problem)
}

fn bench_decode(c: &mut Criterion) {
    let spec = heavy_spec();
    let problem = expand_problem(&spec);
    let glas = glas_genome(&spec);

    let mut g = c.benchmark_group("decode/heavy");
    g.bench_function("glas", |b| {
        b.iter(|| glas_decode(black_box(&problem), black_box(&spec), black_box(&glas)))
    });
    g.finish();

    // Objective::eval alone
    let sol = glas_decode(&problem, &spec, &glas);
    let mut g = c.benchmark_group("eval/heavy");
    g.bench_function("glas", |b| b.iter(|| black_box(&sol).eval(black_box(&problem))));
    g.finish();
}

criterion_group!(benches, bench_decode);
criterion_main!(benches);
