use std::{convert::Infallible, sync::Arc, time::Instant};

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
    ga::{GaEvent, ga_channel, run_ga_mt_bg},
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

const INDEX_HTML: &str = include_str!("index.html");

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
                    let (best_seed, best_ind) = &results[0];
                    let best_sol = decode(&problem, &best_ind.genome);
                    let data = json!({
                        "elapsed":   elapsed,
                        "seed":      best_seed,
                        "objective": best_ind.objective,
                        "sheets":    best_sol.sheets_used(),
                        "solution":  best_sol,
                        "pieces":    problem.pieces,
                        "sheet_w":   problem.sheet.width,
                        "sheet_h":   problem.sheet.height,
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
