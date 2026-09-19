use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum CompileError {
    MapNotFound(PathBuf),
    MapUnreadable(PathBuf, std::io::Error),
    NoMetadataEntity,
    DuplicateMetadata,
    EmptyMapId,
    InvalidMapId(String),
    MissingTexture {
        name: String,
        searched: Vec<PathBuf>,
        existing: Vec<String>,
    },
    InvalidPreview(PathBuf, String),
    ParseError {
        line: usize,
        problem: String,
    },
    OutputWriteFailed(PathBuf, std::io::Error),
    ZipWriteFailed(String),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::MapNotFound(p) => write!(f, "Map file not found: {}", p.display()),
            CompileError::MapUnreadable(p, e) => write!(f, "Cannot read map file {}: {}", p.display(), e),
            CompileError::NoMetadataEntity => write!(f, "No info_level_metadata entity found in map"),
            CompileError::DuplicateMetadata => write!(f, "Multiple info_level_metadata entities found"),
            CompileError::EmptyMapId => write!(f, "map_id is empty"),
            CompileError::InvalidMapId(id) => write!(f, "Invalid map_id: \"{}\" (must match [a-zA-Z0-9_-]+)", id),
            CompileError::MissingTexture { name, searched, existing } => {
                writeln!(f, "Missing texture")?;
                writeln!(f)?;
                writeln!(f, "The texture \"{}\" is referenced in the .map but was not found", name)?;
                writeln!(f, "in the textures/ directory.")?;
                writeln!(f)?;
                writeln!(f, "Searched for:")?;
                for path in searched {
                    writeln!(f, "  {}", path.display())?;
                }
                if !existing.is_empty() {
                    writeln!(f)?;
                    writeln!(f, "Existing files in the target directory:")?;
                    for name in existing {
                        writeln!(f, "  {}", name)?;
                    }
                }
                Ok(())
            }
            CompileError::InvalidPreview(p, reason) => {
                write!(f, "Invalid preview image {}: {}", p.display(), reason)
            }
            CompileError::ParseError { line, problem } => {
                write!(f, "Parse error at line {}: {}", line, problem)
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
