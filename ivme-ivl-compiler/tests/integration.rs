use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use sha2::{Digest, Sha256};

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

fn init_source(ext: &str, props: &str) -> String {
    if ext == "vmf" { format!("entity\n{{\n\"classname\" \"info_level_metadata\"\n{props}}}\n") }
    else { format!("{{\n\"classname\" \"info_level_metadata\"\n{props}}}\n") }
}

fn minimal_init_props() -> &'static str { "\"course_id\" \"init_test\"\n" }

#[test]
fn test_init_metadata_cli_map_vmf_stable_identity() {
    for ext in ["map", "vmf"] {
        let f = OwnedMetadataFixture::new(); let src = f.source(ext); fs::write(&src, init_source(ext, minimal_init_props())).unwrap();
        for (status, word) in [("initialized", "initialized"), ("unchanged", "unchanged")] {
            let (out, err, code) = run_compiler(&["--map",src.to_str().unwrap(),"--init-metadata"]);
            assert_eq!(code,0,"{out}\n{err}"); assert!(out.to_lowercase().contains(word),"{out}");
            let bytes=fs::read(&src).unwrap(); let text=String::from_utf8(bytes.clone()).unwrap();
            assert!(text.contains("\"metadata_schema_version\" \"1\""));
            for key in ["bronze_time_ms","silver_time_ms","gold_time_ms"] { assert!(text.contains(&format!("\"{key}\" \"0\""))); }
            let uuid=text.lines().find(|l|l.contains("\"level_uuid\"")).unwrap().split('\"').nth(3).unwrap();
            assert_eq!(uuid.len(),36); assert_eq!(&uuid[14..15],"4"); assert!(matches!(&uuid[19..20],"8"|"9"|"a"|"b"));
            assert!(uuid.chars().enumerate().all(|(i,c)| if [8,13,18,23].contains(&i) {c=='-'} else {c.is_ascii_digit()||('a'..='f').contains(&c)}));
            if status=="initialized" { let first=bytes; let (out,err,code)=run_compiler(&["--map",src.to_str().unwrap(),"--init-metadata"]); assert_eq!(code,0,"{out}\n{err}"); assert!(out.to_lowercase().contains("unchanged")); assert_eq!(fs::read(&src).unwrap(),first); }
        }
        assert_eq!(fs::read_dir(&f.root).unwrap().count(),1);
    }
}

#[test]
fn test_init_metadata_cli_rejections_preserve_source() {
    for ext in ["map","vmf"] {
        let valid=init_source(ext,minimal_init_props());
        let bad=[init_source(ext,"\"metadata_schema_version\" \"2\"\n"),init_source(ext,"\"level_uuid\" \"bad\"\n"),init_source(ext,"\"bronze_time_ms\" \"0\"\n"),format!("{}{}",init_source(ext,minimal_init_props()),init_source(ext,minimal_init_props()))];
        for source in bad { let f=OwnedMetadataFixture::new(); let p=f.source(ext); fs::write(&p,&source).unwrap(); let (o,e,c)=run_compiler(&["--map",p.to_str().unwrap(),"--init-metadata"]); assert_ne!(c,0,"{o}\n{e}"); assert_eq!(fs::read(&p).unwrap(),source.as_bytes()); assert_eq!(fs::read_dir(&f.root).unwrap().count(),1); }
        let f=OwnedMetadataFixture::new(); let p=f.source(ext); fs::write(&p,[0xff,0xfe]).unwrap(); let (o,e,c)=run_compiler(&["--map",p.to_str().unwrap(),"--init-metadata"]); assert_ne!(c,0,"{o}\n{e}"); assert_eq!(fs::read(&p).unwrap(),[0xff,0xfe]);
        for option in [["--dry-run",""],["--output","forbidden.ivl"],["--preview","missing.png"],["--icon","missing.png"],["--mapping","missing.csv"]] { let f=OwnedMetadataFixture::new(); let p=f.source(ext); fs::write(&p,&valid).unwrap(); let mut args=vec!["--map",p.to_str().unwrap(),"--init-metadata",option[0]]; if !option[1].is_empty(){args.push(option[1]);} let(o,e,c)=run_compiler(&args); assert_ne!(c,0,"{o}\n{e}"); assert_eq!(fs::read(&p).unwrap(),valid.as_bytes()); assert_eq!(fs::read_dir(&f.root).unwrap().count(),1); }
    }
}

