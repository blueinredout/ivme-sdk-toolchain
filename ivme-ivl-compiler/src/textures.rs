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

/// Strict loose-stem rule (mirrors wadgen::valid_loose_stem).
pub fn valid_loose_stem(stem: &str) -> bool {
    if stem.is_empty() || stem.len() > 80 {
        return false;
    }
    let mut s = stem;
    if let Some(rest) = s.strip_prefix('{') {
        s = rest;
    }
    if let Some(rest) = s.strip_prefix('+') {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() || digits.len() > 3 {
            return false;
        }
        if digits.parse::<u32>().map(|n| n > 255).unwrap_or(true) {
            return false;
        }
        s = &rest[digits.len()..];
    } else if let Some(c) = s.chars().next() {
        if matches!(c, '!' | '~' | '-') {
            s = &s[1..];
        }
    }
    if s.is_empty() || s.len() > 64 {
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

/// Split a lowercased stem into (long_base, anim_frame).
/// `+0lava_albedo` -> ("lava", Some(0)); `{rock` -> ("rock", None).
pub fn split_stem(lower: &str) -> (String, Option<u32>) {
    let mut s = lower;
    if let Some(rest) = s.strip_prefix('{') {
        s = rest;
    }
    let mut frame: Option<u32> = None;
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
    }
    (strip_one_suffix(s).to_string(), frame)
}

/// Normalize a raw `.map` face token for lookup: strip dirs + extension,
/// lowercase, rewrite any `+N` anim frame to `+0` (the WAD/CSV holds the
/// set's lowest frame; the game animates from loose files).
pub fn normalize_face(face: &str) -> String {
    let mut s = face.rsplit(['/', '\\']).next().unwrap_or(face).to_string();
    if let Some(idx) = s.rfind('.') {
        // Faces never carry extensions, but tolerate `foo.png` gracefully.
        let (stem, ext) = (&s[..idx], &s[idx + 1..]);
        if ext.eq_ignore_ascii_case("png") || ext.eq_ignore_ascii_case("tga") {
            s = stem.to_string();
        }
    }
    let mut lower = s.to_lowercase();
    // strip ONE leading '{' for comparison, keep it simple
    let stripped = lower.strip_prefix('{').unwrap_or(&lower).to_string();
    // rewrite +N -> +0
    if let Some(rest) = stripped.strip_prefix('+') {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            lower = format!("+0{}", &rest[digits.len()..]);
            return lower;
        }
    }
    stripped
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
    if !invalid.is_empty() {
        return Err(CompileError::InvalidTextureName(format!(
            "{} file(s) — rename to [A-Za-z0-9_] (optional {{/!/~/-[+NNN]):\n  {}",
            invalid.len(),
            invalid.join("\n  ")
        )));
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

fn table_hit(table: &BTreeMap<String, String>, norm: &str) -> Option<String> {
    if let Some(hit) = table.get(norm) {
        return Some(hit.clone());
    }
    let (stripped, _) = split_stem(norm);
    if stripped != norm {
        if let Some(hit) = table.get(&stripped) {
            return Some(hit.clone());
        }
    }
    None
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

    for face in referenced {
        let norm = normalize_face(face);
        let local_hit = mapping.and_then(|t| table_hit(t, &norm));
        let internal_hit = internal.and_then(|t| table_hit(t, &norm));
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
    }

    #[test]
    fn test_normalize_face() {
        assert_eq!(normalize_face("M_TEST"), "m_test");
        assert_eq!(normalize_face("+5C_LAVA"), "+0c_lava");
        assert_eq!(normalize_face("textures/M_TEST"), "m_test");
        assert_eq!(normalize_face("{C_ROCK"), "c_rock");
    }

    #[test]
    fn test_valid_stems() {
        assert!(valid_loose_stem("metal_test_albedo"));
        assert!(valid_loose_stem("+0lava"));
        assert!(valid_loose_stem("{rock"));
        assert!(valid_loose_stem("{+12lava_normal"));
        assert!(!valid_loose_stem("my brick wall (final)"));
        assert!(!valid_loose_stem("with-dash"));
        assert!(!valid_loose_stem("+256lava"));
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
