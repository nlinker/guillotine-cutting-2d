use crate::model::Piece;

/// Best-fitting group of pieces for a guillotine strip, after the GroupSub
/// decoder of Faizrakhmanov et al. (2014): an exact 0/1 reachability DP picks
/// the subset whose total length along the strip axis is maximal but does not
/// exceed `target`, with rotation handled as two mutually exclusive branches
/// per piece.
///
/// `vertical` selects the strip axis: for a vertical strip (column) a piece
/// participates in an orientation `(pw, ph)` when `pw <= cross_cap` and
/// contributes `ph` to the sum; for a horizontal strip (row) the roles of the
/// dimensions swap. Kerf is baked into expanded piece dimensions, so reaching
/// `target` exactly means a flush fit.
///
/// Returns `(piece_idx, rotated)` pairs (order unspecified). Deterministic:
/// candidates are processed in the given order, the first writer of a sum
/// wins, and the unrotated orientation is tried first. `O(candidates * target)`.
pub fn best_group(
    pieces: &[Piece],
    candidates: &[usize],
    cross_cap: u32,
    target: u32,
    vertical: bool,
) -> Vec<(usize, bool)> {
    if target == 0 || candidates.is_empty() {
        return vec![];
    }
    // choice[s] = (candidate position, rotated, previous sum) for the first
    // chain that reached sum s; doubles as the reachability marker (sum 0 is
    // reachable by definition and keeps its None sentinel).
    let mut choice: Vec<Option<(usize, bool, u32)>> = vec![None; target as usize + 1];
    for (pos, &idx) in candidates.iter().enumerate() {
        let p = &pieces[idx];
        let both = [(p.width, p.height, false), (p.height, p.width, true)];
        let n_or = if p.can_rotate && p.width != p.height { 2 } else { 1 };
        // Descending sums: writes land on indices >= s while reads target
        // prev < s, so every chain extends only the state from before this
        // candidate — each piece is used at most once, in one orientation.
        for s in (1..=target).rev() {
            if choice[s as usize].is_some() {
                continue;
            }
            for &(w, h, rotated) in &both[..n_or] {
                let (cross, len) = if vertical { (w, h) } else { (h, w) };
                if cross <= cross_cap && len <= s {
                    let prev = s - len;
                    if prev == 0 || choice[prev as usize].is_some() {
                        choice[s as usize] = Some((pos, rotated, prev));
                        break;
                    }
                }
            }
        }
    }

    let Some(best) = (1..=target).rev().find(|&s| choice[s as usize].is_some()) else {
        return vec![];
    };
    let mut group = Vec::new();
    let mut s = best;
    while s > 0 {
        let (pos, rotated, prev) = choice[s as usize].expect("every sum on a reconstruction chain is reachable");
        group.push((candidates[pos], rotated));
        s = prev;
    }
    group
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(w: u32, h: u32, can_rotate: bool) -> Piece {
        Piece {
            name: String::new(),
            width: w,
            height: h,
            can_rotate,
        }
    }

    fn total_len(pieces: &[Piece], group: &[(usize, bool)], vertical: bool) -> u32 {
        group
            .iter()
            .map(|&(i, rotated)| {
                let p = &pieces[i];
                let (w, h) = if rotated {
                    (p.height, p.width)
                } else {
                    (p.width, p.height)
                };
                if vertical { h } else { w }
            })
            .sum()
    }

    #[test]
    fn exact_fill_beats_single_larger_piece() {
        // Heights {30, 70, 60} with target 100: 30+70 fills exactly, 60+30=90 does not.
        let pieces = vec![piece(50, 30, false), piece(50, 70, false), piece(50, 60, false)];
        let group = best_group(&pieces, &[0, 1, 2], 50, 100, true);
        let mut idxs: Vec<usize> = group.iter().map(|&(i, _)| i).collect();
        idxs.sort_unstable();
        assert_eq!(idxs, vec![0, 1]);
        assert_eq!(total_len(&pieces, &group, true), 100);
    }

    #[test]
    fn rotation_branch_is_used() {
        // 80x40 does not fit a 40-wide column upright, but fits rotated and
        // contributes its width (80) to the column height.
        let pieces = vec![piece(80, 40, true)];
        let group = best_group(&pieces, &[0], 40, 80, true);
        assert_eq!(group, vec![(0, true)]);
    }

    #[test]
    fn cross_cap_filters_wide_pieces() {
        // 60-wide piece cannot enter a 50-wide column (rotation disabled).
        let pieces = vec![piece(60, 40, false), piece(50, 40, false)];
        let group = best_group(&pieces, &[0, 1], 50, 100, true);
        assert_eq!(group, vec![(1, false)]);
    }

    #[test]
    fn horizontal_axis_swaps_roles() {
        // Row of height 50: contributions are widths, cap is on heights.
        let pieces = vec![piece(30, 50, false), piece(70, 50, false), piece(40, 60, false)];
        let group = best_group(&pieces, &[0, 1, 2], 50, 100, false);
        let mut idxs: Vec<usize> = group.iter().map(|&(i, _)| i).collect();
        idxs.sort_unstable();
        assert_eq!(idxs, vec![0, 1], "60-tall piece must be filtered, 30+70 fill the row");
        assert_eq!(total_len(&pieces, &group, false), 100);
    }

    #[test]
    fn piece_is_used_at_most_once() {
        // Single 50-long piece, target 100: the sum 100 must NOT be reached by
        // chaining the same piece twice.
        let pieces = vec![piece(50, 50, false)];
        let group = best_group(&pieces, &[0], 50, 100, true);
        assert_eq!(group.len(), 1);
        assert_eq!(total_len(&pieces, &group, true), 50);
    }

    #[test]
    fn empty_inputs_give_empty_group() {
        let pieces = vec![piece(50, 50, false)];
        assert!(best_group(&pieces, &[], 50, 100, true).is_empty());
        assert!(best_group(&pieces, &[0], 50, 0, true).is_empty());
        assert!(best_group(&pieces, &[0], 40, 100, true).is_empty());
    }

    #[test]
    fn deterministic_for_same_input() {
        let pieces = vec![piece(50, 30, true), piece(40, 25, true), piece(35, 35, false)];
        let a = best_group(&pieces, &[0, 1, 2], 50, 90, true);
        let b = best_group(&pieces, &[0, 1, 2], 50, 90, true);
        assert_eq!(a, b);
    }
}
