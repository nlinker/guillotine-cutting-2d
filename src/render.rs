use std::fmt::Write as _;

use crate::model::{ProblemSpec, SolutionSpec};

/// Pastel palette matching the Excel workbook (VBA `PieceColor`).
const PALETTE: &[&str] = &[
    "#FFB6C1", "#ADD8E6", "#90EE90", "#FFFF99", "#FFC878", "#DDA0DD", "#87CEEB", "#F0B4B4", "#B4FFB4", "#FFE4C4",
    "#C8C8FF", "#FFF0B4",
];

/// Render a `SolutionSpec` as an SVG string.
///
/// Sheets are stacked vertically with a gap. Pieces are colored by `piece_idx`
/// (type index) cycling through [`PALETTE`]. Free rects shown as dashed outlines.
pub fn render_svg(spec: &ProblemSpec, solution: &SolutionSpec) -> String {
    let n_sheets = solution.sheets_used().max(1);
    let sw = spec.sheet.width as f64;
    let sh = spec.sheet.height as f64;
    let margin = 40.0_f64;
    let gap = 50.0_f64;

    let total_w = margin * 2.0 + sw;
    let total_h = margin * 2.0 + n_sheets as f64 * sh + (n_sheets - 1) as f64 * gap;

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
        let ox = margin;
        let oy = margin + si as f64 * (sh + gap);

        writeln!(
            s,
            "  <rect x=\"{ox:.0}\" y=\"{oy:.0}\" width=\"{sw:.0}\" height=\"{sh:.0}\" \
             fill=\"#f8f8f8\" stroke=\"#3c3c3c\" stroke-width=\"1\"/>"
        )
        .unwrap();

        let lfs = gap * 0.36;
        writeln!(
            s,
            "  <text x=\"{cx:.0}\" y=\"{ly:.0}\" text-anchor=\"middle\" \
             dominant-baseline=\"middle\" font-size=\"{lfs:.0}\" fill=\"#666\"\
             >Sheet {si}  ({sw:.0}×{sh:.0})</text>",
            cx = ox + sw / 2.0,
            ly = oy + sh + gap / 2.0,
        )
        .unwrap();

        for fr in solution.leftovers.iter().filter(|f| f.sheet_idx == si) {
            writeln!(
                s,
                "  <rect x=\"{x:.0}\" y=\"{y:.0}\" width=\"{w:.0}\" height=\"{h:.0}\" \
                 fill=\"#e8e8e8\" stroke=\"#bbb\" stroke-width=\"1\" stroke-dasharray=\"8,4\"/>",
                x = ox + fr.x as f64,
                y = oy + fr.y as f64,
                w = fr.w as f64,
                h = fr.h as f64,
            )
            .unwrap();
        }

        for pl in solution.placements.iter().filter(|p| p.sheet_idx == si) {
            let piece = &spec.pieces[pl.piece_idx];
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
                 fill=\"{fill}\" stroke=\"#505050\" stroke-width=\"0.5\"/>"
            )
            .unwrap();

            let fs = (pw.min(ph) * 0.18).clamp(14.0, 80.0);
            let min_dim = sw * 0.04;
            if pw >= min_dim && ph >= min_dim {
                let label = if piece.name.is_empty() {
                    format!("#{}", pl.piece_idx)
                } else {
                    xml_escape(&piece.name)
                };
                let cx = px + pw / 2.0;
                let cy = py + ph / 2.0;
                writeln!(
                    s,
                    "  <text x=\"{cx:.0}\" y=\"{cy:.0}\" text-anchor=\"middle\" \
                     dominant-baseline=\"middle\" font-size=\"{fs:.0}\" font-weight=\"bold\" \
                     fill=\"#1a1a1a\">{label}</text>"
                )
                .unwrap();
                if ph >= fs * 2.4 {
                    let dim = format!("{}×{}", pw as u32, ph as u32);
                    writeln!(
                        s,
                        "  <text x=\"{cx:.0}\" y=\"{y2:.0}\" text-anchor=\"middle\" \
                         dominant-baseline=\"middle\" font-size=\"{fs2:.0}\" fill=\"#444\">{dim}</text>",
                        y2 = cy + fs * 0.95,
                        fs2 = fs * 0.65,
                    )
                    .unwrap();
                }
            }
        }
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
    use crate::model::{PieceSpec, PlacementSpec, Sheet, SolutionSpec};

    fn make_spec() -> ProblemSpec {
        ProblemSpec {
            sheet: Sheet { width: 100, height: 80 },
            kerf: 0,
            pieces: vec![
                PieceSpec {
                    name: "A".into(),
                    width: 60,
                    height: 40,
                    count: 1,
                    can_rotate: false,
                },
                PieceSpec {
                    name: "B".into(),
                    width: 40,
                    height: 40,
                    count: 1,
                    can_rotate: false,
                },
            ],
        }
    }

    #[test]
    fn multi_sheet_stacked_vertically() {
        let spec = make_spec();
        let solution = SolutionSpec {
            placements: vec![
                PlacementSpec {
                    sheet_idx: 0,
                    piece_idx: 0,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 1,
                    piece_idx: 1,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
            ],
            leftovers: vec![],
        };
        let svg = render_svg(&spec, &solution);
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>\n"));
        assert!(svg.contains("Sheet 0"));
        assert!(svg.contains("Sheet 1"));
        assert!(svg.contains(">A<"));
        assert!(svg.contains(">B<"));
        let sw = spec.sheet.width as f64;
        let sh = spec.sheet.height as f64;
        assert!(svg.contains(&format!("width=\"{:.0}\"", 40.0 * 2.0 + sw)));
        assert!(svg.contains(&format!("height=\"{:.0}\"", 40.0 * 2.0 + 2.0 * sh + 50.0)));
    }
}
