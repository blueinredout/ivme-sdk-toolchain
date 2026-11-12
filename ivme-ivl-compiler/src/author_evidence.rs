//! Strict, inert decoder for the frozen local IVAE v1 evidence container.
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::fmt;
use uuid::Uuid;
use crate::metadata::{validate_extended_metadata, LevelMetadata};

const MAX_FILE: usize = 22_000_000;
const MAX_METADATA: usize = 65_536;
const MAX_STEPS: u64 = 257_698_037;
const MAX_TIME_MS: u64 = 2_147_483_647;
const IDENTITY_KEYS: &[&str] = &["schema_version", "provenance", "level_uuid", "release_id", "content_hash_version", "content_hash", "ruleset_id", "game_mode", "targets"];
const TARGET_KEYS: &[&str] = &["metadata_schema_version", "source_metadata_schema_version", "level_uuid", "bronze_time_ms", "silver_time_ms", "gold_time_ms", "metadata_valid", "medal_state", "medals_available", "diagnostics", "author_time_ms", "author_provenance"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodeError { message: &'static str }
impl fmt::Display for DecodeError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(self.message) } }
impl std::error::Error for DecodeError {}
fn err(message: &'static str) -> DecodeError { DecodeError { message } }

/// Bounded decoded data for a future package writer. Samples remain packed in `bytes`.
#[derive(Debug)]
pub struct DecodedAuthorEvidence {
    pub bytes: Vec<u8>,
    pub sha256: [u8; 32],
    pub metadata: Value,
    pub level_uuid: String,
    pub content_hash: String,
    pub content_hash_version: u64,
    pub ruleset_id: String,
    pub game_mode: String,
    pub profile_id: String,
    pub run_id: String,
    pub simulation_target_id: String,
    pub ruleset_fingerprint: String,
    pub targets: [u32; 3], // bronze, silver, gold
    pub total_steps: u32,
    pub reset_ticks: Vec<u32>,
    pub provisional_time_ms: u32,
    sample_range: std::ops::Range<usize>,
}
impl DecodedAuthorEvidence {
    pub fn samples(&self) -> &[u8] { &self.bytes[self.sample_range.clone()] }
}

pub fn decode(input: &[u8]) -> Result<DecodedAuthorEvidence, DecodeError> {
    if input.len() < 9 || input.len() > MAX_FILE { return Err(err("evidence size is outside supported bounds")); }
    if &input[..4] != b"IVAE" { return Err(err("invalid IVAE magic")); }
    if input[4] != 1 { return Err(err("unsupported IVAE version")); }
    let metadata_len = u32::from_le_bytes([input[5], input[6], input[7], input[8]]) as usize;
    if metadata_len > MAX_METADATA || metadata_len > input.len() - 9 { return Err(err("metadata length is outside bounds")); }
    let metadata_bytes = &input[9..9 + metadata_len];
    let source = std::str::from_utf8(metadata_bytes).map_err(|_| err("metadata is not UTF-8"))?;
    let metadata: Value = serde_json::from_str(source).map_err(|_| err("metadata JSON is invalid"))?;
    let root = metadata.as_object().ok_or_else(|| err("metadata must be an object"))?;
    exact_keys(root, &["schema_version", "timeline_contract_id", "simulation_contract_id", "simulation_target_id", "ruleset_fingerprint", "profile_id", "run_id", "identity", "total_steps", "reset_count"])?;
    if uint(&root["schema_version"])? != 1 || root["timeline_contract_id"] != "ivme-author-timeline-v1" || root["simulation_contract_id"] != "ivme-author-sim-v1" { return Err(err("unsupported or invalid metadata contract")); }
    let target_id = string(&root["simulation_target_id"])?;
    if target_id.is_empty() || target_id.trim() != target_id || target_id.len() > 128 || !target_id.bytes().all(|b| (32..=126).contains(&b)) { return Err(err("invalid simulation target identifier")); }
    let fingerprint = string(&root["ruleset_fingerprint"])?;
    lower_hash(fingerprint)?;
    let profile = string(&root["profile_id"])?;
    valid_profile(profile)?;
    let run_id = string(&root["run_id"])?;
    canonical_uuid(run_id)?;
    let identity = root["identity"].as_object().ok_or_else(|| err("identity must be an object"))?;
    exact_keys(identity, IDENTITY_KEYS)?;
    if uint(&identity["schema_version"])? != 1 || uint(&identity["content_hash_version"])? != 1 || identity["provenance"] != "local" || identity["release_id"] != "" || identity["game_mode"] != "std" { return Err(err("identity scope is invalid")); }
    let level_uuid = string(&identity["level_uuid"])?;
    canonical_uuid(level_uuid)?;
    let content_hash = string(&identity["content_hash"])?;
    lower_hash(content_hash)?;
    let ruleset_id = string(&identity["ruleset_id"])?;
    if ruleset_id.is_empty() || ruleset_id.trim() != ruleset_id { return Err(err("ruleset ID is invalid")); }
    let targets_obj = identity["targets"].as_object().ok_or_else(|| err("targets must be an object"))?;
    exact_keys(targets_obj, TARGET_KEYS)?;
    if uint(&targets_obj["metadata_schema_version"])? != 1 || int(&targets_obj["source_metadata_schema_version"])? != 1 || targets_obj["level_uuid"] != level_uuid || targets_obj["metadata_valid"] != true || targets_obj["medal_state"] == "invalid" || targets_obj["diagnostics"].as_array().map_or(true, |a| !a.is_empty()) || !targets_obj["author_time_ms"].is_null() || targets_obj["author_provenance"] != "unavailable" { return Err(err("source target snapshot is invalid")); }
    let mut target_times = [0u32; 3];
    for (i, key) in ["bronze_time_ms", "silver_time_ms", "gold_time_ms"].iter().enumerate() {
        let n = uint(&targets_obj[*key])?;
        if n > 2_147_483_647 { return Err(err("target time is out of range")); }
        target_times[i] = n as u32;
    }
    let normalized_targets = validate_extended_metadata(&LevelMetadata {
        course_id: "author-evidence".into(), level_name: String::new(), level_creator: String::new(), level_description: String::new(), game_mode: "std".into(), checkpoints: 0,
        metadata_schema_version: Some("1".into()), level_uuid: Some(level_uuid.to_owned()),
        bronze_time_ms: Some(target_times[0].to_string()), silver_time_ms: Some(target_times[1].to_string()), gold_time_ms: Some(target_times[2].to_string()),
    }).map_err(|_| err("source target snapshot is invalid"))?;
    let state = &targets_obj["medal_state"];
    let available = targets_obj["medals_available"].as_bool().ok_or_else(|| err("medals_available must be boolean"))?;
    if !((state == "configured" && normalized_targets.medal_configured && available) || (state == "unconfigured" && !normalized_targets.medal_configured && !available)) { return Err(err("target state is incoherent")); }

    let steps = uint(&root["total_steps"])?;
    let reset_count = uint(&root["reset_count"])?;
    if steps == 0 || steps > MAX_STEPS || reset_count > steps { return Err(err("step or reset count is out of range")); }
    let milliseconds = steps.checked_mul(1000).and_then(|n| n.checked_add(119)).ok_or_else(|| err("duration overflow"))? / 120;
    if milliseconds > MAX_TIME_MS { return Err(err("duration exceeds timeline bound")); }
    let samples_len = steps.checked_mul(17).ok_or_else(|| err("sample length overflow"))?;
    let resets_len = reset_count.checked_mul(4).ok_or_else(|| err("reset length overflow"))?;
    let sample_start = 9usize.checked_add(metadata_len).ok_or_else(|| err("length overflow"))?;
    let sample_end = sample_start.checked_add(usize::try_from(samples_len).map_err(|_| err("sample length overflow"))?).ok_or_else(|| err("length overflow"))?;
    let expected = sample_end.checked_add(usize::try_from(resets_len).map_err(|_| err("reset length overflow"))?).ok_or_else(|| err("length overflow"))?;
    if expected != input.len() || expected > MAX_FILE { return Err(err("evidence length does not match declared counts")); }
    for sample in input[sample_start..sample_end].chunks_exact(17) {
        let yaw = f64::from_le_bytes(sample[1..9].try_into().map_err(|_| err("invalid sample"))?);
        let pitch = f64::from_le_bytes(sample[9..17].try_into().map_err(|_| err("invalid sample"))?);
        if !yaw.is_finite() || !(-180.0..180.0).contains(&yaw) || !pitch.is_finite() || !(-89.0..=89.0).contains(&pitch) { return Err(err("sample angles are out of range")); }
    }
    let mut reset_ticks = Vec::with_capacity(reset_count as usize);
    let mut previous = None;
    for chunk in input[sample_end..].chunks_exact(4) {
        let tick = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if tick as u64 >= steps || previous.is_some_and(|p| tick <= p) { return Err(err("reset ticks are invalid")); }
        reset_ticks.push(tick);
        previous = Some(tick);
    }
    // Godot JSON.stringify(normalized, "", true, true) recursively sorts keys.
    // serde_json's Map is ordered; exact comparison also rejects duplicate keys,
    // whitespace, BOMs, alternate number spellings, and noncanonical escapes.
    let canonical = serde_json::to_vec(&metadata).map_err(|_| err("metadata cannot be canonicalized"))?;
    if canonical != metadata_bytes { return Err(err("metadata JSON is not canonical")); }

    let bytes = input.to_vec();
    let sha256 = Sha256::digest(&bytes).into();
    Ok(DecodedAuthorEvidence { bytes, sha256, metadata: metadata.clone(), level_uuid: level_uuid.to_owned(), content_hash: content_hash.to_owned(), content_hash_version: 1, ruleset_id: ruleset_id.to_owned(), game_mode: "std".into(), profile_id: profile.to_owned(), run_id: run_id.to_owned(), simulation_target_id: target_id.to_owned(), ruleset_fingerprint: fingerprint.to_owned(), targets: target_times, total_steps: steps as u32, reset_ticks, provisional_time_ms: milliseconds as u32, sample_range: sample_start..sample_end })
}

fn exact_keys(map: &Map<String, Value>, keys: &[&str]) -> Result<(), DecodeError> {
    if map.len() != keys.len() || keys.iter().any(|key| !map.contains_key(*key)) { return Err(err("object fields do not match the contract")); }
    Ok(())
}
fn string(v: &Value) -> Result<&str, DecodeError> { v.as_str().ok_or_else(|| err("expected string")) }
fn uint(v: &Value) -> Result<u64, DecodeError> { v.as_u64().ok_or_else(|| err("expected canonical nonnegative integer")) }
fn int(v: &Value) -> Result<i64, DecodeError> { v.as_i64().ok_or_else(|| err("expected canonical integer")) }
fn lower_hash(v: &str) -> Result<(), DecodeError> { if v.len() == 64 && v.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) { Ok(()) } else { Err(err("expected lowercase SHA-256")) } }
fn valid_profile(v: &str) -> Result<(), DecodeError> {
    if v.is_empty() || v.len() > 20 || v.starts_with('0') || !v.bytes().all(|b| b.is_ascii_digit()) || (v.len() == 20 && v > "18446744073709551615") { Err(err("profile ID is invalid")) } else { Ok(()) }
}
fn canonical_uuid(v: &str) -> Result<(), DecodeError> {
    let parsed = Uuid::parse_str(v).map_err(|_| err("UUID is invalid"))?;
    if parsed.hyphenated().to_string() != v { return Err(err("UUID is not canonical lowercase hyphenated form")); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::Value;
    fn vector() -> Value { serde_json::from_str(include_str!("../../contracts/author-evidence-v1-vector.json")).expect("frozen vector JSON") }
    fn golden() -> Vec<u8> { base64::engine::general_purpose::STANDARD.decode(vector()["evidence_base64"].as_str().expect("fixture bytes")).expect("fixture base64") }

    #[test]
    fn frozen_shared_vector_decodes_exact_scope_and_samples() {
        let value = vector();
        let bytes = golden();
        let decoded = decode(&bytes).expect("golden IVAE");
        assert_eq!(format!("{:x}", Sha256::digest(&bytes)), value["sha256"].as_str().unwrap());
        assert_eq!(decoded.provisional_time_ms, value["time_ms"].as_u64().unwrap() as u32);
        assert_eq!(decoded.total_steps, 2);
        assert_eq!(decoded.reset_ticks, vec![0]);
        assert_eq!(decoded.samples(), &hex_bytes(value["tick_data_hex"].as_str().unwrap()));
        assert_eq!(decoded.level_uuid, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(decoded.content_hash, "a".repeat(64));
        assert_eq!(decoded.targets, [60_000, 45_000, 30_000]);
    }

    #[test]
    fn representative_structural_corruptions_fail_closed() {
        let original = golden();
        let mut truncated = original.clone(); truncated.pop();
        assert!(decode(&truncated).is_err());
        let mut trailing = original.clone(); trailing.push(0);
        assert!(decode(&trailing).is_err());
        let meta_len = u32::from_le_bytes(original[5..9].try_into().unwrap()) as usize;
        let metadata = std::str::from_utf8(&original[9..9 + meta_len]).unwrap();
        let mut angle = original.clone(); let yaw = 9 + meta_len + 1; angle[yaw..yaw + 8].copy_from_slice(&180.0f64.to_le_bytes());
        assert!(decode(&angle).is_err());
        let alternate = replace_metadata(&original, &metadata.replace("\"total_steps\":2", "\"total_steps\":2.0"));
        assert!(decode(&alternate).is_err());
        let duplicate = replace_metadata(&original, &metadata.replacen("{", "{\"schema_version\":1,", 1));
        assert!(decode(&duplicate).is_err());
        let mut unconfigured: Value = serde_json::from_str(metadata).unwrap();
        let targets = &mut unconfigured["identity"]["targets"];
        for key in ["bronze_time_ms", "silver_time_ms", "gold_time_ms"] { targets[key] = 0.into(); }
        targets["medal_state"] = "unconfigured".into(); targets["medals_available"] = false.into();
        assert!(decode(&replace_metadata(&original, &serde_json::to_string(&unconfigured).unwrap())).is_ok());
        unconfigured["identity"]["targets"]["medals_available"] = Value::Null;
        assert!(decode(&replace_metadata(&original, &serde_json::to_string(&unconfigured).unwrap())).is_err());
        let changed = metadata.replace("\"reset_count\":1", "\"reset_count\":2");
        let mut out_of_order = replace_metadata(&original, &changed);
        out_of_order.truncate(9 + changed.len() + 2 * 17);
        out_of_order.extend(1u32.to_le_bytes()); out_of_order.extend(0u32.to_le_bytes());
        assert!(decode(&out_of_order).is_err());
    }
    fn replace_metadata(original: &[u8], text: &str) -> Vec<u8> {
        let old_len = u32::from_le_bytes(original[5..9].try_into().unwrap()) as usize;
        let mut output = original[..9].to_vec();
        output[5..9].copy_from_slice(&(text.len() as u32).to_le_bytes());
        output.extend(text.as_bytes()); output.extend_from_slice(&original[9 + old_len..]);
        output
    }

    fn hex_bytes(s: &str) -> Vec<u8> { s.as_bytes().chunks_exact(2).filter_map(|p| std::str::from_utf8(p).ok().and_then(|x| u8::from_str_radix(x, 16).ok())).collect() }
}
