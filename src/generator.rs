use std::collections::VecDeque;

use rand::prelude::{Rng, RngExt, SliceRandom};

use crate::model::{FreeRect, Piece, PieceId, Placement, Problem, Sheet, Solution};

#[derive(Debug)]
pub struct Output {
    pub problem: Problem,
    pub optimal_solution: Solution,
}

/// Configuration for the generator.
pub struct GeneratorConfig {
    /// Stock sheet dimensions (all sheets in the problem are identical).
    pub sheet: Sheet,

    /// Number of sheets to cut; this equals the known optimum of the instance.
    pub k: usize,

    /// Minimum side length of any generated piece.
    pub min_size: u32,

    /// Blade kerf width (same unit as sheet dimensions).
    ///
    /// Each internal guillotine cut consumes this many units of material between
    /// the two resulting sub-rectangles.  Boundary edges of the sheet are exempt,
    /// matching the semantics in `model::Problem`.  A value of `0` reproduces
    /// the original kerf-free behavior.
    ///
    /// Constraints imposed by a non-zero kerf:
    /// * A rect of extent `E` is splittable only if `E >= 2 * min_size + kerf`.
    /// * Valid cut positions along extent `E` are `[min_size, E - min_size - kerf]`.
    /// * Total piece area will be strictly less than total sheet area (the
    ///   difference is material lost to the blade).
    pub kerf: u32,

    /// Probability weights controlling how many same-size pieces are cut from a
    /// rectangle in one step.
    ///
    /// `weights[i]` is the non-normalized probability of cutting `i + 1` identical
    /// pieces from the current rectangle in a single step.  For example:
    ///
    /// ```text
    /// weights = [5, 3, 2]   =>   P(1 piece) = 0.5,  P(2) = 0.3,  P(3) = 0.2
    /// ```
    ///
    /// If the sampled count `m` does not physically fit (accounting for
    /// `min_size` and `kerf` on each internal cut), the generator falls back to
    /// `m - 1`, then `m - 2`, and so on; `m = 1` always fits.
    ///
    /// When `m > 1` pieces of size `s` are cut, they are placed side by side
    /// along the chosen direction, separated by kerf gaps.  Any remaining strip
    /// of the rectangle becomes a new rect that is split further in subsequent
    /// steps, consuming the rest of the budget.
    ///
    /// `weights` must be non-empty and contain at least one positive value.
    pub weights: Vec<f32>,
}

/// Generate a 2-D guillotine cutting-stock instance with a known optimal solution.
/// * `cfg` — generator settings (see [`GeneratorConfig`]).
/// * `rng` — random-number source.
pub fn generate<R: Rng>(cfg: &GeneratorConfig, rng: &mut R) -> Output {
    let sheet = cfg.sheet;
    assert!(cfg.min_size >= 1, "min_size must be at least 1");
    assert!(!cfg.weights.is_empty(), "weights must be non-empty");
    assert!(
        cfg.weights.iter().any(|&w| w > 0.0),
        "weights must contain at least one positive value"
    );
    assert!(
        cfg.weights.iter().all(|&w| w >= 0.0),
        "weights must all be non-negative"
    );

    let min_size = cfg.min_size;
    let kerf = cfg.kerf;

    let stage_count = 2_usize;
    let mut all_rects = Vec::with_capacity((cfg.k as usize) * cfg.weights.len().pow(stage_count as u32));

    for sheet_idx in 0..cfg.k {
        let mut queue = VecDeque::with_capacity(cfg.weights.len().pow(stage_count as u32));
        queue.push_back(FreeRect {
            sheet_idx,
            x: 0,
            y: 0,
            w: sheet.width,
            h: sheet.height,
        });
        // the direction of cuts on this stage
        let mut vertical = false;

        for _ in 0..stage_count {
            let mut rects = Vec::with_capacity(queue.len() * cfg.weights.len());
            // exhaust queue for the current stage
            while let Some(fr) = queue.pop_front() {
                let extent = if vertical { fr.w } else { fr.h };
                let lengths = cut_extent(extent, min_size, kerf, &cfg.weights, rng);
                let mut offset = 0;
                for length in lengths {
                    let r = if vertical {
                        FreeRect {
                            sheet_idx,
                            x: fr.x + offset,
                            y: fr.y,
                            w: length,
                            h: fr.h,
                        }
                    } else {
                        FreeRect {
                            sheet_idx,
                            x: fr.x,
                            y: fr.y + offset,
                            w: fr.w,
                            h: length,
                        }
                    };
                    rects.push(r);
                    offset += length + kerf;
                }
            }
            queue.extend(rects);
            vertical = !vertical;
        }
        all_rects.extend(queue);
    }
    // Shuffle before assigning indices so piece_idx == position in problem.pieces.
    all_rects.shuffle(rng);

    let mut placements: Vec<Placement> = Vec::new();
    let mut problem_pieces: Vec<Piece> = Vec::new();
    for (piece_idx, rect) in all_rects.iter().enumerate() {
        placements.push(Placement {
            sheet_idx: rect.sheet_idx,
            piece_idx,
            x: rect.x,
            y: rect.y,
            rotated: false,
        });
        problem_pieces.push(Piece {
            id: (piece_idx + 1) as PieceId,
            width: rect.w,
            height: rect.h,
            can_rotate: true,
        });
    }
    Output {
        problem: Problem {
            sheet: cfg.sheet,
            kerf: cfg.kerf,
            pieces: problem_pieces,
        },
        optimal_solution: Solution {
            placements,
            leftovers: Vec::new(),
        },
    }
}