#[test]
fn test_init_metadata_cli_preserves_existing_metadata_bytes() {
    for ext in ["map","vmf"] {
        let original=format!("\u{feff}{}", init_source(ext,"// note\n\"metadata_schema_version\" \"0001\" // keep\n\"level_uuid\" \" 550E8400-E29B-41D4-A716-446655440000 \"\n\"bronze_time_ms\" \" 060000 \"\n\"silver_time_ms\" \"045000\"\n\"gold_time_ms\" \"30000\"\n\"level_description\" \"caf\u{00e9} \\\"ok\\\"\"\n\"unknown\" \"keep\"\n")).replace("\n", "\r\n");
        let f=OwnedMetadataFixture::new(); let p=f.source(ext); fs::write(&p,original.as_bytes()).unwrap(); let(o,e,c)=run_compiler(&["--map",p.to_str().unwrap(),"--init-metadata"]); assert_eq!(c,0,"{o}\n{e}"); assert!(o.to_lowercase().contains("unchanged")); assert_eq!(fs::read(&p).unwrap(),original.as_bytes());
        let old=init_source(ext,"\"metadata_schema_version\" \"0\"\n\"level_uuid\" \"\"\n\"bronze_time_ms\" \"0\"\n\"silver_time_ms\" \"0\"\n\"gold_time_ms\" \"0\"\n"); let f=OwnedMetadataFixture::new(); let p=f.source(ext); fs::write(&p,&old).unwrap(); let(o,e,c)=run_compiler(&["--map",p.to_str().unwrap(),"--init-metadata"]); assert_eq!(c,0,"{o}\n{e}"); let updated=String::from_utf8(fs::read(&p).unwrap()).unwrap(); let uuid=updated.lines().find(|l|l.contains("\"level_uuid\"")).unwrap().split('\"').nth(3).unwrap(); let expected=old.replace("\"metadata_schema_version\" \"0\"","\"metadata_schema_version\" \"1\"").replace("\"level_uuid\" \"\"",&format!("\"level_uuid\" \"{uuid}\"")); assert_eq!(updated,expected); let snap=updated.into_bytes(); let(o,e,c)=run_compiler(&["--map",p.to_str().unwrap(),"--init-metadata"]); assert_eq!(c,0,"{o}\n{e}"); assert_eq!(fs::read(&p).unwrap(),snap);
    }
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
fn test_author_evidence_cli_dry_run_acceptance_and_rejection_preserve_files() {
    use base64::Engine;
    let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/record-package-v2.json"))).unwrap();
    let golden = base64::engine::general_purpose::STANDARD.decode(fixture["archive_base64"].as_str().unwrap()).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(golden)).unwrap();
    let mut source = Vec::new(); zip.by_name("level.map").unwrap().read_to_end(&mut source).unwrap();
    let mut evidence = Vec::new(); zip.by_name("author-run.ivae").unwrap().read_to_end(&mut evidence).unwrap();
    let f = OwnedMetadataFixture::new();
    let src = f.root.join("source with spaces.map"); let author = f.root.join("evidence with spaces.ivae"); let output = f.root.join("output with spaces.ivl");
    fs::write(&src, &source).unwrap(); fs::write(&author, &evidence).unwrap();
    let src_s=src.to_str().unwrap(); let author_s=author.to_str().unwrap(); let output_s=output.to_str().unwrap();
    let (out,err,code)=run_compiler(&["--map",src_s,"--author-evidence",author_s,"--output",output_s,"--dry-run"]);
    assert_eq!(code,0,"{out}\n{err}"); assert!(!output.exists()); assert_eq!(fs::read(&src).unwrap(),source);
    let (out,err,code)=run_compiler(&["--map",src_s,"--author-evidence",author_s,"--output",output_s]);
    assert_eq!(code,0,"{out}\n{err}");
    let mut packaged=zip::ZipArchive::new(fs::File::open(&output).unwrap()).unwrap();
    let mut attached=Vec::new(); packaged.by_name("author-run.ivae").unwrap().read_to_end(&mut attached).unwrap(); assert_eq!(attached,evidence);
    let mut manifest_bytes=Vec::new(); packaged.by_name("ivl-manifest.json").unwrap().read_to_end(&mut manifest_bytes).unwrap();
    let manifest:serde_json::Value=serde_json::from_slice(&manifest_bytes).unwrap(); assert_eq!(manifest["ivl_schema_version"],2);
    drop(packaged);
    let original_output=fs::read(&output).unwrap();
    let (out,err,code)=run_compiler(&["--map",src_s,"--author-evidence",author_s,"--output",output_s,"--dry-run"]);
    assert_eq!(code,0,"{out}\n{err}"); assert_eq!(fs::read(&output).unwrap(),original_output);
    fs::write(&author,b"bad IVAE").unwrap();
    let (out,err,code)=run_compiler(&["--map",src_s,"--author-evidence",author_s,"--output",output_s]);
    assert_ne!(code,0,"{out}\n{err}"); assert!(format!("{out}{err}").contains("author evidence"));
    assert_eq!(fs::read(&src).unwrap(),source); assert_eq!(fs::read(&output).unwrap(),original_output);
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
use base64::Engine as _;
use serde_json::Value;
use std::io::Read as _;
use zip::ZipArchive;

const PACKAGE_GOLDEN: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/record-package-v1.json"));
fn package_case(n: &str) -> (Value, Vec<u8>) {
    let doc: Value = serde_json::from_str(PACKAGE_GOLDEN).unwrap();
    let c = doc["cases"].as_array().unwrap().iter().find(|c| c["name"] == n).unwrap().clone();
    let bytes = base64::engine::general_purpose::STANDARD.decode(c["files"][0]["bytes_base64"].as_str().unwrap()).unwrap();
    (c, bytes)
}
fn package_read(path: &Path) -> (std::collections::BTreeMap<String, Vec<u8>>, Vec<u8>) {
    let mut z = ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut files = std::collections::BTreeMap::new(); let mut names = Vec::new();
    for i in 0..z.len() { let mut f=z.by_index(i).unwrap(); names.push(f.name().to_owned()); let mut b=Vec::new(); f.read_to_end(&mut b).unwrap(); assert!(files.insert(f.name().to_owned(),b).is_none()); }
    names.sort(); assert_eq!(names.len(),files.len()); (files,names.into_iter().flat_map(|s|s.into_bytes()).collect())
}
fn package_compile(src: &Path, out: &Path, extra: &[String]) -> (String,String,i32) {
    let mut args=vec!["--map".to_owned(),src.to_string_lossy().into_owned(),"--output".into(),out.to_string_lossy().into_owned()]; args.extend_from_slice(extra);
    run_compiler(&args.iter().map(String::as_str).collect::<Vec<_>>())
}
fn package_check(m: &std::collections::BTreeMap<String,Vec<u8>>, c:&Value) {
    let v:Value=serde_json::from_slice(&m["ivl-manifest.json"]).unwrap(); assert_eq!(v.as_object().unwrap().len(),5);
    assert_eq!(v["ivl_schema_version"],1);assert_eq!(v["content_hash_version"],1);assert_eq!(v["level_uuid"],"550e8400-e29b-41d4-a716-446655440000");assert_eq!(v["content_hash"],c["content_hash"]);assert_eq!(v["files"],c["inventory"]);
    for f in v["files"].as_array().unwrap(){assert_eq!(f.as_object().unwrap().len(),3);}
}
#[test]
fn test_manifest_cli_map_vmf_golden_outputs() {
    for (name,ext) in [("map_only","map"),("vmf_only","vmf")] { let(c,b)=package_case(name);let f=OwnedMetadataFixture::new();let src=f.source(ext);let out=f.output();fs::write(&src,&b).unwrap();let(o,e,x)=package_compile(&src,&out,&[]);assert_eq!(x,0,"{o}\n{e}");assert_eq!(fs::read(&src).unwrap(),b);let(m,_)=package_read(&out);assert_eq!(m.len(),2);assert_eq!(m[&format!("level.{ext}")],b);package_check(&m,&c);let(o,e,x)=package_compile(&src,&out,&[]);assert_eq!(x,0,"{o}\n{e}");let(m2,_)=package_read(&out);assert_eq!(m["ivl-manifest.json"],m2["ivl-manifest.json"]);assert_eq!(m[&format!("level.{ext}")],m2[&format!("level.{ext}")]);assert_eq!(fs::read_dir(&f.root).unwrap().count(),2); }
}
#[test]
fn test_manifest_cli_listing_edits_and_payload_change() {
    let(c,b)=package_case("map_only");let f=OwnedMetadataFixture::new();let src=f.source("map");let out=f.output();fs::write(&src,&b).unwrap();
    let p=f.root.join("preview.png");let i=f.root.join("icon.png");image::RgbImage::from_pixel(512,512,image::Rgb([1,2,3])).save(&p).unwrap();image::RgbImage::from_pixel(128,128,image::Rgb([4,5,6])).save(&i).unwrap();
    let(o,e,x)=package_compile(&src,&out,&[]);assert_eq!(x,0,"{o}\n{e}");let(m,_)=package_read(&out);let first=m["preview.png"].clone();let first_icon=m["icon.png"].clone();package_check(&m,&c);
    image::RgbImage::from_pixel(512,512,image::Rgb([7,8,9])).save(&p).unwrap();image::RgbImage::from_pixel(128,128,image::Rgb([10,11,12])).save(&i).unwrap();let(o,e,x)=package_compile(&src,&out,&[]);assert_eq!(x,0,"{o}\n{e}");let(m,_)=package_read(&out);assert_ne!(m["preview.png"],first);assert_ne!(m["icon.png"],first_icon);image::load_from_memory(&m["preview.png"]).unwrap();image::load_from_memory(&m["icon.png"]).unwrap();package_check(&m,&c);assert_eq!(fs::read(&src).unwrap(),b);
    let mut changed=b.clone();changed.extend_from_slice(b"// changed\n");fs::write(&src,&changed).unwrap();let(o,e,x)=package_compile(&src,&out,&[]);assert_eq!(x,0,"{o}\n{e}");let(m,_)=package_read(&out);assert_eq!(m["level.map"],changed);let v:Value=serde_json::from_slice(&m["ivl-manifest.json"]).unwrap();assert_eq!(v["level_uuid"],"550e8400-e29b-41d4-a716-446655440000");assert_eq!(v["content_hash"],"d7b779ec65553109cf22b83cdd515b19ef44c5840b13511f39eb291ff86ab7a2");assert_eq!(v["files"].as_array().unwrap().len(),1);assert_eq!(v["files"][0]["path"],"level.map");assert_eq!(v["files"][0]["size_bytes"],248);assert_eq!(v["files"][0]["sha256"],"1ddf1ccb3f84d6ba3c28d90493c9117d624ac599d1dfef617fc9c57378cd2994");assert_eq!(fs::read(&src).unwrap(),changed);
}
#[test]
fn test_manifest_cli_bom_dry_run_and_invalid_utf8() {
    let(c,b)=package_case("map_only");let f=OwnedMetadataFixture::new();let src=f.source("map");let out=f.output();let mut bom=vec![0xef,0xbb,0xbf];bom.extend_from_slice(&b);fs::write(&src,&bom).unwrap();let(o,e,x)=package_compile(&src,&out,&[]);assert_eq!(x,0,"{o}\n{e}");let original=fs::read(&out).unwrap();let(m,_)=package_read(&out);assert_eq!(m["level.map"],bom);let v:Value=serde_json::from_slice(&m["ivl-manifest.json"]).unwrap();assert_eq!(v["content_hash"],"b218e7ef160f84be39004e716f586590cb6027b8d2365efe34546d120a5b0e4f");assert_eq!(v["files"].as_array().unwrap().len(),1);assert_eq!(v["files"][0]["path"],"level.map");assert_eq!(v["files"][0]["size_bytes"],240);assert_eq!(v["files"][0]["sha256"],"002ccd871ad9d01a832ef6c2e6f5c9bb86e5c958860ad22729777d10d6da42e6");assert_eq!(m["ivl-manifest.json"].len()>0,true);let snap=fs::read(&src).unwrap();let(o,e,x)=package_compile(&src,&out,&["--dry-run".into()]);assert_eq!(x,0,"{o}\n{e}");assert_eq!(fs::read(&src).unwrap(),snap);assert_eq!(fs::read(&out).unwrap(),original);
    fs::write(&src,[0xff,0xfe]).unwrap();for extra in [&[][..],&["--dry-run".into()][..]]{let(o,e,x)=package_compile(&src,&out,extra);assert_ne!(x,0,"{o}\n{e}");assert!(format!("{o}{e}").to_lowercase().contains("utf"),"{o}\n{e}");assert_eq!(fs::read(&src).unwrap(),[0xff,0xfe]);assert_eq!(fs::read(&out).unwrap(),original);}
    let absent=f.root.join("absent.ivl");fs::write(&src,&b).unwrap();let(o,e,x)=package_compile(&src,&absent,&["--dry-run".into()]);assert_eq!(x,0,"{o}\n{e}");assert!(!absent.exists());assert_eq!(fs::read(&src).unwrap(),b);assert_eq!(fs::read_dir(&f.root).unwrap().count(),2);let _=c;
}

#[test]
fn author_key_create_info_lifecycle() {
    let fixture = OwnedMetadataFixture::new();
    let mut missing_default = Command::new(env!("CARGO_BIN_EXE_ivlc"));
    missing_default.args(["author-key", "create"]).current_dir(&fixture.root);
    #[cfg(windows)] missing_default.env("APPDATA", "");
    #[cfg(not(windows))] missing_default.env("XDG_CONFIG_HOME", "");
    assert!(!missing_default.output().unwrap().status.success());
    assert_eq!(fs::read_dir(&fixture.root).unwrap().count(), 0, "missing default must not write keys into the working directory");
    let root = fixture.root.join("keys");
    let root_arg = root.to_str().unwrap();
    let (stdout, stderr, code) = run_compiler(&["author-key", "create", "--directory", root_arg]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let created: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let key_id = created["key_id"].as_str().unwrap();
    assert_eq!(key_id.len(), 64);
    assert!(key_id.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    assert_eq!(created["schema_version"], 1);
    assert!(!stdout.contains("PRIVATE KEY"));
    let key_dir = root.join(key_id);
    let private = key_dir.join("private.pem");
    let public = key_dir.join("public.pem");
    assert!(fs::read_to_string(&private).unwrap().contains("BEGIN PRIVATE KEY"));
    let publisher = "76561198000000000";
    let challenge_id = "550e8400-e29b-41d4-a716-446655440000";
    let challenge_value = serde_json::json!({
        "schema_version": 1,
        "app_id": 440,
        "publisher_profile_id": publisher,
        "author_key_id": key_id,
        "challenge_id": challenge_id,
        "nonce": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        "expires_at_unix": 1_800_000_000u64,
        "operation": "bind",
        "replaces_key_id": ""
    });
    let challenge_bytes = serde_json::to_vec(&challenge_value).unwrap();
    let challenge_path = fixture.root.join("challenge.json");
    fs::write(&challenge_path, &challenge_bytes).unwrap();
    let proof_path = fixture.root.join("proof.ivsg");
    let prove_args = ["author-key", "prove", "--challenge", challenge_path.to_str().unwrap(), "--key-id", key_id, "--publisher", publisher, "--app-id", "440", "--output", proof_path.to_str().unwrap(), "--directory", root_arg];
    let wrong_path = fixture.root.join("wrong-account.ivsg");
    let mut wrong_args = prove_args.to_vec();
    let output_arg = wrong_args.iter().position(|arg| *arg == "--output").unwrap() + 1;
    wrong_args[output_arg] = wrong_path.to_str().unwrap();
    let publisher_arg = wrong_args.iter().position(|arg| *arg == "--publisher").unwrap() + 1;
    wrong_args[publisher_arg] = "76561198000000001";
    let (stdout, stderr, code) = run_compiler(&wrong_args);
    assert_ne!(code, 0, "{stdout}\n{stderr}");
    assert!(!wrong_path.exists());
    let (stdout, stderr, code) = run_compiler(&prove_args);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let proof_info: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(proof_info["schema_version"], 1);
    assert_eq!(proof_info["key_id"], key_id);
    assert_eq!(proof_info["challenge_id"], challenge_id);
    assert_eq!(proof_info["proof_path"], fs::canonicalize(&proof_path).unwrap().to_str().unwrap());
    let proof = fs::read(&proof_path).unwrap();
    assert_eq!(&proof[..8], b"IVSG\x01\x03\0\0");
    let body_len = u32::from_le_bytes(proof[8..12].try_into().unwrap()) as usize;
    assert_eq!(&proof[44..44 + body_len], challenge_bytes);
    let digest = Sha256::digest(&proof).iter().map(|b| format!("{b:02x}")).collect::<String>();
    assert_eq!(proof_info["proof_sha256"], digest);
    let existing_proof = proof.clone();
    let (stdout, stderr, code) = run_compiler(&prove_args);
    assert_ne!(code, 0, "{stdout}\n{stderr}");
    assert_eq!(fs::read(&proof_path).unwrap(), existing_proof);
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&key_dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&private).unwrap().permissions().mode() & 0o777, 0o600);
    }
    fs::remove_file(&private).unwrap();
    let (stdout, stderr, code) = run_compiler(&["author-key", "info", "--key-id", key_id, "--directory", root_arg]);
    assert_eq!(code, 0, "{stdout}\n{stderr}");
    let info: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(info["key_id"], key_id);
    assert_eq!(info["public_key_pem"], created["public_key_pem"]);
    assert!(!stdout.contains("PRIVATE KEY"));
    let foreign: serde_json::Value = serde_json::from_str(include_str!("../../contracts/signed-envelope-v1-vector.json")).unwrap();
    fs::write(&public, foreign["public_key_pem"].as_str().unwrap()).unwrap();
    let (stdout, stderr, code) = run_compiler(&["author-key", "info", "--key-id", key_id, "--directory", root_arg]);
    assert_ne!(code, 0, "{stdout}\n{stderr}");
    assert!(!stdout.contains("PRIVATE KEY"));
}
