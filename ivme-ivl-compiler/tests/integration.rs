use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static METADATA_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

struct OwnedMetadataFixture { root: PathBuf }

impl OwnedMetadataFixture {
    fn new() -> Self {
        let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = parent.join(format!("ivlc-metadata-tests-{}-{stamp}-{}", std::process::id(), METADATA_FIXTURE_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&root).unwrap();
        Self { root }
    }
    fn source(&self, ext: &str) -> PathBuf { self.root.join(format!("source.{ext}")) }
    fn output(&self) -> PathBuf { self.root.join("output.ivl") }
}

impl Drop for OwnedMetadataFixture {
    fn drop(&mut self) {
        let parent = fs::canonicalize(std::env::temp_dir()).unwrap();
        assert_eq!(self.root.parent(), Some(parent.as_path()));
        assert!(self.root.file_name().unwrap().to_string_lossy().starts_with("ivlc-metadata-tests-"));
        if self.root.exists() { fs::remove_dir_all(&self.root).unwrap(); }
    }
}

fn metadata_source(ext: &str, overrides: &[(&str, Option<&str>)], mode: Option<&str>, fz: bool) -> String {
    let defaults = [("course_id", Some("metadata_test")), ("level_name", Some("Metadata Test")), ("level_creator", Some("Tester")), ("level_description", Some("metadata test")), ("checkpoints", Some("0")), ("game_mode", mode), ("metadata_schema_version", Some("1")), ("level_uuid", Some("550e8400-e29b-41d4-a716-446655440000")), ("bronze_time_ms", Some("60000")), ("silver_time_ms", Some("45000")), ("gold_time_ms", Some("30000"))];
    let mut fields = defaults.to_vec();
    for (key, val) in overrides { if let Some(item) = fields.iter_mut().find(|x| x.0 == *key) { item.1 = *val; } }
    let mut out = if ext == "map" { String::new() } else { String::new() };
    let mut entity = |classname: &str, values: Vec<(&str, &str)>| {
        if ext == "vmf" { out.push_str("entity\n"); }
        out.push_str("{\n\"classname\" \""); out.push_str(classname); out.push_str("\"\n");
        for (k,v) in values { out.push_str("\""); out.push_str(k); out.push_str("\" \""); out.push_str(v); out.push_str("\"\n"); }
        out.push_str("}\n");
    };
    let values = fields.iter().filter_map(|(k,v)| v.map(|v| (*k,v))).collect();
    entity("info_level_metadata", values);
    if fz { entity("info_player_start", vec![("team","1")]); entity("info_player_start", vec![("team","2")]); }
    out
}

fn compile_case(ext: &str, source: &str, dry: bool) -> (OwnedMetadataFixture, String, String, i32) {
    let fixture = OwnedMetadataFixture::new(); let src = fixture.source(ext); fs::write(&src, source).unwrap();
    let snapshot = fs::read(&src).unwrap(); let output = fixture.output();
    let mut args = vec!["--map", src.to_str().unwrap(), "--output", output.to_str().unwrap()]; if dry { args.push("--dry-run"); }
    let (stdout, stderr, code) = run_compiler(&args); assert_eq!(fs::read(&src).unwrap(), snapshot);
    (fixture, stdout, stderr, code)
}

#[test]
fn test_metadata_source_compile_is_immutable() {
    for ext in ["map", "vmf"] {
        let source = metadata_source(ext, &[("bronze_time_ms", Some(" 060000 ")), ("level_uuid", Some("550E8400-E29B-41D4-A716-446655440000"))], Some("std"), false);
        let (fixture, stdout, stderr, code) = compile_case(ext, &source, false);
        assert_eq!(code, 0, "{stdout}\n{stderr}");
        let file = fs::File::open(fixture.output()).unwrap(); let mut archive = zip::ZipArchive::new(file).unwrap();
        let entry = if ext == "map" { "level.map" } else { "level.vmf" };
        let mut z = archive.by_name(entry).unwrap(); let mut bytes = Vec::new(); std::io::Read::read_to_end(&mut z, &mut bytes).unwrap();
        assert_eq!(bytes, source.as_bytes()); drop(z); drop(archive);
    }
}

#[test]
fn test_metadata_source_dry_run_is_immutable() {
    for ext in ["map", "vmf"] { let source = metadata_source(ext, &[], Some("std"), false); let fixture = OwnedMetadataFixture::new(); let p = fixture.source(ext); fs::write(&p, &source).unwrap(); let snap = fs::read(&p).unwrap();
        for _ in 0..2 { let (_, stderr, code) = run_compiler(&["--map",p.to_str().unwrap(),"--output",fixture.output().to_str().unwrap(),"--dry-run"]); assert_eq!(code,0,"{stderr}"); assert_eq!(fs::read(&p).unwrap(),snap); assert!(!fixture.output().exists()); }
    }
}

#[test]
fn test_metadata_cli_schema_and_identity_rejections() {
    let cases = [("metadata_schema_version",None,"schema"),("metadata_schema_version",Some("0"),"schema"),("metadata_schema_version",Some("2"),"schema"),("metadata_schema_version",Some("1.0"),"schema"),("level_uuid",None,"uuid"),("level_uuid",Some("bad"),"uuid")];
    for ext in ["map","vmf"] { for (key,value,needle) in cases { let s=metadata_source(ext,&[(key,value)],Some("std"),false); let (f,_,e,c)=compile_case(ext,&s,false); assert_eq!(c,1,"{e}"); assert!(e.to_lowercase().contains(needle),"{e}"); assert!(!f.output().exists()); } }
}

#[test]
fn test_metadata_cli_target_definitions() {
    for ext in ["map","vmf"] {
        for (overrides,valid) in [ (vec![("bronze_time_ms",None),("silver_time_ms",None),("gold_time_ms",None)],true), (vec![("bronze_time_ms",Some("0")),("silver_time_ms",Some("0")),("gold_time_ms",Some("0"))],true), (vec![],true), (vec![("bronze_time_ms",Some("0")),("silver_time_ms",None),("gold_time_ms",None)],false), (vec![("bronze_time_ms",None),("silver_time_ms",Some("0")),("gold_time_ms",None)],false), (vec![("bronze_time_ms",None),("silver_time_ms",None),("gold_time_ms",Some("0"))],false), (vec![("bronze_time_ms",Some("0")),("silver_time_ms",Some("0")),("gold_time_ms",None)],false), (vec![("bronze_time_ms",Some("0")),("silver_time_ms",None),("gold_time_ms",Some("0"))],false), (vec![("bronze_time_ms",None),("silver_time_ms",Some("0")),("gold_time_ms",Some("0"))],false), (vec![("bronze_time_ms",Some("1e3"))],false) ] { let s=metadata_source(ext,&overrides,Some("std"),false); let (f,_,e,c)=compile_case(ext,&s,false); assert_eq!(c,if valid {0}else{1},"{e}"); if !valid {assert!(!f.output().exists());} }
        for mask in 1..7 { let values=["60000","45000","30000"]; let keys=["bronze_time_ms","silver_time_ms","gold_time_ms"]; let overrides:Vec<_>=(0..3).map(|i|(keys[i],if mask&(1<<i)!=0 {Some(values[i])} else {None})).collect(); let s=metadata_source(ext,&overrides,Some("std"),false); let(f,_,e,c)=compile_case(ext,&s,false); assert_eq!(c,1,"{e}"); assert!(!f.output().exists()); }
        for v in [["60000","60000","30000"],["60000","45000","45000"],["45000","60000","30000"],["60000","30000","45000"],["0","45000","30000"],["60000","0","30000"],["60000","45000","0"]] { let ov=[("bronze_time_ms",Some(v[0])),("silver_time_ms",Some(v[1])),("gold_time_ms",Some(v[2]))]; let s=metadata_source(ext,&ov,Some("std"),false); let(f,_,e,c)=compile_case(ext,&s,false); assert_eq!(c,1,"{e}"); assert!(!f.output().exists()); }
    }
}

#[test]
fn test_metadata_cli_declared_modes() {
    for ext in ["map","vmf"] { for mode in [None,Some(""),Some("unknown"),Some("tag"),Some("infection"),Some("hub")] { let s=metadata_source(ext,&[],mode,false); let(f,_,e,c)=compile_case(ext,&s,false); assert_eq!(c,1,"{e}"); assert!(!f.output().exists()); }
        for mode in ["std","pg","fz"] { let s=metadata_source(ext,&[],Some(mode),mode=="fz"); let(f,_,e,c)=compile_case(ext,&s,false); assert_eq!(c,0,"{e}"); assert!(f.output().exists()); }
    }
}

#[test]
fn test_metadata_cli_duplicate_and_owned_fixtures() {
    let a=OwnedMetadataFixture::new(); let b=OwnedMetadataFixture::new(); assert_ne!(a.root,b.root); let sentinel=b.root.join("sentinel"); fs::write(&sentinel,b"alive").unwrap(); let root=a.root.clone(); drop(a); assert!(!root.exists()); assert_eq!(fs::read(sentinel).unwrap(),b"alive");
    for ext in ["map","vmf"] { let s=metadata_source(ext,&[],Some("std"),false); let dup=format!("{s}{s}"); let(f,_,e,c)=compile_case(ext,&dup,false); assert_eq!(c,1,"{e}"); assert!(!f.output().exists()); }
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn create_minimal_png(path: &Path) {
    let img = image::ImageBuffer::from_pixel(1, 1, image::Rgb([255u8, 255, 255]));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).ok();
    }
    img.save(path).expect("Failed to save PNG");
}

