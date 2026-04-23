#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sheet {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    /// Whether the piece may be rotated 90°. When false, it must be placed
    /// in its original (width × height) orientation.
    pub can_rotate: bool,
}

#[derive(Debug, Clone)]
pub struct Problem {
    pub sheet: Sheet,
    /// Blade kerf width in mm, subtracted from child rects at each guillotine split.
    /// Boundary edges of the sheet do not consume kerf.
    pub kerf: u32,
    pub pieces: Vec<Piece>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub sheet_index: usize,
    pub piece_id: u32,
    pub x: u32,
    pub y: u32,
    pub rotated: bool,
}

#[derive(Debug, Clone)]
pub struct Solution {
    pub placements: Vec<Placement>,
}

impl Solution {
    pub fn sheets_used(&self) -> usize {
        self.placements
            .iter()
            .map(|p| p.sheet_index)
            .max()
            .map(|m| m + 1)
            .unwrap_or(0)
    }
}
