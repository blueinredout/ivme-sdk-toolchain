use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PayloadEntry {
    pub path: String,
    pub data: Vec<u8>,
    pub is_directory: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileDescriptor {
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HashResult {
    pub encoded_bytes: Vec<u8>,
    pub content_hash: String,
    pub files: Vec<FileDescriptor>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IdentityError {
    pub code: &'static str,
    pub message: String,
}
impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for IdentityError {}
fn err(code: &'static str, message: impl Into<String>) -> IdentityError {
    IdentityError {
        code,
        message: message.into(),
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn allowed_file(p: &str) -> bool {
    matches!(p, "level.map" | "level.vmf" | "mapping.csv")
        || (p.starts_with("textures/")
            && [".png", ".tga"]
                .iter()
                .any(|e| p.to_ascii_lowercase().ends_with(e)))
}
fn excluded(p: &str) -> bool {
    matches!(p, "preview.png" | "icon.png" | "ivl-manifest.json")
}
fn lexical(e: &PayloadEntry) -> Result<String, IdentityError> {
    let p = &e.path;
    if p.is_empty()
        || !p.bytes().all(|b| (32..=126).contains(&b))
        || p.starts_with('/')
        || p.contains(['\\', ':', '"', '<', '>', '|', '?', '*'])
    {
        return Err(err("INVALID_PATH", format!("invalid path: {p}")));
    }
    let canonical = if e.is_directory && p.ends_with('/') {
        &p[..p.len() - 1]
    } else {
        p.as_str()
    };
    if canonical.is_empty()
        || canonical
            .split('/')
            .any(|s| s.is_empty() || s == "." || s == ".." || s.ends_with('.') || s.ends_with(' '))
    {
        return Err(err("INVALID_PATH", format!("invalid path: {p}")));
    }
    for segment in canonical.split('/') {
        let stem = segment.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                stem.strip_prefix(prefix).is_some_and(|n| {
                    matches!(n, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                })
            })
        {
            return Err(err("INVALID_PATH", format!("reserved device path: {p}")));
        }
    }
    if e.is_directory {
        if canonical != "textures" && !canonical.starts_with("textures/") {
            return Err(err(
                "UNSUPPORTED_FILE",
                format!("unsupported directory: {p}"),
            ));
        }
    } else if !allowed_file(canonical) && !excluded(canonical) {
        return Err(err("UNSUPPORTED_FILE", format!("unsupported file: {p}")));
    }
    Ok(canonical.to_owned())
}
pub(crate) fn validate_paths(entries: &[PayloadEntry]) -> Result<(), IdentityError> {
    let mut exact = BTreeSet::new();
    let mut folded: BTreeMap<String, String> = BTreeMap::new();
    let mut kinds: BTreeMap<String, bool> = BTreeMap::new();
    for e in entries {
        let p = lexical(e)?;
        if !exact.insert(p.clone()) {
            return Err(err("DUPLICATE_PATH", format!("duplicate path: {p}")));
        }
        let parts: Vec<_> = p.split('/').collect();
        for i in 1..=parts.len() {
            let prefix = parts[..i].join("/");
            let lower = prefix.to_ascii_lowercase();
            if let Some(prev) = folded.get(&lower) {
                if prev != &prefix {
                    return Err(err(
                        "CASE_COLLISION",
                        format!("case collision: {prev} and {prefix}"),
                    ));
                }
            } else {
                folded.insert(lower, prefix);
            }
        }
        kinds.insert(p.clone(), e.is_directory);
    }
    for e in entries {
        let p = lexical(e)?;
        let segs: Vec<_> = p.split('/').collect();
        for i in 1..segs.len() {
            let prefix = segs[..i].join("/");
            if let Some(is_dir) = kinds.get(&prefix) {
                if !is_dir {
                    return Err(err(
                        "DUPLICATE_PATH",
                        format!("file used as ancestor: {prefix}"),
                    ));
                }
            }
        }
        if !e.is_directory && kinds.get(&p) == Some(&true) {
            return Err(err(
                "DUPLICATE_PATH",
                format!("file/directory conflict: {p}"),
            ));
        }
    }
    Ok(())
}
pub(crate) fn hash_payload(entries: &[PayloadEntry]) -> Result<HashResult, IdentityError> {
    validate_paths(entries)?;
    let mut files: Vec<_> = entries
        .iter()
        .filter(|e| !e.is_directory && !excluded(&e.path))
        .collect();
    files.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
    let count = u32::try_from(files.len())
        .map_err(|_| err("INTEGER_OVERFLOW", "too many payload files"))?;
    let mut encoded = b"IVME-CONTENT\0".to_vec();
    encoded.extend(1u32.to_le_bytes());
    encoded.extend(count.to_le_bytes());
    let mut descriptors = Vec::new();
    for e in files {
        let path = e.path.as_bytes();
        let plen =
            u32::try_from(path.len()).map_err(|_| err("INTEGER_OVERFLOW", "path too long"))?;
        let len =
            u64::try_from(e.data.len()).map_err(|_| err("INTEGER_OVERFLOW", "file too large"))?;
        let digest = sha(&e.data);
        encoded.extend(plen.to_le_bytes());
        encoded.extend(path);
        encoded.extend(len.to_le_bytes());
        encoded.extend(digest);
        descriptors.push(FileDescriptor {
            path: e.path.clone(),
            size_bytes: len,
            sha256: hex(&digest),
        });
    }
    let content_hash = hex(&sha(&encoded));
    Ok(HashResult {
        encoded_bytes: encoded,
        content_hash,
        files: descriptors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::Value;
    fn fixture() -> Value {
        serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../contracts/record-identity-vectors-v1.json"
        )))
        .unwrap()
    }
    fn entries(v: &Value) -> Vec<PayloadEntry> {
        v["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| PayloadEntry {
                path: f["path"].as_str().unwrap().to_owned(),
                data: base64::engine::general_purpose::STANDARD
                    .decode(f["bytes_base64"].as_str().unwrap())
                    .unwrap(),
                is_directory: f["is_directory"].as_bool().unwrap_or(false),
            })
            .collect()
    }
    fn hex_decode(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }
    #[test]
    fn test_frozen_hash_vectors() {
        let v = fixture();
        let rows = v["vectors"].as_array().unwrap();
        let mut seen = 0;
        for row in rows.iter().filter(|r| r["kind"] == "hash") {
            seen += 1;
            let got = hash_payload(&entries(row)).unwrap();
            let expected_count = row["files"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|f| {
                    !f["is_directory"].as_bool().unwrap_or(false)
                        && !matches!(
                            f["path"].as_str().unwrap(),
                            "preview.png" | "icon.png" | "ivl-manifest.json"
                        )
                })
                .count();
            assert_eq!(got.files.len(), expected_count);
            assert!(got.files.windows(2).all(|w| w[0].path < w[1].path));
            assert_eq!(
                got.encoded_bytes,
                hex_decode(row["encoded_hex"].as_str().unwrap()),
                "{}",
                row["name"]
            );
            assert_eq!(got.content_hash, row["sha256"].as_str().unwrap());
            for d in &got.files {
                let f = row["files"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|f| f["path"] == d.path)
                    .unwrap();
                let data = base64::engine::general_purpose::STANDARD
                    .decode(f["bytes_base64"].as_str().unwrap())
                    .unwrap();
                assert_eq!(d.size_bytes, data.len() as u64);
                assert_eq!(d.sha256, hex(&sha(&data)));
            }
        }
        assert_eq!(seen, 5);
    }
    #[test]
    fn test_frozen_path_vectors() {
        let rows = fixture()["vectors"].as_array().unwrap().clone();
        let mut seen = 0;
        for row in rows.iter().filter(|r| r["kind"] == "paths") {
            seen += 1;
            assert_eq!(
                validate_paths(&entries(row)).unwrap_err().code,
                row["expected_error"].as_str().unwrap(),
                "{}",
                row["name"]
            );
        }
        assert_eq!(seen, 20);
    }
    #[test]
    fn test_order_exclusions_and_payload_changes() {
        let mut a = vec![
            PayloadEntry {
                path: "level.map".into(),
                data: b"abc".to_vec(),
                is_directory: false,
            },
            PayloadEntry {
                path: "preview.png".into(),
                data: vec![1],
                is_directory: false,
            },
            PayloadEntry {
                path: "icon.png".into(),
                data: vec![2],
                is_directory: false,
            },
            PayloadEntry {
                path: "ivl-manifest.json".into(),
                data: vec![3],
                is_directory: false,
            },
        ];
        let base = hash_payload(&a).unwrap();
        a.reverse();
        assert_eq!(hash_payload(&a).unwrap(), base);
        for name in ["preview.png", "icon.png", "ivl-manifest.json"] {
            a.iter_mut().find(|e| e.path == name).unwrap().data = vec![9];
        }
        assert_eq!(hash_payload(&a).unwrap(), base);
        a.iter_mut().find(|e| e.path == "level.map").unwrap().data = b"abd".to_vec();
        assert_ne!(hash_payload(&a).unwrap().content_hash, base.content_hash);
        let entry = a.iter_mut().find(|e| e.path == "level.map").unwrap();
        entry.data = b"abc".to_vec();
        entry.path = "level.vmf".into();
        assert_ne!(hash_payload(&a).unwrap().content_hash, base.content_hash);
        for path in ["textures/a.PNG", "textures/b.TGA"] {
            assert!(validate_paths(&[PayloadEntry {
                path: path.into(),
                data: vec![],
                is_directory: false
            }])
            .is_ok());
        }
    }
    #[test]
    fn test_directory_namespace_and_ownership() {
        let dir = |p: &str| PayloadEntry {
            path: p.into(),
            data: vec![],
            is_directory: true,
        };
        let file = |p: &str| PayloadEntry {
            path: p.into(),
            data: vec![1],
            is_directory: false,
        };
        assert!(validate_paths(&[dir("textures/"), file("textures/a.png")]).is_ok());
        assert_eq!(
            validate_paths(&[dir("textures"), dir("textures")])
                .unwrap_err()
                .code,
            "DUPLICATE_PATH"
        );
        assert_eq!(
            validate_paths(&[file("textures/A/a.png"), file("textures/a/b.png")])
                .unwrap_err()
                .code,
            "CASE_COLLISION"
        );
        assert_eq!(
            validate_paths(&[file("textures/a.png"), file("textures/a.png/b.png")])
                .unwrap_err()
                .code,
            "DUPLICATE_PATH"
        );
        assert_eq!(
            validate_paths(&[file("textures/a.png"), dir("textures/a.png")])
                .unwrap_err()
                .code,
            "DUPLICATE_PATH"
        );
        assert_eq!(
            validate_paths(&[dir("other")]).unwrap_err().code,
            "UNSUPPORTED_FILE"
        );
        let input = vec![file("level.map")];
        let snapshot = input.clone();
        let output = hash_payload(&input).unwrap();
        assert_eq!(input, snapshot);
        let mut changed = output.clone();
        changed.files.clear();
        changed.encoded_bytes.clear();
        assert_eq!(hash_payload(&input).unwrap(), output);
    }
}
