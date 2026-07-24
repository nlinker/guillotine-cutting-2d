use clap::{Parser, Subcommand};

/// Solver algorithm.
#[derive(clap::ValueEnum, serde::Deserialize, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Algorithm {
    /// SLAS genetic algorithm (one gene per physical piece)
    Slas,
    /// Group-SLAS genetic algorithm (one gene per piece type)
    #[default]
    Glas,
    /// BFDH greedy shelf heuristic (no GA, instant result)
    Bfdh,
    /// Jylanki portfolio: 144 greedy guillotine passes, best result wins (no GA, instant result)
    Jylanki,
    /// BPC exact solver - branch-price-and-cut column generation (iterative, stoppable)
    Bpc,
}

impl std::fmt::Display for Algorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Algorithm::Slas => write!(f, "slas"),
            Algorithm::Glas => write!(f, "glas"),
            Algorithm::Bfdh => write!(f, "bfdh"),
            Algorithm::Jylanki => write!(f, "jylanki"),
            Algorithm::Bpc => write!(f, "bpc"),
        }
    }
}

#[derive(Parser)]
#[command(name = "cutting", about = "2D guillotine cutting optimizer")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Run the GA on a problem and print ranked results
    Calc {
        /// Compact problem string. Mutually exclusive with --json.
        #[arg(long)]
        compact: Option<String>,
        /// Path to a JSON problem file. Mutually exclusive with --compact.
        #[arg(long)]
        json: Option<String>,
        /// Base random seed
        #[arg(long, default_value_t = 42)]
        seed: u64,
        /// Number of parallel threads (0 = auto-detect)
        #[arg(long, default_value_t = 0)]
        threads: usize,
        /// Generations per run
        #[arg(long, default_value_t = 2000)]
        gens: usize,
        /// Population size
        #[arg(long, default_value_t = 200)]
        pop: usize,
        /// Elite count
        #[arg(long, default_value_t = 5)]
        elite: usize,
        /// Tournament size
        #[arg(long, default_value_t = 5)]
        k: usize,
        /// Report global best every N generations; 0 = silent
        #[arg(long, default_value_t = 100)]
        progress: usize,
        /// Progress sink: "pipe" (default) or "stdout"
        #[arg(long, default_value = "pipe")]
        sink: String,
        /// Throttle sink: send at most one progress per N ms; 0 = no throttle
        #[arg(long, default_value_t = 1000)]
        sink_interval: u64,
        /// Render the best solution as SVG to stdout instead of JSON
        #[arg(long, default_value_t = false)]
        render: bool,
        /// Solver algorithm
        #[arg(long, default_value = "glas")]
        algorithm: Algorithm,
        /// Min side length (px) for a piece to be "long"; 0 = auto (sheet_max * 0.3)
        #[arg(long, default_value_t = 0)]
        long_dim_threshold: u32,
        /// Sqrt of min area (px) for a long piece to be "large"; 0 = auto (sqrt(sheet_area * 0.05))
        #[arg(long, default_value_t = 0)]
        large_area_threshold: u32,
    },
    /// Start a web server with an interactive UI
    Serve {
        #[arg(long, default_value_t = 8080)]
        port: u16,
    },
    /// Render a solution as SVG to stdout
    Render {
        /// Compact problem string. Mutually exclusive with --json.
        #[arg(long)]
        compact: Option<String>,
        /// Path to JSON problem file. Mutually exclusive with --compact.
        #[arg(long)]
        json: Option<String>,
        /// Path to solution JSON file (the `solution` field from a `done` event, or the object itself)
        #[arg(long)]
        solution: String,
    },
    /// Build the GLF (Guillotine Layout Function) table and print it
    Glf {
        /// Compact problem string, e.g. "10x8F::2x3/4,4x3,8x3,5x2/2"
        problem: String,
    },
}
