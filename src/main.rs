mod cli;
mod error;
mod map;
mod metadata;
mod package;
mod preview;
mod textures;
mod validation;

use clap::Parser;
use cli::Cli;
use error::CompileError;
use std::collections::BTreeSet;
use std::fs;
use std::process;

fn main() {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => process::exit(0),
        Err(e) => {
            eprintln!("Error: {}", e);
            process::exit(1);
        }
    }
}

fn run(cli: Cli) -> Result<(), CompileError> {
    // 1. Resolve paths
    let map_path = fs::canonicalize(&cli.map)
        .map_err(|_| CompileError::MapNotFound(cli.map.clone()))?;

    if !map_path.is_file() {
        return Err(CompileError::MapNotFound(map_path.clone()));
    }

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
    let map_content = String::from_utf8_lossy(&map_bytes);

    // 4. Parse map
    let parse_result = map::parse_map(&map_path, &map_content)?;

    // 5. Resolve textures (missing = warning, not error)
    let texture_resolution = textures::resolve_textures(
        &map_dir.join("textures"),
        &parse_result.textures,
    );

    // Emit missing texture warnings
    for tex_name in &texture_resolution.missing {
        eprintln!("Warning: MISSING_TEXTURE - \"{}\" not found in textures/ directory", tex_name);
    }

    let validation = validation::validate_all(
        parse_result.metadata,
        parse_result.checkpoint_count,
        texture_resolution,
    )?;

    // 6. Emit warnings
    for warning in &validation.warnings {
        eprintln!("{}", warning);
    }

    // 7. Resolve preview
    let preview = preview::resolve_preview(cli.preview.as_deref(), &map_dir)?;
    if preview.is_some() {
        // No warning needed
    } else if cli.preview.is_some() {
        // Explicit preview was missing - error already returned by resolve_preview
    } else {
        eprintln!("Warning: PREVIEW_NOT_FOUND - No preview.png found in {}", map_dir.display());
    }

    // 8. Collect all texture files to include
    let mut texture_files = BTreeSet::new();
    for path in validation.texture_resolution.base.values() {
        texture_files.insert(path.clone());
    }
    for paths in validation.texture_resolution.pbr.values() {
        for path in paths {
            texture_files.insert(path.clone());
        }
    }

    // 9. Orphan detection
    let textures_dir = map_dir.join("textures");
    let mut included_set = BTreeSet::new();
    for path in &texture_files {
        if let Ok(rel) = path.strip_prefix(&textures_dir) {
            included_set.insert(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    let orphans = textures::find_orphans(&textures_dir, &included_set);
    for orphan in &orphans {
        if let Ok(rel) = orphan.strip_prefix(&textures_dir) {
            eprintln!(
                "Warning: ORPHANED_TEXTURE - {}",
                rel.to_string_lossy().replace('\\', "/")
            );
        }
    }

    let base_count = validation.texture_resolution.base.len();
    let pbr_count: usize = validation.texture_resolution.pbr.values().map(|v| v.len()).sum();
    let total_textures = base_count + pbr_count;

    // 10. Dry-run check
    let has_preview = preview.is_some();
    if cli.dry_run {
        println!("Validation successful: {}", validation.metadata.map_id);
        println!("  Textures: {} files ({} base + {} PBR)", total_textures, base_count, pbr_count);
        println!("  Preview:  {}", if has_preview { "included" } else { "absent" });
        println!("  Output:   not written (dry run)");
        return Ok(());
    }

    // 11. Create archive
    let pkg = package::PackageInput {
        map_path: map_path.clone(),
        map_bytes: map_bytes.to_vec(),
        preview,
        texture_files,
        textures_dir,
    };

    let size = package::create_archive(&output_path, &pkg)?;

    // 12. Report success
    let size_kb = size / 1024;
    println!("Compiled successfully: {}", output_path.file_name().unwrap_or_default().to_string_lossy());
    println!("  Metadata:");
    println!("    map_id:       {}", validation.metadata.map_id);
    println!("    level_name:   {}", validation.metadata.level_name);
    println!("    creator:      {}", validation.metadata.level_creator);
    println!("    game_mode:    {}", validation.metadata.game_mode);
    println!("    checkpoints:  {}", validation.metadata.checkpoints);
    println!("  Textures: {} files ({} base + {} PBR)", total_textures, base_count, pbr_count);
    println!("  Preview:  {}", if has_preview { "included" } else { "absent" });
    println!("  Size:     {} KB", size_kb);

    Ok(())
}
