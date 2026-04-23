use crate::model::{Piece, Problem, Sheet};

struct PieceSpec {
    width: u32,
    height: u32,
    count: u32,
    can_rotate: bool,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("missing ':' separator between sheet spec and pieces")]
    MissingSeparator,
    #[error("invalid sheet spec '{0}': expected WxH")]
    InvalidSheet(String),
    #[error("invalid piece spec '{0}': expected WxH, WxHxN, WxHn, or WxHxNn")]
    InvalidPiece(String),
    #[error("invalid integer in '{0}'")]
    InvalidInteger(String),
}

/// Parse a compact problem string into a `Problem`.
///
/// Format: `"<sheet>:<pieces>"` where
/// - `<sheet>` = `WxH`
/// - `<pieces>` = comma-separated `WxH` | `WxHxN` | `WxHn` | `WxHxNn`
///   (`n` suffix = no rotation; `N` = repeat count, defaults to 1)
///
/// To fix orientation, specify dimensions in the desired order: `620x1020` places
/// the 620mm side along X and 1020mm along Y.
///
/// # Example
/// ```
/// # use cutting::parse::parse_problem;
/// let p = parse_problem("3000x4000:835x620x4,1020x620x4n,1750x900", 7).unwrap();
/// assert_eq!(p.sheet.width, 3000);
/// assert_eq!(p.pieces.len(), 9);
/// ```
pub fn parse_problem(s: &str, kerf: u32) -> Result<Problem, ParseError> {
    let (sheet_str, pieces_str) = s.split_once(':').ok_or(ParseError::MissingSeparator)?;
    let sheet = parse_sheet(sheet_str.trim())?;
    let mut pieces = Vec::new();
    let mut next_id = 0u32;
    for piece_str in pieces_str.split(',') {
        let spec = parse_piece_spec(piece_str.trim())?;
        for _ in 0..spec.count {
            pieces.push(Piece {
                id: next_id,
                width: spec.width,
                height: spec.height,
                can_rotate: spec.can_rotate,
            });
            next_id += 1;
        }
    }
    Ok(Problem { sheet, kerf, pieces })
}

fn parse_sheet(s: &str) -> Result<Sheet, ParseError> {
    let (w_str, h_str) = s
        .split_once('x')
        .ok_or_else(|| ParseError::InvalidSheet(s.to_string()))?;
    let width = parse_u32(w_str, s)?;
    let height = parse_u32(h_str, s)?;
    if width == 0 || height == 0 {
        return Err(ParseError::InvalidSheet(s.to_string()));
    }
    Ok(Sheet { width, height })
}

fn parse_piece_spec(s: &str) -> Result<PieceSpec, ParseError> {
    let err = || ParseError::InvalidPiece(s.to_string());
    let (base, can_rotate) = if s.ends_with('n') {
        (&s[..s.len() - 1], false)
    } else {
        (s, true)
    };
    let parts: Vec<&str> = base.splitn(3, 'x').collect();
    let (width, height, count) = match parts.as_slice() {
        [w, h] => (parse_u32(w, s)?, parse_u32(h, s)?, 1),
        [w, h, n] => (parse_u32(w, s)?, parse_u32(h, s)?, parse_u32(n, s)?),
        _ => return Err(err()),
    };
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
        // 4 rotatable + 4 fixed + 4 rotatable + 2 + 1 = 15 pieces
        let p = parse_problem("3000x4000:835x620x4,1020x620x4n,1020x620x4,1490x620x2,1750x900", 7).unwrap();

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
        assert_eq!(
            (p.pieces[14].width, p.pieces[14].height, p.pieces[14].id),
            (1750, 900, 14)
        );
    }

    #[test]
    fn errors() {
        assert_eq!(
            parse_problem("3000x4000 835x620", 7).unwrap_err(),
            ParseError::MissingSeparator
        );
        assert_eq!(
            parse_problem("3000:100x100", 0).unwrap_err(),
            ParseError::InvalidSheet("3000".into())
        );
        assert_eq!(
            parse_problem("0x4000:100x100", 0).unwrap_err(),
            ParseError::InvalidSheet("0x4000".into())
        );
        assert_eq!(
            parse_problem("1000x500:abc", 0).unwrap_err(),
            ParseError::InvalidPiece("abc".into())
        );
        assert_eq!(
            parse_problem("1000x500:100", 0).unwrap_err(),
            ParseError::InvalidPiece("100".into())
        );
        assert_eq!(
            parse_problem("1000x500:100x0", 0).unwrap_err(),
            ParseError::InvalidPiece("100x0".into())
        );
    }
}