fn setup_fixtures() {
    let fixtures = fixtures_dir();
    
    // Happy path
    create_minimal_png(&fixtures.join("happy_path/textures/custom/wood.png"));
    create_minimal_png(&fixtures.join("happy_path/textures/custom/brick.png"));
    create_minimal_png(&fixtures.join("happy_path/preview.png"));
    
    // Orphaned texture fixtures
    create_minimal_png(&fixtures.join("orphaned_texture/textures/custom/used_tex.png"));
    create_minimal_png(&fixtures.join("orphaned_texture/textures/custom/orphan_tex.png"));
}

fn run_compiler(args: &[&str]) -> (String, String, i32) {
    let exe = env!("CARGO_BIN_EXE_ivlc");
    let output = Command::new(exe)
        .args(args)
        .output()
        .expect("Failed to execute compiler");
    
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(1);
    
    (stdout, stderr, code)
}

#[test]
fn test_happy_path_compilation() {
    setup_fixtures();
    let map = fixtures_dir().join("happy_path/level.map");
    let output = fixtures_dir().join("happy_path/output.ivl");
    
    // Clean up any previous output
    let _ = fs::remove_file(&output);
    
    let (stdout, stderr, code) = run_compiler(&[
        "--map", map.to_str().unwrap(),
        "--output", output.to_str().unwrap(),
    ]);
    
    assert_eq!(code, 0, "Expected exit code 0, got {}. Stderr: {}", code, stderr);
    assert!(stdout.contains("Compiled successfully"));
    assert!(stdout.contains("course_id:    happy_level"));
    assert!(stdout.contains("Textures: 2 files (2 sets)"));
    assert!(output.exists(), "Output .ivl should exist");
    
    // Verify ZIP contents
    let file = fs::File::open(&output).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut file_names = Vec::new();
    for i in 0..archive.len() {
        file_names.push(archive.by_index(i).unwrap().name().to_string());
    }
    
    assert!(file_names.contains(&"level.map".to_string()), "level.map should be in archive");
    assert!(file_names.contains(&"preview.png".to_string()), "preview.png should be in archive");
    assert!(file_names.iter().any(|n| n.starts_with("textures/")), "textures should be in archive");
    assert!(!file_names.iter().any(|n| n.strip_prefix("textures/").map(|r| r.contains('/')).unwrap_or(false)), "textures/ must be flat");
    
    // Verify level.map bytes match original
    let original_bytes = fs::read(&map).unwrap();
    let file = fs::File::open(&output).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut map_entry = archive.by_name("level.map").unwrap();
    let mut archive_bytes = Vec::new();
    std::io::Read::read_to_end(&mut map_entry, &mut archive_bytes).unwrap();
    assert_eq!(original_bytes, archive_bytes, "level.map bytes should match exactly");
    
    let _ = fs::remove_file(&output);
}

