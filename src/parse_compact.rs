use chumsky::prelude::*;

use crate::model::{PieceType, ProblemSpec, Sheet};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("missing ':' separator (format: WxHR:kerf,margin:pieces)")]
    MissingSeparator,
    #[error("missing kerf/margin section (format: WxHR:kerf,margin:pieces)")]
    MissingKerfMargin,
    #[error("invalid sheet spec '{0}': expected WxHR or WxHF (R = rotatable default, F = fixed default)")]
    InvalidSheet(String),
    #[error("invalid kerf/margin '{0}': expected empty (both default to 0) or 'kerf,margin' as non-negative integers")]
    InvalidKerfMargin(String),
    #[error("invalid piece type '{0}': expected WxH, WxH/N, WxHr, WxHf, WxH/Nr, or WxH/Nf")]
    InvalidPieceType(String),
}

/// Parse a compact problem string into a `ProblemSpec`.
///
/// Format: `"<sheet>:<kerf>,<margin>:<pieces>"` — see
/// [Compact input format for the parser](../README.md#compact-input-format-for-the-parser)
/// for the full grammar.
///
/// # Example
/// ```
/// # use cut::parse_compact::parse_problem;
/// let p = parse_problem("3000x4000R:7,0:835x620/4,1020x620/4f,1750x900").unwrap();
/// assert_eq!(p.sheet.width, 3000);
/// assert_eq!(p.kerf, 7);
/// assert_eq!(p.piece_types.len(), 3); // 3 piece types; total count = 4+4+1 = 9
///
/// let p = parse_problem("3000x4000R::835x620").unwrap();
/// assert_eq!(p.kerf, 0);
/// assert_eq!(p.margin, 0);
/// ```
pub fn parse_problem(s: &str) -> Result<ProblemSpec, ParseError> {
    let (sheet_str, rest) = s.split_once(':').ok_or(ParseError::MissingSeparator)?;
    let (kerf_margin_str, pieces_str) = rest.split_once(':').ok_or(ParseError::MissingKerfMargin)?;

    let sheet_str = sheet_str.trim();
    let (sheet, default_rotate) = sheet_parser()
        .parse(sheet_str)
        .into_result()
        .ok()
        .filter(|(sheet, _)| sheet.width != 0 && sheet.height != 0)
        .ok_or_else(|| ParseError::InvalidSheet(sheet_str.to_string()))?;

    let kerf_margin_str = kerf_margin_str.trim();
    let (kerf, margin) = kerf_margin_parser()
        .parse(kerf_margin_str)
        .into_result()
        .map_err(|_| ParseError::InvalidKerfMargin(kerf_margin_str.to_string()))?;

    let mut pieces = Vec::new();
    for piece_str in pieces_str.split(',') {
        let piece_str = piece_str.trim();
        let piece = piece_parser(default_rotate)
            .parse(piece_str)
            .into_result()
            .ok()
            .filter(|p| p.width != 0 && p.height != 0 && p.count != 0)
            .ok_or_else(|| ParseError::InvalidPieceType(piece_str.to_string()))?;
        pieces.push(piece);
    }

    let mut spec = ProblemSpec {
        sheet,
        kerf,
        margin,
        piece_types: pieces,
    };
    spec.normalize();
    Ok(spec)
}

fn uint<'a>() -> impl Parser<'a, &'a str, u32, extra::Err<Simple<'a, char>>> + Clone {
    any()
        .filter(char::is_ascii_digit)
        .repeated()
        .at_least(1)
        .to_slice()
        .try_map(|s: &str, span| s.parse::<u32>().map_err(|_| Simple::new(None, span)))
}

fn sheet_parser<'a>() -> impl Parser<'a, &'a str, (Sheet, bool), extra::Err<Simple<'a, char>>> {
    uint()
        .then_ignore(just('x'))
        .then(uint())
        .then(one_of("RF"))
        .then_ignore(end())
        .map(|((width, height), suffix)| (Sheet { width, height }, suffix == 'R'))
}

fn kerf_margin_parser<'a>() -> impl Parser<'a, &'a str, (u32, u32), extra::Err<Simple<'a, char>>> {
    let pair = uint().then_ignore(just(',')).then(uint());
    let empty = end().to((0u32, 0u32));
    pair.then_ignore(end()).or(empty)
}

