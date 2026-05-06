use std::{convert::Infallible, fmt::Write as FmtWrite, sync::Arc, time::Instant};

use axum::{
    Router,
    extract::Query,
    response::{
        Html,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use cutting::{
    decoder::decode,
    ga::{GaEvent, Individual, ga_channel, run_ga_mt_bg},
    model::{Piece, PieceSpec, Problem, Sheet},
};
use futures_util::{Stream, stream};
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct SolveParams {
    sheet_w:  u32,
    sheet_h:  u32,
    kerf:     u32,
    pieces:   String,
    #[serde(default = "default_seed")]
    seed: u64,
    #[serde(default = "default_threads")]
    threads: usize,
    #[serde(default = "default_gens")]
    gens: usize,
    #[serde(default = "default_pop")]
    pop: usize,
    #[serde(default = "default_progress")]
    progress: usize,
}

fn default_seed() -> u64 { 42 }
fn default_threads() -> usize { 8 }
fn default_gens() -> usize { 500 }
fn default_pop() -> usize { 200 }
fn default_progress() -> usize { 50 }

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Cutting optimizer</title>
<style>
  body { font-family: monospace; max-width: 1000px; margin: 2em auto; padding: 0 1em; }
  label { display: inline-flex; align-items: center; gap: .3em; }
  input[type=text], input[type=number] { font-family: monospace; }
  input[type=number] { width: 5.5em; }
  .row { display: flex; gap: 1.2em; align-items: center; margin: .6em 0; flex-wrap: wrap; }
  button { padding: .25em 1em; cursor: pointer; font-family: monospace; }
  #cancel { display: none; }
  #status { color: #888; }
  .error { color: red; }

  /* pieces input table */
  #pieces-table { border-collapse: collapse; margin: .5em 0; }
  #pieces-table th, #pieces-table td { border: 1px solid #ccc; padding: 2px 5px; }
  #pieces-table th { background: #f0f0f0; text-align: center; font-size: .85em; }
  #pieces-table td { text-align: center; }
  #pieces-table input[type=text]   { width: 9em; }
  #pieces-table input[type=number] { width: 4.5em; }
  .rm-btn { background: none; border: none; color: #c00; cursor: pointer; font-size: 1em; padding: 0 .2em; }

  /* results table (from server) */
  #results table { border-collapse: collapse; width: 100%; margin: .8em 0; }
  #results th, #results td { border: 1px solid #ccc; padding: 4px 8px; }
  #results th { background: #f0f0f0; text-align: center; }
  #results td { text-align: right; }
  #results td:last-child { text-align: left; }

  pre { background: #f8f8f8; padding: 1em; overflow-x: auto; }
  h3 { margin: .6em 0 .2em; }
  #chart-wrap { margin: .4em 0; display: none; }
  #layout-wrap { margin: .8em 0; display: none; }
  #layout { width: 100%; display: block; }
</style>
</head>
<body>
<h2>2D Guillotine Cutting Optimizer</h2>

<form id="form">
  <div class="row">
    <label>Sheet W <input type="number" id="sheet-w" value="3050" min="1"></label>
    <label>× H <input type="number" id="sheet-h" value="1300" min="1"></label>
    <label>Kerf <input type="number" id="kerf" value="7" min="0"></label>
  </div>

  <table id="pieces-table">
    <thead><tr>
      <th>Name</th><th>W</th><th>H</th><th>Count</th><th>Rotate</th><th></th>
    </tr></thead>
    <tbody id="pieces-tbody"></tbody>
  </table>
  <div class="row">
    <button type="button" id="add-row">+ Add row</button>
  </div>

  <div class="row">
    <label>Seed <input type="number" id="seed" value="42" min="0"></label>
    <label><input type="checkbox" id="random-seed"> random</label>
    <label>Threads <input type="number" id="threads" value="8" min="1" max="64"></label>
    <label>Gens <input type="number" id="gens" value="500" min="10"></label>
    <label>Pop <input type="number" id="pop" value="200" min="10"></label>
    <label>Progress <input type="number" id="progress" value="50" min="1"></label>
    <button type="submit">Solve</button>
    <button type="button" id="cancel">Cancel</button>
    <span id="status"></span>
  </div>
</form>

<div id="chart-wrap">
  <canvas id="chart" height="60"></canvas>
</div>
<div id="layout-wrap">
  <h3>Layout</h3>
  <canvas id="layout"></canvas>
</div>
<div id="results"></div>

<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4/dist/chart.umd.min.js"></script>
<script>
const PALETTE = ['#ffb6c1','#add8e6','#90ee90','#ffff99','#ffc87a',
                 '#dda0dd','#87ceeb','#f08080','#b4ffb4','#ffd8a8','#c8c8ff','#fff0b4'];

// ── default data ────────────────────────────────────────────────────────────
const DEFAULT_PIECES = [
  {name:'стойка ←',    w:2384, h:600, count:1, rotate:false},
  {name:'стойка →',    w:2384, h:600, count:1, rotate:false},
  {name:'стойка →←',  w:2288, h:500, count:1, rotate:false},
  {name:'низ ↓',       w:1768, h:600, count:1, rotate:false},
  {name:'цоколь ↓',   w:1768, h:80,  count:2, rotate:false},
  {name:'крыша ↑',    w:1800, h:710, count:1, rotate:false},
  {name:'полки ↔',    w:550,  h:490, count:5, rotate:false},
  {name:'полки ↔',    w:1202, h:490, count:2, rotate:false},
  {name:'ребро ↕',    w:1202, h:300, count:1, rotate:false},
  {name:'распорка ↔', w:300,  h:490, count:1, rotate:false},
  {name:'перегородка', w:150,  h:490, count:2, rotate:true},
];

// ── pieces table ─────────────────────────────────────────────────────────────
function addRow(p) {
  p = p || {name:'', w:'', h:'', count:1, rotate:false};
  const tbody = document.getElementById('pieces-tbody');
  const tr = document.createElement('tr');
  tr.innerHTML =
    `<td><input type="text"   value="${p.name}"></td>` +
    `<td><input type="number" value="${p.w}" min="1"></td>` +
    `<td><input type="number" value="${p.h}" min="1"></td>` +
    `<td><input type="number" value="${p.count}" min="1"></td>` +
    `<td><input type="checkbox"${p.rotate ? ' checked' : ''}></td>` +
    `<td><button class="rm-btn" type="button">×</button></td>`;
  tr.querySelector('.rm-btn').addEventListener('click', () => tr.remove());
  tbody.appendChild(tr);
}

function serializePieces() {
  return [...document.querySelectorAll('#pieces-tbody tr')].map(tr => {
    const inp = tr.querySelectorAll('input');
    return {
      name:       inp[0].value,
      width:      +inp[1].value,
      height:     +inp[2].value,
      count:      +inp[3].value,
      can_rotate:  inp[4].checked,
    };
  });
}

DEFAULT_PIECES.forEach(addRow);
document.getElementById('add-row').addEventListener('click', () => addRow());

// ── chart ─────────────────────────────────────────────────────────────────────
let es = null;
let chart = null;

function initChart() {
  const canvas = document.getElementById('chart');
  if (chart) { chart.destroy(); }
  chart = new Chart(canvas, {
    type: 'line',
    data: {
      labels: [],
      datasets: [{
        label: 'objective',
        data: [],
        borderColor: '#3b82f6',
        backgroundColor: 'rgba(59,130,246,0.08)',
        borderWidth: 1.5,
        pointRadius: 0,
        tension: 0.1,
        fill: true,
      }]
    },
    options: {
      animation: false,
      responsive: true,
      plugins: { legend: { display: false } },
      scales: {
        x: { title: { display: true, text: 'generation' } },
        y: { title: { display: true, text: 'objective' } }
      }
    }
  });
  document.getElementById('chart-wrap').style.display = 'block';
}

function addPoint(generation, objective) {
  chart.data.labels.push(generation);
  chart.data.datasets[0].data.push(objective);
  chart.update('none');
}

// ── color helpers ─────────────────────────────────────────────────────────────
function buildRowColors(pieces) {
  const rowColor = [];
  const seen = new Map();
  let ci = 0;
  for (let i = 0; i < pieces.length; i++) {
    const key = pieces[i].name || null;
    if (key && seen.has(key)) {
      rowColor.push(PALETTE[seen.get(key) % PALETTE.length]);
    } else {
      rowColor.push(PALETTE[ci % PALETTE.length]);
      if (key) seen.set(key, ci);
      ci++;
    }
  }
  return rowColor;
}

// ── layout canvas ─────────────────────────────────────────────────────────────
function drawLayout(solution, pieces, sheetW, sheetH) {
  const canvas = document.getElementById('layout');
  const placements = solution.placements;
  if (!placements || placements.length === 0) return;

  let nSheets = 0;
  for (const pl of placements) { if (pl.sheet_idx >= nSheets) nSheets = pl.sheet_idx + 1; }

  const cw = 900, GAP = 8;
  const sdw = (cw - GAP * (nSheets - 1)) / nSheets;
  const scale = sdw / sheetW;
  const sdh = Math.round(sheetH * scale);

  canvas.width  = cw;
  canvas.height = sdh + 22;
  const ctx = canvas.getContext('2d');
  ctx.clearRect(0, 0, canvas.width, canvas.height);

  for (let i = 0; i < nSheets; i++) {
    const ox = i * (sdw + GAP);
    ctx.fillStyle = '#f0f0f0';
    ctx.fillRect(ox, 0, sdw, sdh);
    ctx.strokeStyle = '#333';
    ctx.lineWidth = 1.5;
    ctx.strokeRect(ox, 0, sdw, sdh);
    ctx.fillStyle = '#555';
    ctx.font = '11px monospace';
    ctx.fillText('Sheet ' + i, ox + 4, sdh + 15);
  }

  const rowColor = buildRowColors(pieces);

  for (const pl of placements) {
    const pc = pieces[pl.piece_idx];
    const pw = pl.rotated ? pc.height : pc.width;
    const ph = pl.rotated ? pc.width  : pc.height;
    const ox = pl.sheet_idx * (sdw + GAP);
    const rx = ox + pl.x * scale, ry = pl.y * scale;
    const rw = Math.max(pw * scale, 1), rh = Math.max(ph * scale, 1);
    ctx.fillStyle = rowColor[pl.piece_idx];
    ctx.fillRect(rx, ry, rw, rh);
    ctx.strokeStyle = '#555';
    ctx.lineWidth = 0.5;
    ctx.strokeRect(rx, ry, rw, rh);
    if (rw > 30 && rh > 14) {
      ctx.fillStyle = '#000';
      ctx.font = '9px monospace';
      ctx.fillText(pc.name || ('#' + pl.piece_idx), rx + 2, ry + 11);
    }
  }
}

// ── form submit ───────────────────────────────────────────────────────────────
const seedInput      = document.getElementById('seed');
const randomSeedChk  = document.getElementById('random-seed');

document.getElementById('form').addEventListener('submit', (e) => {
  e.preventDefault();
  if (es) { es.close(); es = null; }
  document.getElementById('results').innerHTML = '';
  document.getElementById('layout-wrap').style.display = 'none';
  document.getElementById('status').textContent = 'running…';
  document.getElementById('cancel').style.display = 'inline';
  initChart();

  if (randomSeedChk.checked) {
    seedInput.value = Math.floor(Math.random() * 10000);
  }

  const params = new URLSearchParams();
  params.set('sheet_w',  document.getElementById('sheet-w').value);
  params.set('sheet_h',  document.getElementById('sheet-h').value);
  params.set('kerf',     document.getElementById('kerf').value);
  params.set('pieces',   JSON.stringify(serializePieces()));
  params.set('seed',     seedInput.value);
  params.set('threads',  document.getElementById('threads').value);
  params.set('gens',     document.getElementById('gens').value);
  params.set('pop',      document.getElementById('pop').value);
  params.set('progress', document.getElementById('progress').value);

  es = new EventSource('/stream?' + params);

  es.addEventListener('progress', (ev) => {
    const d = JSON.parse(ev.data);
    addPoint(d.generation, d.objective);
  });

  es.addEventListener('done', (ev) => {
    const d = JSON.parse(ev.data);
    if (d.solution) {
      drawLayout(d.solution, d.pieces, d.sheet_w, d.sheet_h);
      document.getElementById('layout-wrap').style.display = 'block';
    }
    document.getElementById('results').innerHTML = d.html;
    es.close(); es = null;
    document.getElementById('status').textContent = '';
    document.getElementById('cancel').style.display = 'none';
  });

  es.addEventListener('error', (ev) => {
    if (ev.data) document.getElementById('results').innerHTML =
      '<p class="error">' + ev.data + '</p>';
    es.close(); es = null;
    document.getElementById('status').textContent = '';
    document.getElementById('cancel').style.display = 'none';
  });

  es.onerror = () => {
    document.getElementById('status').textContent = 'connection error';
    document.getElementById('cancel').style.display = 'none';
    es = null;
  };
});

document.getElementById('cancel').addEventListener('click', () => {
  if (es) { es.close(); es = null; }
  document.getElementById('status').textContent = 'cancelled';
  document.getElementById('cancel').style.display = 'none';
});
</script>
</body>
</html>"##;

pub(crate) fn run_serve(port: u16) -> std::io::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let app = Router::new()
                .route("/", get(|| async { Html(INDEX_HTML) }))
                .route("/stream", get(stream_handler));
            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
            println!("Listening on http://localhost:{port}");
            axum::serve(listener, app).await
        })
}

enum StreamState {
    Running {
        handle: cutting::ga::GaHandle,
        problem: Arc<Problem>,
        start: Instant,
    },
    Finished,
}

async fn stream_handler(Query(params): Query<SolveParams>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let initial = build_problem(&params)
        .map(|problem| {
            let problem = Arc::new(problem);
            let cfg = Arc::new(crate::ga_config(params.gens, params.pop, 5, 5));
            let mut rng = Xoshiro256StarStar::seed_from_u64(params.seed);
            let seeds: Vec<u64> = (0..params.threads.max(1)).map(|_| rng.next_u64()).collect();
            let (handle, ctx) = ga_channel(params.progress);
            run_ga_mt_bg(Arc::clone(&problem), cfg, seeds, ctx);
            StreamState::Running {
                handle,
                problem,
                start: Instant::now(),
            }
        });

    let stream = stream::unfold(initial, |state| async move {
        match state {
            Err(msg) => {
                let evt = Event::default().event("error").data(msg);
                Some((Ok(evt), Err(String::new())))
            }
            Ok(StreamState::Finished) => None,
            Ok(StreamState::Running {
                mut handle,
                problem,
                start,
            }) => match handle.rx.recv().await {
                None => None,
                Some(GaEvent::Progress(p)) => {
                    let data = json!({
                        "generation": p.generation,
                        "objective":  p.objective,
                        "sheets":     p.sheets_used,
                        "seed":       p.seed,
                    })
                    .to_string();
                    let evt = Event::default().event("progress").data(data);
                    Some((Ok(evt), Ok(StreamState::Running { handle, problem, start })))
                }
                Some(GaEvent::Done(results)) => {
                    let elapsed = start.elapsed().as_secs_f64();
                    let (_, best_ind) = &results[0];
                    let best_sol = decode(&problem, &best_ind.genome);
                    let html = results_html(&problem, &results, elapsed);
                    let data = json!({
                        "html":    html,
                        "solution": best_sol,
                        "pieces":   problem.pieces,
                        "sheet_w":  problem.sheet.width,
                        "sheet_h":  problem.sheet.height,
                    })
                    .to_string();
                    let evt = Event::default().event("done").data(data);
                    Some((Ok(evt), Ok(StreamState::Finished)))
                }
            },
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn build_problem(params: &SolveParams) -> Result<Problem, String> {
    let specs: Vec<PieceSpec> = serde_json::from_str(&params.pieces)
        .map_err(|e| format!("invalid pieces JSON: {e}"))?;
    if specs.is_empty() {
        return Err("no pieces specified".into());
    }
    let pieces: Vec<Piece> = specs
        .iter()
        .flat_map(|ps| {
            (0..ps.count).map(|_| Piece {
                name:       ps.name.clone(),
                width:      ps.width,
                height:     ps.height,
                can_rotate: ps.can_rotate,
            })
        })
        .collect();
    Ok(Problem {
        sheet: Sheet { width: params.sheet_w, height: params.sheet_h },
        kerf: params.kerf,
        pieces,
    })
}

fn results_html(problem: &Problem, results: &[(u64, Individual)], elapsed: f64) -> String {
    let decoded = crate::decode_results(problem, results);
    let mut out = String::new();
    let _ = writeln!(
        out,
        "<p><b>Done in {elapsed:.1}s</b> — {} pieces on {}×{} sheet, kerf={}</p>",
        problem.pieces.len(),
        problem.sheet.width,
        problem.sheet.height,
        problem.kerf,
    );
    out.push_str("<table><thead><tr><th>seed</th><th>sheets</th><th>objective</th><th>last_n</th><th>last sheet</th></tr></thead><tbody>\n");
    for (seed, obj, sol, n, summary) in &decoded {
        let _ = writeln!(
            out,
            "<tr><td>{seed}</td><td>{}</td><td>{obj}</td><td>{n}</td><td>{}</td></tr>",
            sol.sheets_used(),
            he(summary)
        );
    }
    out.push_str("</tbody></table>\n");

    let (best_seed, best_obj, best_sol, best_n, best_summary) = &decoded[0];
    let _ = writeln!(
        out,
        "<p>Best — seed={best_seed}  obj={best_obj}  sheets={}  last={best_n}: {}</p>",
        best_sol.sheets_used(),
        he(best_summary),
    );
    out
}

fn he(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
