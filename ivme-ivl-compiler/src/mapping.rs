use crate::error::CompileError;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Caps shared with the game loader (see shorten-plan.md "CSV validation").
/// Fail closed here so creators notice what the game would silently skip.
pub const MAX_CSV_BYTES: u64 = 256 * 1024;
pub const MAX_CSV_ROWS: usize = 4096;

const EXPECTED_HEADER: &str = "wad_name,file,long_base,orig_w,orig_h,final_w,final_h";

pub struct MappingTable {
    /// lowercased short name -> long_base
    pub table: BTreeMap<String, String>,
    /// raw CSV bytes as shipped into the .ivl (normalized to LF)
    pub csv_bytes: Vec<u8>,
    pub rows: usize,
}

fn valid_wad_name(s: &str) -> bool {
    // ^\{?(\+\d{1,3})?[A-Z0-9_]{1,15}$  (+NNN anim extension of the
    // shorten-plan regex; the client must adopt the same expression)
    let mut s = s;
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
    }
    if s.is_empty() || s.len() > 15 {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_long_base(s: &str) -> bool {
    // ^[a-z0-9_]{1,64}$ — no / \ . : .. space, ever.
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    s.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn valid_file_field(s: &str) -> bool {
    // Basename only, allowlisted extension. Never a path.
    if s.is_empty() || s.contains('/') || s.contains('\\') || s.contains("..") {
        return false;
    }
    let lower = s.to_lowercase();
    (lower.ends_with(".png") || lower.ends_with(".tga"))
        && !s.starts_with('.')
        && !s.contains(',')
}

/// Minimal CSV record splitter handling wadgen-style quoting.
fn split_record(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    cur.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                cur.push(c);
            }
        } else if c == '"' {
            in_quotes = true;
        } else if c == ',' {
            fields.push(cur);
            cur = String::new();
        } else {
            cur.push(c);
        }
    }
    fields.push(cur);
    fields
}

/// Locate mapping tables: explicit `--mapping` files (repeatable, CLI
/// order), else auto-detect and merge `<map_dir>/mapping.csv` plus every
/// `<map_dir>/*.mapping.csv` (sorted). Returns an empty vec when there is
/// no table (TrenchBroom-only map).
pub fn find_mappings(
    explicit: &[PathBuf],
    map_dir: &Path,
) -> Result<Vec<PathBuf>, CompileError> {
    if !explicit.is_empty() {
        let mut seen: BTreeMap<String, PathBuf> = BTreeMap::new();
        for p in explicit {
            if !p.is_file() {
                return Err(CompileError::MappingInvalid(format!(
                    "--mapping file not found: {}",
                    p.display()
                )));
            }
            // Dedup by canonical path so `--mapping a --mapping a` is harmless.
            let key = std::fs::canonicalize(p)
                .map(|c| c.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.to_string_lossy().to_string());
            seen.entry(key).or_insert_with(|| p.clone());
        }
        return Ok(seen.into_values().collect());
    }
    let mut found: Vec<PathBuf> = vec![];
    let exact = map_dir.join("mapping.csv");
    if exact.is_file() {
        found.push(exact);
    }
    let mut extra: Vec<PathBuf> = vec![];
    if let Ok(entries) = std::fs::read_dir(map_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".mapping.csv") {
                    extra.push(path);
                }
            }
        }
    }
    extra.sort();
    for p in extra {
        if !found.contains(&p) {
            found.push(p);
        }
    }
    for p in &found {
        eprintln!("Note: using mapping table {}", p.display());
    }
    Ok(found)
}

