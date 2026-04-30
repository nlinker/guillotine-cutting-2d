use rand::{Rng, RngExt};

/// Returns a random index sampled proportionally to `weights`.
fn weighted_sample(weights: &[f32], rng: &mut impl Rng) -> usize {
    let total: f32 = weights.iter().sum();
    assert!(total > 0.0, "weights must not all be zero");
    let mut r = rng.random::<f32>() * total;
    weights
        .iter()
        .enumerate()
        .find_map(|(i, &w)| { r -= w; if r <= 0.0 { Some(i) } else { None } })
        .unwrap_or(weights.len() - 1) // fp safety net
}

/// Randomly cuts `extent` into pieces of shape `[a, a, ..., b]` where all but
/// the last piece are equal, and returns their lengths.
///
/// `weights[i]` is the relative probability of making `i+1` cuts, e.g.
/// `[5, 3, 2]` -> P(1 cut)=0.5, P(2 cuts)=0.3, P(3 cuts)=0.2.
/// Each cut consumes `kerf` units (not present in any piece).
/// Every piece is >= `min_size` (`min_size <= extent` required).
/// If the sampled cut count yields no valid `a`, falls back toward 0 cuts.
/// Pieces sum to `extent - n_cuts * kerf`.
pub fn cut_pieces(extent: u32, min_size: u32, kerf: u32, weights: &[f32], rng: &mut impl Rng) -> Vec<u32> {
    assert!(!weights.is_empty(), "weights must not be empty");
    assert!(min_size <= extent, "min_size must be <= extent");

    let desired_cuts = weighted_sample(weights, rng) as u32 + 1;

    // For k cuts producing k+1 pieces [a, a, ..., b] with constraints:
    // min_size <= a <= a_max
    // min_size <= b = extent - k*(a + kerf)
    // a_max: largest a such that b = extent - k*(a + kerf) >= min_size
    // we have: min_size <= extent - k*(a + kerf)
    //  -> k*(a + kerf) <= extent - min_size
    //  -> a + kerf <= (extent - min_size) / k
    //  -> a <= (extent - min_size) / k - kerf
    //  -> a <= (extent - k*kerf - min_size) / k
    // Returns [min_size, a_max], or None if no valid a exists.
    let valid_range = |k: u32| -> Option<(u32, u32)> {
        if k == 0 {
            return Some((extent, extent));
        }
        let a_max = extent
            .checked_sub(k * kerf)?
            .checked_sub(min_size)?
            / k;
        if a_max < min_size { None } else { Some((min_size, a_max)) }
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

#[cfg(test)]
mod tests {
    use super::*;
    use rand_xoshiro::Xoshiro256StarStar;
    use rand_xoshiro::rand_core::SeedableRng;

    fn make_rng(seed: u64) -> Xoshiro256StarStar {
        Xoshiro256StarStar::seed_from_u64(seed)
    }

    fn check(extent: u32, weights: &[f32], kerf: u32, min_size: u32, trials: u32) {
        for seed in 0..trials {
            let mut rng = make_rng(seed as u64);
            let pieces = cut_pieces(extent, min_size, kerf, weights, &mut rng);
            let n_cuts = (pieces.len() as u32).saturating_sub(1);
            assert!(pieces[..pieces.len().saturating_sub(1)].windows(2).all(|w| w[0] == w[1]),
                "non-equal leading pieces: {pieces:?}");
            assert_eq!(pieces.iter().sum::<u32>() + n_cuts * kerf, extent,
                "wrong sum: {pieces:?}");
            assert!(pieces.iter().all(|&p| p >= min_size),
                "piece below min_size: {pieces:?}");
        }
    }

    #[test]
    fn deterministic_cuts() {
        let a = cut_pieces(13, 2, 1, &[5.0, 3.0, 2.0], &mut make_rng(42));
        let b = cut_pieces(13, 2, 1, &[5.0, 3.0, 2.0], &mut make_rng(42));
        assert_eq!(a, b);
    }

    #[test]
    fn weighted_sample_distribution() {
        let weights = [1.0, 2.0, 1.0];
        let mut counts = [0u32; 3];
        for seed in 0..10_000 {
            counts[weighted_sample(&weights, &mut make_rng(seed))] += 1;
        }
        assert!(counts[1] > counts[0] * 3 / 2);
        assert!(counts[1] > counts[2] * 3 / 2);
    }

    #[test]
    fn example_3_pieces() {
        // choices: a=2 b=7, a=3 b=5, a=4 b=3
        let mut seen = std::collections::HashSet::new();
        for seed in 0..1000 {
            let p = cut_pieces(13, 2, 1, &[0.0, 1.0], &mut make_rng(seed));
            assert_eq!(p.len(), 3);
            assert_eq!(p[0], p[1]);
            assert!(p.iter().all(|&x| x >= 2));
            assert_eq!(p.iter().sum::<u32>() + 2, 13);
            seen.insert(p[0]);
        }
        assert_eq!(seen, [2, 3, 4].into());
    }

    #[test]
    fn fallback_when_infeasible() {
        // needs 2+1+2+1+2=8 > 7 -> falls back to 2 pieces: a in [2, 3, 4]
        let mut seen = std::collections::HashSet::new();
        for seed in 0..200 {
            let p = cut_pieces(7, 2, 1, &[0.0, 1.0], &mut make_rng(seed));
            assert_eq!(p.len(), 2, "{p:?}");
            assert!(p.iter().all(|&x| x >= 2));
            seen.insert(p[0]);
        }
        assert_eq!(seen, [2, 3, 4].into());
    }

    #[test]
    fn zero_cuts_single_piece() {
        for seed in 0..1000 {
            assert_eq!(cut_pieces(5, 3, 1, &[1.0], &mut make_rng(seed)), vec![5]);
        }
    }

    #[test]
    fn tight_min_size() {
        // extent=9, kerf=1, min_size=4, 1 cut -> 2 pieces: a in [4,4] -> b=4. Only [4,4].
        for seed in 0..1000 {
            assert_eq!(cut_pieces(9, 4, 1, &[1.0], &mut make_rng(seed)), vec![4, 4]);
        }
    }

    #[test]
    fn zero_kerf() {
        check(20, &[1.0, 2.0, 3.0], 0, 3, 1000);
    }

    #[test]
    fn large_kerf() {
        check(20, &[1.0, 1.0, 1.0], 3, 2, 1000);
    }
}
