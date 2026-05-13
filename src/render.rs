use std::fmt::Write as _;

use crate::model::{Problem, Solution};

const PALETTE: &[&str] = &[
    "#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f", "#edc948", "#b07aa1", "#ff9da7", "#9c755f", "#bab0ac",
    "#d37295", "#a0cbe8",
];

/// Render a solution as an SVG string.
///
/// Sheets are laid out left-to-right. Pieces are colored by `piece_idx` (cycles through
/// [`PALETTE`]). Free rects are drawn as light gray dashed outlines. Labels use a white
/// halo stroke so they are readable on any background color.
pub fn render_svg(problem: &Problem, solution: &Solution) -> String {
    let n_sheets = solution.sheets_used().max(1);
    let sw = problem.sheet.width as f64;
    let sh = problem.sheet.height as f64;
    let margin = 40.0_f64;
    let gap = 60.0_f64;
    let label_row = 30.0_f64;

    let total_w = margin + (sw + gap) * n_sheets as f64 - gap + margin;
    let total_h = margin + sh + label_row + margin;

    let mut s = String::with_capacity(4096);
    writeln!(
        s,
        "<svg width=\"{tw:.0}\" height=\"{th:.0}\" viewBox=\"0 0 {tw:.0} {th:.0}\" \
         xmlns=\"http://www.w3.org/2000/svg\" style=\"font-family:sans-serif\">",
        tw = total_w,
        th = total_h
    )
    .unwrap();
    writeln!(
        s,
        "  <rect width=\"{tw:.0}\" height=\"{th:.0}\" fill=\"#f4f4f4\"/>",
        tw = total_w,
        th = total_h
    )
    .unwrap();

    for si in 0..n_sheets {
        let ox = margin + (sw + gap) * si as f64;
        let oy = margin;

        writeln!(
            s,
            "  <rect x=\"{ox:.0}\" y=\"{oy:.0}\" width=\"{sw:.0}\" height=\"{sh:.0}\" \
             fill=\"white\" stroke=\"#222\" stroke-width=\"3\"/>"
        )
        .unwrap();

        for fr in solution.leftovers.iter().filter(|f| f.sheet_idx == si) {
            writeln!(
                s,
                "  <rect x=\"{x:.0}\" y=\"{y:.0}\" width=\"{w:.0}\" height=\"{h:.0}\" \
                 fill=\"#ebebeb\" stroke=\"#bbb\" stroke-width=\"1\" stroke-dasharray=\"8,4\"/>",
                x = ox + fr.x as f64,
                y = oy + fr.y as f64,
                w = fr.w as f64,
                h = fr.h as f64,
            )
            .unwrap();
        }

        for pl in solution.placements.iter().filter(|p| p.sheet_idx == si) {
            let piece = &problem.pieces[pl.piece_idx];
            let (pw, ph) = if pl.rotated {
                (piece.height as f64, piece.width as f64)
            } else {
                (piece.width as f64, piece.height as f64)
            };
            let px = ox + pl.x as f64;
            let py = oy + pl.y as f64;
            let fill = PALETTE[pl.piece_idx % PALETTE.len()];

            writeln!(
                s,
                "  <rect x=\"{px:.0}\" y=\"{py:.0}\" width=\"{pw:.0}\" height=\"{ph:.0}\" \
                 fill=\"{fill}\" fill-opacity=\"0.80\" stroke=\"#333\" stroke-width=\"1\"/>"
            )
            .unwrap();

            let label = if piece.name.is_empty() {
                format!("#{}", pl.piece_idx)
            } else {
                xml_escape(&piece.name)
            };
            let dim = format!("{}×{}", pw as u32, ph as u32);
            let fs = (pw.min(ph) * 0.20).clamp(18.0, 100.0);
            let cx = px + pw / 2.0;
            let cy = py + ph / 2.0;

            writeln!(
                s,
                "  <text x=\"{cx:.0}\" y=\"{cy:.0}\" text-anchor=\"middle\" \
                 dominant-baseline=\"middle\" font-size=\"{fs:.0}\" font-weight=\"bold\" \
                 stroke=\"white\" stroke-width=\"4\" paint-order=\"stroke\" fill=\"#1a1a1a\">{label}</text>"
            )
            .unwrap();
            if ph >= fs * 2.2 {
                writeln!(
                    s,
                    "  <text x=\"{cx:.0}\" y=\"{y2:.0}\" text-anchor=\"middle\" \
                     dominant-baseline=\"middle\" font-size=\"{fs2:.0}\" \
                     stroke=\"white\" stroke-width=\"3\" paint-order=\"stroke\" fill=\"#333\">{dim}</text>",
                    y2 = cy + fs * 0.9,
                    fs2 = fs * 0.65,
                )
                .unwrap();
            }
        }

        writeln!(
            s,
            "  <text x=\"{cx:.0}\" y=\"{ly:.0}\" text-anchor=\"middle\" \
             font-size=\"22\" fill=\"#555\">Sheet {si}  ({sw:.0}×{sh:.0})</text>",
            cx = ox + sw / 2.0,
            ly = oy + sh + 22.0,
        )
        .unwrap();
    }

    s.push_str("</svg>\n");
    s
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FreeRect, Piece, Placement, Sheet, Solution};

    fn make_problem() -> Problem {
        Problem {
            sheet: Sheet { width: 100, height: 80 },
            kerf: 0,
            pieces: vec![
                Piece {
                    name: "A".into(),
                    width: 60,
                    height: 40,
                    can_rotate: false,
                },
                Piece {
                    name: "B".into(),
                    width: 40,
                    height: 40,
                    can_rotate: false,
                },
            ],
        }
    }

    #[test]
    fn smoke_single_sheet() {
        let problem = make_problem();
        let solution = Solution {
            placements: vec![
                Placement {
                    sheet_idx: 0,
                    piece_idx: 0,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
                Placement {
                    sheet_idx: 0,
                    piece_idx: 1,
                    x: 60,
                    y: 0,
                    rotated: false,
                },
            ],
            leftovers: vec![FreeRect {
                sheet_idx: 0,
                x: 0,
                y: 40,
                w: 100,
                h: 40,
            }],
        };
        let svg = render_svg(&problem, &solution);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>\n"));
        assert!(svg.contains("Sheet 0"));
        assert!(svg.contains(">A<"));
        assert!(svg.contains(">B<"));
    }

    #[test]
    fn multi_sheet_layout() {
        let problem = make_problem();
        let solution = Solution {
            placements: vec![
                Placement {
                    sheet_idx: 0,
                    piece_idx: 0,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
                Placement {
                    sheet_idx: 1,
                    piece_idx: 1,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
            ],
            leftovers: vec![],
        };
        let svg = render_svg(&problem, &solution);
        assert!(svg.contains("Sheet 0"));
        assert!(svg.contains("Sheet 1"));
    }
}