#[test]
fn test_dry_run() {
    setup_fixtures();
    let map = fixtures_dir().join("happy_path/level.map");
    let output = fixtures_dir().join("happy_path/dry_run.ivl");
    
    let (stdout, stderr, code) = run_compiler(&[
        "--map", map.to_str().unwrap(),
        "--output", output.to_str().unwrap(),
        "--dry-run",
    ]);
    
    assert_eq!(code, 0, "Expected exit code 0, got {}. Stderr: {}", code, stderr);
    assert!(stdout.contains("Validation successful"));
    assert!(stdout.contains("not written (dry run)"));
    assert!(!output.exists(), "Output should not exist in dry-run mode");
}

#[test]
fn test_missing_texture_error() {
    let map = fixtures_dir().join("missing_texture/level.map");
    
    let (_stdout, stderr, code) = run_compiler(&[
        "--map", map.to_str().unwrap(),
    ]);
    
    assert_eq!(code, 0, "Missing texture should be a warning, not an error");
    assert!(stderr.contains("MISSING_TEXTURE"), "Should warn about missing texture");
}

#[test]
fn test_no_metadata_error() {
    let map = fixtures_dir().join("no_metadata/level.map");
    
    let (_stdout, stderr, code) = run_compiler(&[
        "--map", map.to_str().unwrap(),
    ]);
    
    assert_eq!(code, 1, "Expected exit code 1 for no metadata");
    assert!(stderr.contains("No info_level_metadata"), "Should report missing metadata");
}

