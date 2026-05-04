use std::convert::Infallible;
use std::fmt::Write as FmtWrite;
use std::sync::Arc;
use std::time::Instant;

use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;

use axum::extract::Query;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::{Router, response::Html, routing::get};
use cutting::{
    ga::{GaEvent, Individual, ga_channel, run_ga_mt_bg},
    model::Problem,
    parse::parse_problem,
};
use futures_util::{Stream, stream};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct SolveParams {
    problem: String,
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
  body { font-family: monospace; max-width: 960px; margin: 2em auto; padding: 0 1em; }
  label { display: block; margin-bottom: .4em; }
  input[type=text] { width: 100%; box-sizing: border-box; font-family: monospace; }
  input[type=number] { width: 6em; font-family: monospace; }
  .row { display: flex; gap: 1.5em; align-items: flex-end; margin-top: .8em; flex-wrap: wrap; }
  button { padding: .3em 1.2em; cursor: pointer; }
  #cancel { display: none; }
  #status { color: #888; }
  .error { color: red; }
  table { border-collapse: collapse; width: 100%; margin: 1em 0; }
  th, td { border: 1px solid #ccc; padding: 4px 8px; }
  th { background: #f0f0f0; text-align: center; }
  td { text-align: right; }
  td:last-child { text-align: left; }
  pre { background: #f8f8f8; padding: 1em; overflow-x: auto; }
  h3 { margin-bottom: .3em; }
  #chart-wrap { margin: 1em 0; display: none; }
</style>
</head>
<body>
<h2>2D Guillotine Cutting Optimizer</h2>
<form id="form">
  <label>Problem string
    <input type="text" name="problem"
      value="200x160F:1:22x26-4,32x20-7,35x20-2,42x21-5,46x26r,67x34-3,75x42-2,76x22-4,83x32-4r,83x82,93x31,106x31,124x26-5,130x22-6,157x31-3,164x21-2,177x31">
  </label>
  <div class="row">
    <label>Seed    <input type="number" name="seed"     value="42"  min="0"></label>
    <label>Threads <input type="number" name="threads"  value="8"   min="1" max="64"></label>
    <label>Gens    <input type="number" name="gens"     value="500" min="10"></label>
    <label>Pop     <input type="number" name="pop"      value="200" min="10"></label>
    <label>Progress<input type="number" name="progress" value="50"  min="1"></label>
    <button type="submit">Solve</button>
    <button type="button" id="cancel">Cancel</button>
    <span id="status"></span>
  </div>
</form>

<div id="chart-wrap">
  <canvas id="chart" height="120"></canvas>
</div>
<div id="results"></div>

<script src="https://cdn.jsdelivr.net/npm/chart.js@4.4/dist/chart.umd.min.js"></script>
<script>
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
        label: 'global best objective',
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

document.getElementById('form').addEventListener('submit', (e) => {
  e.preventDefault();
  if (es) { es.close(); es = null; }
  document.getElementById('results').innerHTML = '';
  document.getElementById('status').textContent = 'running…';
  document.getElementById('cancel').style.display = 'inline';
  initChart();

  const params = new URLSearchParams(new FormData(e.target));
  es = new EventSource('/stream?' + params);

  es.addEventListener('progress', (ev) => {
    const d = JSON.parse(ev.data);
    addPoint(d.generation, d.objective);
  });

  es.addEventListener('done', (ev) => {
    const d = JSON.parse(ev.data);
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
    Running { handle: cutting::ga::GaHandle, problem: Arc<Problem>, start: Instant },
    Finished,
}

async fn stream_handler(
    Query(params): Query<SolveParams>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let initial = match parse_problem(&params.problem) {
        Err(e) => Err(e.to_string()),
        Ok(problem) => {
            let problem = Arc::new(problem);
            let cfg = Arc::new(crate::ga_config(params.gens, params.pop, 5, 5));
            let mut rng = Xoshiro256StarStar::seed_from_u64(params.seed);
            let seeds: Vec<u64> = (0..params.threads.max(1)).map(|_| rng.next_u64()).collect();
            let (handle, ctx) = ga_channel(params.progress);
            run_ga_mt_bg(Arc::clone(&problem), cfg, seeds, ctx);
            Ok(StreamState::Running { handle, problem, start: Instant::now() })
        }
    };

    let stream = stream::unfold(initial, |state| async move {
        match state {
            Err(msg) => {
                let evt = Event::default().event("error").data(msg);
                Some((Ok(evt), Err(String::new())))
            }
            Ok(StreamState::Finished) => None,
            Ok(StreamState::Running { mut handle, problem, start }) => {
                match handle.rx.recv().await {
                    None => None,
                    Some(GaEvent::Progress(p)) => {
                        let data = json!({
                            "generation": p.generation,
                            "objective":  p.objective,
                            "sheets":     p.sheets_used,
                            "seed":       p.seed,
                        }).to_string();
                        let evt = Event::default().event("progress").data(data);
                        Some((Ok(evt), Ok(StreamState::Running { handle, problem, start })))
                    }
                    Some(GaEvent::Done(results)) => {
                        let elapsed = start.elapsed().as_secs_f64();
                        let html = results_html(&problem, &results, elapsed);
                        let data = json!({ "html": html }).to_string();
                        let evt = Event::default().event("done").data(data);
                        Some((Ok(evt), Ok(StreamState::Finished)))
                    }
                }
            }
        }
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn results_html(problem: &Problem, results: &[(u64, Individual)], elapsed: f64) -> String {
    let decoded = crate::decode_results(problem, results);
    let mut out = String::new();
    let _ = writeln!(out,
        "<p><b>Done in {elapsed:.1}s</b> — {} pieces on {}×{} sheet, kerf={}</p>",
        problem.pieces.len(), problem.sheet.width, problem.sheet.height, problem.kerf,
    );
    out.push_str("<table><thead><tr><th>seed</th><th>sheets</th><th>objective</th><th>last_n</th><th>last sheet</th></tr></thead><tbody>\n");
    for (seed, obj, sol, n, summary) in &decoded {
        let _ = writeln!(out,
            "<tr><td>{seed}</td><td>{}</td><td>{obj}</td><td>{n}</td><td>{}</td></tr>",
            sol.sheets_used(), he(summary));
    }
    out.push_str("</tbody></table>\n");

    let (best_seed, best_obj, best_sol, best_n, best_summary) = &decoded[0];
    let _ = writeln!(out,
        "<p>Best — seed={best_seed}  obj={best_obj}  sheets={}  last={best_n}: {}</p>",
        best_sol.sheets_used(), he(best_summary),
    );
    out
}

fn he(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
