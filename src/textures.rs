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
) -> TextureResolution {
    let mut base = BTreeMap::new();
    let mut pbr = BTreeMap::new();
    let mut missing = Vec::new();

    for tex_name in referenced {
        let mut found = false;

        for ext in BASE_EXTENSIONS {
            let file_path = textures_dir.join(format!("{}.{}", tex_name, ext));
            if file_path.is_file() {
                base.insert(tex_name.clone(), file_path);
                found = true;
                break;
            }
        }

        if !found {
            missing.push(tex_name.clone());
            continue;
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

    TextureResolution { base, pbr, missing }
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
