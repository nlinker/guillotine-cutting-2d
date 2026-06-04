use crate::model::{Piece, Placement, Problem};

/// A node in the guillotine cut tree for one sheet.
#[derive(Debug, Clone)]
pub enum CutNode {
    /// Leaf: a single placed piece.
    Piece {
        piece_idx: usize,
        x: u32,
        y: u32,
        pw: u32,
        ph: u32,
    },
    /// Leaf: an unused waste rectangle.
    Waste { x: u32, y: u32, w: u32, h: u32 },
    /// Internal: a horizontal cut at absolute y-coordinate `cut_y`.
    /// `top` covers `[rect.y, cut_y)`, `bottom` covers `[cut_y, rect.y+rect.h)`.
    HSplit {
        cut_y: u32,
        top: Box<CutNode>,
        bottom: Box<CutNode>,
    },
    /// Internal: a vertical cut at absolute x-coordinate `cut_x`.
    /// `left` covers `[rect.x, cut_x)`, `right` covers `[cut_x, rect.x+rect.w)`.
    VSplit {
        cut_x: u32,
        left: Box<CutNode>,
        right: Box<CutNode>,
    },
}

/// Bounding rectangle used during recursion.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl Rect {
    #[allow(dead_code)]
    fn contains_point(&self, px: u32, py: u32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Debug, Clone, Copy)]
struct PlacedPiece {
    piece_idx: usize,
    x: u32,
    y: u32,
    pw: u32,
    ph: u32,
}

impl PlacedPiece {
    fn right(&self) -> u32 {
        self.x + self.pw
    }

    fn bottom(&self) -> u32 {
        self.y + self.ph
    }
}

/// Guillotine cut-tree reconstruction from a known `(Problem, Solution)` pair.
/// Given pieces already placed at known coordinates, `build_cut_tree` tries to
/// recover a valid binary guillotine tree for each sheet.  The algorithm is a
/// recursive splitter: for a rectangle containing a set of placed pieces it
/// tries every H- and V-cut at piece boundaries until it finds one that
/// separates the pieces into two independent halves, then recurses.
///
/// Returns one `CutNode` per sheet (index = sheet index).
/// Returns `Err` if any region cannot be split by a guillotine cut.
pub fn build_cut_tree(problem: &Problem, placements: &[Placement]) -> Result<Vec<CutNode>, String> {
    let n_sheets = placements.iter().map(|p| p.sheet_idx).max().map_or(0, |m| m + 1);
    let sw = problem.sheet.width;
    let sh = problem.sheet.height;

    (0..n_sheets)
        .map(|sheet_idx| {
            let sheet_placements: Vec<PlacedPiece> = placements
                .iter()
                .filter(|p| p.sheet_idx == sheet_idx)
                .map(|p| {
                    let piece = &problem.pieces[p.piece_idx];
                    let (pw, ph) = if p.rotated {
                        (piece.height, piece.width)
                    } else {
                        (piece.width, piece.height)
                    };
                    PlacedPiece {
                        piece_idx: p.piece_idx,
                        x: p.x,
                        y: p.y,
                        pw,
                        ph,
                    }
                })
                .collect();
            let rect = Rect {
                x: 0,
                y: 0,
                w: sw,
                h: sh,
            };
            split(rect, &sheet_placements).ok_or_else(|| {
                format!(
                    "sheet {sheet_idx}: region ({},{} {}×{}) cannot be guillotine-split",
                    rect.x, rect.y, rect.w, rect.h
                )
            })
        })
        .collect()
}

