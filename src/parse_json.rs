use serde::Deserialize;

use crate::model::{Piece, Problem, Sheet};

#[derive(Deserialize)]
pub struct SheetSpec {
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize)]
pub struct PieceSpec {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub count: u32,
    pub can_rotate: bool,
}

#[derive(Deserialize)]
pub struct ProblemSpec {
    pub sheet: SheetSpec,
    pub kerf: u32,
    pub pieces: Vec<PieceSpec>,
}

/// Parse a JSON problem spec and expand each `PieceSpec.count` into individual `Piece` instances.
///
/// ```json
/// {
///   "sheet": {"width": 3000, "height": 4000},
///   "kerf": 7,
///   "pieces": [
///     {"name": "стойка", "width": 835, "height": 620, "count": 4, "can_rotate": true}
///   ]
/// }
/// ```
pub fn parse_problem_json(s: &str) -> Result<Problem, serde_json::Error> {
    let spec: ProblemSpec = serde_json::from_str(s)?;
    let mut pieces = Vec::new();
    for ps in spec.pieces {
        for _ in 0..ps.count {
            pieces.push(Piece {
                name: ps.name.clone(),
                width: ps.width,
                height: ps.height,
                can_rotate: ps.can_rotate,
            });
        }
    }
    Ok(Problem {
        sheet: Sheet { width: spec.sheet.width, height: spec.sheet.height },
        kerf: spec.kerf,
        pieces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_problem_json_expands_count() {
        let json = r#"{
            "sheet": {"width": 1000, "height": 500},
            "kerf": 3,
            "pieces": [
                {"name": "стойка", "width": 200, "height": 100, "count": 3, "can_rotate": false},
                {"name": "полка",  "width": 150, "height": 80,  "count": 2, "can_rotate": true}
            ]
        }"#;
        let p = parse_problem_json(json).unwrap();
        assert_eq!(p.sheet.width, 1000);
        assert_eq!(p.kerf, 3);
        assert_eq!(p.pieces.len(), 5);
        assert_eq!(p.pieces[0].name, "стойка");
        assert_eq!(p.pieces[0].can_rotate, false);
        assert_eq!(p.pieces[3].name, "полка");
        assert_eq!(p.pieces[3].can_rotate, true);
    }

    #[test]
    fn parse_problem_json_invalid() {
        assert!(parse_problem_json("not json").is_err());
    }
}