fn piece_parser<'a>(default_rotate: bool) -> impl Parser<'a, &'a str, PieceType, extra::Err<Simple<'a, char>>> {
    uint()
        .then_ignore(just('x'))
        .then(uint())
        .then(just('/').ignore_then(uint()).or_not())
        .then(one_of("rf").or_not())
        .then_ignore(end())
        .map(move |(((width, height), count), suffix)| PieceType {
            name: String::new(),
            width,
            height,
            count: count.unwrap_or(1),
            can_rotate: match suffix {
                Some('r') => true,
                Some('f') => false,
                _ => default_rotate,
            },
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_example() {
        let tuple = |p: &PieceType| (p.width, p.height, p.count, p.can_rotate);
        let p = parse_problem("3000x4000R:7,0:835x620/4,1020x620/4f,1020x620/4,1490x620/2,1750x900").unwrap();
        assert_eq!(
            p.sheet,
            Sheet {
                width: 3000,
                height: 4000
            }
        );
        assert_eq!(p.kerf, 7);
        assert_eq!(p.margin, 0);
        assert_eq!(p.piece_types.len(), 5);
        assert_eq!(p.piece_types.iter().map(|p| p.count).sum::<u32>(), 15);

        assert_eq!(tuple(&p.piece_types[0]), (620, 835, 4, true));
        assert_eq!(tuple(&p.piece_types[1]), (1020, 620, 4, false));
        assert_eq!(tuple(&p.piece_types[2]), (620, 1020, 4, true));
        assert_eq!(tuple(&p.piece_types[3]), (620, 1490, 2, true));
        assert_eq!(tuple(&p.piece_types[4]), (900, 1750, 1, true));
    }

    #[test]
    fn kerf_margin_forms() {
        let p = parse_problem("3000x4000R:7,10:835x620/4").unwrap();
        assert_eq!(p.kerf, 7);
        assert_eq!(p.margin, 10);

        let p = parse_problem("3000x4000R::835x620").unwrap();
        assert_eq!(p.kerf, 0);
        assert_eq!(p.margin, 0);

        let p = parse_problem("8x100F :: 7x5/4 , 6x4/4 , 4x6/4 , 5x7/4").unwrap();
        assert_eq!(p.sheet.width, 8);
        assert_eq!(p.kerf, 0);
        assert_eq!(p.margin, 0);
        assert_eq!(p.piece_types.len(), 4);
        assert_eq!(p.piece_types.iter().map(|p| p.count).sum::<u32>(), 16);
    }

    #[test]
    fn normalize_effects() {
        // 4x7/3r + 7x4/2r -> same canonical type (4x7r, min first), merged count=5
        // 7x4/4f stays as (7,4,false) - fixed, no swap
        let tuple = |p: &PieceType| (p.width, p.height, p.count, p.can_rotate);
        let p = parse_problem("10x10F::4x7/3r,7x4/2r,7x4/4f").unwrap();
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
        assert!(parse_problem("3000x4000F:7,0:100x100").is_ok());
        assert_eq!(
            parse_problem("3000x4000:100x100").unwrap_err(),
            ParseError::MissingKerfMargin
        );
        assert_eq!(
            parse_problem("3000x4000F:abc:100x100").unwrap_err(),
            ParseError::InvalidKerfMargin("abc".into())
        );
        assert_eq!(
            parse_problem("3000x4000F:7:100x100").unwrap_err(),
            ParseError::InvalidKerfMargin("7".into())
        );
        assert_eq!(
            parse_problem("3000x4000F:7,:100x100").unwrap_err(),
            ParseError::InvalidKerfMargin("7,".into())
        );
        assert_eq!(
            parse_problem("3000x4000F:,10:100x100").unwrap_err(),
            ParseError::InvalidKerfMargin(",10".into())
        );
        assert_eq!(
            parse_problem("3000::100x100").unwrap_err(),
            ParseError::InvalidSheet("3000".into())
        );
        assert_eq!(
            parse_problem("0x4000F::100x100").unwrap_err(),
            ParseError::InvalidSheet("0x4000F".into())
        );
        assert_eq!(
            parse_problem("1000x500F::abc").unwrap_err(),
            ParseError::InvalidPieceType("abc".into())
        );
        assert_eq!(
            parse_problem("1000x500F::100x0").unwrap_err(),
            ParseError::InvalidPieceType("100x0".into())
        );
    }
}