#[test]
fn test_checkpoint_mismatch_warning() {
    setup_fixtures();
    let map = fixtures_dir().join("checkpoint_mismatch/level.map");
    
    let (_stdout, stderr, code) = run_compiler(&[
        "--map", map.to_str().unwrap(),
    ]);
    
    assert_eq!(code, 0, "Checkpoint mismatch should be a warning, not an error");
    assert!(stderr.contains("CHECKPOINT_MISMATCH"), "Should warn about checkpoint mismatch");
}

#[test]
fn test_orphaned_texture_warning() {
    setup_fixtures();
    let map = fixtures_dir().join("orphaned_texture/level.map");
    let output = fixtures_dir().join("orphaned_texture/output.ivl");
    
    let _ = fs::remove_file(&output);
    
    let (_stdout, stderr, code) = run_compiler(&[
        "--map", map.to_str().unwrap(),
        "--output", output.to_str().unwrap(),
    ]);
    
    assert_eq!(code, 0);
    assert!(stderr.contains("ORPHANED_TEXTURE"), "Should warn about orphaned texture");
    
    // Verify orphan is NOT in the archive
    let file = fs::File::open(&output).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut file_names = Vec::new();
    for i in 0..archive.len() {
        file_names.push(archive.by_index(i).unwrap().name().to_string());
    }
    assert!(!file_names.iter().any(|n| n.contains("orphan")), "Orphan should not be in archive");
    assert!(file_names.iter().any(|n| n.contains("used_tex")), "Used texture should be in archive");
    
    let _ = fs::remove_file(&output);
}

const VMF_LEVEL: &str = r#"versioninfo
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
			"material" "vmf_wood"
			"uaxis" "[1 0 0 0] 0.25"
		}
	}
}
entity
{
	"id" "4"
	"classname" "info_level_metadata"
	"metadata_schema_version" "1"
	"level_uuid" "550e8400-e29b-41d4-a716-446655440000"
	"bronze_time_ms" "0"
	"silver_time_ms" "0"
	"gold_time_ms" "0"
	"course_id" "vmf_level"
	"level_name" "VMF Level"
	"level_creator" "Tester"
	"level_description" "vmf test"
	"game_mode" "std"
	"checkpoints" "0"
}
"#;

#[test]
fn test_vmf_compilation() {
    let dir = fixtures_dir().join("vmf_level");
    fs::create_dir_all(dir.join("textures")).ok();
    fs::write(dir.join("level.vmf"), VMF_LEVEL).unwrap();
    create_minimal_png(&dir.join("textures/vmf_wood.png"));
    let output = dir.join("output.ivl");
    let _ = fs::remove_file(&output);

    let (stdout, stderr, code) = run_compiler(&[
        "--map", dir.join("level.vmf").to_str().unwrap(),
        "--output", output.to_str().unwrap(),
    ]);

    assert_eq!(code, 0, "Expected exit code 0, got {}. Stderr: {}", code, stderr);
    assert!(stdout.contains("Compiled successfully"));

    let file = fs::File::open(&output).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut file_names = Vec::new();
    for i in 0..archive.len() {
        file_names.push(archive.by_index(i).unwrap().name().to_string());
    }
    assert!(file_names.contains(&"level.vmf".to_string()), "level.vmf should be in archive, got {:?}", file_names);
    assert!(!file_names.contains(&"level.map".to_string()), "level.map must NOT be in archive");
    assert!(file_names.contains(&"textures/vmf_wood.png".to_string()), "texture should be flat in archive");

    let _ = fs::remove_file(&output);
}

