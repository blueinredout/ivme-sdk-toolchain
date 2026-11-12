# ivlc

A standalone command-line tool that compiles TrenchBroom `.map` projects into self-contained `.ivl` ZIP archives for the IVME game.

## Features

- Parses Quake/TrenchBroom `.map` format (standard + Valve 220 brush syntax)
- Extracts and validates `info_level_metadata` entities
- Resolves referenced textures with PBR variant discovery
- Normalizes preview images to 512×512
- Detects orphaned textures
- Validates checkpoint counts and game modes
- Atomic output with dry-run support
- Writes a versioned `ivl-manifest.json` with a deterministic content identity

## Requirements

- [Rust](https://www.rust-lang.org/tools/install) (1.70+)

## Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release
```

The release binary is produced at `target/release/ivlc` (or `ivlc.exe` on Windows).

## Usage

```
ivlc --map <path> [--preview <path>] [--icon <path>] [--mapping <path>] [--output <path>] [--dry-run]
ivlc --map <path> --author-evidence <path> [--output <path>] [--dry-run]
ivlc --map <path> --init-metadata
```

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `--map <path>` | Yes | Path to `level.map` or `level.vmf` |
| `--preview <path>` | No | Explicit preview image (defaults to `<map_dir>/preview.png`) |
| `--icon <path>` | No | Explicit 128×128 icon (defaults to `<map_dir>/icon.png`) |
| `--mapping <path>` | No | Repeatable explicit mapping inputs; auto-merges `<map_dir>/mapping.csv` and all `<map_dir>/*.mapping.csv` (first wins) |
| `--output <path>` | No | Output `.ivl` path (defaults to `<map_dir>/<source_file_stem>.ivl`) |
| `--dry-run` | No | Validate without creating the archive |
| `--author-evidence <path>` | No | Explicitly attach one strictly decoded local IVAE as inert schema-2 transport data |
| `--init-metadata` | No | Initialize metadata in the source file without creating an archive |

### Examples

```bash
# Basic compilation
ivlc --map my_level/level.map

# Custom output path
ivlc --map my_level/level.map --output dist/custom.ivl

# Validate only
ivlc --map my_level/level.map --dry-run

# Explicit preview
ivlc --map my_level/level.map --preview screenshot.png

# Initialize metadata before compiling
ivlc --map my_level/level.map --init-metadata
```

Initialization fills a missing or empty UUID and preserves an existing valid one.
Running it again leaves valid metadata unchanged. Re-authoring a distinct work requires
a new UUID. It creates no archive and conflicts with compilation flags, including
`--dry-run`. Normal compilation and dry-run never rewrite the source.

Ordinary compilation writes schema 1. Supplying `--author-evidence` opts into
schema 2 and embeds exactly that IVAE after checking it against the prepared
payload and source target metadata. This is structural transport only: it does
not verify ownership, certify a target, or grant approval. Evidence time and
approval are not portable claims; they remain unverified until a separate
authenticated workflow evaluates them.

## Package contract

Default schema-1 packages contain a root `ivl-manifest.json` with `ivl_schema_version: 1`,
`content_hash_version: 1`, the source `level_uuid`, `content_hash`, and a `files`
array. The array lists ASCII-sorted payload paths with their byte sizes and SHA-256
digests. The source, `mapping.csv`, and admitted PNG/TGA textures are payloads;
root `preview.png` and `icon.png` are listing images excluded from the
payload inventory and cannot be runtime assets.

Changing an admitted payload's bytes or path, including source comments or metadata,
changes the content identity. Listing image edits, recompression, and ZIP entry order
do not. Compile and dry-run preserve source bytes, including a UTF-8 BOM. Rebuild
older IVLs with the current compiler to add this manifest. The manifest is recomputed
package data, not an author or master signature; signing, publication, and author
replay are later steps. Archive output is replaced atomically, preserving an existing
valid archive if compilation fails.

## Record identity (Phase 2)

The `content_hash` in `ivl-manifest.json` is the identity the IVME server
registers a release under. It is a domain-separated SHA-256 over the payload
inventory (`IVME-CONTENT\0` + version + path/size/digest records), so it is
independent of ZIP entry order, recompression and listing images, and changes with
any payload byte or path.

Server side (`.aiguide/level-metadata-phase2-contract.md`, sections 11–13): an
offline operator registers a deployment manifest that pins the release to its
content hash, level UUID, course, medal targets and ruleset. The server recomputes
or re-validates that identity from the installed artifact before a run can be
admitted, so a rebuilt archive with different bytes cannot inherit an existing
release's records.

Rebuild older IVLs with the current compiler before registering them: an archive
without `ivl-manifest.json` cannot be registered.

## Project Structure

```
my_level/
├── level.map
├── preview.png
├── icon.png
├── mapping.csv            # optional: from wadgen (auto-detected)
└── textures/              # working folder may nest; ships flat
    ├── metal_test_albedo.png
    ├── metal_test_normal.png
    └── ...
```

## Expected `.map` / `.vmf` Format

Both Quake/TrenchBroom `.map` (standard + Valve 220 brushes) and Valve `.vmf`
(Hammer / JACK export, as read by func_godot) are accepted. For `.vmf`,
textures come from `side` `"material"` fields (any nesting), `tools/*` and
`__` materials are ignored like Quake tool textures. The archive entry mirrors
the source: `level.map` or `level.vmf`. Anything else is rejected.

The map must contain exactly one `info_level_metadata` entity:

```
{
"classname" "info_level_metadata"
"course_id" "my_level"
"level_name" "My Level"
"level_creator" "Author"
"level_description" "Description"
"game_mode" "std"
"checkpoints" "3"
"metadata_schema_version" "1"
"level_uuid" "550e8400-e29b-41d4-a716-446655440000"
"bronze_time_ms" "60000"
"silver_time_ms" "45000"
"gold_time_ms" "30000"
}
```

### Recognized Game Modes

- `std`
- `fz` (FreezeTag)
- `pg`

Only `std`, `pg`, and `fz` are supported. `hub` is reserved for shipped
content. Missing or unknown modes fail compilation.

Target times are milliseconds, each at most `2147483647`, and must satisfy
`bronze > silver > gold > 0`. All three absent or all three set to `0` means
unconfigured targets; partial or malformed target sets are rejected.

FreezeTag (`fz`) maps must contain at least 2 `info_player_start` entities. If
any spawn is team-specific (`"team" "1"` / `"team" "2"`; anything other than 0),
at least one spawn for **each** team is required. Spawns with no `team` key (or
`"team" "0"`) count as team-agnostic and are fine on their own as long as there
are at least two.

```text
FreezeTag must have at least 2 spawns!
Team specific spawns placed, but not enough for each team!
```

## Texture Handling

`textures/` ships **flat** (`textures/<basename>`) with **png/tga only**. Stems must be
`[A-Za-z0-9_]` (plus optional `{`/`!`/`~`/`-` or `+NNN` anim-frame prefix, and one
trailing PBR suffix like `_albedo`/`_normal`). Anything else is a hard bake error, as are
duplicate basenames after flattening.

Only referenced texture *sets* are included: the face's long base pulls in all PBR
siblings and all animation frames (`+0lava` pulls `+1lava`, `+2lava`, …). Unreferenced
sets are excluded with a warning. Faces with no match warn per face and compile
anyway — the game shows checkers at runtime. Short JACK names (`M_TEST`, `+0C_LAVA`)
resolve through `mapping.csv` (auto-detected next to the map, or `--mapping`).

## SDK layout

`ivlc` expects `ivme.csv` (the SDK's internal texture definitions, produced by
`wadgen` from the game's own textures) sitting next to the exe — do not move
`ivlc.exe` out of its SDK folder. Faces resolving through it need no loose files
and stay silent. If it is missing you get one loud warning and the map will most
likely break.

## Testing

```bash
# Run all tests
cargo test

# Run unit tests only
cargo test --lib

# Run integration tests only
cargo test --test integration
```

## Distribution

The compiled binary is fully standalone with no runtime dependencies. Distribute `ivlc.exe` (Windows) or `ivlc` (Linux) directly.

## License

TBD
