use crate::error::CompileError;
use crate::metadata::LevelMetadata;
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug)]
pub struct ParseResult {
    pub metadata: Option<LevelMetadata>,
    pub textures: BTreeSet<String>,
    pub checkpoint_count: i32,
    /// `team` value of every `info_player_start` (0 = any team).
    pub player_spawn_teams: Vec<i32>,
}

pub fn parse_map(path: &Path, content: &str) -> Result<ParseResult, CompileError> {
    let is_vmf = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("vmf"))
        .unwrap_or(false);
    if is_vmf {
        parse_vmf(content)
    } else {
        parse_quake(path, content)
    }
}

/// Valve Map Format (Hammer / JACK VMF export, also read by func_godot).
/// Only three things are extracted: `"material"` values from `side`
/// blocks (any nesting — world solids and brush-entity solids alike),
/// top-level `entity` properties (metadata + checkpoint count).
/// Everything else (planes, UV axes, displacements, visgroups) is ignored.
fn parse_vmf(content: &str) -> Result<ParseResult, CompileError> {
    use std::collections::HashMap;

    let mut metadata = None;
    let mut checkpoint_count = 0;
    let mut player_spawn_teams = Vec::new();
    let mut textures = BTreeSet::new();
    let mut stack: Vec<String> = Vec::new();
    let mut pending: Option<String> = None;
    let mut entity_keys: HashMap<String, String> = HashMap::new();
    let mut side_material: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Block open: `{` alone (VMF style) or `name {` on one line.
        if trimmed == "{" || trimmed.ends_with('{') {
            let name = if trimmed == "{" {
                pending.take().unwrap_or_default().to_lowercase()
            } else {
                pending.take();
                trimmed[..trimmed.len() - 1].trim().to_lowercase()
            };
            if name == "entity" && stack.is_empty() {
                entity_keys.clear();
            } else if name == "side" {
                side_material = None;
            }
            stack.push(name);
            continue;
        }
        if trimmed == "}" {
            if let Some(name) = stack.pop() {
                if name == "side" {
                    if let Some(m) = side_material.take() {
                        if !is_tool_material(&m) {
                            textures.insert(m);
                        }
                    }
                } else if name == "entity" && stack.is_empty() {
                    if let Some(classname) = entity_keys.get("classname") {
                        if classname == "info_level_metadata" {
                            let meta = extract_metadata(&entity_keys)?;
                            match metadata {
                                None => metadata = Some(meta),
                                Some(_) => return Err(CompileError::DuplicateMetadata),
                            }
                        } else if classname == "trigger_checkpoint" {
                            checkpoint_count += 1;
                        } else if classname == "info_player_start" {
                            player_spawn_teams.push(parse_spawn_team(&entity_keys));
                        }
                    }
                    entity_keys.clear();
                }
            }
            pending = None;
            continue;
        }
        if let Some((key, value)) = parse_key_value(trimmed) {
            if stack.len() == 1 && stack.last().map(|s| s == "entity").unwrap_or(false) {
                entity_keys.insert(key, value);
            } else if stack.last().map(|s| s == "side").unwrap_or(false) && key == "material" {
                side_material = Some(value);
            }
            pending = None;
            continue;
        }
        // Bare block-name line (`world`, `solid`, `side`, …). Anything else
        // resets, so stray data can never be mistaken for a block name.
        if !trimmed.starts_with('"')
            && !trimmed.contains([' ', '\t'])
        {
            pending = Some(trimmed.to_string());
        } else {
            pending = None;
        }
    }

    Ok(ParseResult {
        metadata,
        textures,
        checkpoint_count,
        player_spawn_teams,
    })
}

/// Tool / invisible materials carry no visible texture: Quake-style
/// `__` prefixes plus Valve's `tools/` namespace (case-insensitive).
fn is_tool_material(m: &str) -> bool {
    m.starts_with("__") || m.to_lowercase().starts_with("tools/")
}

