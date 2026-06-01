use std::fmt;

/// GA hyperparameters.
#[derive(Debug, Clone)]
pub struct GaConfig {
    /// Number of individuals in the population.
    pub pop_size: usize,

    /// Number of generations to run before returning the best individual found.
    pub n_generations: usize,

    /// Number of top individuals copied unchanged into the next generation (elitism).
    /// Keeps the best solution from being lost to crossover or mutation.
    /// Typical value: 1-2. Set to 0 to disable.
    pub n_elite: usize,

    /// Tournament size: how many individuals compete for each parent slot.
    /// Higher values increase selection pressure (best wins more often).
    /// Typical value: 2-5.
    pub tournament_k: usize,

    /// Probability that two parents produce children via OX crossover.
    /// With probability `1 - crossover_p` children are clones of their parents.
    /// Typical value: 0.7-0.9.
    pub crossover_p: f64,

    /// Per-gene probability of a swap mutation (exchanges this gene with a random other).
    /// Preserves the permutation invariant.
    /// Typical value: 0.05-0.2.
    pub swap_p: f64,

    /// Per-gene probability of flipping the `rotate` flag.
    /// Only has effect when the piece allows rotation.
    /// Typical value: 0.02-0.1.
    pub flip_p: f64,

    /// Per-gene probability of nudging `point_selector` by a random amount.
    /// Controls which free rectangle the decoder tries first for this piece.
    /// Small steps let the GA explore rect choices smoothly rather than jumping.
    /// Typical value: 0.05-0.15.
    pub point_p: f64,

    /// Inclusive range `(lo, hi)` for the nudge magnitude applied to `point_selector`.
    /// A value is drawn uniformly from `lo..=hi` and added or subtracted (wrapping).
    /// Default: `(1, 3)`.
    pub point_delta: (u32, u32),

    /// Per-gene probability of flipping the `inverse` flag.
    /// When flipped, the SLAS split direction is reversed for that piece, letting the GA
    /// represent cut trees that the default `lw <= lh` heuristic cannot.
    /// Typical value: 0.02-0.05.
    pub inverse_p: f64,

    /// Minimum dominant side length (px) for a piece type to be considered "long".
    /// Piece types with max(w,h) < long_dim_threshold go into the "small" class and are placed
    /// last by the glas decoder.
    /// 0 = auto-derive: max(sheet.width, sheet.height) * 0.3.
    pub long_dim_threshold: u32,

    /// Sqrt of the minimum area (px) for a long piece to be "large".
    /// A long piece is "large" if width*height >= large_area_threshold^2; otherwise "medium".
    /// 0 = auto-derive: sqrt(sheet.width * sheet.height * 0.05).
    pub large_area_threshold: u32,
}

impl fmt::Display for GaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pop={} gens={} elite={} k={} crossover_p={:.2} swap_p={:.2} flip_p={:.2} point_p={:.2} delta={}..={} inverse_p={:.2} long_dim_threshold={} large_area_threshold={}",
            self.pop_size,
            self.n_generations,
            self.n_elite,
            self.tournament_k,
            self.crossover_p,
            self.swap_p,
            self.flip_p,
            self.point_p,
            self.point_delta.0,
            self.point_delta.1,
            self.inverse_p,
            self.long_dim_threshold,
            self.large_area_threshold,
        )
    }
}

impl Default for GaConfig {
    fn default() -> Self {
        Self {
            pop_size: 200,
            n_generations: 1000,
            n_elite: 2,
            tournament_k: 3,
            crossover_p: 0.80,
            swap_p: 0.15,
            flip_p: 0.05,
            point_p: 0.10,
            point_delta: (1, 3),
            inverse_p: 0.05,
            long_dim_threshold: 0,
            large_area_threshold: 0,
        }
    }
}
