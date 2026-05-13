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
    let mut entries: Vec<(u32, u32)> = Vec::new(); // (width, height)
    for width in min_w..=max_w {
        let Some(height) = gpf.eval_full_set(width) else {
            continue;
        };
        let Some(sol) = gpf.reconstruct(width) else { continue };

        let spec = ProblemSpec {
            sheet: Sheet { width, height },
            ..base_spec.clone()
        };
        let svg = render_svg(&spec, &sol).expect("render failed");
        let path = out_dir.join(format!("{width}.svg"));
        fs::write(&path, &svg).expect("write failed");
        println!(
            "width={width:3}  height={height:3}  area={:5}  pieces={}",
            width * height,
            sol.placements.len()
        );
        entries.push((width, height));
        written += 1;
    }

    let html = build_index_html(&entries);
    fs::write(out_dir.join("index.html"), &html).expect("write index.html failed");
    println!("\n{written} SVGs written to {}/", out_dir.display());
}

fn build_index_html(entries: &[(u32, u32)]) -> String {
    let files_js = entries
        .iter()
        .map(|(w, h)| format!("[{w},{h}]"))
        .collect::<Vec<_>>()
        .join(",");

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>GPF sweep</title>
<style>
  *, *::before, *::after {{ box-sizing: border-box; }}
  html, body {{ margin: 0; height: 100%; }}
  body {{ background: #1e1e1e; display: flex; flex-direction: column;
          align-items: center; padding: 12px; gap: 10px;
          font-family: sans-serif; color: #ddd; }}
  #controls {{ display: flex; align-items: center; gap: 14px; flex-wrap: wrap;
               justify-content: center; flex-shrink: 0; }}
  #info {{ font-size: 1.05em; letter-spacing: .04em; flex-shrink: 0; }}
  #frame {{ flex: 1 1 0; min-height: 0; max-width: 100%;
            object-fit: contain; background: #fff; border-radius: 6px; }}
  button {{ padding: 4px 14px; border-radius: 4px; border: none; background: #444;
            color: #eee; cursor: pointer; font-size: .95em; }}
  button:hover {{ background: #666; }}
  input[type=range] {{ width: 150px; cursor: pointer; }}
  label {{ display: flex; align-items: center; gap: 6px; }}
  kbd {{ background: #333; border: 1px solid #555; border-radius: 3px;
         padding: 1px 5px; font-size: .8em; color: #aaa; }}
</style>
</head>
<body>
<div id="controls">
  <button id="prev">&#8592; <kbd>&#8592;</kbd></button>
  <button id="pause">Pause <kbd>Space</kbd></button>
  <button id="next">&#8594; <kbd>&#8594;</kbd></button>
  <label>Interval&nbsp;<input type="range" id="speed" min="100" max="3000" step="100" value="500">
    <span id="speed-val">500</span>&nbsp;ms</label>
</div>
<div id="info">—</div>
<img id="frame" alt="layout">
<script>
const FILES = [{files_js}];
let idx = 0, interval = 500, timer = null, paused = false;
const img   = document.getElementById('frame');
const info  = document.getElementById('info');
const btn   = document.getElementById('pause');
const spIn  = document.getElementById('speed');
const spVal = document.getElementById('speed-val');

function show(i) {{
  const [w, h] = FILES[i];
  img.src = w + '.svg';
  info.textContent = `width=${{w}}  height=${{h}}  area=${{w*h}}  (${{i+1}}/${{FILES.length}})`;
}}
function stepPrev() {{ idx = (idx - 1 + FILES.length) % FILES.length; show(idx); }}
function stepNext() {{ idx = (idx + 1) % FILES.length; show(idx); }}
function advance()  {{ stepNext(); }}
function startTimer() {{ timer = setInterval(advance, interval); }}
function stopTimer()  {{ clearInterval(timer); timer = null; }}
function togglePause() {{
  paused = !paused; btn.textContent = (paused ? 'Resume' : 'Pause') + ' ';
  const k = document.createElement('kbd'); k.textContent = 'Space'; btn.appendChild(k);
  paused ? stopTimer() : startTimer();
}}

spIn.addEventListener('input', () => {{
  interval = +spIn.value; spVal.textContent = interval;
  if (!paused) {{ stopTimer(); startTimer(); }}
}});
btn.addEventListener('click', togglePause);
document.getElementById('prev').addEventListener('click', stepPrev);
document.getElementById('next').addEventListener('click', stepNext);

document.addEventListener('keydown', e => {{
  if (e.target.tagName === 'INPUT') return;
  if (e.key === 'ArrowLeft')  {{ stepPrev(); e.preventDefault(); }}
  if (e.key === 'ArrowRight') {{ stepNext(); e.preventDefault(); }}
  if (e.key === ' ')          {{ togglePause(); e.preventDefault(); }}
}});

show(0);
startTimer();
</script>
</body>
</html>
"#,
        files_js = files_js
    )
}