pub fn parse_quake(_path: &Path, content: &str) -> Result<ParseResult, CompileError> {
    let mut metadata = None;
    let mut checkpoint_count = 0;
    let mut player_spawn_teams = Vec::new();
    let mut textures = BTreeSet::new();
    let mut depth = 0;
    let mut in_entity = false;
    let mut entity_keys = std::collections::HashMap::new();
    let mut current_key: Option<String> = None;

    for (line_idx, line) in content.lines().enumerate() {
        let _line_num = line_idx + 1;
        let trimmed = line.trim();

        // Track brace depth for entity structure
        for ch in trimmed.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    if depth == 1 {
                        in_entity = true;
                        entity_keys.clear();
                    }
                }
                '}' => {
                    if depth == 1 && in_entity {
                        // End of entity root - check if this is metadata or checkpoint
                        if let Some(classname) = entity_keys.get("classname") {
                            if classname == "info_level_metadata" {
                                let meta = extract_metadata(&entity_keys)?;
                                match metadata {
                                    None => metadata = Some(meta),
                                    Some(_) => return Err(CompileError::DuplicateMetadata),
                                }
                            } else if classname == "trigger_checkpoint" {
                                checkpoint_count += 1;
                            } else if classname == "info_player_start" {
                                player_spawn_teams.push(parse_spawn_team(&entity_keys));
                            }
                        }
                        in_entity = false;
                        entity_keys.clear();
                    }
                    if depth > 0 {
                        depth -= 1;
                    }
                }
                _ => {}
            }
        }

        // Parse key/value pairs at entity root level (depth == 1)
        if depth == 1 && in_entity {
            if let Some((key, value)) = parse_key_value(trimmed) {
                entity_keys.insert(key.clone(), value.clone());
                current_key = Some(key);
            } else if current_key.is_some() {
                // Continuation line or brush start - stop tracking keys
                current_key = None;
            }
        }

        // Extract texture references from brush faces at depth >= 2
        if depth >= 2 {
            if let Some(tex) = parse_brush_face_texture(trimmed) {
                if !tex.starts_with("__") {
                    textures.insert(tex);
                }
            }
        }
    }

    Ok(ParseResult {
        metadata,
        textures,
        checkpoint_count,
        player_spawn_teams,
    })
}

fn parse_key_value(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    if trimmed.starts_with('"') {
        if let Some(end_quote) = trimmed[1..].find('"') {
            let key = trimmed[1..=end_quote].to_string();
            let rest = trimmed[end_quote + 2..].trim();
            if rest.starts_with('"') {
                if let Some(val_end) = rest[1..].find('"') {
                    let value = rest[1..=val_end].to_string();
                    return Some((key, value));
                }
            }
        }
    }
    None
}

fn parse_brush_face_texture(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if !trimmed.starts_with('(') {
        return None;
    }

    // Count point groups: ( x y z )
    let mut point_groups = 0;
    let mut paren_depth = 0;

    for ch in trimmed.chars() {
        match ch {
            '(' => paren_depth += 1,
            ')' => {
                paren_depth -= 1;
                if paren_depth == 0 {
                    point_groups += 1;
                }
            }
            _ => {}
        }

        if point_groups >= 3 {
            break;
        }
    }

    if point_groups < 3 {
        return None;
    }

    // Find position after the third ") "
    let mut chars = trimmed.char_indices();
    let mut groups_found = 0;
    let mut in_paren = false;

    for (i, ch) in chars.by_ref() {
        match ch {
            '(' => {
                if !in_paren && paren_depth == 0 {
                    groups_found += 1;
                    in_paren = true;
                }
            }
            ')' => {
                if in_paren {
                    in_paren = false;
                    if groups_found == 3 {
                        // The texture name starts after this closing paren
                        let rest = trimmed[i + 1..].trim();
                        // Skip Valve 220 bracket syntax - texture name is before '['
                        let tex_name = if let Some(bracket_pos) = rest.find('[') {
                            rest[..bracket_pos].trim()
                        } else {
                            // Standard Quake: first token after third paren group
                            rest.split_whitespace().next().unwrap_or("")
                        };
                        if !tex_name.is_empty() && !tex_name.starts_with('(') {
                            return Some(tex_name.to_string());
                        }
                        return None;
                    }
                }
            }
            _ => {}
        }
    }

    None
}

