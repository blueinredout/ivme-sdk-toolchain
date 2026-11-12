mod cli;
mod content_identity;
mod author_evidence;
#[allow(dead_code)] // Claim verification API is intentionally not wired to CLI flows.
mod author_release;
mod author_keys;
mod author_release_cli;
mod error;
mod map;
mod mapping;
mod metadata;
mod metadata_init;
mod package;
mod preview;
#[allow(dead_code)] // Cryptographic API is intentionally in-memory and not wired to CLI flows.
mod signed_envelope;
mod textures;
mod validation;

use clap::Parser;
use cli::Cli;
use colored::Colorize;
use error::CompileError;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process;

/// Yellow when stderr is a live terminal, plain when captured (F9 window,
/// files, CI) so no raw ANSI escapes leak into logs.
fn warn_yellow(msg: &str) {
    use std::io::IsTerminal;
    if std::io::stderr().is_terminal() {
        eprintln!("{}", msg.yellow());
    } else {
        eprintln!("{}", msg);
    }
}

/// Format the current date as "MMM DD YYYY" (e.g. "Sep 23 2026").
/// Uses SystemTime::now() with a manual days-since-epoch -> civil date
/// conversion so no extra crates are needed.
fn format_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86400;
    // Howard Hinnant's civil_from_days algorithm (epoch 1970-01-01).
    let mut z = days as i64 + 719468i64;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let mut doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun",
        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mi = (month - 1) as usize;
    format!("{} {:02} {}", MONTHS[mi], day, year)
}

