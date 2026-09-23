use crate::error::CompileError;
use crate::preview::{IconData, PreviewData};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

pub struct PackageInput {
    pub map_path: PathBuf,
    /// Archive entry name: `level.map` or `level.vmf` (mirrors the source).
    pub map_entry_name: String,
    pub map_bytes: Vec<u8>,
    pub preview: Option<PreviewData>,
    pub icon: Option<IconData>,
    /// Basename -> source path (textures/ is flat in the archive).
    pub texture_files: BTreeMap<String, PathBuf>,
    /// Clean mapping.csv bytes as validated (None = TrenchBroom-only map).
    pub mapping_csv: Option<Vec<u8>>,
}

pub fn create_archive(
    output_path: &Path,
    input: &PackageInput,
) -> Result<u64, CompileError> {
    let tmp_path = output_path.with_extension("ivl.tmp");

    let file = File::create(&tmp_path)
        .map_err(|e| CompileError::OutputWriteFailed(tmp_path.clone(), e))?;

    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    // Add the map exactly as-is (entry mirrors the source extension)
    zip.start_file(&input.map_entry_name, options)
        .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
    zip.write_all(&input.map_bytes)
        .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;

    // Add normalized preview
    if let Some(ref preview) = input.preview {
        let png_data = crate::preview::encode_png(&preview.normalized);
        zip.start_file("preview.png", options)
            .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
        zip.write_all(&png_data)
            .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
    }

    // Add normalized icon (level selector, 128x128)
    if let Some(ref icon) = input.icon {
        let png_data = crate::preview::encode_png_size(&icon.normalized, 128);
        zip.start_file("icon.png", options)
            .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
        zip.write_all(&png_data)
            .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
    }

    // Add the validated mapping table (None = full-name-only map).
    if let Some(ref csv) = input.mapping_csv {
        zip.start_file("mapping.csv", options)
            .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
        zip.write_all(csv)
            .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
    }

    // Add textures, flattened: textures/<basename>. BTreeMap iteration
    // is byte-ordered, so archive entries are deterministic.
    for (file_name, tex_path) in &input.texture_files {
        let archive_path = format!("textures/{}", file_name);
        zip.start_file(&archive_path, options)
            .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;

        let mut src_file = File::open(tex_path)
            .map_err(|e| CompileError::ZipWriteFailed(format!("Failed to open {}: {}", tex_path.display(), e)))?;
        let mut buf = Vec::new();
        src_file.read_to_end(&mut buf)
            .map_err(|e| CompileError::ZipWriteFailed(format!("Failed to read {}: {}", tex_path.display(), e)))?;
        zip.write_all(&buf)
            .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
    }

    zip.finish()
        .map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;

    // Atomic rename
    std::fs::rename(&tmp_path, output_path)
        .map_err(|e| CompileError::OutputWriteFailed(output_path.to_path_buf(), e))?;

    let size = std::fs::metadata(output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    Ok(size)
}
