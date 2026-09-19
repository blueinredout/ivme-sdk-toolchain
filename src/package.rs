use crate::error::CompileError;
use crate::preview::PreviewData;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

pub struct PackageInput {
    pub map_path: PathBuf,
    pub map_bytes: Vec<u8>,
    pub preview: Option<PreviewData>,
    pub texture_files: BTreeSet<PathBuf>,
    pub textures_dir: PathBuf,
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

    // Add level.map exactly as-is
    zip.start_file("level.map", options)
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

    // Add textures
    for tex_path in &input.texture_files {
        let rel = tex_path.strip_prefix(&input.textures_dir)
            .unwrap_or(tex_path);
        let archive_path = format!("textures/{}", rel.to_string_lossy().replace('\\', "/"));

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
