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

// "3000x4000" sheet, 7 mm kerf, 9 pieces
let problem = parse_problem("3000x4000:835x620x4,1020x620x4n,1750x900", 7).unwrap();

// Build a genome (one Gene per piece, in placement order)
let genome: Genome = problem.pieces.iter().enumerate()
    .map(|(i, _)| Gene { piece_idx: i, rotate: true, point_selector: 0 })
    .collect();

let solution = decode(&problem, &genome);
println!("{} sheet(s) used", solution.sheets_used());
for p in &solution.placements {
    println!("piece {} → sheet {} at ({}, {}){}", p.piece_id, p.sheet_idx, p.x, p.y,
             if p.rotated { " rotated" } else { "" });
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

## Concepts

- **Genome** — ordered list of `Gene` values, one per piece. Defines placement order, rotation preference, and which free rectangle to try first (`point_selector`). Suitable as the individual in a genetic algorithm.
- **Decoder** — deterministic: given a genome and a problem, produces a `Solution` via the Shorter Leftover Axis (SLAS) guillotine heuristic.
- **Kerf** — blade thickness subtracted from each internal cut; sheet boundary edges are exempt.

## Commands

```
cargo build
cargo test
cargo clippy -- -D warnings
cargo +nightly fmt
```

## References

- [Сиразетдинова Татьяна Юрьевна, "Конструирование прямоугольного раскроя в системах автоматизированного проектирования с учетом дефектных областей материала"](docs/sirazetdinova_t_u.pdf)