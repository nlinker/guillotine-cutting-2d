use crate::model::ProblemSpec;

/// Parse a JSON problem spec into a `ProblemSpec`.
///
/// ```json
/// {
///   "sheet": {"width": 3000, "height": 4000},
///   "kerf": 7,
///   "piece_types": [
///     {"name": "стойка", "width": 835, "height": 620, "count": 4, "can_rotate": true}
///   ]
/// }
/// ```
pub fn parse_problem(s: &str) -> Result<ProblemSpec, serde_json::Error> {
    serde_json::from_str(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_problem_json_preserves_count() {
        let json = r#"{
            "sheet": {"width": 1000, "height": 500},
            "kerf": 3,
            "piece_types": [
                {"name": "стойка", "width": 200, "height": 100, "count": 3, "can_rotate": false},
                {"name": "полка",  "width": 150, "height": 80,  "count": 2, "can_rotate": true}
            ]
        }"#;
        let p = parse_problem(json).unwrap();
        assert_eq!(p.sheet.width, 1000);
        assert_eq!(p.kerf, 3);
        assert_eq!(p.piece_types.len(), 2);
        assert_eq!(p.piece_types.iter().map(|p| p.count).sum::<u32>(), 5);
        assert_eq!(p.piece_types[0].name, "стойка");
        assert_eq!(p.piece_types[0].count, 3);
        assert!(!p.piece_types[0].can_rotate);
        assert_eq!(p.piece_types[1].name, "полка");
        assert_eq!(p.piece_types[1].count, 2);
        assert!(p.piece_types[1].can_rotate);
    }
}
