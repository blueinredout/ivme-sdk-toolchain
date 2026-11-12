use crate::content_identity::{hash_payload, FileDescriptor, PayloadEntry};
use crate::error::CompileError;
use crate::preview::{IconData, PreviewData};
use crate::author_evidence::DecodedAuthorEvidence;
use crate::metadata::{validate_extended_metadata, LevelMetadata};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

pub struct PackageInput {
    pub map_path: PathBuf,
    /// Archive entry name: `level.map` or `level.vmf` (mirrors the source).
    pub map_entry_name: String,
    pub map_bytes: Vec<u8>,
    pub preview: Option<PreviewData>,
    pub icon: Option<IconData>,
    /// Basename -> source path (textures/ is flat in the archive).
    pub texture_files: BTreeMap<String, PathBuf>,
    /// Clean mapping.csv bytes as validated (None = TrenchBroom-only map).
    pub mapping_csv: Option<Vec<u8>>,
    pub level_uuid: String,
}

struct ArchiveFile { path: String, data: Vec<u8> }

pub struct PreparedArchive {
    files: Vec<ArchiveFile>,
}

pub fn prepare_archive(input: &PackageInput) -> Result<PreparedArchive, CompileError> {
    prepare_archive_inner(input, None)
}

/// Decode and prepare one explicitly supplied IVAE as an inert schema-2 attachment.
pub fn prepare_archive_with_author(input: &PackageInput, evidence_bytes: &[u8], source: &LevelMetadata) -> Result<PreparedArchive, CompileError> {
    if source.game_mode != "std" { return Err(CompileError::AuthorEvidence("evidence game mode does not match source metadata".into())); }
    let source_scope = validate_extended_metadata(source)?;
    let evidence = crate::author_evidence::decode(evidence_bytes).map_err(|e| CompileError::AuthorEvidence(e.to_string()))?;
    prepare_archive_inner(input, Some((evidence, source_scope)))
}

