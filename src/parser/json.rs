use std::error::Error;

use crate::model::{ProblemSpec, SolutionSpec};

/// Parse a JSON problem spec into a `ProblemSpec`.
pub fn parse_problem(s: &str) -> Result<ProblemSpec, serde_json::Error> {
    serde_json::from_str(s)
}

pub fn parse_solution_json(s: &str) -> Result<SolutionSpec, Box<dyn Error>> {
    let v: serde_json::Value = serde_json::from_str(s)?;
    let sol_val = if v.get("solution").is_some() {
        &v["solution"]
    } else {
        &v
    };
    Ok(serde_json::from_value(sol_val.clone())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_problem_json_preserves_count() {
        let json = r#"{
            "sheet": {"width": 1000, "height": 500},
            "kerf": 3,
            "margin": 10,
            "piece_types": [
                {"name": "стойка", "width": 200, "height": 100, "count": 3, "can_rotate": false},
                {"name": "полка",  "width": 150, "height": 80,  "count": 2, "can_rotate": true}
            ]
        }"#;
        let p = parse_problem(json).unwrap();
        assert_eq!(p.sheet.width, 1000);
        assert_eq!(p.kerf, 3);
        assert_eq!(p.margin, 10);
        assert_eq!(p.piece_types.len(), 2);
        assert_eq!(p.piece_types.iter().map(|p| p.count).sum::<u32>(), 5);
        assert_eq!(p.piece_types[0].name, "стойка");
        assert_eq!(p.piece_types[0].count, 3);
        assert!(!p.piece_types[0].can_rotate);
        assert_eq!(p.piece_types[1].name, "полка");
        assert_eq!(p.piece_types[1].count, 2);
        assert!(p.piece_types[1].can_rotate);
    }

    #[test]
    fn parse_problem_json_defaults_missing_kerf_margin() {
        let json = r#"{
            "sheet": {"width": 1000, "height": 500},
            "piece_types": [
                {"width": 200, "height": 100, "count": 1, "can_rotate": false}
            ]
        }"#;
        let p = parse_problem(json).unwrap();
        assert_eq!(p.kerf, 0);
        assert_eq!(p.margin, 0);
    }
}
