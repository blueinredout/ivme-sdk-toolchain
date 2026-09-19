use crate::error::CompileError;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const BASE_EXTENSIONS: &[&str] = &["png", "tga", "jpg", "jpeg", "bmp", "webp"];
const PBR_SUFFIXES: &[&str] = &["_normal", "_roughness", "_metallic", "_ao", "_emission", "_height", "_orm"];

#[derive(Debug)]
pub struct TextureResolution {
    pub base: BTreeMap<String, PathBuf>,
    pub pbr: BTreeMap<String, Vec<PathBuf>>,
    pub missing: Vec<String>,
}

pub fn resolve_textures(
    textures_dir: &Path,
    referenced: &BTreeSet<String>,
) -> Result<TextureResolution, CompileError> {
    let mut base = BTreeMap::new();
    let mut pbr = BTreeMap::new();
    let mut missing = Vec::new();

    for tex_name in referenced {
        let mut found = false;
        let mut searched = Vec::new();

        for ext in BASE_EXTENSIONS {
            let file_path = textures_dir.join(format!("{}.{}", tex_name, ext));
            searched.push(file_path.clone());
            if file_path.is_file() {
                base.insert(tex_name.clone(), file_path);
                found = true;
                break;
            }
        }

        if !found {
            missing.push(tex_name.clone());
            // Still report missing texture with search paths
            let existing = list_existing_files(textures_dir, tex_name);
            return Err(CompileError::MissingTexture {
                name: tex_name.clone(),
                searched,
                existing,
            });
        }

        // Resolve PBR variants
        let mut pbr_variants = Vec::new();
        for suffix in PBR_SUFFIXES {
            let pbr_name = format!("{}{}", tex_name, suffix);
            for ext in BASE_EXTENSIONS {
                let file_path = textures_dir.join(format!("{}.{}", pbr_name, ext));
                if file_path.is_file() {
                    pbr_variants.push(file_path);
                    break;
                }
            }
        }
        if !pbr_variants.is_empty() {
            pbr.insert(tex_name.clone(), pbr_variants);
        }
    }

    Ok(TextureResolution { base, pbr, missing })
}

fn list_existing_files(textures_dir: &Path, tex_name: &str) -> Vec<String> {
    let mut results = Vec::new();
    if let Some(parent_name) = tex_name.rsplit('/').last() {
        if let Some(parent_dir) = tex_name.rsplit_once('/') {
            let dir = textures_dir.join(parent_dir.0);
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.starts_with(parent_name) {
                            results.push(name.to_string());
                        }
                    }
                }
            }
        } else {
            let dir = if tex_name.contains('/') {
                textures_dir.join(tex_name.rsplit_once('/').unwrap().0)
            } else {
                textures_dir.to_path_buf()
            };
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if let Some(name) = entry.file_name().to_str() {
                        if name.starts_with(parent_name) {
                            results.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    results
}

pub fn find_orphans(
    textures_dir: &Path,
    included: &BTreeSet<String>,
) -> Vec<PathBuf> {
    let mut orphans = Vec::new();
    if !textures_dir.is_dir() {
        return orphans;
    }
    collect_files(textures_dir, textures_dir, included, &mut orphans);
    orphans
}

fn collect_files(base: &Path, dir: &Path, included: &BTreeSet<String>, orphans: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_files(base, &path, included, orphans);
            } else if let Some(rel) = path.strip_prefix(base).ok().and_then(|r| r.to_str()) {
                let normalized = rel.replace('\\', "/");
                if !included.contains(&normalized) {
                    orphans.push(path);
                }
            }
        }
    }
}
