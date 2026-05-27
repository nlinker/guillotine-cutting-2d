use std::collections::HashMap;

use crate::model::{Problem, ProblemSpec};

/// A piece-type match returned by dimension queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeMatch {
    /// Index into [`ProblemSpec::piespecs`].
    pub type_idx: usize,
    /// `true` when the piece must be **rotated** to achieve the queried dimension.
    pub rotated: bool,
}

/// Inverted-dimension index: maps an expanded (kerf-padded) piece dimension to every
/// piece type whose piece has that dimension in at least one orientation.
///
/// # Build
/// Call [`DimIndex::build`] once, before the decode loop.
/// It scans `spec.piespecs` in order, uses the first flat piece of each type as
/// a representative (all copies within a type are identical after `expand_problem`),
/// and inserts:
///
/// - **normal** orientation `(pw, ph)` → `{ type_idx, rotated: false }`;
/// - **rotated** orientation `(ph, pw)` → `{ type_idx, rotated: true }`,
///   only when `can_rotate` and `pw ≠ ph` (otherwise redundant).
///
/// # Queries
/// - [`DimIndex::by_width`]`(w)` — types whose piece width equals `w`.
/// - [`DimIndex::by_height`]`(h)` — types whose piece height equals `h`.
///
/// Both queries run in **O(1)** and return a slice of [`TypeMatch`] values.
///
/// # Typical use
/// ```ignore
/// let idx = DimIndex::build(spec, &problem);
/// // find types that fill free-rect width exactly (snug horizontal fit):
/// for tm in idx.by_width(fr.w) {
///     // tm.type_idx, tm.rotated
/// }
/// ```
pub struct DimIndex {
    by_w: HashMap<u32, Vec<TypeMatch>>,
    by_h: HashMap<u32, Vec<TypeMatch>>,
}

impl DimIndex {
    /// Build the index from the expanded `problem` and the original `spec`.
    pub fn build(spec: &ProblemSpec, problem: &Problem) -> Self {
        let mut by_w: HashMap<u32, Vec<TypeMatch>> = HashMap::new();
        let mut by_h: HashMap<u32, Vec<TypeMatch>> = HashMap::new();

        let mut flat_offset = 0usize;
        for (type_idx, ps) in spec.piespecs.iter().enumerate() {
            let p = &problem.pieces[flat_offset]; // one representative per type

            // Normal orientation.
            by_w.entry(p.width).or_default().push(TypeMatch { type_idx, rotated: false });
            by_h.entry(p.height).or_default().push(TypeMatch { type_idx, rotated: false });

            // Rotated orientation — only when it differs (non-square rotatable piece).
            if p.can_rotate && p.width != p.height {
                by_w.entry(p.height).or_default().push(TypeMatch { type_idx, rotated: true });
                by_h.entry(p.width).or_default().push(TypeMatch { type_idx, rotated: true });
            }

            flat_offset += ps.count as usize;
        }

        Self { by_w, by_h }
    }

    /// All types whose (possibly rotated) piece **width** equals `w`.
    pub fn by_width(&self, w: u32) -> &[TypeMatch] {
        self.by_w.get(&w).map(Vec::as_slice).unwrap_or(&[])
    }

    /// All types whose (possibly rotated) piece **height** equals `h`.
    pub fn by_height(&self, h: u32) -> &[TypeMatch] {
        self.by_h.get(&h).map(Vec::as_slice).unwrap_or(&[])
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{expand::expand_problem, parse::parse_problem};

    /// Helper: kerf=0 so expanded dims == spec dims.
    fn build(problem_str: &str) -> (ProblemSpec, Problem, DimIndex) {
        let spec = parse_problem(problem_str).expect("parse");
        let problem = expand_problem(&spec);
        let idx = DimIndex::build(&spec, &problem);
        (spec, problem, idx)
    }

    #[test]
    fn non_rotatable_indexed_once() {
        // 300×200 fixed → only one orientation in index.
        let (_, _, idx) = build("1000x1000F:0:300x200/2f");
        // width=300, height=200 should each map to type 0, rotated=false.
        assert_eq!(idx.by_width(300), &[TypeMatch { type_idx: 0, rotated: false }]);
        assert_eq!(idx.by_height(200), &[TypeMatch { type_idx: 0, rotated: false }]);
        // Reversed dims not indexed.
        assert!(idx.by_width(200).is_empty());
        assert!(idx.by_height(300).is_empty());
    }

    #[test]
    fn rotatable_non_square_indexed_both_orientations() {
        // 300×200 rotatable: normalize() swaps to canonical 200×300 (width ≤ height).
        // Canonical orientation: width=200, height=300 (rotated=false).
        // Rotated orientation:   width=300, height=200 (rotated=true).
        let (_, _, idx) = build("1000x1000R:0:300x200/2r");
        assert_eq!(idx.by_width(200), &[TypeMatch { type_idx: 0, rotated: false }]);
        assert_eq!(idx.by_height(300), &[TypeMatch { type_idx: 0, rotated: false }]);
        assert_eq!(idx.by_width(300), &[TypeMatch { type_idx: 0, rotated: true }]);
        assert_eq!(idx.by_height(200), &[TypeMatch { type_idx: 0, rotated: true }]);
    }

    #[test]
    fn square_rotatable_indexed_once() {
        // 200×200 rotatable → w==h so rotation is redundant; index once.
        let (_, _, idx) = build("1000x1000R:0:200x200/3r");
        assert_eq!(idx.by_width(200), &[TypeMatch { type_idx: 0, rotated: false }]);
        assert_eq!(idx.by_height(200), &[TypeMatch { type_idx: 0, rotated: false }]);
        // No duplicate entry.
        assert_eq!(idx.by_width(200).len(), 1);
    }

    #[test]
    fn multiple_types_same_dimension() {
        // Two types with width=100 (different heights).
        // kerf=0 so expanded dims = spec dims.
        let (_, _, idx) = build("1000x1000F:0:100x200/1f,100x300/1f");
        let matches = idx.by_width(100);
        assert_eq!(matches.len(), 2);
        assert!(matches.contains(&TypeMatch { type_idx: 0, rotated: false }));
        assert!(matches.contains(&TypeMatch { type_idx: 1, rotated: false }));
    }

    #[test]
    fn unknown_dimension_returns_empty() {
        let (_, _, idx) = build("1000x1000F:0:300x200/1f");
        assert!(idx.by_width(999).is_empty());
        assert!(idx.by_height(999).is_empty());
    }

    #[test]
    fn kerf_padded_dimensions_are_indexed() {
        // kerf=10 → expanded piece is (300+10)×(200+10) = 310×210.
        let (_, _, idx) = build("1000x1000F:10:300x200/1f");
        // Expanded widths are indexed, not original.
        assert_eq!(idx.by_width(310), &[TypeMatch { type_idx: 0, rotated: false }]);
        assert_eq!(idx.by_height(210), &[TypeMatch { type_idx: 0, rotated: false }]);
        assert!(idx.by_width(300).is_empty()); // raw (non-expanded) width not present
    }
}
