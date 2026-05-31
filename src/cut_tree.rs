use std::collections::HashMap;

use crate::model::{Piece, Placement, Problem, Solution};

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
        let top_pieces: Vec<PlacedPiece> = pieces.iter().copied().filter(|p| p.bottom() <= cut_y).collect();
        let bot_pieces: Vec<PlacedPiece> = pieces.iter().copied().filter(|p| p.y >= cut_y).collect();
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
        let left_pieces: Vec<PlacedPiece> = pieces.iter().copied().filter(|p| p.right() <= cut_x).collect();
        let right_pieces: Vec<PlacedPiece> = pieces.iter().copied().filter(|p| p.x >= cut_x).collect();
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

/// Manufacturability cost of a set of cut trees.
///
/// Models total operator effort to execute the cutting plan:
/// - Rotating a workpiece (axis change between parent and child cut): weight 10
/// - Re-setting the fence (same axis, new position): weight 3
/// - Each guillotine cut: weight 1
pub fn mfg_cost_from_trees(trees: &[CutNode]) -> u32 {
    let mut cost = 0u32;
    for tree in trees {
        walk(tree, None, None, &mut cost);
    }
    cost
}

#[derive(Clone, Copy, PartialEq)]
enum Axis {
    H,
    V,
}

fn walk(node: &CutNode, parent_axis: Option<Axis>, parent_pos: Option<u32>, cost: &mut u32) {
    const W_R: u32 = 10;
    const W_D: u32 = 3;
    const W_C: u32 = 1;
    match node {
        CutNode::Piece { .. } | CutNode::Waste { .. } => {}
        CutNode::HSplit { cut_y, top, bottom } => {
            *cost += W_C;
            if let Some(pa) = parent_axis {
                if pa != Axis::H {
                    *cost += W_R;
                } else if parent_pos != Some(*cut_y) {
                    *cost += W_D;
                }
            }
            walk(top, Some(Axis::H), Some(*cut_y), cost);
            walk(bottom, Some(Axis::H), Some(*cut_y), cost);
        }
        CutNode::VSplit { cut_x, left, right } => {
            *cost += W_C;
            if let Some(pa) = parent_axis {
                if pa != Axis::V {
                    *cost += W_R;
                } else if parent_pos != Some(*cut_x) {
                    *cost += W_D;
                }
            }
            walk(left, Some(Axis::V), Some(*cut_x), cost);
            walk(right, Some(Axis::V), Some(*cut_x), cost);
        }
    }
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
    /// Internal node created by a blueprint split.
    #[allow(dead_code)]
    Split { orient: Orient, left: usize, right: usize },
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

    /// Apply blueprint `bp` to the free leaf at `leaf_idx`, placing a batch of size `(cw, ch)`.
    ///
    /// Returns the batch origin `(batch_x, batch_y)`.
    ///
    /// The consumed leaf is removed from `free_leaves` and marked `Occupied`.
    /// Up to two new `Free` nodes are appended to `nodes` and added to `free_leaves`.
    ///
    /// Panics in debug mode if `leaf_idx` is not in `free_leaves`, or `cw > w` / `ch > h`.
    pub fn apply_blueprint(&mut self, leaf_idx: usize, cw: u32, ch: u32, bp: Blueprint) -> (u32, u32) {
        let (x, y, nw, nh, sheet_idx) = {
            let node = &self.nodes[leaf_idx];
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

        // Mark the consumed leaf as occupied and remove from the free list.
        self.nodes[leaf_idx].kind = ForestNodeKind::Occupied;
        let pos = self
            .free_leaves
            .iter()
            .position(|&i| i == leaf_idx)
            .expect("leaf_idx must be present in free_leaves");
        self.free_leaves.swap_remove(pos);

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
    /// Returns `(node_idx, placed_w, placed_h, rotated)` or `None`.
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
            let node_idx = self.free_leaves[(start + i) % n];
            let node = &self.nodes[node_idx];
            if let Some((pw, ph, rotated)) = piece_fits_in(node.w, node.h, piece, prefer_rotate) {
                return Some((node_idx, pw, ph, rotated));
            }
        }
        None
    }

    /// Convert the forest to a flat `Vec<CutNode>` for use with `mfg_cost_from_trees`.
    ///
    /// **Not yet implemented** — returns an empty vec.
    /// Will be closed once the tree-reconstruction logic is added.
    pub fn to_cut_trees(&self) -> Vec<CutNode> {
        // TODO: walk arena and reconstruct CutNode trees
        Vec::new()
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

// TL-corner improvement

/// Leftmost x of the bounding rect covered by `node`.
fn node_x(node: &CutNode) -> u32 {
    match node {
        CutNode::Piece { x, .. } | CutNode::Waste { x, .. } => *x,
        CutNode::VSplit { left, .. } => node_x(left),
        CutNode::HSplit { top, .. } => node_x(top),
    }
}

/// Topmost y of the bounding rect covered by `node`.
fn node_y(node: &CutNode) -> u32 {
    match node {
        CutNode::Piece { y, .. } | CutNode::Waste { y, .. } => *y,
        CutNode::VSplit { left, .. } => node_y(left),
        CutNode::HSplit { top, .. } => node_y(top),
    }
}

/// Rightmost x (exclusive) of the bounding rect covered by `node`.
fn node_right(node: &CutNode) -> u32 {
    match node {
        CutNode::Piece { x, pw, .. } => x + pw,
        CutNode::Waste { x, w, .. } => x + w,
        CutNode::VSplit { right, .. } => node_right(right),
        CutNode::HSplit { top, bottom, .. } => node_right(top).max(node_right(bottom)),
    }
}

/// Bottom y (exclusive) of the bounding rect covered by `node`.
fn node_bottom(node: &CutNode) -> u32 {
    match node {
        CutNode::Piece { y, ph, .. } => y + ph,
        CutNode::Waste { y, h, .. } => y + h,
        CutNode::VSplit { left, right, .. } => node_bottom(left).max(node_bottom(right)),
        CutNode::HSplit { bottom, .. } => node_bottom(bottom),
    }
}

/// Area of the piece at the top-left corner of the subtree rooted at `node`.
/// Returns 0 for a Waste leaf (no piece there).
fn tl_corner_area(node: &CutNode) -> u64 {
    match node {
        CutNode::Piece { pw, ph, .. } => (*pw as u64) * (*ph as u64),
        CutNode::Waste { .. } => 0,
        CutNode::VSplit { left, .. } => tl_corner_area(left),
        CutNode::HSplit { top, .. } => tl_corner_area(top),
    }
}

/// Shift all absolute x-coordinates in `node` by `dx`.
fn translate_x(node: &mut CutNode, dx: i32) {
    match node {
        CutNode::Piece { x, .. } | CutNode::Waste { x, .. } => {
            *x = x.wrapping_add_signed(dx);
        }
        CutNode::VSplit { cut_x, left, right } => {
            *cut_x = cut_x.wrapping_add_signed(dx);
            translate_x(left, dx);
            translate_x(right, dx);
        }
        CutNode::HSplit { top, bottom, .. } => {
            translate_x(top, dx);
            translate_x(bottom, dx);
        }
    }
}

/// Shift all absolute y-coordinates in `node` by `dy`.
fn translate_y(node: &mut CutNode, dy: i32) {
    match node {
        CutNode::Piece { y, .. } | CutNode::Waste { y, .. } => {
            *y = y.wrapping_add_signed(dy);
        }
        CutNode::VSplit { left, right, .. } => {
            translate_y(left, dy);
            translate_y(right, dy);
        }
        CutNode::HSplit { cut_y, top, bottom } => {
            *cut_y = cut_y.wrapping_add_signed(dy);
            translate_y(top, dy);
            translate_y(bottom, dy);
        }
    }
}

/// Rightmost x (exclusive) of all Piece leaves in `node`; `None` if only Waste.
fn piece_right(node: &CutNode) -> Option<u32> {
    match node {
        CutNode::Piece { x, pw, .. } => Some(x + pw),
        CutNode::Waste { .. } => None,
        CutNode::VSplit { left, right, .. } => {
            [piece_right(left), piece_right(right)].into_iter().flatten().max()
        }
        CutNode::HSplit { top, bottom, .. } => {
            [piece_right(top), piece_right(bottom)].into_iter().flatten().max()
        }
    }
}

/// Bottom y (exclusive) of all Piece leaves in `node`; `None` if only Waste.
fn piece_bottom(node: &CutNode) -> Option<u32> {
    match node {
        CutNode::Piece { y, ph, .. } => Some(y + ph),
        CutNode::Waste { .. } => None,
        CutNode::VSplit { left, right, .. } => {
            [piece_bottom(left), piece_bottom(right)].into_iter().flatten().max()
        }
        CutNode::HSplit { top, bottom, .. } => {
            [piece_bottom(top), piece_bottom(bottom)].into_iter().flatten().max()
        }
    }
}

/// At the top-level split of a sheet tree, swap children if the right/bottom child
/// has a larger TL-corner piece than the left/top child.  Only the root node is
/// examined — no recursion into children.
///
/// The cut coordinate is set to the content extent of the moved child (piece_right /
/// piece_bottom, ignoring trailing Waste), so trailing free space ends up at the
/// sheet boundary rather than in the middle of the layout.
fn sort_tl_corners(node: &mut CutNode) {
    match node {
        CutNode::Piece { .. } | CutNode::Waste { .. } => {}
        CutNode::VSplit { cut_x, left, right } => {
            if tl_corner_area(left) < tl_corner_area(right) {
                let x0 = node_x(left);
                // Content width of old right: up to the last Piece, not trailing Waste.
                let content_w = piece_right(right)
                    .map(|r| r - *cut_x)
                    .unwrap_or_else(|| node_right(right) - *cut_x);
                // Old right shifts left to start at x0.
                translate_x(right, x0 as i32 - *cut_x as i32);
                // Old left shifts right by content_w only, pushing free space to the right.
                translate_x(left, content_w as i32);
                std::mem::swap(left, right);
                *cut_x = x0 + content_w;
            }
        }
        CutNode::HSplit { cut_y, top, bottom } => {
            if tl_corner_area(top) < tl_corner_area(bottom) {
                let y0 = node_y(top);
                // Content height of old bottom: up to the last Piece, not trailing Waste.
                let content_h = piece_bottom(bottom)
                    .map(|b| b - *cut_y)
                    .unwrap_or_else(|| node_bottom(bottom) - *cut_y);
                // Old bottom shifts up to start at y0.
                translate_y(bottom, y0 as i32 - *cut_y as i32);
                // Old top shifts down by content_h only, pushing free space to the bottom.
                translate_y(top, content_h as i32);
                std::mem::swap(top, bottom);
                *cut_y = y0 + content_h;
            }
        }
    }
}

/// Collect `piece_idx → (x, y)` from all `Piece` leaves in the tree.
fn collect_positions(node: &CutNode, map: &mut HashMap<usize, (u32, u32)>) {
    match node {
        CutNode::Piece { piece_idx, x, y, .. } => {
            map.insert(*piece_idx, (*x, *y));
        }
        CutNode::Waste { .. } => {}
        CutNode::VSplit { left, right, .. } => {
            collect_positions(left, map);
            collect_positions(right, map);
        }
        CutNode::HSplit { top, bottom, .. } => {
            collect_positions(top, map);
            collect_positions(bottom, map);
        }
    }
}

/// Post-decode local improvement: one recursive pass over the cut tree that
/// pushes the largest piece toward the top-left corner of each split node.
///
/// Builds the cut tree from `sol.placements`, applies `sort_tl_corners` to every
/// sheet tree, updates placement coordinates, and recomputes `mfg_cost`.
/// Returns `sol` unchanged if the cut tree cannot be reconstructed (non-guillotine layout).
pub fn improve_tl_corners(problem: &Problem, mut sol: Solution) -> Solution {
    let Ok(mut trees) = build_cut_tree(problem, &sol.placements) else {
        return sol;
    };
    for tree in &mut trees {
        sort_tl_corners(tree);
    }
    let mut pos_map: HashMap<usize, (u32, u32)> = HashMap::new();
    for tree in &trees {
        collect_positions(tree, &mut pos_map);
    }
    for p in &mut sol.placements {
        if let Some(&(x, y)) = pos_map.get(&p.piece_idx) {
            p.x = x;
            p.y = y;
        }
    }
    sol.mfg_cost = mfg_cost_from_trees(&trees);
    sol
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
        let leaf = forest.free_leaves[0];
        let (bx, by) = forest.apply_blueprint(leaf, 100, 100, Blueprint::TlH);
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
        let leaf = forest.free_leaves[0];
        let (bx, by) = forest.apply_blueprint(leaf, 100, 100, Blueprint::TlV);
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
        let leaf = forest.free_leaves[0];
        let (bx, by) = forest.apply_blueprint(leaf, 100, 100, Blueprint::TlH);
        assert_eq!((bx, by), (0, 0));
        assert!(forest.free_leaves.is_empty());
    }

    #[test]
    fn forest_exact_w() {
        // Sheet 100×200, batch 100×100: lw=0, lh=100.
        // TlH: batch at (0,0), only bottom free: (0,100,100,100).
        let mut forest = CutForest::new(100, 200);
        let leaf = forest.free_leaves[0];
        let (bx, by) = forest.apply_blueprint(leaf, 100, 100, Blueprint::TlH);
        assert_eq!((bx, by), (0, 0));
        assert_eq!(free_rects(&forest), vec![(0, 100, 100, 100)]);
    }

    #[test]
    fn forest_exact_h() {
        // Sheet 200×100, batch 100×100: lw=100, lh=0.
        // TlH: batch at (0,0), only right free: (100,0,100,100).
        let mut forest = CutForest::new(200, 100);
        let leaf = forest.free_leaves[0];
        let (bx, by) = forest.apply_blueprint(leaf, 100, 100, Blueprint::TlH);
        assert_eq!((bx, by), (0, 0));
        assert_eq!(free_rects(&forest), vec![(100, 0, 100, 100)]);
    }

    // sort_tl_corners tests

    /// VSplit where left piece (10×10=100) is smaller than right piece (20×20=400)
    /// → after sort_tl_corners the large piece must be on the left.
    #[test]
    fn sort_corners_vsplit_swaps() {
        // Build tree manually: VSplit at x=10, left=Piece(10×10), right=Piece(20×20).
        // Sheet would be 30×20.
        let mut tree = CutNode::VSplit {
            cut_x: 10,
            left: Box::new(CutNode::Piece {
                piece_idx: 0,
                x: 0,
                y: 0,
                pw: 10,
                ph: 20,
            }),
            right: Box::new(CutNode::Piece {
                piece_idx: 1,
                x: 10,
                y: 0,
                pw: 20,
                ph: 20,
            }),
        };
        sort_tl_corners(&mut tree);
        match &tree {
            CutNode::VSplit { cut_x, left, right } => {
                assert_eq!(*cut_x, 20, "cut_x should shift to width of new left child");
                // Large piece (idx=1, 20×20) is now left
                assert!(matches!(left.as_ref(), CutNode::Piece { piece_idx: 1, x: 0, .. }));
                // Small piece (idx=0, 10×10) is now right
                assert!(matches!(
                    right.as_ref(),
                    CutNode::Piece {
                        piece_idx: 0,
                        x: 20,
                        ..
                    }
                ));
            }
            _ => panic!("expected VSplit"),
        }
    }

    /// VSplit where left piece is already larger — no swap should occur.
    #[test]
    fn sort_corners_no_swap_when_left_larger() {
        let mut tree = CutNode::VSplit {
            cut_x: 20,
            left: Box::new(CutNode::Piece {
                piece_idx: 0,
                x: 0,
                y: 0,
                pw: 20,
                ph: 20,
            }),
            right: Box::new(CutNode::Piece {
                piece_idx: 1,
                x: 20,
                y: 0,
                pw: 10,
                ph: 20,
            }),
        };
        sort_tl_corners(&mut tree);
        match &tree {
            CutNode::VSplit { cut_x, left, .. } => {
                assert_eq!(*cut_x, 20);
                assert!(matches!(left.as_ref(), CutNode::Piece { piece_idx: 0, .. }));
            }
            _ => panic!("expected VSplit"),
        }
    }

    /// HSplit where top piece (10×5=50) is smaller than bottom piece (10×15=150)
    /// → after sort_tl_corners the large piece must be on top.
    #[test]
    fn sort_corners_hsplit_swaps() {
        let mut tree = CutNode::HSplit {
            cut_y: 5,
            top: Box::new(CutNode::Piece {
                piece_idx: 0,
                x: 0,
                y: 0,
                pw: 10,
                ph: 5,
            }),
            bottom: Box::new(CutNode::Piece {
                piece_idx: 1,
                x: 0,
                y: 5,
                pw: 10,
                ph: 15,
            }),
        };
        sort_tl_corners(&mut tree);
        match &tree {
            CutNode::HSplit { cut_y, top, bottom } => {
                assert_eq!(*cut_y, 15, "cut_y should shift to height of new top child");
                assert!(matches!(top.as_ref(), CutNode::Piece { piece_idx: 1, y: 0, .. }));
                assert!(matches!(
                    bottom.as_ref(),
                    CutNode::Piece {
                        piece_idx: 0,
                        y: 15,
                        ..
                    }
                ));
            }
            _ => panic!("expected HSplit"),
        }
    }

    /// After improve_tl_corners the solution must still be valid (build_cut_tree succeeds).
    #[test]
    fn improve_roundtrip_stays_guillotine() {
        // Two pieces of different sizes on one sheet.
        let spec = parse_problem("100x100F:0:30x30/1,60x60/1").unwrap();
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
        let improved = crate::cut_tree::improve_tl_corners(&problem, sol);
        assert!(build_cut_tree(&problem, &improved.placements).is_ok());
    }
}
