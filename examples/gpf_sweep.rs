/// Sweep all feasible widths for a fixed piece set, compute optimal placement via GPF
/// cut-tree reconstruction, and render each solution to tmp/{width}.svg.
///
/// Run with:  cargo run --example gpf_sweep --release
use std::{fs, path::Path};

use cutting::{
    gpf::build_gpf,
    model::{ProblemSpec, Sheet},
    parse::parse_problem,
    render::render_svg,
};

const SPEC_STR: &str = "1x1F:0:7x5/4,6x4/4,4x6/4,5x7/4";

fn main() {
    let base_spec = parse_problem(SPEC_STR).expect("parse error");
    let gpf = build_gpf(&base_spec);

    let min_w: u32 = base_spec.pieces.iter().map(|p| p.width.min(p.height)).min().unwrap();
    let max_w: u32 = base_spec.pieces.iter().map(|p| p.width * p.count).sum();

    let out_dir = Path::new("tmp");
    fs::create_dir_all(out_dir).expect("failed to create tmp/");

    let mut written = 0u32;
    for width in min_w..=max_w {
        let Some(height) = gpf.eval_full_set(width) else { continue };
        let Some(sol) = gpf.reconstruct(width) else { continue };

        let spec = ProblemSpec { sheet: Sheet { width, height }, ..base_spec.clone() };
        let svg = render_svg(&spec, &sol);
        let path = out_dir.join(format!("{width}.svg"));
        fs::write(&path, &svg).expect("write failed");
        println!("width={width:3}  height={height:3}  area={:5}  pieces={}", width * height, sol.placements.len());
        written += 1;
    }

    println!("\n{written} SVGs written to {}/", out_dir.display());
}
