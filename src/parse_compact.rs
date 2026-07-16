use crate::model::{PieceType, ProblemSpec, Sheet};

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
    #[error("invalid piece type '{0}': expected WxH, WxH/N, WxHr, WxHf, WxH/Nr, or WxH/Nf")]
    InvalidPieceType(String),
    #[error("invalid integer in '{0}'")]
    InvalidInteger(String),
}

/// Parse a compact problem string into a `ProblemSpec`.
///
/// Format: `"<sheet>:<kerf>:<pieces>"` where
/// - `<sheet>` = `WxHR` or `WxHF`
///   (`R` = pieces rotatable by default, `F` = pieces fixed by default)
/// - `<kerf>` = blade kerf width in the same units as sheet dimensions (non-negative integer)
/// - `<pieces>` = comma-separated `WxH` | `WxH/N` | `WxHr` | `WxHf` | `WxH/Nr` | `WxH/Nf`
///   (no suffix = sheet default; `r`/`f` override per piece; `/N` = repeat count, defaults to 1)
///
/// To control orientation of fixed pieces, specify dimensions in the desired order:
/// `620x1020` places the 620mm side along X and 1020mm along Y.
///
/// # Example
/// ```
/// # use cut::parse_compact::parse_problem;
/// let p = parse_problem("3000x4000R:7:835x620/4,1020x620/4f,1750x900").unwrap();
/// assert_eq!(p.sheet.width, 3000);
/// assert_eq!(p.kerf, 7);
/// assert_eq!(p.piece_types.len(), 3); // 3 piece types; total count = 4+4+1 = 9
/// ```
pub fn parse_problem(s: &str) -> Result<ProblemSpec, ParseError> {
    let (sheet_str, rest) = s.split_once(':').ok_or(ParseError::MissingSeparator)?;
    let (kerf_str, pieces_str) = rest.split_once(':').ok_or(ParseError::MissingKerf)?;
    let (sheet, default_rotate) = parse_sheet(sheet_str.trim())?;
    let kerf = kerf_str
        .trim()
        .parse::<u32>()
        .map_err(|_| ParseError::InvalidKerf(kerf_str.trim().to_string()))?;
    let mut pieces = Vec::new();
    for piece_str in pieces_str.split(',') {
        pieces.push(parse_piece_spec(piece_str.trim(), default_rotate)?);
    }
    let mut spec = ProblemSpec {
        sheet,
        kerf,
        margin: 0,
        piece_types: pieces,
    };
    spec.normalize();
    Ok(spec)
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

fn parse_piece_spec(s: &str, default_rotate: bool) -> Result<PieceType, ParseError> {
    let err = || ParseError::InvalidPieceType(s.to_string());
    let (base, can_rotate) = if let Some(stripped) = s.strip_suffix('r') {
        (stripped, true)
    } else if let Some(stripped) = s.strip_suffix('f') {
        (stripped, false)
    } else {
        (s, default_rotate)
    };
    let (dims, count) = match base.split_once('/') {
        Some((d, n)) => (d, parse_u32(n, s)?),
        None => (base, 1),
    };
    let (w_str, h_str) = dims.split_once('x').ok_or_else(err)?;
    let width = parse_u32(w_str, s)?;
    let height = parse_u32(h_str, s)?;
    if width == 0 || height == 0 || count == 0 {
        return Err(err());
    }
    Ok(PieceType {
        name: String::new(),
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
        let tuple = |p: &PieceType| (p.width, p.height, p.count, p.can_rotate);
        let p = parse_problem("3000x4000R:7:835x620/4,1020x620/4f,1020x620/4,1490x620/2,1750x900").unwrap();
        assert_eq!(
            p.sheet,
            Sheet {
                width: 3000,
                height: 4000
            }
        );
        assert_eq!(p.kerf, 7);
        assert_eq!(p.piece_types.len(), 5);
        assert_eq!(p.piece_types.iter().map(|p| p.count).sum::<u32>(), 15);

        assert_eq!(tuple(&p.piece_types[0]), (620, 835, 4, true));
        assert_eq!(tuple(&p.piece_types[1]), (1020, 620, 4, false));
        assert_eq!(tuple(&p.piece_types[2]), (620, 1020, 4, true));
        assert_eq!(tuple(&p.piece_types[3]), (620, 1490, 2, true));
        assert_eq!(tuple(&p.piece_types[4]), (900, 1750, 1, true));
    }

    #[test]
    fn spaces_around_delimiters() {
        let p = parse_problem("8x100F : 0 : 7x5/4 , 6x4/4 , 4x6/4 , 5x7/4").unwrap();
        assert_eq!(p.sheet.width, 8);
        assert_eq!(p.kerf, 0);
        assert_eq!(p.piece_types.len(), 4);
        assert_eq!(p.piece_types.iter().map(|p| p.count).sum::<u32>(), 16);
    }

    #[test]
    fn normalize_effects() {
        // 4x7/3r + 7x4/2r -> same canonical type (4x7r, min first), merged count=5
        // 7x4/4f stays as (7,4,false) - fixed, no swap
        let tuple = |p: &PieceType| (p.width, p.height, p.count, p.can_rotate);
        let p = parse_problem("10x10F:0:4x7/3r,7x4/2r,7x4/4f").unwrap();
        assert_eq!(p.piece_types.len(), 2);
        assert_eq!(tuple(&p.piece_types[0]), (4, 7, 5, true));
        assert_eq!(tuple(&p.piece_types[1]), (7, 4, 4, false));
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
            ParseError::InvalidPieceType("abc".into())
        );
        assert_eq!(
            parse_problem("1000x500F:0:100x0").unwrap_err(),
            ParseError::InvalidPieceType("100x0".into())
        );
    }
}