fn main() {
    if matches!(std::env::args_os().nth(1).as_deref(), Some(x) if x == "author-key" || x == "author-release") {
        author_keys::dispatch();
    }
    let cli = Cli::parse();

    println!(
        "IVME Level Compiler - ivlc.exe ( {} )",
        format_date()
    );

    match run(cli) {
        Ok(()) => process::exit(0),
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<(), CompileError> {
    if cli.init_metadata && cli.author_evidence.is_some() { return Err(CompileError::AuthorEvidence("--author-evidence cannot be combined with --init-metadata".into())); }
    // 1. Resolve paths
    let map_path = fs::canonicalize(&cli.map)
        .map_err(|_| CompileError::MapNotFound(cli.map.clone()))?;

    if !map_path.is_file() {
        return Err(CompileError::MapNotFound(map_path.clone()));
    }

    // Only the formats the game reads (func_godot imports both).
    let is_vmf = map_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("vmf"))
        .unwrap_or(false);
    let is_dot_map = map_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("map"))
        .unwrap_or(false);
    if !is_vmf && !is_dot_map {
        return Err(CompileError::UnsupportedMapFormat(
            map_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("(none)")
                .to_string(),
        ));
    }
    if cli.init_metadata {
        let changed = metadata_init::initialize_file(&map_path, is_vmf)?;
        println!("Metadata initialization {}.", if changed { "initialized" } else { "unchanged" });
        return Ok(());
    }
    let map_entry_name = if is_vmf { "level.vmf" } else { "level.map" }.to_string();

    let map_dir = map_path.parent()
        .ok_or_else(|| CompileError::MapNotFound(map_path.clone()))?
        .to_path_buf();

    // 2. Determine output path
    let output_path = if let Some(out) = cli.output {
        if out.is_absolute() {
            out
        } else {
            map_dir.join(out)
        }
    } else {
        let map_stem = map_path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("level");
        map_dir.join(format!("{}.ivl", map_stem))
    };

    // 3. Read map file
    let map_bytes = fs::read(&map_path)
        .map_err(|e| CompileError::MapUnreadable(map_path.clone(), e))?;
    let map_text = std::str::from_utf8(&map_bytes)
        .map_err(|e| CompileError::InvalidExtendedMetadata(format!("map source is not valid UTF-8: {}", e)))?;
    let map_content = map_text.strip_prefix('\u{feff}').unwrap_or(map_text);

    // 4. Parse map
    let parse_result = map::parse_map(&map_path, &map_content)?;

    // 5. Mapping tables: explicit repeatable --mapping, else auto-detect
    //    and merge `<map_dir>/mapping.csv` + every `*.mapping.csv`
    //    (multiple user WADs). None = full-name-only map.
    let mapping_paths = mapping::find_mappings(&cli.mapping, &map_dir)?;
    let mut loaded = Vec::with_capacity(mapping_paths.len());
    for p in &mapping_paths {
        loaded.push((p.clone(), mapping::load_mapping(p)?));
    }
    let mapping_table = if loaded.is_empty() {
        None
    } else {
        if loaded.len() > 1 {
            eprintln!("Note: merging {} mapping tables", loaded.len());
        }
        Some(mapping::merge_tables(loaded)?)
    };

    if let Some(ref t) = mapping_table {
        eprintln!("Note: mapping table loaded ({} rows)", t.rows);
    }

    // 6b. Internal SDK definitions: `ivme.csv` next to the ivlc exe.
    //     Faces resolving through it need no loose files (the game ships
    //     those) and stay silent. Keep the SDK layout — do not move the exe.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let internal_table = match exe_dir.map(|d| d.join("ivme.csv")) {
        Some(p) if p.is_file() => {
            eprintln!("Note: loading internal definitions from ivme.csv...");
            match mapping::load_mapping(&p) {
                Ok(t) => Some(t),
                Err(e) => {
                    warn_yellow(&format!(
                        "Warning: INTERNAL_DEFINITIONS_BROKEN - {} ({}). Map will most likely break; reinstall the SDK.",
                        p.display(),
                        e
                    ));
                    None
                }
            }
        }
        _ => {
            warn_yellow("Warning: INTERNAL_DEFINITIONS_MISSING - ivme.csv not found next to ivlc.exe. Keep the SDK layout (do not move the exe); map will most likely break.");
            None
        }
    };
    // 6. Scan + strictly validate the working textures folder.
    //    Invalid stems and duplicate basenames are HARD errors.
    let textures_dir = map_dir.join("textures");
    let scan = textures::scan_textures(&textures_dir)?;

    // 7. Resolve faces (missing = warning only; runtime shows checkers).
    //    Internal (SDK) hits with no loose files are silently ignored.
    let resolution = textures::resolve_textures(
        &scan,
        &parse_result.textures,
        mapping_table.as_ref().map(|t| &t.table),
        internal_table.as_ref().map(|t| &t.table),
    );
    for face in &resolution.missing {
        eprintln!(
            "Warning: MISSING_TEXTURE - \"{}\" has no matching loose texture{} (likely a typo; runtime shows checkers)",
            face,
            mapping_table
                .as_ref()
                .map(|_| " and no mapping row")
                .unwrap_or(""),
        );
    }
    for face in &resolution.conflicts {
        eprintln!(
            "Warning: TABLE_CONFLICT - \"{}\" is claimed by both the local table and ivme.csv with different bases (the game prefers its builtin; rename your texture)",
            face
        );
    }

    let validation = validation::validate_all(
        parse_result.metadata,
        parse_result.checkpoint_count,
        &parse_result.player_spawn_teams,
    )?;
    let level_uuid = metadata::validate_extended_metadata(&validation.metadata)?.level_uuid;

    // 8. Emit validation warnings
    for warning in &validation.warnings {
        eprintln!("{}", warning);
    }

    // 9. Resolve preview + icon (missing = warn, explicit-but-broken = error)
    let preview = preview::resolve_preview(cli.preview.as_deref(), &map_dir)?;
    if preview.is_none() && cli.preview.is_none() {
        eprintln!("Warning: PREVIEW_NOT_FOUND - No preview.png found in {}", map_dir.display());
    }
    let icon = preview::resolve_icon(cli.icon.as_deref(), &map_dir)?;
    if icon.is_none() && cli.icon.is_none() {
        eprintln!("Warning: ICON_NOT_FOUND - No icon.png found in {}", map_dir.display());
    }

    // 10. Build the flat ship set: basename -> source path.
    let mut texture_files: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut used_bases: BTreeSet<String> = BTreeSet::new();
    for &idx in &resolution.included {
        let f = &scan.files[idx];
        texture_files.insert(f.file_name.clone(), f.path.clone());
        used_bases.insert(f.long_base.clone());
    }

    // 11. Orphan warnings (per set, so a 256-frame anim set warns once).
    for base in textures::find_orphan_sets(&scan, &used_bases) {
        let n = scan.by_base.get(&base).map(|v| v.len()).unwrap_or(0);
        eprintln!(
            "Warning: ORPHANED_TEXTURE_SET - \"{}\" ({} file(s), no face references it; excluded)",
            base, n
        );
    }
    let junk = textures::list_junk_files(&textures_dir);
    if !junk.is_empty() {
        eprintln!(
            "Warning: IGNORED_FILES - {} non png/tga file(s) in textures/ (excluded)",
            junk.len()
        );
    }

    let total_textures = texture_files.len();
    let has_preview = preview.is_some();
    let has_icon = icon.is_some();
    let has_mapping = mapping_table.is_some();

    let pkg = package::PackageInput {
        map_path: map_path.clone(), map_entry_name, map_bytes: map_bytes.to_vec(), preview, icon,
        texture_files, mapping_csv: mapping_table.map(|t| t.csv_bytes), level_uuid,
    };
    let prepared = if let Some(path) = cli.author_evidence.as_ref() {
        const MAX_AUTHOR_BYTES: u64 = 22_000_000;
        let mut file = fs::File::open(path).map_err(|e| CompileError::AuthorEvidenceUnreadable(path.clone(), e))?;
        let expected = file.metadata().map_err(|e| CompileError::AuthorEvidenceUnreadable(path.clone(), e))?.len();
        if expected == 0 || expected > MAX_AUTHOR_BYTES { return Err(CompileError::AuthorEvidence("file size is outside supported bounds (maximum 22000000 bytes)".into())); }
        let mut bytes = Vec::with_capacity(expected as usize);
        file.by_ref().take(MAX_AUTHOR_BYTES + 1).read_to_end(&mut bytes).map_err(|e| CompileError::AuthorEvidenceUnreadable(path.clone(), e))?;
        if bytes.len() as u64 != expected || bytes.len() as u64 > MAX_AUTHOR_BYTES {
            return Err(CompileError::AuthorEvidence("file changed size or could not be read completely".into()));
        }
        package::prepare_archive_with_author(&pkg, &bytes, &validation.metadata)?
    } else { package::prepare_archive(&pkg)? };

    // 12. Dry-run check
    if cli.dry_run {
        println!("Validation successful: {}", validation.metadata.course_id);
        println!("  Textures: {} files ({} sets)", total_textures, resolution.sets);
        println!("  Mapping:  {}", if has_mapping { "included" } else { "absent" });
        println!("  Preview:  {}", if has_preview { "included" } else { "absent" });
        println!("  Icon:     {}", if has_icon { "included" } else { "absent" });
        println!("  Output:   not written (dry run)");
        return Ok(());
    }

    // 13. Create archive (map entry mirrors the source extension;
    //     no editor.wad — the .ivl is the game deliverable).
    let size = package::create_archive(&output_path, &prepared)?;

    // 14. Report success
    let size_kb = size / 1024;
    println!("Compiled successfully: {}", output_path.file_name().unwrap_or_default().to_string_lossy());
    println!("  Metadata:");
    println!("    course_id:    {}", validation.metadata.course_id);
    println!("    level_name:   {}", validation.metadata.level_name);
    println!("    creator:      {}", validation.metadata.level_creator);
    println!("    game_mode:    {}", validation.metadata.game_mode);
    println!("    checkpoints:  {}", validation.metadata.checkpoints);
    println!("  Textures: {} files ({} sets)", total_textures, resolution.sets);
    println!("  Mapping:  {}", if has_mapping { "included" } else { "absent" });
    println!("  Preview:  {}", if has_preview { "included" } else { "absent" });
    println!("  Icon:     {}", if has_icon { "included" } else { "absent" });
    println!("  Size:     {} KB", size_kb);

    Ok(())
}
