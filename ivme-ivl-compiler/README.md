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
ivlc --map <path> [--preview <path>] [--output <path>] [--dry-run]
```

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `--map <path>` | Yes | Path to `level.map` |
| `--preview <path>` | No | Explicit preview image (defaults to `<map_dir>/preview.png`) |
| `--output <path>` | No | Output `.ivl` path (defaults to `<map_dir>/<map_id>.ivl`) |
| `--dry-run` | No | Validate without creating the archive |

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
```

## Project Structure

```
my_level/
├── level.map
├── preview.png
└── textures/
    ├── custom/
    │   ├── wood.png
    │   ├── wood_normal.png
    │   └── wood_roughness.png
    └── ...
```

## Expected `.map` Format

The map must contain exactly one `info_level_metadata` entity:

```
{
"classname" "info_level_metadata"
"map_id" "my_level"
"level_name" "My Level"
"level_creator" "Author"
"level_description" "Description"
"game_mode" "std"
"checkpoints" "3"
}
```

### Recognized Game Modes

- `std`
- `tag`
- `infection`

Unknown modes produce a warning but do not fail compilation.

## Texture Handling

Only textures found in the project's `textures/` directory are included in the archive. Textures referenced in the map but not present locally (e.g., built-in game textures) produce a warning and are skipped. This means:

- Maps referencing engine textures (`dev/`, `common/`, etc.) will compile successfully
- Only your custom textures get packaged into the `.ivl`
- PBR variants (`_normal`, `_roughness`, `_metallic`, `_ao`, `_emission`, `_height`, `_orm`) are auto-discovered for included textures

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
