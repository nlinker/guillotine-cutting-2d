# Cutting

Rust library for 2D guillotine cutting stock problems.

## What it does

Given a stock sheet size, a blade kerf, and a list of rectangular pieces, 
the library finds placements that minimizes the number of sheets used.
All cuts are guillotine cuts (straight lines across the full 
remaining rectangle).

## Usage

```rust
use cutting::parse::parse_problem;
use cutting::decoder::{Gene, Genome, decode};

fn main() {
  // "3000x4000" sheet, 7 mm kerf, 9 pieces
  let problem = parse_problem("3000x4000:835x620x4,1020x620x4n,1750x900", 7).unwrap();

  // Build a genome (one Gene per piece, in placement order)
  let genome: Genome = problem.pieces.iter().enumerate()
          .map(|(i, _)| Gene { piece_idx: i, rotate: true, point_selector: 0 })
          .collect();

  let solution = decode(&problem, &genome);
  println!("{} sheet(s) used", solution.sheets_used());
  println!("the objective value is {}", solution.objective());
}
```

## Input format

`parse_problem` accepts a compact string: `"<sheet>:<pieces>"`.

| Piece token | Meaning                       |
|-------------|-------------------------------|
| `WxH`       | one piece, rotatable          |
| `WxHxN`     | N identical pieces, rotatable |
| `WxHn`      | one piece, fixed orientation  |
| `WxHxNn`    | N pieces, fixed orientation   |
(pieces can be rotated by default, suffix `n` here means _no rotation_)

Example: `"3000x4000:835x620x4,1020x620x4n,1020x620x4,1490x620x2,1750x900"`

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
- **Kerf** — blade thickness subtracted from each internal cut; sheet boundary
  edges are exempt.

## Commands

```
cargo build
cargo test
cargo clippy -- -D warnings
cargo +nightly fmt
```

## Demos

Interactive visualizations (open in browser, no server needed):

| File | What it shows                                                                |
|------|------------------------------------------------------------------------------|
| [demos/ga_decoder.html](demos/ga_decoder.html) | Guillotine Decoder — genome → sheet placements step by step                  |
| [demos/ga_ox_cx_gsap.html](demos/ga_ox_cx_gsap.html) | GA Crossover — OX and CX operators animated                                  |
| [demos/ga_mutation_gsap.html](demos/ga_mutation_gsap.html) | GA Mutation — swap / flip / point-selector mutation animated                 |
| [demos/guillotine_generator.html](demos/guillotine_generator.html) | Guillotine Generator — random problem generation with known optimal solution |

## References

- [Сиразетдинова Татьяна Юрьевна, "Конструирование прямоугольного раскроя в системах автоматизированного проектирования с учетом дефектных областей материала"](docs/sirazetdinova_t_u.pdf)
- Genetic Algorithm in Java [2d-cutting-stock-problem-master](third-party/2d-cutting-stock-problem-master)
- Simple heuristic in Python [guillotine-cutting-master](third-party/guillotine-cutting-master)