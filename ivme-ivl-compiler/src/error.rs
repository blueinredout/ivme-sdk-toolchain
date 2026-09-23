use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum CompileError {
    MapNotFound(PathBuf),
    MapUnreadable(PathBuf, std::io::Error),
    UnsupportedMapFormat(String),
    NoMetadataEntity,
    DuplicateMetadata,
    EmptyMapId,
    InvalidMapId(String),
    InvalidPreview(PathBuf, String),
    InvalidTextureName(String),
    DuplicateBasename(String, PathBuf, PathBuf),
    MappingInvalid(String),
    OutputWriteFailed(PathBuf, std::io::Error),
    ZipWriteFailed(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::MapNotFound(p) => write!(f, "Map file not found: {}", p.display()),
            CompileError::MapUnreadable(p, e) => write!(f, "Cannot read map file {}: {}", p.display(), e),
            CompileError::UnsupportedMapFormat(ext) => write!(f, "Unsupported map format \"{}\" (expected .map or .vmf)", ext),
            CompileError::NoMetadataEntity => write!(f, "No info_level_metadata entity found in map"),
            CompileError::DuplicateMetadata => write!(f, "Multiple info_level_metadata entities found"),
            CompileError::EmptyMapId => write!(f, "map_id is empty"),
            CompileError::InvalidMapId(id) => write!(f, "Invalid map_id: \"{}\" (must match [a-zA-Z0-9_-]+)", id),
            CompileError::InvalidPreview(p, reason) => {
                write!(f, "Invalid preview image {}: {}", p.display(), reason)
            }
            CompileError::InvalidTextureName(msg) => {
                write!(f, "Invalid texture name: {}", msg)
            }
            CompileError::DuplicateBasename(name, a, b) => {
                write!(f, "Duplicate texture basename \"{}\" (flattened textures/ must be unique):\n  {}\n  {}", name, a.display(), b.display())
            }
            CompileError::MappingInvalid(msg) => {
                write!(f, "Invalid mapping.csv: {}", msg)
            }
            CompileError::OutputWriteFailed(p, e) => {
                write!(f, "Failed to write output file {}: {}", p.display(), e)
            }
            CompileError::ZipWriteFailed(msg) => write!(f, "ZIP write error: {}", msg),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<std::io::Error> for CompileError {
    fn from(e: std::io::Error) -> Self {
        CompileError::ZipWriteFailed(e.to_string())
    }
}