pub fn load_mapping(path: &Path) -> Result<MappingTable, CompileError> {
    let meta = std::fs::metadata(path).map_err(|e| {
        CompileError::MappingInvalid(format!("cannot stat {}: {}", path.display(), e))
    })?;
    if meta.len() > MAX_CSV_BYTES {
        return Err(CompileError::MappingInvalid(format!(
            "{} is {} bytes (cap {} KB)",
            path.display(),
            meta.len(),
            MAX_CSV_BYTES / 1024
        )));
    }
    let text = std::fs::read_to_string(path).map_err(|e| {
        CompileError::MappingInvalid(format!("cannot read {}: {}", path.display(), e))
    })?;
    let text = text.trim_start_matches('\u{FEFF}').replace("\r\n", "\n");
    let mut lines = text.lines();
    let header = lines.next().ok_or_else(|| {
        CompileError::MappingInvalid(format!("{} is empty", path.display()))
    })?;
    if header.trim() != EXPECTED_HEADER {
        return Err(CompileError::MappingInvalid(format!(
            "bad header in {}: expected `{}`",
            path.display(),
            EXPECTED_HEADER
        )));
    }

    let mut table: BTreeMap<String, String> = BTreeMap::new();
    let mut shipped_rows: Vec<String> = vec![];
    let mut row_count = 0usize;
    let mut skipped = 0u32;
    for (lineno, line) in lines.enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        row_count += 1;
        if row_count > MAX_CSV_ROWS {
            return Err(CompileError::MappingInvalid(format!(
                "more than {} rows (cap)",
                MAX_CSV_ROWS
            )));
        }
        let f = split_record(line);
        if f.len() != 7 || !valid_wad_name(&f[0]) || !valid_file_field(&f[1]) || !valid_long_base(&f[2]) {
            skipped += 1;
            eprintln!(
                "Warning: MAPPING_ROW_SKIPPED - {} line {} failed validation (game would skip it)",
                path.display(),
                lineno + 2
            );
            continue;
        }
        // dims columns must be plain integers (debug-only, but reject junk)
        if [&f[3], &f[4], &f[5], &f[6]]
            .iter()
            .any(|d| d.is_empty() || !d.bytes().all(|b| b.is_ascii_digit()))
        {
            skipped += 1;
            eprintln!(
                "Warning: MAPPING_ROW_SKIPPED - {} line {} has non-numeric dims",
                path.display(),
                lineno + 2
            );
            continue;
        }
        let key = f[0].to_lowercase();
        if let Some(prev) = table.get(&key) {
            if prev != &f[2] {
                eprintln!(
                    "Warning: MAPPING_CONFLICT - {} = {} vs {} (kept {})",
                    f[0], prev, f[2], prev
                );
            }
            continue; // first wins
        }
        table.insert(key, f[2].clone());
        shipped_rows.push(line.to_string());
    }
    if table.is_empty() {
        return Err(CompileError::MappingInvalid(format!(
            "no valid rows in {} ({} skipped)",
            path.display(),
            skipped
        )));
    }
    // Ship a clean table: header + valid rows sorted by wad_name byte order.
    shipped_rows.sort();
    let mut csv = String::from(EXPECTED_HEADER);
    csv.push('\n');
    for row in &shipped_rows {
        csv.push_str(row);
        csv.push('\n');
    }
    Ok(MappingTable {
        table,
        rows: shipped_rows.len(),
        csv_bytes: csv.into_bytes(),
    })
}