/// Recursively split `rect` to accommodate all `pieces`.
/// Returns `None` if no guillotine cut can partition the pieces.
fn split(rect: Rect, pieces: &[PlacedPiece]) -> Option<CutNode> {
    match pieces.len() {
        0 => {
            return Some(CutNode::Waste {
                x: rect.x,
                y: rect.y,
                w: rect.w,
                h: rect.h,
            });
        }
        1 => {
            let p = pieces[0];
            if p.x == rect.x && p.y == rect.y && p.pw == rect.w && p.ph == rect.h {
                return Some(CutNode::Piece {
                    piece_idx: p.piece_idx,
                    x: p.x,
                    y: p.y,
                    pw: p.pw,
                    ph: p.ph,
                });
            }
            // Single piece doesn't fill the rect — still guillotine-splittable.
            // Fall through to the general case.
        }
        _ => {}
    }

    // Collect candidate H-cut positions: y-coordinates of all piece top/bottom edges
    // that are strictly inside the rect (i.e. rect.y < cut_y < rect.y + rect.h).
    let mut h_cuts: Vec<u32> = pieces
        .iter()
        .flat_map(|p| [p.y, p.bottom()])
        .filter(|&y| y > rect.y && y < rect.y + rect.h)
        .collect();
    h_cuts.sort_unstable();
    h_cuts.dedup();

    for cut_y in &h_cuts {
        let cut_y = *cut_y;
        // A valid H-cut must not pass through the interior of any piece.
        if pieces.iter().any(|p| p.y < cut_y && p.bottom() > cut_y) {
            continue;
        }
        let top_pieces = pieces
            .iter()
            .copied()
            .filter(|p| p.bottom() <= cut_y)
            .collect::<Vec<_>>();
        let bot_pieces = pieces.iter().copied().filter(|p| p.y >= cut_y).collect::<Vec<_>>();
        if top_pieces.len() + bot_pieces.len() != pieces.len() {
            continue; // some piece straddles the cut (shouldn't happen after the guard above)
        }
        let top_rect = Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: cut_y - rect.y,
        };
        let bot_rect = Rect {
            x: rect.x,
            y: cut_y,
            w: rect.w,
            h: rect.y + rect.h - cut_y,
        };
        if let (Some(top_node), Some(bot_node)) = (split(top_rect, &top_pieces), split(bot_rect, &bot_pieces)) {
            return Some(CutNode::HSplit {
                cut_y,
                top: Box::new(top_node),
                bottom: Box::new(bot_node),
            });
        }
    }

    // Collect candidate V-cut positions.
    let mut v_cuts: Vec<u32> = pieces
        .iter()
        .flat_map(|p| [p.x, p.right()])
        .filter(|&x| x > rect.x && x < rect.x + rect.w)
        .collect();
    v_cuts.sort_unstable();
    v_cuts.dedup();

    for cut_x in &v_cuts {
        let cut_x = *cut_x;
        if pieces.iter().any(|p| p.x < cut_x && p.right() > cut_x) {
            continue;
        }
        let left_pieces = pieces
            .iter()
            .copied()
            .filter(|p| p.right() <= cut_x)
            .collect::<Vec<_>>();
        let right_pieces = pieces.iter().copied().filter(|p| p.x >= cut_x).collect::<Vec<_>>();
        if left_pieces.len() + right_pieces.len() != pieces.len() {
            continue;
        }
        let left_rect = Rect {
            x: rect.x,
            y: rect.y,
            w: cut_x - rect.x,
            h: rect.h,
        };
        let right_rect = Rect {
            x: cut_x,
            y: rect.y,
            w: rect.x + rect.w - cut_x,
            h: rect.h,
        };
        if let (Some(left_node), Some(right_node)) = (split(left_rect, &left_pieces), split(right_rect, &right_pieces))
        {
            return Some(CutNode::VSplit {
                cut_x,
                left: Box::new(left_node),
                right: Box::new(right_node),
            });
        }
    }
    None
}


/// Orientation of a guillotine cut produced by a blueprint.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Orient {
    H,
    V,
}

/// Blueprint: one of two corner-placement scenarios for a batch inside a free leaf.
///
/// The composite box is always placed at the **top-left** corner `(x, y)` of the leaf.
/// Given free leaf `(x, y, nw, nh)`, batch `(cw, ch)`, `lw = nw−cw`, `lh = nh−ch`:
///
/// | # | Name | Cut  | Free leaves                                      |
/// |---|------|------|--------------------------------------------------|
/// | 0 | TlH  | H    | (x+cw, y, lw, ch)  +  (x, y+ch, nw, lh)         |
/// | 1 | TlV  | V    | (x+cw, y, lw, nh)  +  (x, y+ch, cw, lh)         |
///
/// TlH corresponds to the SLAS `inv=false` split; TlV to `inv=true`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blueprint {
    TlH = 0,
    TlV = 1,
}

