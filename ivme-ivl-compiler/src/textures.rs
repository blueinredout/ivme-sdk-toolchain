use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Only full-res runtime formats ship. Breaking change (v1): jpg/jpeg/bmp/
/// webp are rejected — re-export as png/tga.
const ALLOWED_EXTENSIONS: &[&str] = &["png", "tga"];

/// PACK (WAD representative) + SKIP (PBR aux) suffixes, lowercased.
/// Must stay in sync with wadgen. One trailing token is stripped to get
/// the long base: `metal_test_albedo` -> `metal_test`.
const KNOWN_SUFFIXES: &[&str] = &[
    "albedo", "diffuse", "basecolor", "base", "color", "normal", "roughness",
    "metallic", "metalness", "ao", "ambientocclusion", "emission", "emissive",
    "height", "orm", "rough",
];

/// Strict loose-stem rule (mirrors wadgen::valid_loose_stem and WADWeaver's
/// `names::parse_file_stem`): `[{] [+NNN] [!/~/-N] body`, where `{` may only
/// combine with `~` and `+NNN`, `+NNN` takes no second prefix, and `-`
/// REQUIRES a digit. Body: 1..64 ASCII alphanumerics/underscores.
pub fn valid_loose_stem(stem: &str) -> bool {
    if stem.is_empty() || stem.len() > 80 {
        return false;
    }
    let mut s = stem;
    let brace = match s.strip_prefix('{') {
        Some(rest) => {
            s = rest;
            true
        }
        None => false,
    };
    let mut random = false;
    if let Some(rest) = s.strip_prefix('+') {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.len() > 3 {
            return false;
        }
        if digits.parse::<u32>().map(|n| n > 255).unwrap_or(true) {
            return false;
        }
        s = &rest[digits.len()..];
        // An animation frame cannot carry a second prefix.
        if s.starts_with(['!', '~', '-']) {
            return false;
        }
    } else if let Some(rest) = s.strip_prefix('!') {
        // `{!X` is no longer water: the engine reads ONE prefix.
        if brace {
            return false;
        }
        s = rest;
    } else if let Some(rest) = s.strip_prefix('~') {
        s = rest;
    } else if let Some(rest) = s.strip_prefix('-') {
        // `-N` random tiling needs its digit; `{-N` is no longer random.
        if brace {
            return false;
        }
        match rest.as_bytes() {
            [d, ..] if d.is_ascii_digit() => {
                random = true;
                s = &rest[1..];
            }
            _ => return false,
        }
    }
    let max_body = if random { 63 } else { 64 };
    if s.is_empty() || s.len() > max_body {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn strip_one_suffix(lower: &str) -> &str {
    match lower.rfind('_') {
        Some(idx) if KNOWN_SUFFIXES.contains(&&lower[idx + 1..]) => &lower[..idx],
        _ => lower,
    }
}

/// Split a lowercased stem into (long_base, anim_frame). Mirrors wadgen's
/// `parse_file_stem` so the key equals the `long_base` column wadgen and
/// WADWeaver write into mapping.csv: the `{`, `+NNN`, and `!` / `~` / `-N`
/// prefixes are not part of the base, but a `-N` random digit is
/// (`-0stone` -> "0stone"). `+0lava_albedo` -> ("lava", Some(0)).
pub fn split_stem(lower: &str) -> (String, Option<u32>) {
    let mut s = lower;
    if let Some(rest) = s.strip_prefix('{') {
        s = rest;
    }
    let mut frame: Option<u32> = None;
    let mut random: Option<u8> = None;
    if let Some(rest) = s.strip_prefix('+') {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && digits.len() <= 3 {
            if let Ok(n) = digits.parse::<u32>() {
                if n <= 255 {
                    frame = Some(n);
                    s = &rest[digits.len()..];
                }
            }
        }
    } else if let Some(rest) = s.strip_prefix('-') {
        // Random tiling keeps its digit: `-0stone` -> "0stone".
        match rest.as_bytes() {
            [d, ..] if d.is_ascii_digit() => {
                random = Some(d - b'0');
                s = &rest[1..];
            }
            _ => {}
        }
    } else if let Some(rest) = s.strip_prefix('!').or_else(|| s.strip_prefix('~')) {
        s = rest;
    }
    let base = strip_one_suffix(s).to_string();
    match random {
        Some(d) => (format!("{d}{base}"), frame),
        None => (base, frame),
    }
}

/// Normalize a raw `.map` face token: strip dirs + extension, lowercase.
/// Valve prefixes are KEPT (`{M_GRATE` -> `{m_grate`, `+2C_LAVA` ->
/// `+2c_lava`) because mapping.csv keys are WAD names and carry them. The
/// fuzzy variants a lookup may also accept are produced by [`face_keys`].
pub fn normalize_face(face: &str) -> String {
    let mut s = face.rsplit(['/', '\\']).next().unwrap_or(face).to_string();
    if let Some(idx) = s.rfind('.') {
        // Faces never carry extensions, but tolerate `foo.png` gracefully.
        let (stem, ext) = (&s[..idx], &s[idx + 1..]);
        if ext.eq_ignore_ascii_case("png") || ext.eq_ignore_ascii_case("tga") {
            s = stem.to_string();
        }
    }
    s.to_lowercase()
}

/// `+N` -> `+0` rewrite of one key, keeping a `{` if there was one:
/// `+2c_lava` -> `+0c_lava`, `{+2g_pane` -> `{+0g_pane`. `None` when the key
/// has no animation frame.
fn anim_zero(key: &str) -> Option<String> {
    let (brace, rest) = match key.strip_prefix('{') {
        Some(r) => ("{", r),
        None => ("", key),
    };
    let rest = rest.strip_prefix('+')?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    Some(format!("{brace}+0{}", &rest[digits.len()..]))
}

/// Table-lookup keys for a normalized face, most specific first:
/// the exact WAD name, then the `{`-less form, then the `+0` frame variant
/// of each (the CSV holds the set's lowest frame, which is not always `+0`),
/// and finally every variant with one PBR suffix stripped (TrenchBroom
/// faces carry `_albedo`/`_normal`).
fn face_keys(norm: &str) -> Vec<String> {
    fn add(keys: &mut Vec<String>, key: String) {
        if !key.is_empty() && !keys.contains(&key) {
            keys.push(key);
        }
    }
    let unbraced = norm.strip_prefix('{').unwrap_or(norm).to_string();
    let mut keys: Vec<String> = Vec::new();
    add(&mut keys, norm.to_string());
    add(&mut keys, unbraced.clone());
    for base in [norm, unbraced.as_str()] {
        if let Some(zeroed) = anim_zero(base) {
            add(&mut keys, zeroed);
        }
    }
    for key in keys.clone() {
        let (stripped, _) = split_stem(&key);
        add(&mut keys, stripped);
    }
    keys
}

#[derive(Debug, Clone)]
pub struct LooseFile {
    pub path: PathBuf,
    pub file_name: String,
    pub long_base: String,
}

#[derive(Debug)]
pub struct TextureScan {
    pub files: Vec<LooseFile>,
    /// long_base -> indices into files (all frames + PBR siblings share it)
    pub by_base: BTreeMap<String, Vec<usize>>,
}

fn walk_images(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort_by(|a, b| {
        a.to_string_lossy()
            .to_lowercase()
            .cmp(&b.to_string_lossy().to_lowercase())
    });
    for path in paths {
        if path.is_dir() {
            walk_images(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

/// Scan + strictly validate the working textures folder.
/// Recursive read (artists nest), flat ship: duplicates by basename
/// (case-insensitive) and invalid stems are hard errors.
pub fn scan_textures(textures_dir: &Path) -> Result<TextureScan, crate::error::CompileError> {
    use crate::error::CompileError;
    let mut raw: Vec<PathBuf> = vec![];
    walk_images(textures_dir, &mut raw);

    let mut invalid: Vec<String> = vec![];
    let mut files: Vec<LooseFile> = vec![];
    let mut by_name: BTreeMap<String, usize> = BTreeMap::new();
    let mut dup: Option<(String, PathBuf, PathBuf)> = None;

    for path in raw {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !ALLOWED_EXTENSIONS.contains(&ext.as_str()) {
            continue; // non-shippable files surface via orphan warnings
        }
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let stem = match path.file_stem().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => {
                invalid.push(path.display().to_string());
                continue;
            }
        };
        if !valid_loose_stem(stem) {
            invalid.push(format!("{} (from {})", file_name, path.display()));
            continue;
        }
        let key = file_name.to_lowercase();
        if let Some(&prev_idx) = by_name.get(&key) {
            dup = Some((file_name, files[prev_idx].path.clone(), path));
            break;
        }
        let (long_base, _frame) = split_stem(&stem.to_lowercase());
        let idx = files.len();
        files.push(LooseFile {
            path,
            file_name,
            long_base,
        });
        by_name.insert(key, idx);
    }

    if let Some((name, a, b)) = dup {
        return Err(CompileError::DuplicateBasename(name, a, b));
    }
    // Invalid stems are skipped, not fatal: one stray `my texture (1).png` in
    // the artist's folder must not block the whole level (wadgen and the
    // WAD editor skip such files too). A face that needed one still surfaces
    // as MISSING_TEXTURE below.
    if !invalid.is_empty() {
        eprintln!(
            "Warning: INVALID_TEXTURE_NAME - {} file(s) skipped (want [A-Za-z0-9_] with an optional {{/!/~/-N/+NNN prefix):\n  {}",
            invalid.len(),
            invalid.join("\n  ")
        );
    }

    let mut by_base: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, f) in files.iter().enumerate() {
        by_base.entry(f.long_base.clone()).or_default().push(i);
    }
    Ok(TextureScan { files, by_base })
}

#[derive(Debug)]
pub struct TextureResolution {
    /// indices into the scan's files
    pub included: BTreeSet<usize>,
    pub sets: usize,
    /// faces with no match anywhere: probable typos, warn per face
    pub missing: Vec<String>,
    /// shorts claimed by both the local and the internal (SDK) table with
    /// different longs: the game prefers its builtin, so the mapper must
    /// rename. Rare, always loud.
    pub conflicts: Vec<String>,
}

/// First table row whose key matches any [`face_keys`] variant. Keys are
/// WAD names (prefixes included), so `{C_ROCK` and `+2C_LAVA` both hit.
fn table_hit(table: &BTreeMap<String, String>, norm: &str) -> Option<String> {
    face_keys(norm)
        .into_iter()
        .find_map(|key| table.get(&key).cloned())
}

/// Resolve faces against the scan, via the merged local table and the
/// internal SDK table (`ivme.csv` next to the exe). Local hits package
/// loose files. Internal hits with no loose files are silently ignored
/// (the game ships those). Anything else unmatched is a warning —
/// never fatal. A short claimed by both tables with different longs is
/// a conflict (the game prefers its builtin): loud, per face.
pub fn resolve_textures(
    scan: &TextureScan,
    referenced: &BTreeSet<String>,
    mapping: Option<&BTreeMap<String, String>>,
    internal: Option<&BTreeMap<String, String>>,
) -> TextureResolution {
    let mut included = BTreeSet::new();
    let mut missing = Vec::new();
    let mut conflicts = Vec::new();
    let mut sets: BTreeSet<String> = BTreeSet::new();

    // Long bases the game ships (internal table VALUES, e.g. `floor`).
    // TB faces carry full names (`dev/floor` -> norm `floor`) which never
    // match short-name keys (`c_floor`), so match values too to stay silent.
    let internal_longs: BTreeSet<String> = internal
        .map(|t| t.values().cloned().collect())
        .unwrap_or_default();

    for face in referenced {
        let norm = normalize_face(face);
        let local_hit = mapping.and_then(|t| table_hit(t, &norm));
        let mut internal_hit = internal.and_then(|t| table_hit(t, &norm));
        if internal_hit.is_none() {
            let (stripped, _) = split_stem(&norm);
            if internal_longs.contains(&stripped) {
                internal_hit = Some(stripped);
            } else if internal_longs.contains(&norm) {
                internal_hit = Some(norm.clone());
            }
        }
        // Candidate long bases, in order:
        //  1. local mapping hit on the normalized key (+N rewritten to +0)
        //  2. local mapping hit on the suffix-stripped key
        //  3. internal (SDK) hit — same two-step lookup
        //  4. passthrough: suffix-stripped key itself (must be a clean base)
        let (resolved, from_internal): (Option<String>, bool) =
            match (local_hit, internal_hit) {
                (Some(l), Some(b)) if l != b => {
                    conflicts.push(face.clone());
                    (Some(b), true) // runtime truth: the game prefers builtin
                }
                (Some(l), _) => (Some(l), false),
                (None, Some(b)) => (Some(b), true),
                (None, None) => {
                    let (stripped, _) = split_stem(&norm);
                    (Some(stripped), false)
                }
            };

        // Table values are pre-validated; passthrough must ALSO be a clean
        // base, else the face is unresolvable (warn, runtime checkers it).
        let base = resolved.unwrap_or_default();
        let clean = !base.is_empty()
            && base.len() <= 64
            && base
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        match (clean, scan.by_base.get(&base)) {
            (true, Some(idxs)) => {
                sets.insert(base);
                for &i in idxs {
                    included.insert(i);
                }
            }
            _ if from_internal && clean => {
                // Game builtin: nothing to ship, nothing to say.
            }
            _ => {
                missing.push(face.clone());
            }
        }
    }

    TextureResolution {
        included,
        sets: sets.len(),
        missing,
        conflicts,
    }
}

/// Loose files with no face referencing their set (bloat warnings).
/// Compares by set (long_base), so PBR siblings/anim frames of a used
/// texture are never reported.
pub fn find_orphan_sets(
    scan: &TextureScan,
    used_bases: &BTreeSet<String>,
) -> Vec<String> {
    scan
        .by_base
        .keys()
        .filter(|b| !used_bases.contains(*b))
        .cloned()
        .collect()
}

/// Non-shippable files (wrong extension) sitting in the working folder.
/// Reported as one summary warning; they never enter the archive.
pub fn list_junk_files(textures_dir: &Path) -> Vec<PathBuf> {
    let mut raw: Vec<PathBuf> = vec![];
    walk_images(textures_dir, &mut raw);
    raw.into_iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .map(|e| !ALLOWED_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                .unwrap_or(true)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_stem() {
        assert_eq!(split_stem("metal_test_albedo"), ("metal_test".to_string(), None));
        assert_eq!(split_stem("+0lava_albedo"), ("lava".to_string(), Some(0)));
        assert_eq!(split_stem("+12lava"), ("lava".to_string(), Some(12)));
        assert_eq!(split_stem("{rock"), ("rock".to_string(), None));
        assert_eq!(split_stem("plain"), ("plain".to_string(), None));
        // The Valve prefixes leave no trace in the base (mapping.csv keys).
        assert_eq!(split_stem("!water"), ("water".to_string(), None));
        assert_eq!(split_stem("~metal_lamp"), ("metal_lamp".to_string(), None));
        assert_eq!(split_stem("{~grate"), ("grate".to_string(), None));
        assert_eq!(split_stem("{+2glass_pane"), ("glass_pane".to_string(), Some(2)));
        // Random tiling keeps its digit, exactly like wadgen/WADWeaver.
        assert_eq!(split_stem("-0stone"), ("0stone".to_string(), None));
        assert_eq!(split_stem("-9x_albedo"), ("9x".to_string(), None));
        // A bare `-` is not random tiling; it stays in the name (worst case a
        // missing texture, never a wrong match).
        assert_eq!(split_stem("-stone"), ("-stone".to_string(), None));
    }

    #[test]
    fn test_normalize_face() {
        assert_eq!(normalize_face("M_TEST"), "m_test");
        assert_eq!(normalize_face("+5C_LAVA"), "+5c_lava");
        assert_eq!(normalize_face("textures/M_TEST"), "m_test");
        assert_eq!(normalize_face("{C_ROCK"), "{c_rock");
        assert_eq!(normalize_face("textures/{+2C_ROCK.png"), "{+2c_rock");
    }

    #[test]
    fn test_face_keys() {
        // Exact WAD name first, then the brace-less form, then the frames.
        assert_eq!(face_keys("m_test"), ["m_test"]);
        assert_eq!(face_keys("{c_rock"), ["{c_rock", "c_rock"]);
        assert_eq!(
            face_keys("{+2g_pane"),
            ["{+2g_pane", "+2g_pane", "{+0g_pane", "+0g_pane", "g_pane"]
        );
        assert_eq!(face_keys("+5c_lava"), ["+5c_lava", "+0c_lava", "c_lava"]);
        // TrenchBroom faces carry the PBR suffix and no prefix.
        assert_eq!(
            face_keys("metal_test_albedo"),
            ["metal_test_albedo", "metal_test"]
        );
    }

    fn table_of(rows: &[(&str, &str)]) -> BTreeMap<String, String> {
        rows.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn test_table_hit_prefixes_and_frames() {
        // A table exactly as WADWeaver/wadgen writes it: keys keep `{`/`!`/
        // `~`/`-N`/`+N`.
        let table = table_of(&[
            ("m_test", "metal_test"),
            ("{m_grate", "metal_grate"),
            ("{w_fence", "wood_fence"),
            ("{+2g_pane", "glass_pane"),
            ("!c_water", "water"),
            ("~m_lamp", "metal_lamp"),
            ("-0c_stone", "0stone"),
        ]);
        for (face, want) in [
            ("M_TEST", "metal_test"),
            ("{M_GRATE", "metal_grate"),
            ("textures/{W_FENCE", "wood_fence"),
            ("{+2G_PANE", "glass_pane"),
            ("!C_WATER", "water"),
            ("~M_LAMP", "metal_lamp"),
            ("-0C_STONE", "0stone"),
        ] {
            assert_eq!(table_hit(&table, &normalize_face(face)), Some(want.to_string()), "{face}");
        }
        // A face picking a later frame of a set resolves to the set's row.
        assert_eq!(
            table_hit(&table, &normalize_face("+5C_LAVA")),
            None,
            "no row for that set in this table"
        );
        let lava = table_of(&[("+0c_lava", "lava")]);
        assert_eq!(
            table_hit(&lava, &normalize_face("+3C_LAVA")),
            Some("lava".to_string())
        );
        assert_eq!(table_hit(&table, &normalize_face("NOT_A_TEXTURE")), None);
    }

    #[test]
    fn test_valid_stems() {
        assert!(valid_loose_stem("metal_test_albedo"));
        assert!(valid_loose_stem("+0lava"));
        assert!(valid_loose_stem("{rock"));
        assert!(valid_loose_stem("{+12lava_normal"));
        // D2 prefixes, same set wadgen and WADWeaver accept.
        assert!(valid_loose_stem("!water"));
        assert!(valid_loose_stem("~metal_lamp"));
        assert!(valid_loose_stem("{~grate"));
        assert!(valid_loose_stem("-0stone"));
        assert!(valid_loose_stem("-9x_albedo"));
        assert!(!valid_loose_stem("my brick wall (final)"));
        assert!(!valid_loose_stem("with-dash"));
        assert!(!valid_loose_stem("+256lava"));
        // One prefix only, and `-` needs a digit.
        assert!(!valid_loose_stem("{!water"));
        assert!(!valid_loose_stem("{-0x"));
        assert!(!valid_loose_stem("-stone"));
        assert!(!valid_loose_stem("-"));
        assert!(!valid_loose_stem("-0"));
        assert!(!valid_loose_stem("+0~lamp"));
        assert!(!valid_loose_stem("+0-0x"));
        assert!(!valid_loose_stem("{"));
    }

    fn test_scan(base: &str) -> TextureScan {
        let files = vec![LooseFile {
            path: PathBuf::from(format!("{}.png", base)),
            file_name: format!("{}.png", base),
            long_base: base.to_string(),
        }];
        let mut by_base = BTreeMap::new();
        by_base.insert(base.to_string(), vec![0]);
        TextureScan { files, by_base }
    }

    #[test]
    fn test_resolve_internal_silence() {
        // Internal (SDK) hit with no loose files: neither shipped nor warned.
        let scan = test_scan("metal_test");
        let mut faces = BTreeSet::new();
        faces.insert("C_BUILTIN".to_string());
        let mut internal = BTreeMap::new();
        internal.insert("c_builtin".to_string(), "concrete_builtin".to_string());
        let r = resolve_textures(&scan, &faces, None, Some(&internal));
        assert!(r.included.is_empty());
        assert!(r.missing.is_empty());
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn test_resolve_conflict() {
        // Same short, different longs: builtin wins, conflict reported.
        let scan = test_scan("local_base");
        let mut faces = BTreeSet::new();
        faces.insert("M_BOTH".to_string());
        let mut local = BTreeMap::new();
        local.insert("m_both".to_string(), "local_base".to_string());
        let mut internal = BTreeMap::new();
        internal.insert("m_both".to_string(), "sdk_base".to_string());
        let r = resolve_textures(&scan, &faces, Some(&local), Some(&internal));
        assert_eq!(r.conflicts, vec!["M_BOTH".to_string()]);
        assert!(r.missing.is_empty());
    }
}
