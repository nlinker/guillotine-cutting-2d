use crate::model::{Placement, Problem};

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
    /// Internal: horizontal cut at `cut_y`. `top`=[rect.y,cut_y), `bottom`=[cut_y,rect.y+rect.h).
    HSplit {
        cut_y: u32,
        top: Box<CutNode>,
        bottom: Box<CutNode>,
    },
    /// Internal: vertical cut at `cut_x`. `left`=[rect.x,cut_x), `right`=[cut_x,rect.x+rect.w).
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

/// Reconstructs a guillotine cut tree per sheet from already-placed pieces, by
/// recursively trying H/V cuts at piece boundaries until each rectangle splits
/// into two independent halves.
///
/// Returns one `CutNode` per sheet, or `Err` if a region isn't guillotine-splittable.
pub fn build_cut_tree(problem: &Problem, placements: &[Placement]) -> Result<Vec<CutNode>, String> {
    let n_sheets = placements.iter().map(|p| p.sheet_idx).max().map_or(0, |m| m + 1);
    let sw = problem.sheet.width;
    let sh = problem.sheet.height;

    (0..n_sheets)
        .map(|sheet_idx| {
            let sheet_placements = placements
                .iter()
                .filter(|p| p.sheet_idx == sheet_idx)
                .map(|p| {
                    let piece = &problem.pieces[p.piece_idx];
                    let (pw, ph) = if p.rotated {
                        (piece.height, piece.width)
                    } else {
                        (piece.width, piece.height)
                    };
                    PlacedPiece { piece_idx: p.piece_idx, x: p.x, y: p.y, pw, ph }
                })
                .collect::<Vec<PlacedPiece>>();
            let rect = Rect { x: 0, y: 0, w: sw, h: sh };
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
            return Some(CutNode::Waste { x: rect.x, y: rect.y, w: rect.w, h: rect.h });
        }
        1 => {
            let p = pieces[0];
            if p.x == rect.x && p.y == rect.y && p.pw == rect.w && p.ph == rect.h {
                return Some(CutNode::Piece { piece_idx: p.piece_idx, x: p.x, y: p.y, pw: p.pw, ph: p.ph });
            }
            // Doesn't fill the rect - fall through to the general case.
        }
        _ => {}
    }

    // Candidate H-cuts: piece top/bottom edges strictly inside the rect.
    let mut h_cuts = pieces
        .iter()
        .flat_map(|p| [p.y, p.bottom()])
        .filter(|&y| y > rect.y && y < rect.y + rect.h)
        .collect::<Vec<u32>>();
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
        let top_rect = Rect { x: rect.x, y: rect.y, w: rect.w, h: cut_y - rect.y };
        let bot_rect = Rect { x: rect.x, y: cut_y, w: rect.w, h: rect.y + rect.h - cut_y };
        if let (Some(top_node), Some(bot_node)) = (split(top_rect, &top_pieces), split(bot_rect, &bot_pieces)) {
            return Some(CutNode::HSplit { cut_y, top: Box::new(top_node), bottom: Box::new(bot_node) });
        }
    }

    // Collect candidate V-cut positions.
    let mut v_cuts = pieces
        .iter()
        .flat_map(|p| [p.x, p.right()])
        .filter(|&x| x > rect.x && x < rect.x + rect.w)
        .collect::<Vec<u32>>();
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
        let left_rect = Rect { x: rect.x, y: rect.y, w: cut_x - rect.x, h: rect.h };
        let right_rect = Rect { x: cut_x, y: rect.y, w: rect.x + rect.w - cut_x, h: rect.h };
        if let (Some(left_node), Some(right_node)) = (split(left_rect, &left_pieces), split(right_rect, &right_pieces))
        {
            return Some(CutNode::VSplit { cut_x, left: Box::new(left_node), right: Box::new(right_node) });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        expand::expand_problem,
        model::{Piece, Sheet},
        parser::compact::parse_problem,
        slas::decoder::decode,
    };

    #[test]
    fn single_piece_fills_sheet() {
        let spec = parse_problem("10x8F::10x8").unwrap();
        let problem = expand_problem(&spec);
        let genome =
            vec![crate::slas::decoder::Gene { piece_idx: 0, rotate: false, point_selector: 0, inverse: false }];
        let sol = decode(&problem, &genome);
        let trees = build_cut_tree(&problem, &sol.placements).unwrap();
        assert_eq!(trees.len(), 1);
        assert!(matches!(trees[0], CutNode::Piece { .. }));
    }

    #[test]
    fn two_pieces_side_by_side() {
        // Sheet 10×5, two pieces 5×5.
        let spec = parse_problem("10x5F::5x5/2").unwrap();
        let problem = expand_problem(&spec);
        let genome = vec![
            crate::slas::decoder::Gene { piece_idx: 0, rotate: false, point_selector: 0, inverse: false },
            crate::slas::decoder::Gene { piece_idx: 1, rotate: false, point_selector: 0, inverse: false },
        ];
        let sol = decode(&problem, &genome);
        assert_eq!(sol.sheets_used(), 1);
        let trees = build_cut_tree(&problem, &sol.placements).unwrap();
        assert_eq!(trees.len(), 1);
        assert!(matches!(trees[0], CutNode::VSplit { .. } | CutNode::HSplit { .. }));
    }

    #[test]
    fn windmill_is_not_guillotine_splittable() {
        // Windmill taken from docs/img/guillotine_explain.png.
        let piece = |w, h| Piece { name: String::new(), width: w, height: h, can_rotate: false };
        let problem = Problem {
            sheet: Sheet { width: 30, height: 30 },
            pieces: vec![
                piece(10, 20),
                piece(20, 10),
                piece(10, 10),
                piece(10, 20),
                piece(20, 10),
            ],
        };
        let placements = vec![
            Placement { sheet_idx: 0, piece_idx: 0, x: 0, y: 0, rotated: false },
            Placement { sheet_idx: 0, piece_idx: 1, x: 10, y: 0, rotated: false },
            Placement { sheet_idx: 0, piece_idx: 2, x: 10, y: 10, rotated: false },
            Placement { sheet_idx: 0, piece_idx: 3, x: 20, y: 10, rotated: false },
            Placement { sheet_idx: 0, piece_idx: 4, x: 0, y: 20, rotated: false },
        ];
        assert!(build_cut_tree(&problem, &placements).is_err());
    }
}