impl Blueprint {
    pub const N: u8 = 2;

    /// Convert any `u8` to a `Blueprint` (wraps modulo 2).
    pub fn from_u8(v: u8) -> Self {
        if v.is_multiple_of(2) {
            Blueprint::TlH
        } else {
            Blueprint::TlV
        }
    }
}

/// A node in the `CutForest` arena.
#[derive(Debug, Clone)]
pub struct ForestNode {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub sheet_idx: usize,
    pub(crate) kind: ForestNodeKind,
}

/// Semantic state of a `ForestNode`.
#[derive(Debug, Clone)]
pub(crate) enum ForestNodeKind {
    /// Available for future batch placement; kept in `CutForest::free_leaves`.
    Free,
    /// A batch has been placed here; no longer available.
    Occupied,
}

/// Arena-based guillotine cut forest — one forest per decode call.
///
/// Tracks free regions (leaves) across all open sheets.
/// `free_leaves` is maintained incrementally: `apply_blueprint` removes the
/// consumed leaf and adds up to two new free leaves produced by the split.
pub struct CutForest {
    /// All nodes allocated during this forest's lifetime (free, occupied, and split).
    pub nodes: Vec<ForestNode>,
    /// Root node index for each sheet (one root per call to `new` / `open_new_sheet`).
    roots: Vec<usize>,
    /// Indices into `nodes` for every currently-free leaf.
    pub free_leaves: Vec<usize>,
}

impl CutForest {
    /// Create a new forest with one free leaf `(0, 0, w, h)` on sheet 0.
    pub fn new(w: u32, h: u32) -> Self {
        let root = ForestNode {
            x: 0,
            y: 0,
            w,
            h,
            sheet_idx: 0,
            kind: ForestNodeKind::Free,
        };
        CutForest {
            nodes: vec![root],
            roots: vec![0],
            free_leaves: vec![0],
        }
    }

    /// Open a new sheet and add a full-size free leaf `(0, 0, w, h)` for it.
    ///
    /// Returns the new sheet index (`roots.len() - 1` before this call).
    pub fn open_new_sheet(&mut self, w: u32, h: u32) -> usize {
        let sheet_idx = self.roots.len();
        let node_idx = self.nodes.len();
        self.nodes.push(ForestNode {
            x: 0,
            y: 0,
            w,
            h,
            sheet_idx,
            kind: ForestNodeKind::Free,
        });
        self.roots.push(node_idx);
        self.free_leaves.push(node_idx);
        sheet_idx
    }

    /// Number of sheets currently open.
    pub fn sheets_open(&self) -> usize {
        self.roots.len()
    }

    /// Apply blueprint `bp` to the free leaf at position `free_pos` in `free_leaves`,
    /// placing a batch of size `(cw, ch)`.
    ///
    /// Returns the batch origin `(batch_x, batch_y)`.
    ///
    /// The consumed leaf is removed from `free_leaves` (O(1) swap_remove) and marked
    /// `Occupied`.  Up to two new `Free` nodes are appended to `nodes` and added to
    /// `free_leaves`.
    ///
    /// `free_pos` is the index into `free_leaves` returned by `find_fitting_leaf`; passing
    /// it directly avoids a linear scan.
    ///
    /// Panics in debug mode if `free_pos >= free_leaves.len()`, or `cw > w` / `ch > h`.
    pub fn apply_blueprint(&mut self, free_pos: usize, cw: u32, ch: u32, bp: Blueprint) -> (u32, u32) {
        let node_idx = self.free_leaves[free_pos];
        let (x, y, nw, nh, sheet_idx) = {
            let node = &self.nodes[node_idx];
            debug_assert!(matches!(node.kind, ForestNodeKind::Free));
            debug_assert!(
                cw <= node.w && ch <= node.h,
                "batch ({cw}×{ch}) exceeds leaf ({}×{})",
                node.w,
                node.h
            );
            (node.x, node.y, node.w, node.h, node.sheet_idx)
        };
        let lw = nw - cw;
        let lh = nh - ch;

        // Compute batch origin and up to two new free rectangles.
        // Batch is always at the top-left corner (x, y).
        let (fr1, fr2) = match bp {
            Blueprint::TlH => (
                (lw > 0).then_some((x + cw, y, lw, ch)),
                (lh > 0).then_some((x, y + ch, nw, lh)),
            ),
            Blueprint::TlV => (
                (lw > 0).then_some((x + cw, y, lw, nh)),
                (lh > 0).then_some((x, y + ch, cw, lh)),
            ),
        };
        let (batch_x, batch_y) = (x, y);

        // Mark the consumed leaf as occupied and remove from the free list in O(1).
        self.nodes[node_idx].kind = ForestNodeKind::Occupied;
        self.free_leaves.swap_remove(free_pos);

        // Append new free nodes.
        for (fx, fy, fw, fh) in [fr1, fr2].into_iter().flatten() {
            let new_idx = self.nodes.len();
            self.nodes.push(ForestNode {
                x: fx,
                y: fy,
                w: fw,
                h: fh,
                sheet_idx,
                kind: ForestNodeKind::Free,
            });
            self.free_leaves.push(new_idx);
        }

        (batch_x, batch_y)
    }

