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
ivlc --map <path> [--preview <path>] [--icon <path>] [--mapping <path>] [--output <path>] [--dry-run]
```

### Arguments

| Argument | Required | Description |
|----------|----------|-------------|
| `--map <path>` | Yes | Path to `level.map` or `level.vmf` |
| `--preview <path>` | No | Explicit preview image (defaults to `<map_dir>/preview.png`) |
| `--icon <path>` | No | Explicit 128×128 icon (defaults to `<map_dir>/icon.png`) |
| `--mapping <path>` | No | Explicit `mapping.csv` (defaults to auto-detect: `<map_dir>/mapping.csv`, then a single `<map_dir>/*.mapping.csv`) |
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
