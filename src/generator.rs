use rand::prelude::{IndexedRandom, Rng, RngExt, SliceRandom};
use serde::{Deserialize, Serialize};

use crate::model::{Piece, PieceId, Placement, Problem, Sheet, Solution};

/// A rectangle used transiently during the recursive splitting phase.
#[derive(Debug, Clone)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Certificate {
    sufficiency: String,
    necessity: String,
    total_piece_area: u64,
    sheet_area: u64,
    area_lower_bound_sheets: u32,
    optimum_tight: bool,
}

#[derive(Debug)]
pub struct Output {
    pub problem: Problem,
    pub optimal_solution: Solution,
    pub certificate: Certificate,
}

pub fn generate<R: Rng>(sheet: Sheet, k: u32, n: u32, min_size: u32, rng: &mut R) -> Output {
    let w = sheet.width;
    let h = sheet.height;
    let allow_rotation = false;
    let counts = distribute(n, k, rng);

    // placements and pieces are built in parallel; piece IDs are 1-based u32
    // to match PieceId = u32 in the model.
    let mut placements: Vec<Placement> = Vec::new();
    let mut pieces_ordered: Vec<Piece> = Vec::new(); // in generation order
    let mut next_id: PieceId = 1;

    for (sheet_idx, &count) in counts.iter().enumerate() {
        let root = Rect { x: 0, y: 0, w, h };
        let budget = count.min(w * h);
        let mut leaves = split_rect(root, min_size, budget, rng);

        // If we got fewer leaves than requested, try splitting larger ones.
        while leaves.len() < count as usize {
            let splittable: Vec<usize> = leaves
                .iter()
                .enumerate()
                .filter(|(_, r)| r.w > min_size * 2 || r.h > min_size * 2)
                .map(|(i, _)| i)
                .collect();
            if splittable.is_empty() {
                break;
            }
            let pick = *splittable.choose(rng).unwrap();
            let r = leaves.remove(pick);
            let new_leaves = split_rect(r, min_size, 2, rng);
            leaves.extend(new_leaves);
        }

        for leaf in leaves {
            let id: PieceId = next_id;
            next_id += 1;

            // Placement always records the piece in its canonical (non-rotated) orientation.
            placements.push(Placement {
                sheet_idx, // 0-indexed, as required by the model
                piece_id: id,
                x: leaf.x,
                y: leaf.y,
                rotated: false,
            });

            pieces_ordered.push(Piece {
                id,
                width: leaf.w,
                height: leaf.h,
                can_rotate: allow_rotation,
            });
        }
    }

    // Build the Problem: shuffle pieces, optionally swap w/h to simulate rotated input.
    let mut problem_pieces: Vec<Piece> = pieces_ordered
        .iter()
        .map(|p| {
            let (pw, ph) = if allow_rotation && rng.random_bool(0.5) {
                (p.height, p.width) // present the piece rotated in the problem statement
            } else {
                (p.width, p.height)
            };
            Piece {
                id: p.id,
                width: pw,
                height: ph,
                can_rotate: p.can_rotate,
            }
        })
        .collect();
    problem_pieces.shuffle(rng);

    let problem = Problem {
        sheet: Sheet { width: w, height: h },
        kerf: 0, // generator produces zero-kerf instances
        pieces: problem_pieces,
    };

    // The optimal solution has no leftover rectangles (pieces tile the sheets perfectly).
    let solution = Solution {
        placements,
        leftovers: Vec::new(),
    };

    // Certificate of optimality.
    let sheet_area = w as u64 * h as u64;
    let total_piece_area: u64 = pieces_ordered.iter().map(|p| p.width as u64 * p.height as u64).sum();
    let area_lb = ((total_piece_area + sheet_area - 1) / sheet_area) as u32;
    let optimum_tight = area_lb == k;

    let sufficiency = format!(
        "Constructed by applying random guillotine cuts to exactly {k} sheets of size {w}x{h}. \
         The {n} resulting pieces tile the sheets with zero waste, so {k} sheets are sufficient.",
        n = pieces_ordered.len()
    );

    let necessity = if optimum_tight {
        format!(
            "Total piece area = {total_piece_area}, sheet area = {sheet_area}. \
             ceil({total_piece_area}/{sheet_area}) = {area_lb} = K={k}, \
             so {k} sheets are necessary by the area lower bound. Optimality is proven.",
        )
    } else {
        format!(
            "Total piece area = {total_piece_area}, sheet area = {sheet_area}. \
             Area lower bound = {area_lb} < K={k}. \
             Optimality is guaranteed constructively: the pieces were obtained by cutting \
             exactly {k} sheets, so no fewer than {k} sheets can hold them in this layout. \
             A combinatorial proof that no (K-1)-sheet guillotine packing exists \
             requires a solver.",
        )
    };
    Output {
        problem,
        optimal_solution: solution,
        certificate: Certificate {
            sufficiency,
            necessity,
            total_piece_area,
            sheet_area,
            area_lower_bound_sheets: area_lb,
            optimum_tight,
        },
    }
}

/// Distribute `total` into `k` positive integers that sum to `total`.
fn distribute(total: u32, k: u32, rng: &mut impl Rng) -> Vec<u32> {
    assert!(total >= k, "Need at least one piece per sheet");
    if k == 1 {
        return vec![total];
    }
    let mut dividers: std::collections::HashSet<u32> = std::collections::HashSet::new();
    while dividers.len() < (k - 1) as usize {
        dividers.insert(rng.random_range(1..total));
    }
    let mut dividers: Vec<u32> = dividers.into_iter().collect();
    dividers.sort_unstable();

    let mut counts = Vec::with_capacity(k as usize);
    let mut prev = 0u32;
    for &d in &dividers {
        counts.push(d - prev);
        prev = d;
    }
    counts.push(total - prev);
    counts
}

/// Recursively split `rect` into at most `budget` pieces using guillotine cuts.
fn split_rect<R: Rng>(rect: Rect, min_size: u32, budget: u32, rng: &mut R) -> Vec<Rect> {
    if budget <= 1 || (rect.w <= min_size * 2 && rect.h <= min_size * 2) {
        return vec![rect];
    }
    let can_vertical = rect.w > min_size * 2;
    let can_horizontal = rect.h > min_size * 2;
    if !can_vertical && !can_horizontal {
        return vec![rect];
    }
    let vertical = match (can_vertical, can_horizontal) {
        (true, false) => true,
        (false, true) => false,
        _ => rng.random_bool(0.5),
    };
    let half = budget / 2;
    if vertical {
        let cut = rng.random_range(min_size..rect.w - min_size + 1);
        let left = Rect {
            x: rect.x,
            y: rect.y,
            w: cut,
            h: rect.h,
        };
        let right = Rect {
            x: rect.x + cut,
            y: rect.y,
            w: rect.w - cut,
            h: rect.h,
        };
        let mut pieces = split_rect(left, min_size, half, rng);
        pieces.extend(split_rect(right, min_size, budget - half, rng));
        pieces
    } else {
        let cut = rng.random_range(min_size..rect.h - min_size + 1);
        let top = Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: cut,
        };
        let bottom = Rect {
            x: rect.x,
            y: rect.y + cut,
            w: rect.w,
            h: rect.h - cut,
        };
        let mut pieces = split_rect(top, min_size, half, rng);
        pieces.extend(split_rect(bottom, min_size, budget - half, rng));
        pieces
    }
}
