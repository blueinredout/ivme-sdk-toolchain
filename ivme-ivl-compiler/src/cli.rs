use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "ivlc", about = "Compiles TrenchBroom .map projects into .ivl archives", arg_required_else_help = true)]
pub struct Cli {
    /// Path to level.map or level.vmf
    #[arg(long = "map")]
    pub map: PathBuf,

    /// Explicit preview image path
    #[arg(long = "preview")]
    pub preview: Option<PathBuf>,

    /// Explicit icon image path (128x128 level-selector icon)
    #[arg(long = "icon")]
    pub icon: Option<PathBuf>,

    /// Explicit mapping.csv path(s) (short JACK names -> long bases).
    /// Repeatable: `--mapping sdk.csv --mapping mymap.csv`. Tables merge
    /// in CLI order (first wins on conflicting shorts). When omitted,
    /// ivlc auto-detects and merges `<map_dir>/mapping.csv` plus every
    /// `<map_dir>/*.mapping.csv`. No table at all = full-name-only
    /// (TrenchBroom) map; short-like faces without a table warn
    /// (runtime shows checkers).
    #[arg(long = "mapping")]
    pub mapping: Vec<PathBuf>,

    /// Output .ivl path
    #[arg(long = "output")]
    pub output: Option<PathBuf>,

    /// Validate only, do not create the archive
    #[arg(long = "dry-run")]
    pub dry_run: bool,
}
