# Cutting

Rust library and CLI tool for 2D guillotine cutting stock problems.

## What it does

Given a stock sheet size, a blade kerf, and a list of rectangular pieces, 
the library finds placements that minimizes the number of sheets used.
All cuts are guillotine cuts (straight lines across the full 
remaining rectangle).

![Cutting example](docs/cutting_example.png)

## Usage

Solve a problem and print ranked results 
```
cargo run --release -- calc "2600x1800F:3:400x400-6,495x495-6,270x320-10,150x450-17r" \
    --seeds 12 --gens 2000
```

Start the web UI at http://localhost:8080
```
cargo run --release -- serve --port 8080
```

You can use the library directly with the code:
```rust
use cutting::parse::parse_problem;
use cutting::decoder::{Gene, Genome, decode};

fn main() {
  // 3000x4000 sheet, 7 mm kerf, 9 pieces (R = rotatable by default)
  let problem = parse_problem("3000x4000R:7:835x620-4,1020x620-4f,1750x900").unwrap();

  // Build a genome (one Gene per piece, in placement order)
  let genome: Genome = problem.pieces.iter().enumerate()
          .map(|(i, _)| Gene { piece_idx: i, rotate: true, point_selector: 0 })
          .collect();

  let solution = decode(&problem, &genome);
  println!("{} sheet(s) used", solution.sheets_used());
  println!("the objective value is {}", solution.objective(&problem));
}
```

## Input format for the parser

`parse_problem` accepts a compact string: `"<sheet>:<kerf>:<pieces>"`.

- `<sheet>` — `WxHR` or `WxHF` in mm; the suffix sets the **default rotation** for pieces:
  - `R` — pieces are rotatable by default
  - `F` — pieces are fixed (no rotation) by default
- `<kerf>` — blade kerf width in mm (non-negative integer)
- `<pieces>` — comma-separated piece tokens; per-piece suffix overrides the sheet default:

| Piece token | Meaning                                        |
|-------------|------------------------------------------------|
| `WxH`       | one piece, rotation = sheet default            |
| `WxH-N`     | N identical pieces, rotation = sheet default   |
| `WxHr`      | one piece, **rotatable** (overrides default)   |
| `WxHf`      | one piece, **fixed** (overrides default)       |
| `WxH-Nr`    | N pieces, rotatable                            |
| `WxH-Nf`    | N pieces, fixed                                |

To control orientation of a fixed piece, put the shorter side first or last as desired:
`620x1020` places 620 mm along X and 1020 mm along Y.

Examples:
- `"3000x4000R:7:835x620-4,1020x620-4f,1750x900"` — R default; only the `1020x620` batch is fixed
- `"2600x1800F:3:400x400-6,495x495-6,270x320-10,150x450-17r"` — F default; only `150x450` is rotatable


## Concepts

- **Problem** — stock sheet dimensions, blade kerf, and an ordered list of `Piece` values.
  Each piece has an opaque external label (`id`), dimensions, and a rotation flag.
  Pieces are addressed internally by their 0-based index in the list.
- **Solution** — vector of placements + vector of free rectangles.
- **Genome** — ordered list of `Gene` values, one per piece. Defines placement order,
  rotation preference, and which free rectangle to try first (`point_selector`).
  Suitable as the individual in a genetic algorithm.
- **Decoder** — deterministic: given a genome and a problem, produces a `Solution`
  via the Shorter Leftover Axis (SLAS) guillotine heuristic.
- **Generator** — creates random problem instances with a known optimal solution.
  Applies guillotine-cut passes to `k` blank sheets, producing a set of pieces
  that tile those sheets exactly. Useful for benchmarking the GA against a ground truth.
- **GA** — genetic algorithm that searches for a good genome. Operators: OX/CX
  crossover, swap/flip/point-selector mutation. Configured via `GaConfig`.
- **Kerf** — blade thickness subtracted from each internal cut; sheet boundary
  edges are exempt.


## Development commands

```
cargo build
cargo test
cargo test <test_name>                              # single test
cargo clippy -- -D warnings
cargo +nightly fmt
cargo run --example ga_benchmark --release          # GA quality benchmark
```

## Demos

Interactive visualizations (open in browser, no server needed):

| Demo | What it shows                                                                |
|------|------------------------------------------------------------------------------|
| [Guillotine Decoder](https://nlinker.github.io/gullotine-cutting-2d/demos/ga_decoder.html) | genome → sheet placements step by step                  |
| [GA Crossover](https://nlinker.github.io/gullotine-cutting-2d/demos/ga_ox_cx_gsap.html) | OX and CX operators animated                                  |
| [GA Mutation](https://nlinker.github.io/gullotine-cutting-2d/demos/ga_mutation_gsap.html) | swap / flip / point-selector mutation animated                 |
| [Guillotine Generator](https://nlinker.github.io/gullotine-cutting-2d/demos/guillotine_generator.html) | random problem generation with known optimal solution |

## References

- [Сиразетдинова Татьяна Юрьевна, "Конструирование прямоугольного раскроя в системах автоматизированного проектирования с учетом дефектных областей материала"](docs/sirazetdinova_t_u.pdf)
- Genetic Algorithm in Java [2d-cutting-stock-problem-master](https://github.com/fabiofdsantos/2d-cutting-stock-problem)
- Simple heuristic in Python [guillotine-cutting-master](third-party/guillotine-cutting-master)
- Cutting algorithm in C++ from Gadsky [gadsky-cutting](third-party/gadsky-cutting)
