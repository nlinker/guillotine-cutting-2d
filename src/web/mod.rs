mod sse;

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
use cutting::model::{Piece, PieceSpec, Problem, Sheet};
use futures_util::{Stream, stream};
use rand::{Rng, SeedableRng};
use rand_xoshiro::Xoshiro256StarStar;
use serde::Deserialize;
use sse::SseSink;

#[derive(Deserialize)]
struct SolveParams {
    sheet_w: u32,
    sheet_h: u32,
    kerf: u32,
    pieces: String,
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

fn default_seed() -> u64 {
    42
}
fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(8, |p| p.get())
}
fn default_gens() -> usize {
    1000
}
fn default_pop() -> usize {
    200
}
fn default_progress() -> usize {
    50
}

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

async fn stream_handler(Query(params): Query<SolveParams>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();

    match build_problem(&params) {
        Err(msg) => {
            let _ = tx.send(Event::default().event("error").data(msg));
        }
        Ok(problem) => {
            let sheet_w = problem.sheet.width;
            let sheet_h = problem.sheet.height;
            let problem = Arc::new(problem);
            let cfg = Arc::new(crate::ga_config(params.gens, params.pop, 5, 5));
            let mut rng = Xoshiro256StarStar::seed_from_u64(params.seed);
            let seeds: Vec<u64> = (0..params.threads.max(1)).map(|_| rng.next_u64()).collect();
            let mut sink = SseSink {
                tx,
                start: Instant::now(),
                sheet_w,
                sheet_h,
            };
            std::thread::spawn(move || {
                crate::run_with_sink(problem, cfg, &seeds, params.progress, &mut sink, 0).ok();
            });
        }
    }

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|evt| (Ok::<_, Infallible>(evt), rx))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn build_problem(params: &SolveParams) -> Result<Problem, String> {
    let specs: Vec<PieceSpec> =
        serde_json::from_str(&params.pieces).map_err(|e| format!("invalid pieces JSON: {e}"))?;
    if specs.is_empty() {
        return Err("no pieces specified".into());
    }
    let pieces: Vec<Piece> = specs
        .iter()
        .flat_map(|ps| {
            (0..ps.count).map(|_| Piece {
                name: ps.name.clone(),
                width: ps.width,
                height: ps.height,
                can_rotate: ps.can_rotate,
            })
        })
        .collect();
    Ok(Problem {
        sheet: Sheet {
            width: params.sheet_w,
            height: params.sheet_h,
        },
        kerf: params.kerf,
        pieces,
    })
}
