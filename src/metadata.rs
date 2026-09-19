use crate::error::CompileError;

#[derive(Debug, Clone)]
pub struct LevelMetadata {
    pub map_id: String,
    pub level_name: String,
    pub level_creator: String,
    pub level_description: String,
    pub game_mode: String,
    pub checkpoints: i32,
}

pub const RECOGNIZED_GAME_MODES: &[&str] = &["std", "tag", "infection"];

pub fn validate_metadata(meta: &LevelMetadata) -> Result<(), CompileError> {
    if meta.map_id.is_empty() {
        return Err(CompileError::EmptyMapId);
    }
    if !meta.map_id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(CompileError::InvalidMapId(meta.map_id.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(map_id: &str) -> LevelMetadata {
        LevelMetadata {
            map_id: map_id.to_string(),
            level_name: "Test".to_string(),
            level_creator: "Author".to_string(),
            level_description: "Desc".to_string(),
            game_mode: "std".to_string(),
            checkpoints: 0,
        }
    }

    #[test]
    fn test_valid_metadata() {
        assert!(validate_metadata(&make_meta("cool_level")).is_ok());
        assert!(validate_metadata(&make_meta("level-01")).is_ok());
        assert!(validate_metadata(&make_meta("my_level_2")).is_ok());
    }

    #[test]
    fn test_empty_map_id() {
        let result = validate_metadata(&make_meta(""));
        assert!(matches!(result, Err(CompileError::EmptyMapId)));
    }

    #[test]
    fn test_invalid_map_id_space() {
        let result = validate_metadata(&make_meta("has space"));
        assert!(matches!(result, Err(CompileError::InvalidMapId(_))));
    }

    #[test]
    fn test_invalid_map_id_special_chars() {
        let result = validate_metadata(&make_meta("level/file"));
        assert!(matches!(result, Err(CompileError::InvalidMapId(_))));
    }

    #[test]
    fn test_invalid_map_id_dots() {
        let result = validate_metadata(&make_meta("level.map"));
        assert!(matches!(result, Err(CompileError::InvalidMapId(_))));
    }
}
