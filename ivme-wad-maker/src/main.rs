use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use colored::Colorize;
use image::imageops::FilterType;
use image::{GenericImageView, RgbaImage};

/// wadgen: folder-list txt -> WAD3 (JACK / Hammer / TrenchBroom / func_godot)
#[derive(Parser, Debug)]
#[command(name = "wadgen", version)]
struct Cli {
    /// Path to the .txt list file (one folder per line, non-recursive)
    list_file: PathBuf,

    /// Output .wad path. Default: same folder as list file, same stem + .wad
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Max texture dimension (must be multiple of 16). Larger images are
    /// downscaled preserving aspect. Default 512 (GoldSrc-safe max).
    #[arg(long, default_value_t = 512)]
    max_size: u32,
}

// ---------------------------------------------------------------------------
// Naming spec (must match shorten-plan.md + engine script)
// ---------------------------------------------------------------------------

const KNOWN_SURFACES: &[&str] = &[
    "metal", "wood", "concrete", "glass", "plastic", "fabric", "stone", "brick", "dirt",
    "rock", "sand", "grass", "snow", "ice",
];

/// Suffixes that mean "this file is the diffuse representative -> pack it".
const PACK_SUFFIXES: &[&str] = &["albedo", "diffuse", "basecolor", "base", "color"];
/// Suffixes that mean "this is a PBR aux map -> skip for WAD, used at runtime".
const SKIP_SUFFIXES: &[&str] = &[
    "normal",
    "roughness",
    "metallic",
    "metalness",
    "ao",
    "ambientocclusion",
    "emission",
    "emissive",
    "height",
    "orm",
    "rough",
];

const SPECIAL_PREFIXES: &[char] = &['{', '!', '+', '~', '-'];

struct ParsedName {
    /// long base without PBR suffix, lowercase, no leading special. e.g. "metal_test"
    long_base: String,
    surface_letter: char,
    /// sanitized upper core without surface. e.g. "TEST", "BRICKWALL01"
    core: String,
    /// leading special from source filename, if any
    special: Option<char>,
    /// false => skip this file for WAD (PBR aux)
    pack: bool,
}

