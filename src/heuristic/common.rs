use std::cmp::Ordering;

use crate::model::{FreeRect, Problem};

/// Piece ordering criterion for a Jylanki pass. All keys compare on u64 to
/// avoid overflow; Ratio compares w/h by cross-multiplication (no floats).
#[derive(Clone, Copy)]
pub(crate) enum SortKey {
    Area,
    ShortSide,
    LongSide,
    Perimeter,
    Diff,
    Ratio,
}

#[derive(Clone, Copy)]
pub(crate) enum SortDir {
    Asc,
    Desc,
}

/// Free-rect selection criterion; the candidate (rect, orientation) with the
/// minimum score wins.
#[derive(Clone, Copy)]
pub(crate) enum SelectionRule {
    /// Smallest rect area (Best-Area-Fit); orientation-independent, ties resolved by tie-break.
    Area,
    /// Smallest min(fr.w - pw, fr.h - ph) — tightest short-side fit.
    ShortSide,
    /// Smallest max(fr.w - pw, fr.h - ph) — tightest long-side fit.
    LongSide,
}

/// Cut direction rule applied after placing a piece into a free rect.
/// `horizontal` in `split_directional` terms: the full-span cut runs horizontally.
#[derive(Clone, Copy)]
pub(crate) enum SplitRule {
    /// Horizontal when the right leftover strip is the narrower one (== SLAS).
    ShorterLeftover,
    /// Horizontal when the right leftover strip is the wider one (== LLAS).
    LongerLeftover,
    /// Horizontal when the free rect is taller than wide.
    ShortAxis,
    /// Horizontal when the free rect is wider than tall.
    LongAxis,
}

pub(crate) const SORT_KEYS: [SortKey; 6] = [
    SortKey::Area,
    SortKey::ShortSide,
    SortKey::LongSide,
    SortKey::Perimeter,
    SortKey::Diff,
    SortKey::Ratio,
];
pub(crate) const SORT_DIRS: [SortDir; 2] = [SortDir::Asc, SortDir::Desc];
pub(crate) const SELECTION_RULES: [SelectionRule; 3] =
    [SelectionRule::Area, SelectionRule::ShortSide, SelectionRule::LongSide];
pub(crate) const SPLIT_RULES: [SplitRule; 4] = [
    SplitRule::ShorterLeftover,
    SplitRule::LongerLeftover,
    SplitRule::ShortAxis,
    SplitRule::LongAxis,
];

pub(crate) fn sort_cmp(problem: &Problem, a: usize, b: usize, key: SortKey) -> Ordering {
    let pa = &problem.pieces[a];
    let pb = &problem.pieces[b];
    let (aw, ah) = (pa.width as u64, pa.height as u64);
    let (bw, bh) = (pb.width as u64, pb.height as u64);
    match key {
        SortKey::Area => (aw * ah).cmp(&(bw * bh)),
        SortKey::ShortSide => (aw.min(ah), aw.max(ah)).cmp(&(bw.min(bh), bw.max(bh))),
        SortKey::LongSide => (aw.max(ah), aw.min(ah)).cmp(&(bw.max(bh), bw.min(bh))),
        SortKey::Perimeter => (aw + ah).cmp(&(bw + bh)),
        SortKey::Diff => aw.abs_diff(ah).cmp(&bw.abs_diff(bh)),
        SortKey::Ratio => (aw * bh).cmp(&(bw * ah)),
    }
}

pub(crate) fn selection_score(sel: SelectionRule, fr: &FreeRect, pw: u32, ph: u32) -> u64 {
    let lw = (fr.w - pw) as u64;
    let lh = (fr.h - ph) as u64;
    match sel {
        SelectionRule::Area => fr.w as u64 * fr.h as u64,
        SelectionRule::ShortSide => lw.min(lh),
        SelectionRule::LongSide => lw.max(lh),
    }
}
