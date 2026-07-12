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
pub fn render_svg(spec: &ProblemSpec, solution: &SolutionSpec) -> Result<String, std::fmt::Error> {
    let n_sheets = solution.sheets_used().max(1);
    let sw = spec.sheet.width as f64;
    let sh = spec.sheet.height as f64;

    // Normalize to a fixed display width so both 12×41 and 3000×2000 look reasonable.
    const DISPLAY_W: f64 = 700.0;
    let scale = DISPLAY_W / sw;
    let dw = DISPLAY_W;
    let dh = sh * scale;
    let margin = (dw * 0.04).max(8.0);
    let sep = 6.0; // gap between sheets when n_sheets > 1

    let total_w = margin * 2.0 + dw;
    let total_h = margin * 2.0 + n_sheets as f64 * dh + (n_sheets - 1) as f64 * sep;

    let mut s = String::with_capacity(4096);
    writeln!(
        s,
        "<svg width=\"{tw:.0}\" height=\"{th:.0}\" viewBox=\"0 0 {tw:.0} {th:.0}\" \
         xmlns=\"http://www.w3.org/2000/svg\" style=\"font-family:sans-serif\">",
        tw = total_w,
        th = total_h
    )?;
    writeln!(
        s,
        "  <rect width=\"{tw:.0}\" height=\"{th:.0}\" fill=\"#f4f4f4\"/>",
        tw = total_w,
        th = total_h
    )?;

    for si in 0..n_sheets {
        let ox = margin;
        let oy = margin + si as f64 * (dh + sep);

        writeln!(
            s,
            "  <rect x=\"{ox:.0}\" y=\"{oy:.0}\" width=\"{dw:.0}\" height=\"{dh:.0}\" \
             fill=\"#f8f8f8\" stroke=\"#3c3c3c\" stroke-width=\"1\"/>"
        )?;

        for fr in solution.leftovers.iter().filter(|f| f.sheet_idx == si) {
            writeln!(
                s,
                "  <rect x=\"{x:.1}\" y=\"{y:.1}\" width=\"{w:.1}\" height=\"{h:.1}\" \
                 fill=\"#e8e8e8\" stroke=\"#bbb\" stroke-width=\"1\" stroke-dasharray=\"8,4\"/>",
                x = ox + fr.x as f64 * scale,
                y = oy + fr.y as f64 * scale,
                w = fr.w as f64 * scale,
                h = fr.h as f64 * scale,
            )?;
        }

        for pl in solution.placements.iter().filter(|p| p.sheet_idx == si) {
            let piece = &spec.piece_types[pl.piespec_idx];
            let (pw, ph) = if pl.rotated {
                (piece.height as f64 * scale, piece.width as f64 * scale)
            } else {
                (piece.width as f64 * scale, piece.height as f64 * scale)
            };
            let px = ox + pl.x as f64 * scale;
            let py = oy + pl.y as f64 * scale;
            let fill = PALETTE[pl.piespec_idx % PALETTE.len()];

            writeln!(
                s,
                "  <rect x=\"{px:.1}\" y=\"{py:.1}\" width=\"{pw:.1}\" height=\"{ph:.1}\" \
                 fill=\"{fill}\" stroke=\"#505050\" stroke-width=\"0.5\"/>"
            )?;

            let fs = (pw.min(ph) * 0.14).clamp(7.0, 60.0);
            let min_dim = dw * 0.04;
            if pw >= min_dim && ph >= min_dim {
                let label = if piece.name.is_empty() {
                    format!("#{}", pl.piespec_idx)
                } else {
                    xml_escape(&piece.name)
                };
                let cx = px + pw / 2.0;
                let cy = py + ph / 2.0;
                let show_dim = ph >= fs * 2.4;
                let (ow, oh) = if pl.rotated {
                    (piece.height, piece.width)
                } else {
                    (piece.width, piece.height)
                };
                let (ly, dy) = if show_dim {
                    (cy - fs * 0.55, cy + fs * 0.55)
                } else {
                    (cy, cy)
                };
                writeln!(
                    s,
                    "  <text x=\"{cx:.0}\" y=\"{ly:.0}\" text-anchor=\"middle\" \
                     dominant-baseline=\"middle\" font-size=\"{fs:.0}\" fill=\"#333\">{label}</text>"
                )?;
                if show_dim {
                    let dim = format!("{}×{}", ow, oh);
                    writeln!(
                        s,
                        "  <text x=\"{cx:.0}\" y=\"{dy:.0}\" text-anchor=\"middle\" \
                         dominant-baseline=\"middle\" font-size=\"{fs:.0}\" fill=\"#555\">{dim}</text>",
                    )?;
                }
            }
        }
    }

    s.push_str("</svg>\n");
    Ok(s)
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{PieceType, PlacementSpec, Sheet, SolutionSpec};

    fn make_spec() -> ProblemSpec {
        ProblemSpec {
            sheet: Sheet { width: 100, height: 80 },
            kerf: 0,
            margin: 0,
            piece_types: vec![
                PieceType {
                    name: "A".into(),
                    width: 60,
                    height: 40,
                    count: 1,
                    can_rotate: false,
                },
                PieceType {
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
                    piespec_idx: 0,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
                PlacementSpec {
                    sheet_idx: 1,
                    piespec_idx: 1,
                    x: 0,
                    y: 0,
                    rotated: false,
                },
            ],
            leftovers: vec![],
        };
        let svg = render_svg(&spec, &solution).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.ends_with("</svg>\n"));
        assert!(svg.contains(">A<"));
        assert!(svg.contains(">B<"));
        // With DISPLAY_W=700, sw=100: scale=7, dw=700, dh=80*7=560, margin=max(28,8)=28, sep=6
        let dw = 700.0_f64;
        let scale = dw / spec.sheet.width as f64;
        let dh = spec.sheet.height as f64 * scale;
        let margin = (dw * 0.04_f64).max(8.0);
        let sep = 6.0_f64;
        let expected_w = margin * 2.0 + dw;
        let expected_h = margin * 2.0 + 2.0 * dh + sep;
        assert!(svg.contains(&format!("width=\"{:.0}\"", expected_w)));
        assert!(svg.contains(&format!("height=\"{:.0}\"", expected_h)));
    }
}