fn sanitize_core(raw_upper: &str) -> String {
    let mut out = String::with_capacity(raw_upper.len());
    let mut prev_underscore = false;
    for ch in raw_upper.chars() {
        let c = if ch.is_ascii_alphanumeric() {
            ch
        } else {
            '_'
        };
        if c == '_' {
            if !prev_underscore && !out.is_empty() {
                out.push('_');
            }
            prev_underscore = true;
        } else {
            out.push(c);
            prev_underscore = false;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("TEX");
    }
    out
}

fn parse_file_stem(stem: &str) -> ParsedName {
    let lower = stem.to_lowercase();
    let (special, mut rest) = match lower.chars().next() {
        Some(c) if SPECIAL_PREFIXES.contains(&c) => (Some(c), lower[1..].to_string()),
        _ => (None, lower.clone()),
    };
    if rest.is_empty() {
        rest = "tex".to_string();
    }

    // Strip one trailing _suffix if it is a known PBR token.
    let (long_base_rest, suffix_kind): (String, &str) = match rest.rfind('_') {
        Some(idx) => {
            let suf = &rest[idx + 1..];
            if PACK_SUFFIXES.contains(&suf) {
                (rest[..idx].to_string(), "pack")
            } else if SKIP_SUFFIXES.contains(&suf) {
                (rest[..idx].to_string(), "skip")
            } else {
                (rest.clone(), "none")
            }
        }
        None => (rest.clone(), "none"),
    };
    let long_base = if long_base_rest.is_empty() {
        "tex".to_string()
    } else {
        long_base_rest.clone()
    };

    let pack = suffix_kind != "skip";
    // Surface detection on the long base.
    let (surface, core_remainder) = match long_base.find('_') {
        Some(idx) => {
            let head = &long_base[..idx];
            if KNOWN_SURFACES.contains(&head) {
                (head.to_string(), long_base[idx + 1..].to_string())
            } else {
                ("concrete".to_string(), long_base.clone())
            }
        }
        None => ("concrete".to_string(), long_base.clone()),
    };
    let surface_letter = surface
        .chars()
        .find(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C');
    let core = sanitize_core(&core_remainder.to_uppercase());

    ParsedName {
        long_base,
        surface_letter,
        core,
        special,
        pack,
    }
}

/// Build the pre-collision short base (<=15 chars before _N dedup).
/// Transparent forces '{' prefix. Otherwise preserves source ! + ~ - prefix.
fn build_base(parsed: &ParsedName, has_transparency: bool) -> String {
    let stem = format!("{}_{}", parsed.surface_letter, parsed.core);
    let with_special = if has_transparency || parsed.special == Some('{') {
        format!("{{{stem}")
    } else if let Some(c) = parsed.special {
        if c == '+' {
            // '+' is an animation-frame prefix in GoldSrc; for a reference
            // pack keep it simple and drop it (warned by caller).
            stem
        } else {
            format!("{c}{stem}")
        }
    } else {
        stem
    };
    if with_special.len() <= 15 {
        with_special
    } else {
        with_special[..15].to_string()
    }
}

/// Make unique within 15 chars: BASE, BASE_1, BASE_2 ... (sorted processing
/// makes this deterministic; engine script must resolve collisions the same
/// way by sorting, see shorten-plan.md).
fn make_unique(base15: &str, used: &mut HashSet<String>) -> String {
    if !used.contains(base15) {
        used.insert(base15.to_string());
        return base15.to_string();
    }
    let mut n = 1u32;
    loop {
        let suffix = format!("_{n}");
        let keep = 15usize.saturating_sub(suffix.len());
        let truncated = if base15.len() > keep {
            &base15[..keep]
        } else {
            base15
        };
        // avoid double underscore glue artefacts, keep simple
        let cand = format!("{truncated}{suffix}");
        let cand = if cand.len() > 15 {
            cand[..15].to_string()
        } else {
            cand
        };
        if !used.contains(&cand) {
            used.insert(cand.clone());
            return cand;
        }
        n += 1;
        if n > 9999 {
            // practically unreachable; fallback with hash-ish suffix
            let cand = format!("{}_X", &base15[..13.min(base15.len())]);
            if !used.contains(&cand) {
                used.insert(cand.clone());
                return cand;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Image helpers
// ---------------------------------------------------------------------------

fn compliant_size(w: u32, h: u32, max_size: u32) -> (u32, u32, bool) {
    if w % 16 == 0 && h % 16 == 0 && w <= max_size && h <= max_size && w >= 16 && h >= 16 {
        return (w, h, false);
    }
    let scale = (max_size as f32 / w as f32)
        .min(max_size as f32 / h as f32)
        .min(1.0);
    let mut nw = ((w as f32 * scale / 16.0).round() as u32 * 16).max(16).min(max_size);
    let mut nh = ((h as f32 * scale / 16.0).round() as u32 * 16).max(16).min(max_size);
    // rounding can still push over max when max is not %16 (we validate, but be safe)
    if nw > max_size {
        nw = max_size - (max_size % 16);
    }
    if nh > max_size {
        nh = max_size - (max_size % 16);
    }
    if nw < 16 {
        nw = 16;
    }
    if nh < 16 {
        nh = 16;
    }
    (nw, nh, true)
}

/// Median-cut to `max_colors` palette entries. Pure Rust, no extra deps.
/// Subsamples huge images for training so 512x512 stays fast.
fn median_cut_quantize(opaque: &[[u8; 3]], max_colors: usize) -> Vec<[u8; 3]> {
    if opaque.is_empty() || max_colors == 0 {
        return vec![];
    }
    // Subsample for training only.
    let stride = if opaque.len() > 60_000 {
        opaque.len() / 60_000 + 1
    } else {
        1
    };
    let mut sample: Vec<[u8; 3]> = opaque.iter().step_by(stride).cloned().collect();
    if sample.is_empty() {
        sample = opaque.to_vec();
    }
    let mut boxes: Vec<Vec<[u8; 3]>> = vec![sample];
    while boxes.len() < max_colors {
        // pick box with largest channel range that can split
        let mut best: Option<usize> = None;
        let mut best_range = 0u16;
        let mut best_ch = 0usize;
        for (i, b) in boxes.iter().enumerate() {
            if b.len() < 2 {
                continue;
            }
            let (mut mn, mut mx) = ([255u8; 3], [0u8; 3]);
            for p in b.iter() {
                for c in 0..3 {
                    if p[c] < mn[c] {
                        mn[c] = p[c];
                    }
                    if p[c] > mx[c] {
                        mx[c] = p[c];
                    }
                }
            }
            for c in 0..3 {
                let r = (mx[c] as u16).saturating_sub(mn[c] as u16);
                if r > best_range {
                    best_range = r;
                    best = Some(i);
                    best_ch = c;
                }
            }
        }
        let Some(idx) = best else { break };
        if best_range == 0 {
            break;
        }
        let mut b = boxes.swap_remove(idx);
        b.sort_unstable_by_key(|p| p[best_ch]);
        let mid = b.len() / 2;
        let right = b.split_off(mid);
        boxes.push(b);
        boxes.push(right);
    }
    boxes
        .iter()
        .map(|b| {
            if b.is_empty() {
                return [0, 0, 0];
            }
            let (mut sr, mut sg, mut sb) = (0u64, 0u64, 0u64);
            for p in b.iter() {
                sr += p[0] as u64;
                sg += p[1] as u64;
                sb += p[2] as u64;
            }
            let n = b.len() as u64;
            [
                (sr / n) as u8,
                (sg / n) as u8,
                (sb / n) as u8,
            ]
        })
        .collect()
}

fn nearest_index(px: [u8; 3], palette: &[[u8; 3]]) -> u8 {
    let mut best = 0usize;
    let mut best_d = u32::MAX;
    for (i, p) in palette.iter().enumerate() {
        let dr = px[0] as i32 - p[0] as i32;
        let dg = px[1] as i32 - p[1] as i32;
        let db = px[2] as i32 - p[2] as i32;
        let d = (dr * dr + dg * dg + db * db) as u32;
        if d < best_d {
            best_d = d;
            best = i;
            if d == 0 {
                break;
            }
        }
    }
    best as u8
}

// ---------------------------------------------------------------------------
// WAD3 writer (matches func_godot quake_wad_import_plugin.gd expectations)
// ---------------------------------------------------------------------------

fn name16(s: &str) -> [u8; 16] {
    let mut out = [0u8; 16];
    let bytes = s.as_bytes();
    let n = bytes.len().min(16);
    out[..n].copy_from_slice(&bytes[..n]);
    out
}

struct WadTexture {
    name: String, // <=15 chars, may start with { ! ~ -
    w: u32,
    h: u32,
    mip0: Vec<u8>,
    mip1: Vec<u8>,
    mip2: Vec<u8>,
    mip3: Vec<u8>,
    palette: [[u8; 3]; 256],
}

fn lump_bytes(t: &WadTexture) -> Vec<u8> {
    let mut out = Vec::with_capacity(40 + t.mip0.len() + t.mip1.len() + t.mip2.len() + t.mip3.len() + 2 + 768);
    out.extend_from_slice(&name16(&t.name));
    out.extend_from_slice(&t.w.to_le_bytes());
    out.extend_from_slice(&t.h.to_le_bytes());
    let o0 = 40u32;
    let o1 = o0 + t.mip0.len() as u32;
    let o2 = o1 + t.mip1.len() as u32;
    let o3 = o2 + t.mip2.len() as u32;
    out.extend_from_slice(&o0.to_le_bytes());
    out.extend_from_slice(&o1.to_le_bytes());
    out.extend_from_slice(&o2.to_le_bytes());
    out.extend_from_slice(&o3.to_le_bytes());
    out.extend_from_slice(&t.mip0);
    out.extend_from_slice(&t.mip1);
    out.extend_from_slice(&t.mip2);
    out.extend_from_slice(&t.mip3);
    // 2-byte gap func_godot skips via get_16() before the palette.
    // Docs describe these as 0x00,0x01; value is irrelevant to the importer.
    out.push(0x00);
    out.push(0x01);
    for c in t.palette.iter() {
        out.push(c[0]);
        out.push(c[1]);
        out.push(c[2]);
    }
    out
}

fn write_wad3(path: &Path, textures: &[WadTexture]) -> Result<()> {
    // layout: header(12) + lump blobs (4-aligned) + directory(32 each)
    let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(textures.len());
    for t in textures {
        blobs.push(lump_bytes(t));
    }
    let mut offsets: Vec<u32> = Vec::with_capacity(textures.len());
    let mut cursor = 12u32;
    for b in blobs.iter() {
        // align lump start to DWORD like qlumpy
        let pad = (4 - (cursor % 4)) % 4;
        cursor += pad;
        offsets.push(cursor);
        cursor += b.len() as u32;
    }
    let dir_pad = (4 - (cursor % 4)) % 4;
    cursor += dir_pad;
    let dir_offset = cursor;

    let mut out: Vec<u8> = Vec::with_capacity((cursor + textures.len() as u32 * 32) as usize);
    out.extend_from_slice(b"WAD3");
    out.extend_from_slice(&(textures.len() as u32).to_le_bytes());
    out.extend_from_slice(&dir_offset.to_le_bytes());

    for (i, b) in blobs.iter().enumerate() {
        while (out.len() as u32) < offsets[i] {
            out.push(0);
        }
        out.extend_from_slice(b);
    }
    while (out.len() as u32) < dir_offset {
        out.push(0);
    }
    for (i, t) in textures.iter().enumerate() {
        let size = blobs[i].len() as u32;
        out.extend_from_slice(&offsets[i].to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes()); // in-wad size
        out.extend_from_slice(&size.to_le_bytes()); // full size
        out.push(0x43); // miptex
        out.push(0x00); // no compression
        out.extend_from_slice(&0u16.to_le_bytes()); // dummy
        out.extend_from_slice(&name16(&t.name));
    }
    fs::write(path, &out).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// CSV helper (paths may contain commas)
// ---------------------------------------------------------------------------

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

struct Pending {
    source: PathBuf,
    stem: String,
    parsed: ParsedName,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.max_size < 16 || cli.max_size % 16 != 0 || cli.max_size > 1024 {
        bail!("--max-size must be a multiple of 16 between 16 and 1024 (got {})", cli.max_size);
    }

    let list_path = cli.list_file.clone();
    let list_dir = list_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let list_text = fs::read_to_string(&list_path)
        .with_context(|| format!("reading list file {}", list_path.display()))?;
    let list_text = list_text.trim_start_matches('\u{FEFF}');

    let mut dirs: Vec<PathBuf> = vec![];
    for (lineno, raw) in list_text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let p = PathBuf::from(line);
        let resolved = if p.is_absolute() {
            p
        } else {
            list_dir.join(p)
        };
        if !resolved.is_dir() {
            eprintln!(
                "{} line {}: not a directory, skipping: {}",
                "error".red().bold(),
                lineno + 1,
                resolved.display()
            );
            continue;
        }
        dirs.push(resolved);
    }
    if dirs.is_empty() {
        bail!("no valid directories in {}", list_path.display());
    }

    // Non-recursive collect of png/tga only.
    let mut pending: Vec<Pending> = vec![];
    let mut skipped_aux = 0u32;
    for dir in dirs.iter() {
        let entries = fs::read_dir(dir)
            .with_context(|| format!("reading dir {}", dir.display()))?;
        for ent in entries {
            let ent = ent?;
            let ft = ent.file_type()?;
            if !ft.is_file() {
                continue; // non-recursive: ignore subdirectories
            }
            let path = ent.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if ext != "png" && ext != "tga" {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("tex")
                .to_string();
            let parsed = parse_file_stem(&stem);
            if !parsed.pack {
                skipped_aux += 1;
                println!(
                    "  {} {} (PBR aux, kept for runtime, not packed)",
                    "skip".cyan(),
                    path.display()
                );
                continue;
            }
            pending.push(Pending {
                source: path,
                stem,
                parsed,
            });
        }
    }
    if pending.is_empty() {
        bail!("no packable png/tga found ({} PBR aux files ignored)", skipped_aux);
    }
    // Deterministic order so _1/_2 collisions match the engine script.
    pending.sort_by(|a, b| {
        a.source
            .to_string_lossy()
            .to_lowercase()
            .cmp(&b.source.to_string_lossy().to_lowercase())
    });

    let stem_name = list_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("pack");
    let wad_path = match cli.output {
        Some(p) => p,
        None => list_dir.join(format!("{stem_name}.wad")),
    };
    let csv_path = {
        // foo.wad -> foo.mapping.csv ; foo (no ext) -> foo.mapping.csv
        let base = wad_path.with_extension("");
        let mut s = base.to_string_lossy().to_string();
        s.push_str(".mapping.csv");
        PathBuf::from(s)
    };

    let mut textures: Vec<WadTexture> = Vec::with_capacity(pending.len());
    let mut used: HashSet<String> = HashSet::new();
    let mut rows: Vec<(String, String, String, u32, u32, u32, u32)> = vec![];

    for p in pending.iter() {
        let img = image::open(&p.source)
            .with_context(|| format!("opening {}", p.source.display()))?;
        let (ow, oh) = img.dimensions();
        if ow < 2 || oh < 2 {
            eprintln!("{} {}: image too small, skipping", "error".red().bold(), p.source.display());
            continue;
        }
        let (nw, nh, resized) = compliant_size(ow, oh, cli.max_size);
        let rgba: RgbaImage = if resized {
            let resized_img =
                image::imageops::resize(&img.to_rgba8(), nw, nh, FilterType::Lanczos3);
            eprintln!(
                "{} {}: {}x{} -> {}x{} (aspect preserved, multiple-of-16)",
                "warning".yellow().bold(),
                p.stem,
                ow,
                oh,
                nw,
                nh
            );
            resized_img
        } else {
            img.to_rgba8()
        };
        if ow % 16 != 0 || oh % 16 != 0 {
            eprintln!(
                "{} {}: source not multiple-of-16, resized form is editor-safe but UV density changed",
                "warning".yellow().bold(),
                p.stem
            );
        }

        let px = rgba.as_raw();
        let has_alpha = px.chunks_exact(4).any(|c| c[3] < 128);

        // Palette training on opaque pixels only.
        let mut opaque: Vec<[u8; 3]> = Vec::with_capacity((nw * nh) as usize);
        for c in px.chunks_exact(4) {
            if c[3] >= 128 {
                opaque.push([c[0], c[1], c[2]]);
            }
        }
        let mut palette_arr = [[0u8; 3]; 256];
        if opaque.is_empty() {
            // fully transparent: palette irrelevant except index 255
            for i in 0..255 {
                palette_arr[i] = [0, 0, 0];
            }
        } else {
            let mut pal = median_cut_quantize(&opaque, 255);
            // de-dup pure blue so only index 255 is transparent for func_godot
            for c in pal.iter_mut() {
                if *c == [0, 0, 255] {
                    *c = [0, 0, 254];
                }
            }
            for (i, c) in pal.iter().enumerate().take(255) {
                palette_arr[i] = *c;
            }
            // pad remainder with last color (or black)
            let fill = pal.last().cloned().unwrap_or([0, 0, 0]);
            for i in pal.len()..255 {
                palette_arr[i] = fill;
            }
        }
        palette_arr[255] = [0, 0, 255];

        // Map level 0 through palette.
        let pal_slice: Vec<[u8; 3]> = palette_arr[..255].to_vec();
        let mut mip0 = vec![0u8; (nw * nh) as usize];
        for (i, c) in px.chunks_exact(4).enumerate() {
            if c[3] < 128 {
                mip0[i] = 255;
            } else {
                mip0[i] = nearest_index([c[0], c[1], c[2]], &pal_slice);
            }
        }

        // Mipmaps 1..3: downscale RGBA, remap through same palette.
        let mut mips = vec![mip0.clone()];
        let mut mw = nw;
        let mut mh = nh;
        for _ in 1..4 {
            mw /= 2;
            mh /= 2;
            let small = image::imageops::resize(&rgba, mw, mh, FilterType::Triangle);
            let raw = small.as_raw();
            let mut idx = vec![0u8; (mw * mh) as usize];
            for (i, c) in raw.chunks_exact(4).enumerate() {
                if c[3] < 128 {
                    idx[i] = 255;
                } else {
                    idx[i] = nearest_index([c[0], c[1], c[2]], &pal_slice);
                }
            }
            mips.push(idx);
        }

        let base15 = build_base(&p.parsed, has_alpha);
        if p.parsed.special == Some('+') {
            eprintln!(
                "{} {}: '+' animation prefix dropped for reference pack",
                "warning".yellow().bold(),
                p.stem
            );
        }
        let final_name = make_unique(&base15, &mut used);
        if final_name != base15 {
            eprintln!(
                "{} {}: name collision, renamed {} -> {}",
                "warning".yellow().bold(),
                p.stem,
                base15,
                final_name
            );
        }
        if has_alpha && !final_name.starts_with('{') {
            // build_base already forces '{'; this is a sanity net
            eprintln!(
                "{} {}: has transparency but name lacks '{{' prefix",
                "warning".yellow().bold(),
                p.stem
            );
        }

        // Ship-safe row: basename only, NEVER an absolute creator-local path.
        // mapping.csv is packed into the .ivl, so absolute paths would leak
        // the creator's disk layout and break merging. long_base is the
        // runtime key; dims are debug-only.
        let file_name = p
            .source
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("tex")
            .to_string();
        rows.push((
            final_name.clone(),
            file_name,
            p.parsed.long_base.clone(),
            ow,
            oh,
            nw,
            nh,
        ));
        textures.push(WadTexture {
            name: final_name,
            w: nw,
            h: nh,
            mip0: mips[0].clone(),
            mip1: mips[1].clone(),
            mip2: mips[2].clone(),
            mip3: mips[3].clone(),
            palette: palette_arr,
        });
    }

    if textures.is_empty() {
        bail!("nothing packable after filtering");
    }

    write_wad3(&wad_path, &textures)?;

    // mapping.csv for the .ivl maker: wad_name,file,long_base + debug dims.
    // The ivl maker merges these (one per wad) into a single shipped
    // mapping.csv. The game MUST validate it (see shorten-plan.md) —
    // hand-made .ivl zips can contain forged rows.
    let mut csv = String::from("wad_name,file,long_base,orig_w,orig_h,final_w,final_h\n");
    // sort rows by wad_name for stable output
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    for (w, s, b, ow, oh, nw, nh) in rows.iter() {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            csv_field(w),
            csv_field(s),
            csv_field(b),
            ow,
            oh,
            nw,
            nh
        ));
    }
    fs::write(&csv_path, csv)?;
    // Also write a JSON-ish lookup the engine script can validate against?
    // Keep it simple: CSV is the contract; see shorten-plan.md.
    let _ = HashMap::<String, String>::new(); // (keeps import honest if extended)

    println!(
        "{} {} textures -> {} (+ {})",
        "ok".green().bold(),
        textures.len(),
        wad_path.display(),
        csv_path.display()
    );
    Ok(())
}
