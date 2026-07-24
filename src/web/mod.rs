mod sse;

use std::{convert::Infallible, sync::Arc, time::Instant};

use axum::{
    Router,
    body::Body,
    extract::Query,
    http::header,
    response::{
        Html, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::get,
};
use cut::model::{PieceType, ProblemSpec, Sheet};
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
    #[serde(default)]
    algorithm: crate::cli::Algorithm,
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
const CHART_JS: &[u8] = include_bytes!("chart.min.js");
const SVG_JS: &[u8] = include_bytes!("svg.min.js");
const REAL1_JSON: &[u8] = include_bytes!("real1.json");
const REAL2_JSON: &[u8] = include_bytes!("real2.json");

async fn serve_chartjs() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/javascript; charset=utf-8")
        .body(Body::from(CHART_JS))
        .unwrap()
}

async fn serve_svgjs() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/javascript; charset=utf-8")
        .body(Body::from(SVG_JS))
        .unwrap()
}

async fn serve_real1() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(REAL1_JSON))
        .unwrap()
}

async fn serve_real2() -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .body(Body::from(REAL2_JSON))
        .unwrap()
}

pub(crate) fn run_serve(port: u16) -> std::io::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            let app = Router::new()
                .route("/", get(|| async { Html(INDEX_HTML) }))
                .route("/web/chart.min.js", get(serve_chartjs))
                .route("/web/svg.min.js", get(serve_svgjs))
                .route("/web/real1.json", get(serve_real1))
                .route("/web/real2.json", get(serve_real2))
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
        Ok(spec) => {
            let sheet_w = spec.sheet.width;
            let sheet_h = spec.sheet.height;
            let cfg = Arc::new(crate::ga::GaConfig::new(&spec, params.gens, params.pop, 5, 5, 0, 0));
            let problem = Arc::new(spec);
            let mut rng = Xoshiro256StarStar::seed_from_u64(params.seed);
            let seeds = (0..params.threads.max(1)).map(|_| rng.next_u64()).collect::<Vec<_>>();
            let mut sink = SseSink {
                tx,
                start: Instant::now(),
                sheet_w,
                sheet_h,
            };
            let algorithm = params.algorithm;
            std::thread::spawn(move || {
                crate::run_with_sink_any(problem, cfg, &seeds, params.progress, algorithm, &mut sink, 0).ok();
            });
        }
    }

    let stream = stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|evt| (Ok::<_, Infallible>(evt), rx))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn build_problem(params: &SolveParams) -> Result<ProblemSpec, String> {
    let pieces =
        serde_json::from_str::<Vec<PieceType>>(&params.pieces).map_err(|e| format!("invalid pieces JSON: {e}"))?;
    if pieces.is_empty() {
        return Err("no pieces specified".into());
    }
    let mut spec = ProblemSpec {
        sheet: Sheet {
            width: params.sheet_w,
            height: params.sheet_h,
        },
        kerf: params.kerf,
        margin: 0,
        piece_types: pieces,
    };
    spec.normalize();
    Ok(spec)
}