    /// Scan free leaves starting at `selector % |free_leaves|` (wrapping) and return the first
    /// that fits `piece`.
    ///
    /// Returns `(free_pos, placed_w, placed_h, rotated)` where `free_pos` is the index into
    /// `free_leaves` (pass directly to `apply_blueprint` — no re-scan needed).
    pub fn find_fitting_leaf(
        &self,
        piece: &Piece,
        prefer_rotate: bool,
        selector: u32,
    ) -> Option<(usize, u32, u32, bool)> {
        let n = self.free_leaves.len();
        if n == 0 {
            return None;
        }
        let start = (selector as usize) % n;
        for i in 0..n {
            let free_pos = (start + i) % n;
            let node = &self.nodes[self.free_leaves[free_pos]];
            if let Some((pw, ph, rotated)) = piece_fits_in(node.w, node.h, piece, prefer_rotate) {
                return Some((free_pos, pw, ph, rotated));
            }
        }
        None
    }

    /// Like [`find_fitting_leaf`], but only considers leaves where the batch count
    /// `max(fr_w / pw, fr_h / ph)` is >= `min_fit`. Returns `None` if no such leaf exists.
    pub fn find_fitting_leaf_min_batch(
        &self,
        piece: &Piece,
        prefer_rotate: bool,
        selector: u32,
        min_fit: u32,
    ) -> Option<(usize, u32, u32, bool)> {
        let candidates: Vec<(usize, u32, u32, bool)> = (0..self.free_leaves.len())
            .filter_map(|free_pos| {
                let node = &self.nodes[self.free_leaves[free_pos]];
                let (pw, ph, rotated) = piece_fits_in(node.w, node.h, piece, prefer_rotate)?;
                let fits = (node.w / pw).max(node.h / ph);
                if fits >= min_fit { Some((free_pos, pw, ph, rotated)) } else { None }
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        Some(candidates[(selector as usize) % candidates.len()])
    }
}

/// Check whether `piece` fits in a `w × h` region, trying preferred orientation first.
/// Returns `(placed_w, placed_h, rotated)` or `None`.
pub(crate) fn piece_fits_in(w: u32, h: u32, piece: &Piece, prefer_rotate: bool) -> Option<(u32, u32, bool)> {
    let try_rotated = prefer_rotate && piece.can_rotate;
    let (pw_a, ph_a) = if try_rotated {
        (piece.height, piece.width)
    } else {
        (piece.width, piece.height)
    };
    if pw_a <= w && ph_a <= h {
        return Some((pw_a, ph_a, try_rotated));
    }
    if piece.can_rotate {
        let (pw_b, ph_b) = (ph_a, pw_a);
        if pw_b <= w && ph_b <= h {
            return Some((pw_b, ph_b, !try_rotated));
        }
    }
    None
}



#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, parse::parse_problem, slas::decoder::decode};

    #[test]
    fn single_piece_fills_sheet() {
        let spec = parse_problem("10x8F:0:10x8").unwrap();
        let problem = expand_problem(&spec);
        let genome = vec![crate::slas::decoder::Gene {
            piece_idx: 0,
            rotate: false,
            point_selector: 0,
            inverse: false,
        }];
        let sol = decode(&problem, &genome);
        let trees = build_cut_tree(&problem, &sol.placements).unwrap();
        assert_eq!(trees.len(), 1);
        assert!(matches!(trees[0], CutNode::Piece { .. }));
    }

    #[test]
    fn two_pieces_side_by_side() {
        // Sheet 10×5, two pieces 5×5.
        let spec = parse_problem("10x5F:0:5x5/2").unwrap();
        let problem = expand_problem(&spec);
        let genome = vec![
            crate::slas::decoder::Gene {
                piece_idx: 0,
                rotate: false,
                point_selector: 0,
                inverse: false,
            },
            crate::slas::decoder::Gene {
                piece_idx: 1,
                rotate: false,
                point_selector: 0,
                inverse: false,
            },
        ];
        let sol = decode(&problem, &genome);
        assert_eq!(sol.sheets_used(), 1);
        let trees = build_cut_tree(&problem, &sol.placements).unwrap();
        assert_eq!(trees.len(), 1);
        assert!(matches!(trees[0], CutNode::VSplit { .. } | CutNode::HSplit { .. }));
    }

    /// Helper: extract (x, y, w, h) of every free leaf in insertion order.
    fn free_rects(forest: &CutForest) -> Vec<(u32, u32, u32, u32)> {
        forest
            .free_leaves
            .iter()
            .map(|&i| {
                let n = &forest.nodes[i];
                (n.x, n.y, n.w, n.h)
            })
            .collect()
    }

    #[test]
    fn forest_tl_h() {
        // Sheet 200×300, batch 100×100.  lw=100, lh=200.
        // TlH: batch at (0,0); free: (100,0,100,100) + (0,100,200,200).
        let mut forest = CutForest::new(200, 300);
        let (bx, by) = forest.apply_blueprint(0, 100, 100, Blueprint::TlH);
        assert_eq!((bx, by), (0, 0));
        let rects = free_rects(&forest);
        assert_eq!(rects.len(), 2);
        assert!(rects.contains(&(100, 0, 100, 100)));
        assert!(rects.contains(&(0, 100, 200, 200)));
    }

    #[test]
    fn forest_tl_v() {
        // Sheet 200×300, batch 100×100.  lw=100, lh=200.
        // TlV: batch at (0,0); free: (100,0,100,300) + (0,100,100,200).
        let mut forest = CutForest::new(200, 300);
        let (bx, by) = forest.apply_blueprint(0, 100, 100, Blueprint::TlV);
        assert_eq!((bx, by), (0, 0));
        let rects = free_rects(&forest);
        assert_eq!(rects.len(), 2);
        assert!(rects.contains(&(100, 0, 100, 300)));
        assert!(rects.contains(&(0, 100, 100, 200)));
    }

    #[test]
    fn forest_exact_fit() {
        // Sheet 100×100, batch 100×100: exact fit — no free leaves after.
        let mut forest = CutForest::new(100, 100);
        let (bx, by) = forest.apply_blueprint(0, 100, 100, Blueprint::TlH);
        assert_eq!((bx, by), (0, 0));
        assert!(forest.free_leaves.is_empty());
    }

    #[test]
    fn forest_exact_w() {
        // Sheet 100×200, batch 100×100: lw=0, lh=100.
        // TlH: batch at (0,0), only bottom free: (0,100,100,100).
        let mut forest = CutForest::new(100, 200);
        let (bx, by) = forest.apply_blueprint(0, 100, 100, Blueprint::TlH);
        assert_eq!((bx, by), (0, 0));
        assert_eq!(free_rects(&forest), vec![(0, 100, 100, 100)]);
    }

    #[test]
    fn forest_exact_h() {
        // Sheet 200×100, batch 100×100: lw=100, lh=0.
        // TlH: batch at (0,0), only right free: (100,0,100,100).
        let mut forest = CutForest::new(200, 100);
        let (bx, by) = forest.apply_blueprint(0, 100, 100, Blueprint::TlH);
        assert_eq!((bx, by), (0, 0));
        assert_eq!(free_rects(&forest), vec![(100, 0, 100, 100)]);
    }

}
