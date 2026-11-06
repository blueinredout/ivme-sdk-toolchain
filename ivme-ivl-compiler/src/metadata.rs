use crate::error::CompileError;

#[derive(Debug, Clone)]
pub struct LevelMetadata {
    pub course_id: String,
    pub level_name: String,
    pub level_creator: String,
    pub level_description: String,
    pub game_mode: String,
    pub checkpoints: i32,
    pub metadata_schema_version: Option<String>,
    pub level_uuid: Option<String>,
    pub bronze_time_ms: Option<String>,
    pub silver_time_ms: Option<String>,
    pub gold_time_ms: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExtendedMetadata {
    pub metadata_schema_version: u32,
    pub level_uuid: String,
    pub bronze_time_ms: u32,
    pub silver_time_ms: u32,
    pub gold_time_ms: u32,
    pub medal_configured: bool,
}

pub fn decode_ascii_decimal(value: &str) -> Option<u32> {
    let value = value.trim_matches(|c: char| c.is_ascii() && (c as u32) <= 32);
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) { return None; }
    let mut n = 0u32;
    for b in value.bytes() { n = n.checked_mul(10)?.checked_add((b - b'0') as u32)?; }
    (n <= 2_147_483_647).then_some(n)
}

pub(crate) fn validate_extended_metadata(meta: &LevelMetadata) -> Result<ExtendedMetadata, CompileError> {
    let schema = meta.metadata_schema_version.as_deref().and_then(decode_ascii_decimal)
        .ok_or_else(|| CompileError::InvalidExtendedMetadata("missing or invalid metadata_schema_version; rebuild/initialize metadata".into()))?;
    if schema != 1 { return Err(CompileError::InvalidExtendedMetadata(format!("unsupported metadata_schema_version {}", schema))); }
    let uuid = meta.level_uuid.as_deref().unwrap_or("").trim_matches(|c: char| c.is_ascii() && (c as u32) <= 32);
    let bytes = uuid.as_bytes();
    if bytes.len() != 36 || ![8,13,18,23].iter().all(|&i| bytes[i] == b'-') || !bytes.iter().enumerate().all(|(i,b)| [8,13,18,23].contains(&i) || b.is_ascii_hexdigit()) {
        return Err(CompileError::InvalidExtendedMetadata("missing or invalid level_uuid; rebuild/initialize metadata".into()));
    }
    let targets = [&meta.bronze_time_ms, &meta.silver_time_ms, &meta.gold_time_ms];
    let (b,s,g,configured) = if targets.iter().all(|v| v.is_none()) { (0,0,0,false) } else {
        if targets.iter().any(|v| v.is_none()) { return Err(CompileError::InvalidExtendedMetadata("medal target fields must be all present or all absent".into())); }
        let vals: Vec<u32> = targets.iter().map(|v| v.as_deref().and_then(decode_ascii_decimal).ok_or_else(|| CompileError::InvalidExtendedMetadata("invalid medal target decimal".into()))).collect::<Result<_,_>>()?;
        if vals.iter().all(|&v| v == 0) { (0,0,0,false) } else if vals.iter().all(|&v| v > 0) && vals[2] < vals[1] && vals[1] < vals[0] { (vals[0], vals[1], vals[2], true) } else { return Err(CompileError::InvalidExtendedMetadata("medal targets must all be zero or satisfy gold < silver < bronze".into())); }
    };
    Ok(ExtendedMetadata { metadata_schema_version: 1, level_uuid: uuid.to_ascii_lowercase(), bronze_time_ms:b, silver_time_ms:s, gold_time_ms:g, medal_configured:configured })
}

pub const RECOGNIZED_GAME_MODES: &[&str] = &["std", "pg", "fz"];

