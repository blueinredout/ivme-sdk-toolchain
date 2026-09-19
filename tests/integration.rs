use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let exe = env!("CARGO_BIN_EXE_ivl-compile");
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
    assert!(stdout.contains("map_id:       happy_level"));
    assert!(stdout.contains("Textures: 2 files (2 base + 0 PBR)"));
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
    assert!(file_names.iter().any(|n| n.starts_with("textures/custom/")), "textures should be in archive");
    
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
    
    assert_eq!(code, 1, "Expected exit code 1 for missing texture");
    assert!(stderr.contains("Missing texture"), "Should report missing texture");
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

#[test]
fn test_invalid_map_path() {
    let (_stdout, _stderr, code) = run_compiler(&[
        "--map", "nonexistent/level.map",
    ]);
    
    assert_eq!(code, 1);
}

#[test]
fn test_missing_required_argument() {
    let (_stdout, _stderr, code) = run_compiler(&[]);
    
    assert_eq!(code, 2, "Missing required --map should exit with code 2");
}