fn parse_spawn_team(keys: &std::collections::HashMap<String, String>) -> i32 {
    keys.get("team")
        .and_then(|v| v.trim().parse::<i32>().ok())
        .unwrap_or(0)
}

fn extract_metadata(keys: &std::collections::HashMap<String, String>) -> Result<LevelMetadata, CompileError> {
    let course_id = keys.get("course_id").cloned().unwrap_or_default();
    let level_name = keys.get("level_name").cloned().unwrap_or_default();
    let level_creator = keys.get("level_creator").cloned().unwrap_or_default();
    let level_description = keys.get("level_description").cloned().unwrap_or_default();
    let game_mode = keys.get("game_mode").cloned().unwrap_or_default();
    let checkpoints_str = keys.get("checkpoints").cloned().unwrap_or_default();

    let checkpoints = if checkpoints_str.is_empty() {
        0
    } else {
        checkpoints_str.parse::<i32>().unwrap_or(0)
    };

    Ok(LevelMetadata {
        course_id,
        level_name,
        level_creator,
        level_description,
        game_mode,
        checkpoints,
        metadata_schema_version: keys.get("metadata_schema_version").cloned(),
        level_uuid: keys.get("level_uuid").cloned(),
        bronze_time_ms: keys.get("bronze_time_ms").cloned(),
        silver_time_ms: keys.get("silver_time_ms").cloned(),
        gold_time_ms: keys.get("gold_time_ms").cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::validate_extended_metadata;

    #[test]
    fn test_extended_metadata_property_order_and_nesting() {
        for ext in ["map", "vmf"] { for before in [false,true] {
            let fields = r#""metadata_schema_version" "0001"
"level_uuid" "550E8400-E29B-41D4-A716-446655440000"
"bronze_time_ms" " 060000 "
"silver_time_ms" "045000"
"gold_time_ms" "30000""#;
            let root = r#""course_id" "root_course"
"level_name" "Root Name"
"level_creator" "Creator"
"level_description" "Description"
"game_mode" "std"
"checkpoints" "3""#;
            let props=if before {format!("{fields}\n{root}\n\"classname\" \"info_level_metadata\"")} else {format!("\"classname\" \"info_level_metadata\"\n{fields}\n{root}")};
            let nested = r#""metadata_schema_version" "9"
"level_uuid" "bad"
"course_id" "nested"
"game_mode" "nested"
"bronze_time_ms" "1""#;
            let input=if ext=="map" {format!("{{\n{props}\n{{\n\"classname\" \"nested\"\n{nested}\n}}\n}}\n{{\n\"classname\" \"other\"\n\"bronze_time_ms\" \"2\"\n}}") } else {format!("entity\n{{\n{props}\nsolid\n{{\nentity\n{{\n\"classname\" \"info_level_metadata\"\n{nested}\n}}\n}}\n}}\nentity\n{{\n\"classname\" \"other\"\n\"bronze_time_ms\" \"2\"\n}}")};
            let snapshot=input.clone(); let result=parse_map(Path::new(if ext=="map"{"x.map"}else{"x.vmf"}),&input).unwrap(); assert_eq!(input,snapshot); let m=result.metadata.unwrap();
            assert_eq!((&m.course_id,&m.level_name,&m.level_creator,&m.level_description,&m.game_mode,m.checkpoints),(&"root_course".to_string(),&"Root Name".to_string(),&"Creator".to_string(),&"Description".to_string(),&"std".to_string(),3));
            assert_eq!(m.metadata_schema_version.as_deref(),Some("0001")); assert_eq!(m.level_uuid.as_deref(),Some("550E8400-E29B-41D4-A716-446655440000")); assert_eq!(m.bronze_time_ms.as_deref(),Some(" 060000 ")); assert_eq!(m.silver_time_ms.as_deref(),Some("045000")); assert_eq!(m.gold_time_ms.as_deref(),Some("30000"));
            let normalized=validate_extended_metadata(&m).unwrap(); assert_eq!(normalized.level_uuid,"550e8400-e29b-41d4-a716-446655440000"); assert_eq!(normalized.metadata_schema_version,1); assert_eq!((normalized.bronze_time_ms,normalized.silver_time_ms,normalized.gold_time_ms),(60000,45000,30000)); assert!(normalized.medal_configured);
        }}
    }

    #[test]
    fn test_extended_metadata_presence_and_raw_lexemes() {
        for ext in ["map","vmf"] { let path=Path::new(if ext=="map"{"x.map"}else{"x.vmf"}); let start=if ext=="map"{"{\n\"classname\" \"info_level_metadata\"\n"}else{"entity\n{\n\"classname\" \"info_level_metadata\"\n"}; let absent=format!("{start}}}"); let m=parse_map(path,&absent).unwrap().metadata.unwrap(); assert!(m.metadata_schema_version.is_none()&&m.level_uuid.is_none()&&m.bronze_time_ms.is_none()&&m.silver_time_ms.is_none()&&m.gold_time_ms.is_none()); assert_eq!(m.game_mode,"");
            let zeros=format!("{start}\"bronze_time_ms\" \"0\"\n\"silver_time_ms\" \"0\"\n\"gold_time_ms\" \"0\"\n}}"); let m=parse_map(path,&zeros).unwrap().metadata.unwrap(); assert_eq!((m.bronze_time_ms.as_deref(),m.silver_time_ms.as_deref(),m.gold_time_ms.as_deref()),(Some("0"),Some("0"),Some("0")));
            let base=format!("{start}\"metadata_schema_version\" \"1\"\n\"level_uuid\" \"550e8400-e29b-41d4-a716-446655440000\"\n\"bronze_time_ms\" \"1000\"\n\"silver_time_ms\" \"500\"\n\"gold_time_ms\" \"100\"\n}}"); let invalid_bronze=base.replace("\"1000\"","\"1e3\""); let m=parse_map(path,&invalid_bronze).unwrap().metadata.unwrap(); assert_eq!(m.bronze_time_ms.as_deref(),Some("1e3")); assert!(validate_extended_metadata(&m).is_err()); let invalid_schema=base.replace("\"1\"","\"2\""); let m=parse_map(path,&invalid_schema).unwrap().metadata.unwrap(); assert_eq!(m.metadata_schema_version.as_deref(),Some("2")); assert!(validate_extended_metadata(&m).is_err());
            let authored=base.replace("\n}", "\n\"author_time_ms\" \"ignored\"\n\"author_provenance\" \"ignored\"\n}"); assert_ne!(authored,base); assert!(authored.contains("author_time_ms")); let base_m=parse_map(path,&base).unwrap().metadata.unwrap(); let authored_m=parse_map(path,&authored).unwrap().metadata.unwrap(); assert_eq!((&base_m.course_id,&base_m.level_name,&base_m.level_creator,&base_m.level_description,&base_m.game_mode,base_m.checkpoints,&base_m.metadata_schema_version,&base_m.level_uuid,&base_m.bronze_time_ms,&base_m.silver_time_ms,&base_m.gold_time_ms),(&authored_m.course_id,&authored_m.level_name,&authored_m.level_creator,&authored_m.level_description,&authored_m.game_mode,authored_m.checkpoints,&authored_m.metadata_schema_version,&authored_m.level_uuid,&authored_m.bronze_time_ms,&authored_m.silver_time_ms,&authored_m.gold_time_ms)); let b=validate_extended_metadata(&base_m).unwrap(); let a=validate_extended_metadata(&authored_m).unwrap(); assert_eq!((b.level_uuid,b.bronze_time_ms,b.silver_time_ms,b.gold_time_ms),(a.level_uuid,a.bronze_time_ms,a.silver_time_ms,a.gold_time_ms));
        }
    }

    #[test]
    fn test_extended_duplicate_metadata_root_only() {
        for ext in ["map","vmf"] { let path=Path::new(if ext=="map"{"x.map"}else{"x.vmf"}); let ent=if ext=="map"{"{\n\"classname\" \"info_level_metadata\"\n}"}else{"entity\n{\n\"classname\" \"info_level_metadata\"\n}"}; assert!(matches!(parse_map(path,&format!("{ent}\n{ent}")),Err(CompileError::DuplicateMetadata))); assert!(parse_map(path,if ext=="map"{"{\n\"classname\" \"worldspawn\"\n{\n\"classname\" \"info_level_metadata\"\n}\n}"}else{"entity\n{\n\"classname\" \"worldspawn\"\nentity\n{\n\"classname\" \"info_level_metadata\"\n}\n}"}).unwrap().metadata.is_none()); assert!(parse_map(path,"").unwrap().metadata.is_none());
            let valid=if ext=="map" {"{\n\"classname\" \"info_level_metadata\"\n\"course_id\" \"root\"\n\"metadata_schema_version\" \"1\"\n\"level_uuid\" \"550e8400-e29b-41d4-a716-446655440000\"\n\"bronze_time_ms\" \"1000\"\n\"silver_time_ms\" \"500\"\n\"gold_time_ms\" \"100\"\n}\n{\n\"classname\" \"other\"\n{\n\"classname\" \"info_level_metadata\"\n}\n}"}else{"entity\n{\n\"classname\" \"info_level_metadata\"\n\"course_id\" \"root\"\n\"metadata_schema_version\" \"1\"\n\"level_uuid\" \"550e8400-e29b-41d4-a716-446655440000\"\n\"bronze_time_ms\" \"1000\"\n\"silver_time_ms\" \"500\"\n\"gold_time_ms\" \"100\"\n}\nentity\n{\n\"classname\" \"other\"\nentity\n{\n\"classname\" \"info_level_metadata\"\n}\n}"}; let m=parse_map(path,valid).unwrap().metadata.unwrap(); assert_eq!(m.course_id,"root"); assert_eq!(m.metadata_schema_version.as_deref(),Some("1")); assert_eq!(m.level_uuid.as_deref(),Some("550e8400-e29b-41d4-a716-446655440000")); assert_eq!((m.bronze_time_ms.as_deref(),m.silver_time_ms.as_deref(),m.gold_time_ms.as_deref()),(Some("1000"),Some("500"),Some("100")));
        }
    }

    #[test]
    fn test_parse_key_value() {
        assert_eq!(
            parse_key_value(r#""key" "value""#),
            Some(("key".to_string(), "value".to_string()))
        );
    }

    #[test]
    fn test_parse_key_value_empty() {
        assert_eq!(
            parse_key_value(r#""key" """#),
            Some(("key".to_string(), "".to_string()))
        );
    }

    #[test]
    fn test_parse_key_value_no_value() {
        assert_eq!(parse_key_value(r#""key" "#), None);
    }

    #[test]
    fn test_parse_key_value_not_kv() {
        assert_eq!(parse_key_value("( 0 0 0 )"), None);
    }

    #[test]
    fn test_parse_brush_face_quake() {
        let line = "( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) TEXTURE 0 0 0 1 1";
        assert_eq!(parse_brush_face_texture(line), Some("TEXTURE".to_string()));
    }

    #[test]
    fn test_parse_brush_face_valve220() {
        let line = "( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) TEXTURE [ 1 0 0 0 ] [ 0 1 0 0 ] 0 1 1";
        assert_eq!(parse_brush_face_texture(line), Some("TEXTURE".to_string()));
    }

    #[test]
    fn test_tool_texture_filtered() {
        let line = "( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) __TB_empty 0 0 0 1 1";
        let tex = parse_brush_face_texture(line);
        assert!(tex.is_some());
        assert!(tex.unwrap().starts_with("__"));
    }

    #[test]
    fn test_texture_with_path() {
        let line = "( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) custom/my_wood 0 0 0 1 1";
        assert_eq!(parse_brush_face_texture(line), Some("custom/my_wood".to_string()));
    }

    #[test]
    fn test_not_a_face() {
        assert_eq!(parse_brush_face_texture("not a face"), None);
    }

    #[test]
    fn test_two_point_groups() {
        assert_eq!(parse_brush_face_texture("( 0 0 0 ) ( 0 1 0 ) TEXTURE"), None);
    }

    #[test]
    fn test_full_entity_parsing() {
        let input = r#"
{
"classname" "info_level_metadata"
"course_id" "test_level"
"level_name" "Test Level"
"level_creator" "Author"
"level_description" "A test"
"game_mode" "std"
"checkpoints" "3"
}
{
"classname" "worldspawn"
}
{
"classname" "trigger_checkpoint"
}
{
"classname" "trigger_checkpoint"
}
"#;
        let result = parse_map(Path::new("test.map"), input).unwrap();
        assert!(result.metadata.is_some());
        let meta = result.metadata.unwrap();
        assert_eq!(meta.course_id, "test_level");
        assert_eq!(meta.level_name, "Test Level");
        assert_eq!(meta.checkpoints, 3);
        assert_eq!(result.checkpoint_count, 2);
    }

    #[test]
    fn test_texture_extraction_from_brushes() {
        let input = r#"
{
"classname" "worldspawn"
{
( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) custom/wood 0 0 0 1 1
( 1 0 0 ) ( 1 1 0 ) ( 1 0 1 ) custom/brick 0 0 0 1 1
}
}
"#;
        let result = parse_map(Path::new("test.map"), input).unwrap();
        assert!(result.textures.contains("custom/wood"));
        assert!(result.textures.contains("custom/brick"));
    }

    #[test]
    fn test_tool_texture_excluded() {
        let input = r#"
{
"classname" "worldspawn"
{
( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) __TB_empty 0 0 0 1 1
( 1 0 0 ) ( 1 1 0 ) ( 1 0 1 ) custom/wood 0 0 0 1 1
}
}
"#;
        let result = parse_map(Path::new("test.map"), input).unwrap();
        assert!(!result.textures.iter().any(|t| t.starts_with("__")));
        assert!(result.textures.contains("custom/wood"));
    }

    #[test]
    fn test_duplicate_metadata_fails() {
        let input = r#"
{
"classname" "info_level_metadata"
"course_id" "test1"
}
{
"classname" "info_level_metadata"
"course_id" "test2"
}
"#;
        let result = parse_map(Path::new("test.map"), input);
        assert!(matches!(result, Err(CompileError::DuplicateMetadata)));
    }

    #[test]
    fn test_no_metadata() {
        let input = r#"
{
"classname" "worldspawn"
}
"#;
        let result = parse_map(Path::new("test.map"), input).unwrap();
        assert!(result.metadata.is_none());
    }

    #[test]
    fn test_nested_braces_in_entity() {
        let input = r#"
{
"classname" "worldspawn"
{
( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) tex1 0 0 0 1 1
{
( 0 0 0 ) ( 0 1 0 ) ( 0 0 1 ) tex2 0 0 0 1 1
}
}
}
"#;
        let result = parse_map(Path::new("test.map"), input).unwrap();
        assert!(result.textures.contains("tex1"));
        assert!(result.textures.contains("tex2"));
    }

    const VMF_SAMPLE: &str = r#"
versioninfo
{
	"editorversion" "400"
	"mapversion" "1"
}
world
{
	"id" "1"
	"classname" "worldspawn"
	solid
	{
		"id" "2"
		side
		{
			"id" "3"
			"plane" "(0 0 0) (1 0 0) (1 1 0)"
			"material" "BRICK/BRICKWALL01"
			"uaxis" "[1 0 0 0] 0.25"
		}
		side
		{
			"id" "4"
			"plane" "(0 0 1) (1 0 1) (1 1 1)"
			"material" "TOOLS/TOOLSNODRAW"
			"uaxis" "[1 0 0 0] 0.25"
		}
	}
}
entity
{
	"id" "5"
	"classname" "info_level_metadata"
	"course_id" "vmf_test"
	"level_name" "VMF Test"
	"level_creator" "Author"
	"level_description" "desc"
	"game_mode" "std"
	"checkpoints" "1"
	solid
	{
		"id" "6"
		side
		{
			"id" "7"
			"plane" "(0 0 0) (1 0 0) (0 0 1)"
			"material" "+0LAVA"
			"uaxis" "[1 0 0 0] 0.25"
		}
	}
}
entity
{
	"id" "8"
	"classname" "trigger_checkpoint"
	"origin" "0 0 64"
}
"#;

    #[test]
    fn test_vmf_materials_and_metadata() {
        let result = parse_map(Path::new("test.vmf"), VMF_SAMPLE).unwrap();
        // World solid + brush-entity solid both extracted; tools/ skipped.
        assert!(result.textures.contains("BRICK/BRICKWALL01"));
        assert!(result.textures.contains("+0LAVA"));
        assert!(!result.textures.iter().any(|t| t.to_lowercase().starts_with("tools/")));
        let meta = result.metadata.expect("metadata entity");
        assert_eq!(meta.course_id, "vmf_test");
        assert_eq!(result.checkpoint_count, 1);
    }

    #[test]
    fn test_vmf_extension_dispatch() {
        // Same bytes as .map must NOT yield VMF materials (Quake faces
        // don't exist in VMF; only `"material"` keys count there).
        let result = parse_map(Path::new("test.map"), VMF_SAMPLE).unwrap();
        assert!(result.textures.is_empty());
    }

    #[test]
    fn test_vmf_duplicate_metadata_fails() {
        let input = r#"
entity
{
	"classname" "info_level_metadata"
	"course_id" "a"
}
entity
{
	"classname" "info_level_metadata"
	"course_id" "b"
}
"#;
        let result = parse_map(Path::new("x.vmf"), input);
        assert!(matches!(result, Err(CompileError::DuplicateMetadata)));
    }

    #[test]
    fn test_player_spawn_teams_quake() {
        let input = r#"
{
"classname" "info_level_metadata"
"course_id" "spawns"
"game_mode" "fz"
}
{
"classname" "info_player_start"
"origin" "0 0 0"
}
{
"classname" "info_player_start"
"team" "1"
}
{
"classname" "info_player_start"
"team" "2"
}
{
"classname" "info_player_start"
"team" "bogus"
}
"#;
        let result = parse_map(Path::new("test.map"), input).unwrap();
        assert_eq!(result.player_spawn_teams, vec![0, 1, 2, 0]);
    }

    #[test]
    fn test_player_spawn_teams_vmf() {
        let input = r#"
entity
{
	"classname" "info_level_metadata"
	"course_id" "spawns"
	"game_mode" "fz"
}
entity
{
	"classname" "info_player_start"
	"origin" "0 0 0"
}
entity
{
	"classname" "info_player_start"
	"team" "1"
}
entity
{
	"classname" "info_player_start"
	"team" "2"
}
"#;
        let result = parse_map(Path::new("test.vmf"), input).unwrap();
        assert_eq!(result.player_spawn_teams, vec![0, 1, 2]);
    }
}
