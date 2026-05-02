use std::collections::BTreeMap;
use std::fmt::{self, Write};
use std::sync::mpsc;
use std::time::Instant;

use axum::{Form, Router, response::Html, routing::{get, post}};
use cutting::{
    ga::{ProgressEvent, ProgressLog, run_ga_mt},
    model::{Placement, Problem, Solution},
    parse::parse_problem,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SolveForm {
    problem: String,
    #[serde(default = "default_seeds")]
    seeds: usize,
    #[serde(default = "default_gens")]
    gens: usize,
    #[serde(default = "default_pop")]
    pop: usize,
}

fn default_seeds() -> usize { 8 }
fn default_gens() -> usize { 500 }
fn default_pop() -> usize { 200 }

const INDEX_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Cutting optimizer</title>
<script src="https://unpkg.com/htmx.org@2.0.4"></script>
<style>
  body { font-family: monospace; max-width: 960px; margin: 2em auto; padding: 0 1em; }
  label { display: block; margin-bottom: .4em; }
  input[type=text] { width: 100%; box-sizing: border-box; font-family: monospace; }
  input[type=number] { width: 6em; font-family: monospace; }
  .row { display: flex; gap: 1.5em; align-items: flex-end; margin-top: .8em; }
  button { padding: .3em 1.2em; }
  .htmx-indicator { display: none; color: #888; }
  .htmx-request .htmx-indicator { display: inline; }
  .error { color: red; }
  table { border-collapse: collapse; width: 100%; margin: 1em 0; }
  th, td { border: 1px solid #ccc; padding: 4px 8px; }
  th { background: #f0f0f0; text-align: center; }
  td { text-align: right; }
  td:last-child { text-align: left; }
  pre { background: #f8f8f8; padding: 1em; overflow-x: auto; }
  h3 { margin-bottom: .3em; }
</style>
</head>
<body>
<h2>2D Guillotine Cutting Optimizer</h2>
<form hx-post="/solve" hx-target="#results" hx-indicator="#spinner">
  <label>Problem string
    <input type="text" name="problem"
      value="2600x1800F:3:400x400-6,495x495-6,270x320-10,150x450-17r">
  </label>
  <div class="row">
    <label>Seeds <input type="number" name="seeds" value="8" min="1" max="64"></label>
    <label>Generations <input type="number" name="gens" value="500" min="10"></label>
    <label>Population <input type="number" name="pop" value="200" min="10"></label>
    <button type="submit">Solve</button>
    <span id="spinner" class="htmx-indicator">running…</span>
  </div>
</form>
<div id="results"></div>
</body>
</html>"##;

pub(crate) fn run_serve(port: u16) -> std::io::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let app = Router::new()
                .route("/", get(|| async { Html(INDEX_HTML) }))
                .route("/solve", post(solve_handler));
            let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{port}")).await?;
            println!("Listening on http://localhost:{port}");
            axum::serve(listener, app).await
        })
}

async fn solve_handler(Form(form): Form<SolveForm>) -> Html<String> {
    let result = tokio::task::spawn_blocking(move || {
        solve_to_html(&form.problem, form.seeds, form.gens, form.pop)
    })
    .await;
    match result {
        Ok(Ok(html)) => Html(html),
        Ok(Err(e)) => Html(format!("<p class='error'>Error: {e}</p>")),
        Err(_)     => Html("<p class='error'>Internal error: solver panicked</p>".to_string()),
    }
}

fn solve_to_html(problem_str: &str, n_seeds: usize, gens: usize, pop: usize) -> Result<String, fmt::Error> {
    let problem = match parse_problem(problem_str) {
        Ok(p) => p,
        Err(e) => return Ok(format!("<p class='error'>Parse error: {}</p>", he(&e.to_string()))),
    };
    let cfg = crate::ga_config(gens, pop, 5, 5);
    let seeds: Vec<u64> = (0..n_seeds.max(1) as u64).collect();

    let (tx, rx) = mpsc::channel::<ProgressEvent>();
    let t0 = Instant::now();
    let results = run_ga_mt(&problem, &cfg, &seeds, Some(ProgressLog { tx, progress_interval: 50, seed: 0 }));
    let elapsed = t0.elapsed().as_secs_f64();
    let events: Vec<ProgressEvent> = rx.try_iter().collect();

    let decoded = crate::decode_results(&problem, &results);
    let mut out = String::new();

    writeln!(out,
        "<p><b>Done in {elapsed:.1}s</b> — {} pieces on {}×{} sheet, kerf={}</p>",
        problem.pieces.len(), problem.sheet.width, problem.sheet.height, problem.kerf,
    )?;
    out.push_str("<table><thead><tr><th>seed</th><th>sheets</th><th>objective</th><th>last_n</th><th>last sheet</th></tr></thead><tbody>\n");
    for (seed, obj, sol, n, summary) in &decoded {
        writeln!(out, "<tr><td>{seed}</td><td>{}</td><td>{obj}</td><td>{n}</td><td>{}</td></tr>",
            sol.sheets_used(), he(summary))?;
    }
    out.push_str("</tbody></table>\n");

    let (best_seed, best_obj, best_sol, best_n, best_summary) = &decoded[0];
    writeln!(out,
        "<h3>Best — seed={best_seed}  obj={best_obj}  sheets={}  last={best_n}: {}</h3>",
        best_sol.sheets_used(), he(best_summary),
    )?;
    out.push_str("<pre>");
    out.push_str(&render_solution(&problem, best_sol)?);
    out.push_str("</pre>");

    if !events.is_empty() {
        out.push_str("<h3>Progress</h3>");
        out.push_str("<table><thead><tr><th>gen</th><th>best_obj</th><th>sheets</th><th>seed</th></tr></thead><tbody>\n");
        for evt in &events {
            writeln!(out, "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                     evt.generation, evt.objective, evt.sheets_used, evt.seed)?;
        }
        out.push_str("</tbody></table>\n");
    }

    Ok(out)
}

fn render_solution(problem: &Problem, sol: &Solution) -> Result<String, fmt::Error> {
    let mut out = String::new();
    let mut by_sheet: BTreeMap<usize, Vec<&Placement>> = BTreeMap::new();
    for pl in &sol.placements {
        by_sheet.entry(pl.sheet_idx).or_default().push(pl);
    }
    for (sheet_idx, mut pls) in by_sheet {
        writeln!(out, "Sheet {sheet_idx} ({}×{}):", problem.sheet.width, problem.sheet.height)?;
        pls.sort_by_key(|p| (p.y, p.x));
        for pl in pls {
            let p = &problem.pieces[pl.piece_idx];
            let (pw, ph) = if pl.rotated { (p.height, p.width) } else { (p.width, p.height) };
            writeln!(out, "  idx={:2}  {pw}×{ph}  at ({:4},{:4}){}",
                pl.piece_idx, pl.x, pl.y,
                if pl.rotated { "  [rot]" } else { "" })?;
        }
    }
    Ok(out)
}

fn he(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
