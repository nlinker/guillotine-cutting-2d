use std::sync::Arc;
/// Sweep all feasible widths for a fixed piece set, compute optimal placement via GLF
/// cut-tree reconstruction, and render each solution to tmp/{width}_opt.svg.
/// Also runs the GA for each width and renders the best found solution to
/// tmp/{width}_enc.svg.  index.html shows both side by side.
///
/// Run with:  cargo run --example glf_sweep --release
use std::{fs, path::Path};

use cut::{
    exact::glf::build_glf,
    expand::{expand_problem, shrink_solution},
    ga::GaConfig,
    model::{ProblemSpec, Sheet, Solution},
    parser::compact::parse_problem,
    render::render_svg,
    slas::{decoder::decode_spec, ga as slas_ga},
};

const SPEC_STR: &str = "1x1F:: 12x3/2, 3x12/2, 8x4/4r, 7x5/4r, 6x4/4r";

fn main() {
    let base_spec = parse_problem(SPEC_STR).expect("parse error");
    let base_problem = expand_problem(&base_spec);
    let glf = build_glf(&base_problem);
    // println!("{}", glf.render(15));

    let (min_w, max_w) = glf.feasible_width_range().expect("problem is infeasible");

    let out_dir = Path::new("tmp/glf");
    fs::create_dir_all(out_dir).expect("failed to create tmp/glf directory");

    let ga_cfg = Arc::new(GaConfig { n_elite: 5, tournament_k: 5, ..GaConfig::default() });

    let mut written = 0u32;
    let mut entries: Vec<(u32, u32)> = Vec::new(); // (width, height)

    for width in min_w..=max_w {
        let Some(height) = glf.eval_full_set(width) else {
            continue;
        };
        let Some(placements) = glf.reconstruct_flat(width) else {
            continue;
        };

        let spec = Arc::new(ProblemSpec { sheet: Sheet { width, height }, ..base_spec.clone() });
        let sol = shrink_solution(&Solution { placements, leftovers: vec![] }, &spec);

        // GLF exact solution
        let svg = render_svg(&spec, &sol).expect("render failed");
        fs::write(out_dir.join(format!("{width}_opt.svg")), &svg).expect("write failed");

        // GA solution
        let seeds = vec![0u64];
        let handle = slas_ga::run_ga_mt(Arc::clone(&spec), Arc::clone(&ga_cfg), seeds, 0, 0);
        let results = handle.blocking_wait();
        let (_, best) = results.first().expect("no GA result");
        let sol_ga = decode_spec(&spec, &best.genome);
        let svg_ga = render_svg(&spec, &sol_ga).expect("render ga failed");
        fs::write(out_dir.join(format!("{width}_enc.svg")), &svg_ga).expect("write ga failed");

        println!(
            "width={width:3}  height={height:3}  area={:5}  glf_sheets={}  ga_sheets={}",
            width * height,
            sol.sheets_used(),
            sol_ga.sheets_used(),
        );
        entries.push((width, height));
        written += 1;
    }

    let html = build_index_html(&entries);
    fs::write(out_dir.join("index.html"), &html).expect("write index.html failed");
    println!("\n{written} pairs written to {}/", out_dir.display());
    println!("index.html written to {}/\n", out_dir.display());
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
<title>GLF sweep</title>
<style>
  *, *::before, *::after {{ box-sizing: border-box; }}
  html, body {{ margin: 0; height: 100%; }}
  body {{ background: #1e1e1e; display: flex; flex-direction: column;
          align-items: center; padding: 12px; gap: 10px;
          font-family: sans-serif; color: #ddd; }}
  #controls {{ display: flex; align-items: center; gap: 14px; flex-wrap: wrap;
               justify-content: center; flex-shrink: 0; }}
  #info {{ font-size: 1.05em; letter-spacing: .04em; flex-shrink: 0; }}
  #frames {{ display: flex; gap: 16px; flex: 1 1 0; min-height: 0;
             max-width: 100%; width: 100%; }}
  .col {{ flex: 1 1 0; min-width: 0; display: flex; flex-direction: column;
          align-items: center; gap: 4px; }}
  .col-label {{ font-size: .85em; color: #aaa; flex-shrink: 0; }}
  .frame {{ flex: 1 1 0; min-height: 0; max-width: 100%;
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
  <label>Interval&nbsp;<input type="range" id="speed" min="100" max="3000" step="100" value="1000">
    <span id="speed-val">1000</span>&nbsp;ms</label>
</div>
<div id="info">—</div>
<div id="frames">
  <div class="col">
    <span class="col-label">GLF (exact)</span>
    <img id="frame-glf" class="frame" alt="GLF">
  </div>
  <div class="col">
    <span class="col-label">GA</span>
    <img id="frame-enc" class="frame" alt="GA">
  </div>
</div>
<script>
const FILES = [{files_js}];
let idx = 0, interval = 1000, timer = null, paused = true;
const imgGlf = document.getElementById('frame-glf');
const imgEnc = document.getElementById('frame-enc');
const info   = document.getElementById('info');
const btn    = document.getElementById('pause');
const spIn   = document.getElementById('speed');
const spVal  = document.getElementById('speed-val');

function show(i) {{
  const [w, h] = FILES[i];
  imgGlf.src = w + '_opt.svg';
  imgEnc.src = w + '_enc.svg';
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

btn.textContent = 'Resume ';
const k0 = document.createElement('kbd'); k0.textContent = 'Space'; btn.appendChild(k0);
show(0);
</script>
</body>
</html>
"#,
        files_js = files_js
    )
}