/// Randomly cuts `extent` into pieces of shape `[a, a, ..., b]` where all but
/// the last piece are equal, and returns their lengths.
///
/// `weights[i]` is the relative probability of making `i+1` cuts, e.g.
/// `[5, 3, 2]` -> P(1 cut)=0.5, P(2 cuts)=0.3, P(3 cuts)=0.2.
/// Pieces sum to `extent - n_cuts * kerf`.
pub fn cut_extent(extent: u32, min_size: u32, kerf: u32, weights: &[f32], rng: &mut impl Rng) -> Vec<u32> {
    assert!(!weights.is_empty(), "weights must not be empty");
    assert!(min_size <= extent, "min_size must be <= extent");

    let desired_cuts = weighted_sample(weights, rng) as u32 + 1;

    // for k cuts producing k+1 pieces [a, a, ..., b] with constraints:
    // min_size <= a <= a_max
    // min_size <= b = extent - k*(a + kerf)
    // a_max: largest a such that b = extent - k*(a + kerf) >= min_size
    // we have: min_size <= extent - k*(a + kerf)
    //   -> k*(a + kerf) <= extent - min_size
    //   -> a + kerf <= (extent - min_size) / k
    //   -> a <= (extent - min_size) / k - kerf
    //   -> a <= (extent - k*kerf - min_size) / k
    // Returns [min_size, a_max], or None if no valid a exists.
    let valid_range = |k: u32| -> Option<(u32, u32)> {
        if k == 0 {
            return Some((extent, extent));
        }
        let a_max = extent.checked_sub(k * kerf)?.checked_sub(min_size)? / k;
        if a_max < min_size {
            None
        } else {
            Some((min_size, a_max))
        }
    };

    // Fall back from desired_cuts toward 0 until a valid range exists.
    let (n_cuts, (a_min, a_max)) = (0..=desired_cuts)
        .rev()
        .find_map(|k| valid_range(k).map(|r| (k, r)))
        .unwrap(); // k=0 is always valid by precondition

    let a = rng.random_range(a_min..=a_max);
    if n_cuts == 0 {
        return vec![a];
    }
    let b = extent - n_cuts * (a + kerf);

    let mut pieces = Vec::with_capacity(n_cuts as usize + 1);
    pieces.extend(std::iter::repeat_n(a, n_cuts as usize));
    pieces.push(b);

    debug_assert_eq!(pieces.len(), (n_cuts + 1) as usize);
    debug_assert!(pieces.iter().all(|&p| p >= min_size));
    debug_assert_eq!(pieces.iter().sum::<u32>() + n_cuts * kerf, extent);

    pieces
}

