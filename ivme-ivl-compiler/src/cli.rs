use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "ivlc", about = "Compiles TrenchBroom .map projects into .ivl archives")]
pub struct Cli {
    /// Path to level.map
    #[arg(long = "map")]
    pub map: PathBuf,

    /// Explicit preview image path
    #[arg(long = "preview")]
    pub preview: Option<PathBuf>,

    /// Output .ivl path
    #[arg(long = "output")]
    pub output: Option<PathBuf>,

    /// Validate only, do not create the archive
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}