pub fn validate_metadata(meta: &LevelMetadata) -> Result<(), CompileError> {
    if meta.course_id.is_empty() {
        return Err(CompileError::EmptyCourseId);
    }
    if !meta.course_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(CompileError::InvalidCourseId(meta.course_id.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(course_id: &str) -> LevelMetadata {
        LevelMetadata {
            course_id: course_id.to_string(),
            level_name: "Test".to_string(),
            level_creator: "Author".to_string(),
            level_description: "Desc".to_string(),
            game_mode: "std".to_string(),
            checkpoints: 0,
            metadata_schema_version: Some("1".into()), level_uuid: Some("550e8400-e29b-41d4-a716-446655440000".into()), bronze_time_ms: Some("0".into()), silver_time_ms: Some("0".into()), gold_time_ms: Some("0".into()),
        }
    }

    #[test]
    fn test_valid_metadata() {
        assert!(validate_metadata(&make_meta("cool_level")).is_ok());
        assert!(validate_metadata(&make_meta("level-01")).is_ok());
        assert!(validate_metadata(&make_meta("my_level_2")).is_ok());
    }

    #[test]
    fn test_empty_course_id() {
        let result = validate_metadata(&make_meta(""));
        assert!(matches!(result, Err(CompileError::EmptyCourseId)));
    }

    #[test]
    fn test_invalid_course_id_space() {
        let result = validate_metadata(&make_meta("has space"));
        assert!(matches!(result, Err(CompileError::InvalidCourseId(_))));
    }

    #[test]
    fn test_invalid_course_id_special_chars() {
        let result = validate_metadata(&make_meta("level/file"));
        assert!(matches!(result, Err(CompileError::InvalidCourseId(_))));
    }

    #[test]
    fn test_invalid_course_id_dots() {
        let result = validate_metadata(&make_meta("level.map"));
        assert!(matches!(result, Err(CompileError::InvalidCourseId(_))));
    }

    #[test]
    fn test_extended_schema_versions() {
        for raw in [None, Some("0"), Some("2"), Some("1.0"), Some("1e0"), Some("+1"), Some(""), Some("2147483648")] {
            let mut meta = make_meta("schema");
            meta.metadata_schema_version = raw.map(str::to_owned);
            assert!(validate_extended_metadata(&meta).is_err(), "unexpectedly accepted schema {raw:?}");
        }
        for raw in ["1", "0001", " 1 "] {
            let mut meta = make_meta("schema");
            meta.metadata_schema_version = Some(raw.into());
            assert_eq!(validate_extended_metadata(&meta).unwrap().metadata_schema_version, 1, "schema {raw:?}");
        }
    }

    #[test]
    fn test_extended_uuid_layout() {
        for raw in [None, Some(""), Some(" "), Some("550e8400e29b41d4a716446655440000"), Some("550e8400_e29b-41d4-a716-446655440000"), Some("550e8400-e29b-41d4-a716-44665544000g"), Some("550e8400-e29b-41d4-a716-44665544000"), Some("550e8400-e29b-41d4-a716-4466554400000")] {
            let mut meta = make_meta("uuid");
            meta.level_uuid = raw.map(str::to_owned);
            assert!(validate_extended_metadata(&meta).is_err(), "unexpectedly accepted UUID {raw:?}");
        }
        let cases = [
            ("550e8400-e29b-41d4-a716-446655440000", "550e8400-e29b-41d4-a716-446655440000"),
            ("  550E8400-E29B-41D4-A716-446655440000\t", "550e8400-e29b-41d4-a716-446655440000"),
            ("00000000-0000-0000-0000-000000000000", "00000000-0000-0000-0000-000000000000"),
            ("550e8400-e29b-91d4-a716-446655440000", "550e8400-e29b-91d4-a716-446655440000"),
        ];
        for (raw, expected) in cases {
            let mut meta = make_meta("uuid"); meta.level_uuid = Some(raw.into());
            assert_eq!(validate_extended_metadata(&meta).unwrap().level_uuid, expected);
        }
    }

    #[test]
    fn test_extended_unconfigured_and_partial_targets() {
        for values in [[None, None, None], [Some("0"), Some("0"), Some("0")]] {
            let mut meta = make_meta("targets");
            meta.bronze_time_ms = values[0].map(str::to_owned); meta.silver_time_ms = values[1].map(str::to_owned); meta.gold_time_ms = values[2].map(str::to_owned);
            let result = validate_extended_metadata(&meta).unwrap();
            assert_eq!((result.bronze_time_ms, result.silver_time_ms, result.gold_time_ms, result.medal_configured), (0, 0, 0, false));
        }
        for values in [["0", "0", "0"], ["60000", "45000", "30000"]] {
            for mask in 1u8..7 {
                let mut meta = make_meta("targets");
                let mut fields = [Some(values[0].to_owned()), Some(values[1].to_owned()), Some(values[2].to_owned())];
                for (i, field) in fields.iter_mut().enumerate() { if mask & (1 << i) == 0 { *field = None; } }
                meta.bronze_time_ms = fields[0].clone(); meta.silver_time_ms = fields[1].clone(); meta.gold_time_ms = fields[2].clone();
                let raw = fields.clone();
                assert!(validate_extended_metadata(&meta).is_err(), "mask {mask}, values {values:?}");
                assert_eq!([meta.bronze_time_ms, meta.silver_time_ms, meta.gold_time_ms], raw);
            }
        }
        for values in [["0", "45000", "30000"], ["60000", "0", "30000"], ["60000", "45000", "0"]] {
            let mut meta = make_meta("targets");
            meta.bronze_time_ms = Some(values[0].into()); meta.silver_time_ms = Some(values[1].into()); meta.gold_time_ms = Some(values[2].into());
            let raw = [meta.bronze_time_ms.clone(), meta.silver_time_ms.clone(), meta.gold_time_ms.clone()];
            assert!(validate_extended_metadata(&meta).is_err());
            assert_eq!([meta.bronze_time_ms, meta.silver_time_ms, meta.gold_time_ms], raw);
        }
    }

    #[test]
    fn test_extended_order_and_maximum() {
        for (values, expected) in [(["60000", "45000", "30000"], [60000, 45000, 30000]), (["2147483647", "2147483646", "2147483645"], [2147483647, 2147483646, 2147483645]), ([" 060000 ", "045000", " 30000"], [60000, 45000, 30000])] {
            let mut meta = make_meta("ordered"); meta.bronze_time_ms = Some(values[0].into()); meta.silver_time_ms = Some(values[1].into()); meta.gold_time_ms = Some(values[2].into());
            for mode in ["std", "pg", "fz"] {
                meta.game_mode = mode.into();
                let result = validate_extended_metadata(&meta).unwrap();
                assert_eq!((result.bronze_time_ms, result.silver_time_ms, result.gold_time_ms, result.medal_configured), (expected[0], expected[1], expected[2], true));
            }
        }
        for values in [["60000", "60000", "30000"], ["60000", "45000", "45000"], ["45000", "60000", "30000"], ["60000", "30000", "45000"], ["60000", "45000", "0"], ["0", "45000", "30000"], ["60000", "0", "30000"]] {
            let mut meta = make_meta("ordered"); meta.bronze_time_ms = Some(values[0].into()); meta.silver_time_ms = Some(values[1].into()); meta.gold_time_ms = Some(values[2].into());
            assert!(validate_extended_metadata(&meta).is_err(), "unexpectedly accepted {values:?}");
        }
    }

    #[test]
    fn test_extended_invalid_time_lexemes() {
        let invalid = ["", " ", "-1", "+1", "1.0", "1e3", "nan", "true", "false", "2147483648", "999999999999999999999999999999999999999999", "\u{ff11}", "\u{0661}"];
        for field in 0..3 {
            for raw in invalid {
                let mut meta = make_meta("lexeme");
                let mut values = [Some("60000".into()), Some("45000".into()), Some("30000".into())];
                values[field] = Some(raw.into());
                meta.bronze_time_ms = values[0].clone(); meta.silver_time_ms = values[1].clone(); meta.gold_time_ms = values[2].clone();
                assert!(validate_extended_metadata(&meta).is_err(), "field {field}, lexeme {raw:?}");
            }
        }
    }

    #[test]
    fn test_decimal_leading_zeroes_and_bounds() {
        assert_eq!(decode_ascii_decimal("0"), Some(0));
        assert_eq!(decode_ascii_decimal("2147483647"), Some(2147483647));
        assert_eq!(decode_ascii_decimal(" \t2147483647\r\n"), Some(2147483647));
        let long_valid = format!("{}2147483647", "0".repeat(10000));
        assert_eq!(decode_ascii_decimal(&long_valid), Some(2147483647));
        assert_eq!(decode_ascii_decimal(&"0".repeat(10000)), Some(0));
        for raw in ["", " ", "+1", "-1", "1.0", "1e3", "\u{ff11}", "\u{0661}", "2147483648"] {
            assert_eq!(decode_ascii_decimal(raw), None, "lexeme {raw:?}");
        }
    }
}
