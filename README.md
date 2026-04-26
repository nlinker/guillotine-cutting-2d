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
(suffix `n` here means _no rotation_)

Example: `"3000x4000:835x620x4,1020x620x4n,1020x620x4,1490x620x2,1750x900"`

## Concepts

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

## References

- [Сиразетдинова Татьяна Юрьевна, "Конструирование прямоугольного раскроя в системах автоматизированного проектирования с учетом дефектных областей материала"](docs/sirazetdinova_t_u.pdf)
- Genetic Algorithm in Java [2d-cutting-stock-problem-master](third-party/2d-cutting-stock-problem-master)
- Simple heuristic in Python [guillotine-cutting-master](third-party/guillotine-cutting-master)