/// Merge N loaded tables (SDK + user WADs) into the single shipped table.
/// First wins on a conflicting short (same `wad_name`, different
/// `long_base`) with a loud warning naming both files — the fix for a
/// real conflict is to bake the folders in ONE wadgen run so the `_N`
/// walk shares a namespace. Caps apply to the merged result.
pub fn merge_tables(inputs: Vec<(PathBuf, MappingTable)>) -> Result<MappingTable, CompileError> {
    let mut table: BTreeMap<String, String> = BTreeMap::new();
    let mut rows: BTreeMap<String, String> = BTreeMap::new();
    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    for (path, t) in &inputs {
        let label = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mapping.csv")
            .to_string();
        for (short, long) in &t.table {
            match table.get(short) {
                Some(prev) if prev != long => {
                    eprintln!(
                        "Warning: MAPPING_CONFLICT - {} = {} (from {}) vs {} (from {}; kept {})",
                        short,
                        prev,
                        sources.get(short).map(|s| s.as_str()).unwrap_or("?"),
                        long,
                        label,
                        prev
                    );
                }
                Some(_) => {}
                None => {
                    table.insert(short.clone(), long.clone());
                    sources.insert(short.clone(), label.clone());
                }
            }
        }
        // Re-split this table's shipped rows back out for the union.
        let mut lines = t.csv_bytes
            .as_slice()
            .split(|&b| b == b'\n')
            .map(|l| String::from_utf8_lossy(l).to_string())
            .collect::<Vec<_>>();
        if !lines.is_empty() {
            lines.remove(0); // header
        }
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            let wad = split_record(&line).first().cloned().unwrap_or_default();
            rows.entry(wad.to_lowercase()).or_insert(line);
        }
    }
    if table.is_empty() {
        return Err(CompileError::MappingInvalid(
            "no valid rows in any mapping table".to_string(),
        ));
    }
    if table.len() > MAX_CSV_ROWS {
        return Err(CompileError::MappingInvalid(format!(
            "merged table has {} rows (cap {})",
            table.len(),
            MAX_CSV_ROWS
        )));
    }
    let mut csv = String::from(EXPECTED_HEADER);
    csv.push('\n');
    let mut keys: Vec<&String> = rows.keys().collect();
    keys.sort();
    for k in keys {
        csv.push_str(&rows[k]);
        csv.push('\n');
    }
    if csv.len() as u64 > MAX_CSV_BYTES {
        return Err(CompileError::MappingInvalid(format!(
            "merged mapping.csv is {} bytes (cap {} KB)",
            csv.len(),
            MAX_CSV_BYTES / 1024
        )));
    }
    Ok(MappingTable {
        rows: table.len(),
        table,
        csv_bytes: csv.into_bytes(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_names() {
        assert!(valid_wad_name("M_TEST"));
        assert!(valid_wad_name("{C_TRANS_100X10"));
        assert!(valid_wad_name("+0C_LAVA"));
        assert!(valid_wad_name("{+1M_X"));
        assert!(!valid_wad_name("m_test"));
        assert!(!valid_wad_name("M TEST"));
        assert!(!valid_wad_name("M/TEST"));
        assert!(!valid_wad_name("+256C_X"));
        assert!(valid_long_base("metal_test"));
        assert!(!valid_long_base("Metal_Test"));
        assert!(!valid_long_base("../../evil"));
        assert!(!valid_long_base("has space"));
    }

    #[test]
    fn test_split_record() {
        assert_eq!(split_record("a,b,c"), vec!["a", "b", "c"]);
        assert_eq!(
            split_record("\"a,b\",c"),
            vec!["a,b".to_string(), "c".to_string()]
        );
    }

    fn table_from(rows: &[(&str, &str)]) -> MappingTable {
        let mut table = BTreeMap::new();
        let mut csv = String::from(EXPECTED_HEADER);
        csv.push('\n');
        for (w, l) in rows {
            table.insert(w.to_lowercase(), l.to_string());
            csv.push_str(&format!("{},{}.png,{},16,16,16,16\n", w, l, l));
        }
        MappingTable {
            rows: table.len(),
            table,
            csv_bytes: csv.into_bytes(),
        }
    }

    #[test]
    fn test_merge_first_wins() {
        let sdk = table_from(&[("M_TEST", "metal_test"), ("C_ROCK", "rock")]);
        let user = table_from(&[("W_WOOD", "wood_floor"), ("M_TEST", "other_metal")]);
        let merged = merge_tables(vec![
            (PathBuf::from("sdk.mapping.csv"), sdk),
            (PathBuf::from("user.mapping.csv"), user),
        ])
        .unwrap();
        assert_eq!(merged.table.get("m_test").unwrap(), "metal_test");
        assert_eq!(merged.table.get("w_wood").unwrap(), "wood_floor");
        assert_eq!(merged.table.get("c_rock").unwrap(), "rock");
        assert_eq!(merged.rows, 3);
        let text = String::from_utf8(merged.csv_bytes).unwrap();
        assert!(text.lines().count() == 4); // header + 3 rows
    }
}
