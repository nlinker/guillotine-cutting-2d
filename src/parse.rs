use crate::model::{Piece, Problem, Sheet};

struct PieceSpec {
    width: u32,
    height: u32,
    count: u32,
    can_rotate: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("missing ':' separator (format: WxHR:kerf:pieces)")]
    MissingSeparator,
    #[error("missing kerf field (format: WxHR:kerf:pieces)")]
    MissingKerf,
    #[error("invalid kerf value '{0}': expected a non-negative integer")]
    InvalidKerf(String),
    #[error("invalid sheet spec '{0}': expected WxHR or WxHF (R = rotatable default, F = fixed default)")]
    InvalidSheet(String),
    #[error("invalid piece spec '{0}': expected WxH, WxH-N, WxHr, WxHf, WxH-Nr, or WxH-Nf")]
    InvalidPiece(String),
    #[error("invalid integer in '{0}'")]
    InvalidInteger(String),
}

/// Parse a compact problem string into a `Problem`.
///
/// Format: `"<sheet>:<kerf>:<pieces>"` where
/// - `<sheet>` = `WxHR` or `WxHF`
///   (`R` = pieces rotatable by default, `F` = pieces fixed by default)
/// - `<kerf>` = blade kerf width in the same units as sheet dimensions (non-negative integer)
/// - `<pieces>` = comma-separated `WxH` | `WxH-N` | `WxHr` | `WxHf` | `WxH-Nr` | `WxH-Nf`
///   (no suffix = sheet default; `r`/`f` override per piece; `-N` = repeat count, defaults to 1)
///
/// To control orientation of fixed pieces, specify dimensions in the desired order:
/// `620x1020` places the 620mm side along X and 1020mm along Y.
///
/// # Example
/// ```
/// # use cutting::parse::parse_problem;
/// let p = parse_problem("3000x4000R:7:835x620-4,1020x620-4f,1750x900").unwrap();
/// assert_eq!(p.sheet.width, 3000);
/// assert_eq!(p.kerf, 7);
/// assert_eq!(p.pieces.len(), 9);
/// ```
pub fn parse_problem(s: &str) -> Result<Problem, ParseError> {
    let (sheet_str, rest) = s.split_once(':').ok_or(ParseError::MissingSeparator)?;
    let (kerf_str, pieces_str) = rest.split_once(':').ok_or(ParseError::MissingKerf)?;
    let (sheet, default_rotate) = parse_sheet(sheet_str.trim())?;
    let kerf = kerf_str
        .trim()
        .parse::<u32>()
        .map_err(|_| ParseError::InvalidKerf(kerf_str.trim().to_string()))?;
    let mut pieces = Vec::new();
    for piece_str in pieces_str.split(',') {
        let spec = parse_piece_spec(piece_str.trim(), default_rotate)?;
        for _ in 0..spec.count {
            pieces.push(Piece {
                name: String::new(),
                width: spec.width,
                height: spec.height,
                can_rotate: spec.can_rotate,
            });
        }
    }
    Ok(Problem { sheet, kerf, pieces })
}

fn parse_sheet(s: &str) -> Result<(Sheet, bool), ParseError> {
    let err = || ParseError::InvalidSheet(s.to_string());
    let (dims, default_rotate) = if let Some(stripped) = s.strip_suffix('R') {
        (stripped, true)
    } else if let Some(stripped) = s.strip_suffix('F') {
        (stripped, false)
    } else {
        return Err(err());
    };
    let (w_str, h_str) = dims.split_once('x').ok_or_else(err)?;
    let width = parse_u32(w_str, s)?;
    let height = parse_u32(h_str, s)?;
    if width == 0 || height == 0 {
        return Err(err());
    }
    Ok((Sheet { width, height }, default_rotate))
}

fn parse_piece_spec(s: &str, default_rotate: bool) -> Result<PieceSpec, ParseError> {
    let err = || ParseError::InvalidPiece(s.to_string());
    let (base, can_rotate) = if let Some(stripped) = s.strip_suffix('r') {
        (stripped, true)
    } else if let Some(stripped) = s.strip_suffix('f') {
        (stripped, false)
    } else {
        (s, default_rotate)
    };
    let (dims, count) = match base.split_once('-') {
        Some((d, n)) => (d, parse_u32(n, s)?),
        None => (base, 1),
    };
    let (w_str, h_str) = dims.split_once('x').ok_or_else(err)?;
    let width = parse_u32(w_str, s)?;
    let height = parse_u32(h_str, s)?;
    if width == 0 || height == 0 || count == 0 {
        return Err(err());
    }
    Ok(PieceSpec {
        width,
        height,
        count,
        can_rotate,
    })
}

fn parse_u32(s: &str, context: &str) -> Result<u32, ParseError> {
    s.parse::<u32>()
        .map_err(|_| ParseError::InvalidInteger(context.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_example() {
        // R default: 4 rotatable + 4 fixed(f) + 4 rotatable + 2 rotatable + 1 rotatable = 15
        let p = parse_problem("3000x4000R:7:835x620-4,1020x620-4f,1020x620-4,1490x620-2,1750x900").unwrap();

        assert_eq!(
            p.sheet,
            Sheet {
                width: 3000,
                height: 4000
            }
        );
        assert_eq!(p.kerf, 7);
        assert_eq!(p.pieces.len(), 15);

        for i in 0..4 {
            assert_eq!(
                (p.pieces[i].width, p.pieces[i].height, p.pieces[i].can_rotate),
                (835, 620, true)
            );
        }
        for i in 4..8 {
            assert_eq!(
                (p.pieces[i].width, p.pieces[i].height, p.pieces[i].can_rotate),
                (1020, 620, false)
            );
        }
        for i in 8..12 {
            assert_eq!(
                (p.pieces[i].width, p.pieces[i].height, p.pieces[i].can_rotate),
                (1020, 620, true)
            );
        }
        for i in 12..14 {
            assert_eq!((p.pieces[i].width, p.pieces[i].height), (1490, 620));
        }
        assert_eq!((p.pieces[14].width, p.pieces[14].height), (1750, 900));
    }

    #[test]
    fn errors() {
        assert_eq!(
            parse_problem("3000x4000 835x620").unwrap_err(),
            ParseError::MissingSeparator
        );
        assert!(parse_problem("3000x4000F:7:100x100").is_ok());
        assert_eq!(parse_problem("3000x4000:100x100").unwrap_err(), ParseError::MissingKerf);
        assert_eq!(
            parse_problem("3000x4000F:abc:100x100").unwrap_err(),
            ParseError::InvalidKerf("abc".into())
        );
        assert_eq!(
            parse_problem("3000:0:100x100").unwrap_err(),
            ParseError::InvalidSheet("3000".into())
        );
        assert_eq!(
            parse_problem("0x4000F:0:100x100").unwrap_err(),
            ParseError::InvalidSheet("0x4000F".into())
        );
        assert_eq!(
            parse_problem("1000x500F:0:abc").unwrap_err(),
            ParseError::InvalidPiece("abc".into())
        );
        assert_eq!(
            parse_problem("1000x500F:0:100x0").unwrap_err(),
            ParseError::InvalidPiece("100x0".into())
        );
    }
}