#[test]
fn test_unsupported_extension() {
    let dir = fixtures_dir().join("vmf_level");
    fs::create_dir_all(&dir).ok();
    let fake = dir.join("level.txt");
    fs::write(&fake, "hello").unwrap();

    let (_stdout, stderr, code) = run_compiler(&[
        "--map", fake.to_str().unwrap(),
    ]);

    assert_eq!(code, 1);
    assert!(stderr.contains("Unsupported map format"), "got: {}", stderr);
    let _ = fs::remove_file(&fake);
}

#[test]
fn test_invalid_map_path() {    let (_stdout, _stderr, code) = run_compiler(&[
        "--map", "nonexistent/level.map",
    ]);
    
    assert_eq!(code, 1);
}

#[test]
fn test_missing_required_argument() {
    let (_stdout, _stderr, code) = run_compiler(&[]);
    
    assert_eq!(code, 2, "Missing required --map should exit with code 2");
}

fn write_fz_level(dir_name: &str, spawns: &str) -> PathBuf {
    let dir = std::env::temp_dir().join("ivlc-fz-tests").join(dir_name);
    fs::create_dir_all(&dir).ok();
    let map = format!(
        r#"{{
"classname" "info_level_metadata"
"course_id" "{dir_name}"
"level_name" "FZ Test"
"level_creator" "Tester"
"level_description" "fz spawn test"
"game_mode" "fz"
"checkpoints" "0"
"metadata_schema_version" "1"
"level_uuid" "550e8400-e29b-41d4-a716-446655440000"
"bronze_time_ms" "0"
"silver_time_ms" "0"
"gold_time_ms" "0"
}}
{spawns}
"#
    );
    fs::write(dir.join("level.map"), map).unwrap();
    dir
}

#[test]
fn test_fz_needs_two_spawns() {
    let spawns = r#"{
"classname" "info_player_start"
"origin" "0 0 0"
}"#;
    let dir = write_fz_level("fz_one_spawn", spawns);

    let (_stdout, stderr, code) = run_compiler(&["--map", dir.join("level.map").to_str().unwrap()]);

    assert_eq!(code, 1, "FreezeTag with one spawn must fail");
    assert!(
        stderr.contains("FreezeTag must have at least 2 spawns!"),
        "got: {}",
        stderr
    );
}

#[test]
fn test_fz_team_spawns_need_both_teams() {
    let spawns = r#"{
"classname" "info_player_start"
"team" "1"
}
{
"classname" "info_player_start"
"origin" "0 0 0"
}"#;
    let dir = write_fz_level("fz_missing_team", spawns);

    let (_stdout, stderr, code) = run_compiler(&["--map", dir.join("level.map").to_str().unwrap()]);

    assert_eq!(code, 1, "Team-specific spawns without both teams must fail");
    assert!(
        stderr.contains("Team specific spawns placed, but not enough for each team!"),
        "got: {}",
        stderr
    );
}

#[test]
fn test_fz_team_agnostic_spawns_are_enough() {
    let spawns = r#"{
"classname" "info_player_start"
"origin" "0 0 0"
}
{
"classname" "info_player_start"
"origin" "64 0 0"
}"#;
    let dir = write_fz_level("fz_two_spawns", spawns);
    let output = dir.join("level.ivl");
    let _ = fs::remove_file(&output);

    let (_stdout, stderr, code) = run_compiler(&["--map", dir.join("level.map").to_str().unwrap()]);

    assert_eq!(code, 0, "Two team-agnostic spawns should compile. Stderr: {}", stderr);

    let _ = fs::remove_file(&output);
}

#[test]
fn test_fz_both_teams_compile() {
    let spawns = r#"{
"classname" "info_player_start"
"team" "1"
}
{
"classname" "info_player_start"
"team" "2"
}"#;
    let dir = write_fz_level("fz_both_teams", spawns);
    let output = dir.join("level.ivl");
    let _ = fs::remove_file(&output);

    let (_stdout, stderr, code) = run_compiler(&["--map", dir.join("level.map").to_str().unwrap()]);

    assert_eq!(code, 0, "Team 1 + team 2 spawns should compile. Stderr: {}", stderr);

    let _ = fs::remove_file(&output);
}
