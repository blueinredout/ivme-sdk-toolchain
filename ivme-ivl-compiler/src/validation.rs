use crate::error::CompileError;
use crate::metadata::{LevelMetadata, RECOGNIZED_GAME_MODES, validate_metadata};

pub struct ValidationResult {
    pub metadata: LevelMetadata,
    pub checkpoint_count: i32,
    pub warnings: Vec<String>,
}

pub fn validate_all(
    metadata: Option<LevelMetadata>,
    checkpoint_count: i32,
) -> Result<ValidationResult, CompileError> {
    let meta = metadata.ok_or(CompileError::NoMetadataEntity)?;
    validate_metadata(&meta)?;

    let mut warnings = Vec::new();

    // Checkpoint mismatch
    if meta.checkpoints != checkpoint_count {
        warnings.push(format!(
            "Warning: CHECKPOINT_MISMATCH - Metadata declares {} checkpoints, found {} trigger_checkpoint entities",
            meta.checkpoints, checkpoint_count
        ));
    }

    // Unknown game mode
    if !RECOGNIZED_GAME_MODES.contains(&meta.game_mode.as_str()) {
        warnings.push(format!(
            "Warning: UNKNOWN_GAME_MODE - \"{}\" is not a recognized game mode",
            meta.game_mode
        ));
    }

    Ok(ValidationResult {
        metadata: meta,
        checkpoint_count,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(map_id: &str, game_mode: &str, checkpoints: i32) -> LevelMetadata {
        LevelMetadata {
            map_id: map_id.to_string(),
            level_name: "Test".to_string(),
            level_creator: "Author".to_string(),
            level_description: "Desc".to_string(),
            game_mode: game_mode.to_string(),
            checkpoints,
        }
    }

    #[test]
    fn test_no_metadata_fails() {
        let result = validate_all(None, 0);
        assert!(matches!(result, Err(CompileError::NoMetadataEntity)));
    }

    #[test]
    fn test_valid_metadata() {
        let result = validate_all(Some(make_meta("test", "std", 0)), 0);
        assert!(result.is_ok());
        let v = result.unwrap();
        assert!(v.warnings.is_empty());
    }

    #[test]
    fn test_checkpoint_mismatch() {
        let result = validate_all(Some(make_meta("test", "std", 5)), 3);
        assert!(result.is_ok());
        assert!(!result.unwrap().warnings.is_empty());
    }

    #[test]
    fn test_unknown_game_mode() {
        let result = validate_all(Some(make_meta("test", "unknown_mode", 0)), 0);
        assert!(result.is_ok());
        let v = result.unwrap();
        assert!(v.warnings.iter().any(|w| w.contains("UNKNOWN_GAME_MODE")));
    }

    #[test]
    fn test_recognized_game_modes() {
        for mode in RECOGNIZED_GAME_MODES {
            let result = validate_all(Some(make_meta("test", mode, 0)), 0);
            assert!(result.is_ok(), "Mode {} should be recognized", mode);
        }
    }
}
