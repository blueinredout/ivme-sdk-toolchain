use crate::error::CompileError;
use crate::metadata::{LevelMetadata, validate_metadata, validate_extended_metadata, RECOGNIZED_GAME_MODES};

pub struct ValidationResult {
    pub metadata: LevelMetadata,
    pub checkpoint_count: i32,
    pub warnings: Vec<String>,
}

pub fn validate_all(
    metadata: Option<LevelMetadata>,
    checkpoint_count: i32,
    player_spawn_teams: &[i32],
) -> Result<ValidationResult, CompileError> {
    let meta = metadata.ok_or(CompileError::NoMetadataEntity)?;
    validate_metadata(&meta)?;
    validate_extended_metadata(&meta)?;
    if !RECOGNIZED_GAME_MODES.contains(&meta.game_mode.as_str()) {
        return Err(CompileError::InvalidExtendedMetadata(format!("unsupported game_mode: {}", meta.game_mode)));
    }

    // FreezeTag needs enough spawns, and team-specific spawns need both teams.
    if meta.game_mode == "fz" {
        validate_freeze_tag_spawns(player_spawn_teams)?;
    }

    let mut warnings = Vec::new();

    // Checkpoint mismatch
    if meta.checkpoints != checkpoint_count {
        warnings.push(format!(
            "Warning: CHECKPOINT_MISMATCH - Metadata declares {} checkpoints, found {} trigger_checkpoint entities",
            meta.checkpoints, checkpoint_count
        ));
    }

    Ok(ValidationResult {
        metadata: meta,
        checkpoint_count,
        warnings,
    })
}

/// FreezeTag requires at least two player spawns. If any spawn is
/// team-specific (`team != 0`), both team 1 and team 2 must be present.
fn validate_freeze_tag_spawns(player_spawn_teams: &[i32]) -> Result<(), CompileError> {
    if player_spawn_teams.len() < 2 {
        return Err(CompileError::FreezeTagNotEnoughSpawns(
            player_spawn_teams.len(),
        ));
    }

    let team1 = player_spawn_teams.iter().filter(|&&t| t == 1).count();
    let team2 = player_spawn_teams.iter().filter(|&&t| t == 2).count();
    let has_team_specific = player_spawn_teams.iter().any(|&t| t != 0);

    if has_team_specific && (team1 == 0 || team2 == 0) {
        return Err(CompileError::FreezeTagMissingTeamSpawns(team1, team2));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_meta(course_id: &str, game_mode: &str, checkpoints: i32) -> LevelMetadata {
        LevelMetadata {
            course_id: course_id.to_string(),
            level_name: "Test".to_string(),
            level_creator: "Author".to_string(),
            level_description: "Desc".to_string(),
            game_mode: game_mode.to_string(),
            checkpoints,
            metadata_schema_version: Some("1".into()), level_uuid: Some("550e8400-e29b-41d4-a716-446655440000".into()), bronze_time_ms: Some("0".into()), silver_time_ms: Some("0".into()), gold_time_ms: Some("0".into()),
        }
    }

    #[test]
    fn test_no_metadata_fails() {
        let result = validate_all(None, 0, &[0, 0]);
        assert!(matches!(result, Err(CompileError::NoMetadataEntity)));
    }

    #[test]
    fn test_valid_metadata() {
        let result = validate_all(Some(make_meta("test", "std", 0)), 0, &[0, 0]);
        assert!(result.is_ok());
        let v = result.unwrap();
        assert!(v.warnings.is_empty());
    }

    #[test]
    fn test_checkpoint_mismatch() {
        let result = validate_all(Some(make_meta("test", "std", 5)), 3, &[0, 0]);
        assert!(result.is_ok());
        assert!(!result.unwrap().warnings.is_empty());
    }

    #[test]
    fn test_unknown_game_mode() {
        let result = validate_all(Some(make_meta("test", "unknown_mode", 0)), 0, &[0, 0]);
        assert!(result.is_err());
    }

    #[test]
    fn test_recognized_game_modes() {
        for mode in ["std", "pg", "fz"] {
            let result = validate_all(Some(make_meta("test", mode, 0)), 0, &[0, 0]);
            assert!(result.is_ok(), "Mode {} should be recognized", mode);
        }
    }

    #[test]
    fn test_fz_two_team_agnostic_spawns_ok() {
        let result = validate_all(Some(make_meta("test", "fz", 0)), 0, &[0, 0]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_fz_single_spawn_fails() {
        let result = validate_all(Some(make_meta("test", "fz", 0)), 0, &[0]);
        assert!(matches!(
            result,
            Err(CompileError::FreezeTagNotEnoughSpawns(1))
        ));
    }

    #[test]
    fn test_fz_no_spawns_fails() {
        let result = validate_all(Some(make_meta("test", "fz", 0)), 0, &[]);
        assert!(matches!(
            result,
            Err(CompileError::FreezeTagNotEnoughSpawns(0))
        ));
    }

    #[test]
    fn test_fz_team_spawns_need_both_teams() {
        // Team 1 present, team 2 missing.
        let result = validate_all(Some(make_meta("test", "fz", 0)), 0, &[1, 1, 0]);
        assert!(matches!(
            result,
            Err(CompileError::FreezeTagMissingTeamSpawns(2, 0))
        ));

        // Team 2 present, team 1 missing.
        let result = validate_all(Some(make_meta("test", "fz", 0)), 0, &[2, 2]);
        assert!(matches!(
            result,
            Err(CompileError::FreezeTagMissingTeamSpawns(0, 2))
        ));
    }

    #[test]
    fn test_fz_both_teams_ok() {
        let result = validate_all(Some(make_meta("test", "fz", 0)), 0, &[1, 2]);
        assert!(result.is_ok());

        // Team-agnostic spawns may be mixed in.
        let result = validate_all(Some(make_meta("test", "fz", 0)), 0, &[0, 0, 1, 2]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_fz_spawn_rules_apply_only_to_fz() {
        for mode in ["std", "pg"] {
            let result = validate_all(Some(make_meta("test", mode, 0)), 0, &[]);
            assert!(result.is_ok(), "Mode {} should ignore spawn rules", mode);
            let result = validate_all(Some(make_meta("test", mode, 0)), 0, &[1]);
            assert!(result.is_ok(), "Mode {} should ignore spawn rules", mode);
        }
    }
}
