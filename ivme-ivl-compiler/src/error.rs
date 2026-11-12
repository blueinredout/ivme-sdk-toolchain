use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum CompileError {
    MapNotFound(PathBuf),
    MapUnreadable(PathBuf, std::io::Error),
    UnsupportedMapFormat(String),
    NoMetadataEntity,
    DuplicateMetadata,
    FreezeTagNotEnoughSpawns(usize),
    FreezeTagMissingTeamSpawns(usize, usize),
    EmptyCourseId,
    InvalidCourseId(String),
    InvalidExtendedMetadata(String),
    InvalidPreview(PathBuf, String),
    DuplicateBasename(String, PathBuf, PathBuf),
    MappingInvalid(String),
    OutputWriteFailed(PathBuf, std::io::Error),
    ZipWriteFailed(String),
    ContentIdentity(crate::content_identity::IdentityError),
    AuthorEvidence(String),
    AuthorEvidenceUnreadable(PathBuf, std::io::Error),
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CompileError::MapNotFound(p) => write!(f, "Map file not found: {}", p.display()),
            CompileError::MapUnreadable(p, e) => write!(f, "Cannot read map file {}: {}", p.display(), e),
            CompileError::UnsupportedMapFormat(ext) => write!(f, "Unsupported map format \"{}\" (expected .map or .vmf)", ext),
            CompileError::NoMetadataEntity => write!(f, "No info_level_metadata entity found in map"),
            CompileError::DuplicateMetadata => write!(f, "Multiple info_level_metadata entities found"),
            CompileError::FreezeTagNotEnoughSpawns(found) => write!(
                f,
                "FreezeTag must have at least 2 spawns! (found {})",
                found
            ),
            CompileError::FreezeTagMissingTeamSpawns(team1, team2) => write!(
                f,
                "Team specific spawns placed, but not enough for each team! (info_player_start team 1: {}, team 2: {})",
                team1, team2
            ),
            CompileError::EmptyCourseId => write!(f, "course_id is empty"),
            CompileError::InvalidCourseId(id) => write!(f, "Invalid course_id: \"{}\" (must match [a-zA-Z0-9_-]+)", id),
            CompileError::InvalidExtendedMetadata(msg) => write!(f, "Invalid compiler metadata: {}", msg),
            CompileError::InvalidPreview(p, reason) => {
                write!(f, "Invalid preview image {}: {}", p.display(), reason)
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
            CompileError::ContentIdentity(err) => write!(f, "{}", err),
            CompileError::AuthorEvidence(msg) => write!(f, "Invalid author evidence: {}", msg),
            CompileError::AuthorEvidenceUnreadable(p, e) => write!(f, "Cannot read author evidence {}: {}", p.display(), e),
        }
    }
}

impl std::error::Error for CompileError {}

impl From<std::io::Error> for CompileError {
    fn from(e: std::io::Error) -> Self {
        CompileError::ZipWriteFailed(e.to_string())
    }
}

impl From<crate::content_identity::IdentityError> for CompileError {
    fn from(e: crate::content_identity::IdentityError) -> Self { CompileError::ContentIdentity(e) }
}