fn prepare_archive_inner(input: &PackageInput, author: Option<(DecodedAuthorEvidence, crate::metadata::ExtendedMetadata)>) -> Result<PreparedArchive, CompileError> {
    let mut files = vec![ArchiveFile { path: input.map_entry_name.clone(), data: input.map_bytes.clone() }];
    if let Some(csv) = &input.mapping_csv { files.push(ArchiveFile { path: "mapping.csv".into(), data: csv.clone() }); }
    for (name, source) in &input.texture_files {
        let data = fs::read(source).map_err(|e| CompileError::ZipWriteFailed(format!("Failed to read {}: {}", source.display(), e)))?;
        files.push(ArchiveFile { path: format!("textures/{name}"), data });
    }
    let preview = input.preview.as_ref().map(|p| crate::preview::encode_png(&p.normalized));
    let icon = input.icon.as_ref().map(|i| crate::preview::encode_png_size(&i.normalized, 128));
    let mut payload: Vec<PayloadEntry> = files.iter().map(|f| PayloadEntry { path: f.path.clone(), data: f.data.clone(), is_directory: false }).collect();
    let hash = hash_payload(&payload)?;
    let mut manifest = serde_json::json!({
        "ivl_schema_version": 1,
        "content_hash_version": 1,
        "level_uuid": input.level_uuid,
        "content_hash": hash.content_hash,
        "files": hash.files.iter().map(|f: &FileDescriptor| serde_json::json!({"path":f.path,"size_bytes":f.size_bytes,"sha256":f.sha256})).collect::<Vec<_>>(),
    });
    if let Some((evidence, source)) = author {
        let expected_targets = [source.bronze_time_ms, source.silver_time_ms, source.gold_time_ms];
        if evidence.level_uuid != source.level_uuid || input.level_uuid != source.level_uuid
            || evidence.content_hash_version != 1 || evidence.content_hash != hash.content_hash
            || evidence.targets != expected_targets {
            return Err(CompileError::AuthorEvidence("evidence identity or target snapshot does not match the prepared source payload".into()));
        }
        manifest["ivl_schema_version"] = 2.into();
        manifest["author_evidence"] = serde_json::json!({"path":"author-run.ivae", "size_bytes":evidence.bytes.len(), "sha256":hex(&evidence.sha256)});
        files.push(ArchiveFile { path: "author-run.ivae".into(), data: evidence.bytes });
    }
    let manifest_bytes = serde_json::to_vec(&manifest).map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
    if manifest["ivl_schema_version"] == 2 && manifest_bytes.len() > 1_048_576 {
        return Err(CompileError::AuthorEvidence("schema-2 manifest exceeds 1048576 bytes".into()));
    }
    files.extend(preview.map(|data| ArchiveFile { path: "preview.png".into(), data }));
    files.extend(icon.map(|data| ArchiveFile { path: "icon.png".into(), data }));
    files.push(ArchiveFile { path: "ivl-manifest.json".into(), data: manifest_bytes });
    payload.clear();
    Ok(PreparedArchive { files })
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

pub fn create_archive(output_path: &Path, prepared: &PreparedArchive) -> Result<u64, CompileError> {
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = output_path.file_name().unwrap_or_default().to_string_lossy();
    let (tmp_path, file) = loop {
        let candidate = parent.join(format!(".{}.{}.tmp", file_name, uuid::Uuid::new_v4()));
        match OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(CompileError::OutputWriteFailed(candidate, e)),
        }
    };
    let result = (|| {
        let mut zip = zip::ZipWriter::new(file);
        let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for item in &prepared.files {
            zip.start_file(&item.path, options).map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
            zip.write_all(&item.data).map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
        }
        let file = zip.finish().map_err(|e| CompileError::ZipWriteFailed(e.to_string()))?;
        file.sync_all().map_err(|e| CompileError::OutputWriteFailed(tmp_path.clone(), e))?;
        let size = file.metadata().map_err(|e| CompileError::OutputWriteFailed(tmp_path.clone(), e))?.len();
        drop(file);
        fs::rename(&tmp_path, output_path).map_err(|e| CompileError::OutputWriteFailed(output_path.to_path_buf(), e))?;
        Ok(size)
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp_path); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use image::{ImageBuffer, Rgb};
    use serde_json::Value;
    use std::{collections::BTreeMap, fs::File, io::Read};
    use zip::ZipArchive;

    const FIXTURE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/record-package-v1.json"));
    const V2_FIXTURE: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../contracts/record-package-v2.json"));
    const UUID: &str = "550e8400-e29b-41d4-a716-446655440000";
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("ivlc-package-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&p).unwrap(); Self(p)
        }
        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf { let p=self.0.join(name); fs::write(&p,bytes).unwrap(); p }
        fn assert_clean(&self, expected: &[&str]) {
            let mut got: Vec<_> = fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            got.sort(); let mut want: Vec<_> = expected.iter().map(|s| s.to_string()).collect(); want.sort(); assert_eq!(got,want);
        }
    }
    impl Drop for Temp { fn drop(&mut self) { let root=fs::canonicalize(&self.0).unwrap(); let parent=fs::canonicalize(std::env::temp_dir()).unwrap(); assert_eq!(root.parent(), Some(parent.as_path())); let name=root.file_name().unwrap().to_str().unwrap(); assert!(name.starts_with("ivlc-package-test-")); uuid::Uuid::parse_str(&name[18..]).unwrap(); fs::remove_dir_all(&root).unwrap(); } }
    fn case(name:&str)->Value { serde_json::from_str::<Value>(FIXTURE).unwrap()["cases"].as_array().unwrap().iter().find(|c|c["name"]==name).unwrap().clone() }
    fn payloads(c:&Value)->BTreeMap<String,Vec<u8>> { c["files"].as_array().unwrap().iter().map(|f|(f["path"].as_str().unwrap().into(),base64::engine::general_purpose::STANDARD.decode(f["bytes_base64"].as_str().unwrap()).unwrap())).collect() }
    fn input(t:&Temp,c:&Value)->PackageInput {
        let p=payloads(c); let map_path=t.file("source.map",&p[if p.contains_key("level.map"){"level.map"}else{"level.vmf"}]);
        let tex=p.iter().find(|(n,_)|n.starts_with("textures/")).map(|(n,b)| (n.rsplit('/').next().unwrap().to_string(),t.file("texture",b))).into_iter().collect();
        PackageInput { map_path, map_entry_name: if p.contains_key("level.vmf"){"level.vmf".into()}else{"level.map".into()}, map_bytes:p.iter().find(|(n,_)|n.starts_with("level.")).unwrap().1.clone(), preview:None, icon:None, texture_files:tex, mapping_csv:p.get("mapping.csv").cloned(), level_uuid:UUID.into() }
    }
    fn archive(path:&Path)->BTreeMap<String,Vec<u8>> { let mut z=ZipArchive::new(File::open(path).unwrap()).unwrap(); let mut m=BTreeMap::new(); for i in 0..z.len(){let mut f=z.by_index(i).unwrap();let mut b=vec![];f.read_to_end(&mut b).unwrap();assert!(m.insert(f.name().to_string(),b).is_none(), "duplicate ZIP entry");}m }
    fn check_manifest(m:&BTreeMap<String,Vec<u8>>, c:&Value) { let v:Value=serde_json::from_slice(&m["ivl-manifest.json"]).unwrap(); assert_eq!(v.as_object().unwrap().len(),5); assert_eq!(v["ivl_schema_version"],1);assert_eq!(v["content_hash_version"],1);assert_eq!(v["level_uuid"],UUID);assert_eq!(v["content_hash"],c["content_hash"]);assert_eq!(v["files"],c["inventory"]); for f in v["files"].as_array().unwrap(){assert_eq!(f.as_object().unwrap().len(),3);} }
    fn emit(t:&Temp,c:&Value)->BTreeMap<String,Vec<u8>> { let i=input(t,c);let p=prepare_archive(&i).unwrap();let out=t.0.join("out.ivl");let size=create_archive(&out,&p).unwrap();assert_eq!(size,fs::metadata(&out).unwrap().len());archive(&out) }

    #[test] fn test_package_manifest_matches_frozen_payloads(){for n in ["map_only","vmf_only","full"]{let t=Temp::new();let c=case(n);let expected=payloads(&c);let m=emit(&t,&c);for (p,b) in expected{assert_eq!(m[&p],b);}assert_eq!(m.len(),payloads(&c).len()+1);check_manifest(&m,&c); let source=payloads(&c).into_iter().find(|(p,_)|p.starts_with("level.")).unwrap().1; assert_eq!(fs::read(t.0.join("source.map")).unwrap(),source);}}
    #[test] fn test_package_preparation_freezes_payload_bytes(){let t=Temp::new();let c=case("full");let mut i=input(&t,&c);let tex=i.texture_files.values().next().unwrap().clone();let p=prepare_archive(&i).unwrap();fs::write(&tex,b"changed").unwrap();fs::remove_file(&tex).unwrap();fs::write(&i.map_path,b"changed map").unwrap();i.map_bytes=b"changed buffer".to_vec();i.mapping_csv=Some(b"changed csv".to_vec());let out=t.0.join("out.ivl");create_archive(&out,&p).unwrap();let m=archive(&out);for (n,b) in payloads(&c){assert_eq!(m[&n],b);}check_manifest(&m,&c);}
    #[test] fn test_package_listing_images_do_not_change_identity(){let t=Temp::new();let c=case("full");let mut a=input(&t,&c);a.preview=Some(PreviewData{path:"p".into(),normalized:ImageBuffer::from_pixel(512,512,Rgb([1,2,3]))});a.icon=Some(IconData{path:"i".into(),normalized:ImageBuffer::from_pixel(128,128,Rgb([4,5,6]))});let p=prepare_archive(&a).unwrap();let o=t.0.join("a.ivl");create_archive(&o,&p).unwrap();let one=archive(&o);a.preview.as_mut().unwrap().normalized=ImageBuffer::from_pixel(512,512,Rgb([8,9,10]));a.icon.as_mut().unwrap().normalized=ImageBuffer::from_pixel(128,128,Rgb([11,12,13]));let p=prepare_archive(&a).unwrap();create_archive(&o,&p).unwrap();let two=archive(&o);assert_ne!(one["preview.png"],two["preview.png"]);assert_ne!(one["icon.png"],two["icon.png"]);for key in ["preview.png","icon.png"]{image::load_from_memory(&two[key]).unwrap();}check_manifest(&two,&c);assert_eq!(one["ivl-manifest.json"],two["ivl-manifest.json"]);for n in ["full_changed","full_renamed"]{let x=case(n);let m=emit(&t,&x);check_manifest(&m,&x);assert_ne!(x["content_hash"],c["content_hash"]);} assert_ne!(case("full_changed")["content_hash"],case("full_renamed")["content_hash"]);}
    #[test] fn test_package_failures_preserve_existing_output(){let t=Temp::new();let c=case("full");let i=input(&t,&c);let p=prepare_archive(&i).unwrap();let out=t.0.join("output.ivl");create_archive(&out,&p).unwrap();let original=fs::read(&out).unwrap();fs::write(t.0.join("output.ivl.tmp"),b"sentinel").unwrap();let mut bad=input(&t,&c);bad.texture_files.insert("test.PNG".into(),t.0.join("missing"));assert!(prepare_archive(&bad).is_err());assert_eq!(fs::read(&out).unwrap(),original);let mut bad=input(&t,&c);bad.texture_files.insert("../escape.PNG".into(),bad.texture_files.values().next().unwrap().clone());assert!(prepare_archive(&bad).is_err());assert_eq!(fs::read(&out).unwrap(),original);let dest=t.0.join("destination");fs::create_dir(&dest).unwrap();fs::write(dest.join("marker"),b"kept").unwrap();assert!(create_archive(&dest,&p).is_err());assert_eq!(fs::read(dest.join("marker")).unwrap(),b"kept");assert_eq!(fs::read(t.0.join("output.ivl.tmp")).unwrap(),b"sentinel");
        t.assert_clean(&["source.map","texture","output.ivl","output.ivl.tmp","destination"]);
        #[cfg(windows)] {
            use std::os::windows::fs::OpenOptionsExt;
            let lock=OpenOptions::new().read(true).share_mode(0).open(&out).unwrap();
            assert!(create_archive(&out,&p).is_err());
            drop(lock);
            assert_eq!(fs::read(&out).unwrap(),original);
            assert_eq!(fs::read(t.0.join("output.ivl.tmp")).unwrap(),b"sentinel");
            t.assert_clean(&["source.map","texture","output.ivl","output.ivl.tmp","destination"]);
        }
    }

    #[test]
    fn test_author_schema2_matches_independent_golden_and_default_stays_v1() {
        use sha2::{Digest, Sha256};
        let fixture: Value = serde_json::from_str(V2_FIXTURE).unwrap();
        let golden = base64::engine::general_purpose::STANDARD.decode(fixture["archive_base64"].as_str().unwrap()).unwrap();
        let mut golden_zip = ZipArchive::new(std::io::Cursor::new(golden)).unwrap();
        let read_entry = |zip: &mut ZipArchive<std::io::Cursor<Vec<u8>>>, name: &str| { let mut file=zip.by_name(name).unwrap(); let mut bytes=Vec::new(); file.read_to_end(&mut bytes).unwrap(); bytes };
        let source = read_entry(&mut golden_zip, "level.map");
        let evidence_bytes = read_entry(&mut golden_zip, "author-run.ivae");
        let expected_manifest_bytes = read_entry(&mut golden_zip, "ivl-manifest.json");
        let temp = Temp::new();
        let input = PackageInput { map_path:temp.file("source.map", &source), map_entry_name:"level.map".into(), map_bytes:source, preview:None, icon:None, texture_files:BTreeMap::new(), mapping_csv:None, level_uuid:"550e8400-e29b-41d4-a716-446655440000".into() };
        let source_metadata = LevelMetadata { course_id:"golden".into(), level_name:"Golden".into(), level_creator:"Tester".into(), level_description:String::new(), game_mode:"std".into(), checkpoints:0, metadata_schema_version:Some("1".into()), level_uuid:Some(input.level_uuid.clone()), bronze_time_ms:Some("60000".into()), silver_time_ms:Some("45000".into()), gold_time_ms:Some("30000".into()) };
        let v2 = prepare_archive_with_author(&input, &evidence_bytes, &source_metadata).unwrap();
        let out = temp.0.join("v2.ivl"); create_archive(&out, &v2).unwrap();
        let entries = archive(&out);
        assert_eq!(entries["level.map"], input.map_bytes);
        assert_eq!(entries["author-run.ivae"], evidence_bytes);
        let manifest_bytes = &entries["ivl-manifest.json"];
        let manifest: Value = serde_json::from_slice(manifest_bytes).unwrap();
        assert_eq!(*manifest_bytes, expected_manifest_bytes);
        assert_eq!(manifest["files"][0]["sha256"], format!("{:x}", Sha256::digest(&input.map_bytes)));
        assert_eq!(manifest["author_evidence"]["sha256"], format!("{:x}", Sha256::digest(&evidence_bytes)));
        let v1 = prepare_archive(&input).unwrap();
        let out_v1 = temp.0.join("v1.ivl"); create_archive(&out_v1, &v1).unwrap();
        let v1_entries = archive(&out_v1);
        assert!(!v1_entries.contains_key("author-run.ivae"));
        assert_eq!(serde_json::from_slice::<Value>(&v1_entries["ivl-manifest.json"]).unwrap()["ivl_schema_version"], 1);
    }
}
