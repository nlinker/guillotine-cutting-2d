/// Stock sheet — all sheets in the problem are identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sheet {
    pub width: u32,
    pub height: u32,
}

/// A rectangular piece to be cut from a sheet. `id`, `width`, `height` are obvious.
/// `can_rotate` - when false, must be placed in original (width × height) orientation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub can_rotate: bool,
}

/// A cutting problem: identical sheets, pieces to place, and a blade kerf (mm)
/// subtracted at each guillotine split; boundary edges of the sheet are exempt.
#[derive(Debug, Clone)]
pub struct Problem {
    pub sheet: Sheet,
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