/// Returns a random index sampled proportionally to `weights`.
fn weighted_sample(weights: &[f32], rng: &mut impl Rng) -> usize {
    let total: f32 = weights.iter().sum();
    assert!(total > 0.0, "weights must not all be zero");
    let mut r = rng.random::<f32>() * total;
    weights
        .iter()
        .enumerate()
        .find_map(|(i, &w)| {
            r -= w;
            if r <= 0.0 { Some(i) } else { None }
        })
        .unwrap_or(weights.len() - 1) // fp safety net
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand_xoshiro::Xoshiro256StarStar;

    use super::*;
    use crate::model::Sheet;

    fn sheet_10x8() -> Sheet {
        Sheet { width: 10, height: 8 }
    }

    fn cfg(kerf: u32, weights: Vec<f32>) -> GeneratorConfig {
        GeneratorConfig {
            sheet: sheet_10x8(),
            k: 2,
            min_size: 2,
            kerf,
            weights,
        }
    }

    fn rng(seed: u64) -> Xoshiro256StarStar {
        Xoshiro256StarStar::seed_from_u64(seed)
    }

    fn w_single() -> Vec<f32> {
        vec![1.0]
    }

    fn w_multi() -> Vec<f32> {
        vec![5.0, 3.0, 2.0]
    }

    #[test]
    fn weighted_sample_distribution() {
        let weights = [1.0, 2.0, 1.0];
        let mut counts = [0u32; 3];
        for seed in 0..10_000 {
            counts[weighted_sample(&weights, &mut rng(seed))] += 1;
        }
        assert!(counts[1] > counts[0] * 3 / 2);
        assert!(counts[1] > counts[2] * 3 / 2);
    }

    #[test]
    fn cut_extent_3_pieces() {
        // choices: a=2 b=7, a=3 b=5, a=4 b=3
        let mut seen = std::collections::HashSet::new();
        for seed in 0..1000 {
            let p = cut_extent(13, 2, 1, &[0.0, 1.0], &mut rng(seed));
            assert_eq!(p.len(), 3);
            assert_eq!(p[0], p[1]);
            assert!(p.iter().all(|&x| x >= 2));
            assert_eq!(p.iter().sum::<u32>() + 2, 13);
            seen.insert(p[0]);
        }
        assert_eq!(seen, [2, 3, 4].into());
    }

    #[test]
    fn cut_extent_fallback_when_infeasible() {
        // needs 2+1+2+1+2=8 > 7 -> falls back to 2 pieces: a in [2, 3, 4]
        let mut seen = std::collections::HashSet::new();
        for seed in 0..200 {
            let p = cut_extent(7, 2, 1, &[0.0, 1.0], &mut rng(seed));
            assert_eq!(p.len(), 2, "{p:?}");
            assert!(p.iter().all(|&x| x >= 2));
            seen.insert(p[0]);
        }
        assert_eq!(seen, [2, 3, 4].into());
    }

    #[test]
    fn cut_extent_corner_cases() {
        for seed in 0..1000 {
            let rng = &mut rng(seed);
            // zero cuts
            assert_eq!(cut_extent(5, 3, 1, &[1.0], rng), vec![5]);
            // extent=9, kerf=1, min_size=4, 1 cut -> 2 pieces: a in [4,4] -> b=4. Only [4,4].
            assert_eq!(cut_extent(9, 4, 1, &[1.0], rng), vec![4, 4]);
        }
    }

    #[test]
    fn pieces_tile_sheets_exactly_when_kerf_zero() {
        // With kerf=0 the pieces still tile the sheets perfectly.
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let out = generate(&cfg(0, w_single()), &mut rng);
        let total: u32 = out.problem.pieces.iter().map(|p| p.width * p.height).sum();
        assert_eq!(total, 10 * 8 * 2, "total piece area must equal total sheet area");
    }

    #[test]
    fn pieces_are_less_than_sheet_area_when_kerf_nonzero() {
        // With kerf>0 the pieces cover strictly less than the full sheet area.
        let mut rng = Xoshiro256StarStar::seed_from_u64(66);
        let cfg = cfg(1, w_multi());
        let out = generate(&cfg, &mut rng);
        let total: u32 = out.problem.pieces.iter().map(|p| p.width * p.height).sum();
        let sheet_total = 10 * 8 * 2;
        assert!(
            total < sheet_total,
            "kerf>0: piece area ({total}) must be < sheet area ({sheet_total})"
        );
        assert_eq!(out.problem.kerf, 1);
    }

    #[test]
    fn no_piece_exceeds_sheet() {
        // Every piece must fit within the sheet bounds from its placement origin.
        let sheet = sheet_10x8();
        for kerf in [0, 1, 2] {
            let mut rng = Xoshiro256StarStar::seed_from_u64(7);
            let out = generate(
                &GeneratorConfig {
                    sheet,
                    k: 2,
                    min_size: 1,
                    kerf,
                    weights: w_single(),
                },
                &mut rng,
            );
            for pl in &out.optimal_solution.placements {
                let p = &out.problem.pieces[pl.piece_idx];
                assert!(
                    pl.x + p.width <= sheet.width,
                    "kerf={kerf}: piece {} overflows width",
                    p.id
                );
                assert!(
                    pl.y + p.height <= sheet.height,
                    "kerf={kerf}: piece {} overflows height",
                    p.id
                );
            }
        }
    }

    #[test]
    fn min_size_respected() {
        let mut rng = Xoshiro256StarStar::seed_from_u64(99);
        let c = GeneratorConfig {
            sheet: sheet_10x8(),
            k: 2,
            min_size: 2,
            kerf: 1,
            weights: w_single(),
        };
        let out = generate(&c, &mut rng);
        for p in &out.problem.pieces {
            assert!(p.width >= c.min_size, "piece {} width  < min_size", p.id);
            assert!(p.height >= c.min_size, "piece {} height < min_size", p.id);
        }
    }

    #[test]
    fn pieces_do_not_overlap() {
        // Pieces on the same sheet must not overlap.
        for kerf in [0u32, 1] {
            let mut rng = Xoshiro256StarStar::seed_from_u64(123);
            let out = generate(
                &GeneratorConfig {
                    sheet: sheet_10x8(),
                    k: 2,
                    min_size: 1,
                    kerf,
                    weights: w_single(),
                },
                &mut rng,
            );
            for sheet_idx in 0..2usize {
                let on: Vec<_> = out
                    .optimal_solution
                    .placements
                    .iter()
                    .filter(|pl| pl.sheet_idx == sheet_idx)
                    .collect();
                for i in 0..on.len() {
                    for j in i + 1..on.len() {
                        let (a, b) = (on[i], on[j]);
                        let ap = &out.problem.pieces[a.piece_idx];
                        let bp = &out.problem.pieces[b.piece_idx];
                        let overlap = a.x < b.x + bp.width
                            && a.x + ap.width > b.x
                            && a.y < b.y + bp.height
                            && a.y + ap.height > b.y;
                        assert!(
                            !overlap,
                            "kerf={kerf}: pieces {} and {} overlap on sheet {}",
                            a.piece_idx, b.piece_idx, sheet_idx
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn seed_is_deterministic() {
        let c = cfg(1, vec![1.0, 1.0, 1.0]);
        let o1 = generate(&c, &mut Xoshiro256StarStar::seed_from_u64(42));
        let o2 = generate(&c, &mut Xoshiro256StarStar::seed_from_u64(42));
        let ids1: Vec<_> = o1.problem.pieces.iter().map(|p| (p.id, p.width, p.height)).collect();
        let ids2: Vec<_> = o2.problem.pieces.iter().map(|p| (p.id, p.width, p.height)).collect();
        assert_eq!(ids1, ids2);
    }

    #[test]
    fn no_leftovers_in_optimal_solution() {
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let out = generate(&cfg(0, w_single()), &mut rng);
        assert!(out.optimal_solution.leftovers.is_empty());
    }

    #[test]
    fn sheet_indices_are_zero_based() {
        let mut rng = Xoshiro256StarStar::seed_from_u64(42);
        let out = generate(
            &GeneratorConfig {
                sheet: sheet_10x8(),
                k: 3,
                min_size: 1,
                kerf: 0,
                weights: w_single(),
            },
            &mut rng,
        );
        let max_idx = out
            .optimal_solution
            .placements
            .iter()
            .map(|p| p.sheet_idx)
            .max()
            .unwrap_or(0);
        assert_eq!(max_idx, 2);
    }

    #[test]
    fn weights_fallback_when_count_exceeds_capacity() {
        // Sheet 4x4, min_size=2, kerf=1.
        let mut rng = Xoshiro256StarStar::seed_from_u64(0);
        let out = generate(
            &GeneratorConfig {
                sheet: Sheet { width: 4, height: 4 },
                k: 1,
                min_size: 2,
                kerf: 1,
                weights: vec![0.0, 0.0, 1.0], // always try 3 pieces first
            },
            &mut rng,
        );
        // Must produce exactly 1 piece covering the whole sheet (nothing fits the multi-piece
        // counts).
        assert_eq!(out.problem.pieces.len(), 1);
        let p = &out.problem.pieces[0];
        assert_eq!(p.width, 4);
        assert_eq!(p.height, 4);
    }
}
