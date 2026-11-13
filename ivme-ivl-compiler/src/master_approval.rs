//! Typed verification of frozen master approval claims. This verifies signed claims
//! only and installs no trust or publication authority.

use crate::{author_release, signed_envelope};
use serde_json::{Number, Value};
use std::collections::BTreeMap;
use rsa::{pkcs8::{DecodePublicKey, EncodePublicKey}, traits::PublicKeyParts, RsaPublicKey};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const FIELDS: [&str; 22] = [
    "schema_version", "app_id", "approved_at_unix", "author_key_id",
    "author_release_sha256", "content_hash", "content_hash_version", "level_uuid",
    "package_sha256", "package_size", "previous_revision_id", "publisher_profile_id",
    "revision_id", "ruleset_fingerprint", "ruleset_id", "simulation_contract_id",
    "simulation_target_id", "submission_id", "timeline_contract_id",
    "verified_author_time_ms", "verifier_manifest_sha256", "workshop_item_id",
];
const MAX_ENVELOPE: usize = 44 + 65_536 + 384;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterApprovalError { InvalidEnvelope, InvalidClaims }

/// Validated, owned claims and exact canonical body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedMasterApproval { pub claims: Value, pub canonical_body: Vec<u8> }

/// Authenticate kind-2 claims against a separately trusted master key and caller context.
pub fn verify(bytes: &[u8], trusted_master_pem: &str, expected_app: u32,
              expected_item: &str, expected_revision: &str)
    -> Result<VerifiedMasterApproval, MasterApprovalError> {
    if bytes.len() > MAX_ENVELOPE || expected_app == 0 || !steam_id(expected_item)
        || !canonical_nonzero_uuid(expected_revision) {
        return Err(MasterApprovalError::InvalidClaims);
    }
    let selected_pem = select_trusted_pem(bytes, trusted_master_pem)?;
    let env = signed_envelope::verify(bytes, selected_pem, 2)
        .map_err(|_| MasterApprovalError::InvalidEnvelope)?;
    let claims = validate_body(&env.body)?;
    if claims["app_id"].as_u64() != Some(expected_app as u64)
        || claims["workshop_item_id"].as_str() != Some(expected_item)
        || claims["revision_id"].as_str() != Some(expected_revision)
        || claims["author_key_id"].as_str() == Some(hex(&env.key_id).as_str()) {
        return Err(MasterApprovalError::InvalidClaims);
    }
    Ok(VerifiedMasterApproval { claims, canonical_body: env.body })
}

/// Resolve a signed envelope only through the caller-installed bounded public PEM bundle.
fn select_trusted_pem<'a>(bytes: &[u8], bundle: &'a str) -> Result<&'a str, MasterApprovalError> {
    const MAX_KEYS: usize = 4;
    const MAX_PEM: usize = 2048;
    if bytes.len() < 44 || bundle.is_empty() || bundle.len() > MAX_KEYS * MAX_PEM || !bundle.is_ascii() {
        return Err(MasterApprovalError::InvalidEnvelope);
    }
    let mut rest = bundle.trim();
    let mut selected = None;
    let mut seen = Vec::<[u8; 32]>::new();
    while !rest.is_empty() {
        if seen.len() == MAX_KEYS || !rest.starts_with("-----BEGIN PUBLIC KEY-----") {
            return Err(MasterApprovalError::InvalidEnvelope);
        }
        let end_marker = "-----END PUBLIC KEY-----";
        let end = rest.find(end_marker).ok_or(MasterApprovalError::InvalidEnvelope)? + end_marker.len();
        let pem = &rest[..end];
        if pem.len() > MAX_PEM { return Err(MasterApprovalError::InvalidEnvelope); }
        let key = RsaPublicKey::from_public_key_pem(pem).map_err(|_| MasterApprovalError::InvalidEnvelope)?;
        if key.n().bits() != 3072 { return Err(MasterApprovalError::InvalidEnvelope); }
        let der = key.to_public_key_der().map_err(|_| MasterApprovalError::InvalidEnvelope)?;
        let key_id: [u8; 32] = Sha256::digest(der.as_bytes()).into();
        if seen.contains(&key_id) { return Err(MasterApprovalError::InvalidEnvelope); }
        if bytes[12..44] == key_id { selected = Some(pem); }
        seen.push(key_id);
        rest = rest[end..].trim();
    }
    selected.ok_or(MasterApprovalError::InvalidEnvelope)
}

/// Authenticate the approval, then the exact author envelope under its endorsed key.
pub fn verify_pair(master_bytes: &[u8], trusted_master_pem: &str,
                   author_bytes: &[u8], trusted_author_pem: &str,
                   expected_app: u32, expected_item: &str, expected_revision: &str)
    -> Result<(VerifiedMasterApproval, author_release::VerifiedAuthorRelease), MasterApprovalError> {
    if master_bytes.len() > MAX_ENVELOPE || author_bytes.len() > MAX_ENVELOPE {
        return Err(MasterApprovalError::InvalidEnvelope);
    }
    let master = verify(master_bytes, trusted_master_pem, expected_app, expected_item,
                        expected_revision)?;
    let author = author_release::verify(author_bytes, trusted_author_pem,
        master.claims["app_id"].as_u64().unwrap() as u32,
        master.claims["publisher_profile_id"].as_str().unwrap(),
        master.claims["workshop_item_id"].as_str().unwrap())
        .map_err(|_| MasterApprovalError::InvalidClaims)?;
    if master.claims["author_release_sha256"].as_str() != Some(author.release_sha256.as_str())
        || master.claims["previous_revision_id"] != author.claims["expected_previous_revision_id"]
        || master.claims["verified_author_time_ms"] != author.claims["claimed_author_time_ms"]
        || ["app_id", "submission_id", "workshop_item_id", "publisher_profile_id",
            "author_key_id", "package_sha256", "package_size", "content_hash_version",
            "content_hash", "level_uuid", "ruleset_id", "ruleset_fingerprint",
            "simulation_target_id", "simulation_contract_id", "timeline_contract_id"]
            .iter().any(|field| master.claims[*field] != author.claims[*field]) {
        return Err(MasterApprovalError::InvalidClaims);
    }
    Ok((master, author))
}

/// Validate and encode the canonical compact ordinal-key body.
pub fn canonical_body(claims: &Value) -> Result<Vec<u8>, MasterApprovalError> {
    let sorted = validate_value(claims)?;
    serde_json::to_vec(&sorted).map_err(|_| MasterApprovalError::InvalidClaims)
}
fn validate_body(body: &[u8]) -> Result<Value, MasterApprovalError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| MasterApprovalError::InvalidClaims)?;
    if canonical_body(&value)? != body { return Err(MasterApprovalError::InvalidClaims); }
    Ok(value)
}
fn validate_value(value: &Value) -> Result<BTreeMap<String, Value>, MasterApprovalError> {
    let object = value.as_object().ok_or(MasterApprovalError::InvalidClaims)?;
    if object.len() != FIELDS.len() || FIELDS.iter().any(|f| !object.contains_key(*f)) {
        return Err(MasterApprovalError::InvalidClaims);
    }
    let mut out = BTreeMap::new();
    for field in FIELDS {
        let v = &object[field];
        let normalized = match field {
            "schema_version" | "content_hash_version" => Value::Number(Number::from(uint(v, 1, 1)?)),
            "app_id" => Value::Number(Number::from(uint(v, 1, u32::MAX as u64)?)),
            "package_size" => Value::Number(Number::from(uint(v, 1, 67_108_864)?)),
            "verified_author_time_ms" => Value::Number(Number::from(uint(v, 1, i32::MAX as u64)?)),
            "approved_at_unix" => Value::Number(Number::from(uint(v, 1, 9_007_199_254_740_991)?)),
            _ => {
                let s = v.as_str().ok_or(MasterApprovalError::InvalidClaims)?;
                if !s.is_ascii() { return Err(MasterApprovalError::InvalidClaims); }
                match field {
                    "revision_id" | "submission_id" | "level_uuid" => if !canonical_nonzero_uuid(s) { return Err(MasterApprovalError::InvalidClaims); },
                    "previous_revision_id" => if !s.is_empty() && (!canonical_nonzero_uuid(s) || s == object["revision_id"].as_str().unwrap_or("")) { return Err(MasterApprovalError::InvalidClaims); },
                    "workshop_item_id" | "publisher_profile_id" => if !steam_id(s) { return Err(MasterApprovalError::InvalidClaims); },
                    "author_key_id" | "author_release_sha256" | "package_sha256" | "content_hash" | "ruleset_fingerprint" | "verifier_manifest_sha256" => if !lower_hex_32(s) { return Err(MasterApprovalError::InvalidClaims); },
                    "ruleset_id" | "simulation_target_id" => if s.is_empty() || s.trim() != s || s.len() > 128 || !s.bytes().all(|b| (32..=126).contains(&b)) { return Err(MasterApprovalError::InvalidClaims); },
                    "simulation_contract_id" => if s != "ivme-author-sim-v1" { return Err(MasterApprovalError::InvalidClaims); },
                    "timeline_contract_id" => if s != "ivme-author-timeline-v1" { return Err(MasterApprovalError::InvalidClaims); },
                    _ => return Err(MasterApprovalError::InvalidClaims),
                }
                Value::String(s.to_owned())
            }
        };
        out.insert(field.to_owned(), normalized);
    }
    Ok(out)
}
fn uint(v: &Value, min: u64, max: u64) -> Result<u64, MasterApprovalError> {
    let n = v.as_u64().ok_or(MasterApprovalError::InvalidClaims)?;
    if !(min..=max).contains(&n) { return Err(MasterApprovalError::InvalidClaims); }
    Ok(n)
}
fn canonical_nonzero_uuid(s: &str) -> bool {
    Uuid::parse_str(s).map(|u| !u.is_nil() && u.to_string() == s).unwrap_or(false)
}
fn steam_id(s: &str) -> bool { !s.is_empty() && !s.starts_with('0') && s.bytes().all(|b| b.is_ascii_digit()) && s.parse::<u64>().map(|n| n != 0).unwrap_or(false) }
fn lower_hex_32(s: &str) -> bool { s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }
fn hex(bytes: &[u8]) -> String {
    const D: &[u8;16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len()*2);
    for &b in bytes { s.push(D[(b>>4) as usize] as char); s.push(D[(b&15) as usize] as char); }
    s
}
#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn golden_pair_context_digest_canonicality_and_owned_claims() {
        let master: Value = serde_json::from_str(include_str!("../../contracts/master-approval-v1-vector.json")).unwrap();
        let author: Value = serde_json::from_str(include_str!("../../contracts/author-release-v1-vector.json")).unwrap();
        let decode = |v: &Value, key: &str| base64::engine::general_purpose::STANDARD.decode(v[key].as_str().unwrap()).unwrap();
        let context = &master["expected_context"];
        let app = context["app_id"].as_u64().unwrap() as u32;
        let item = context["workshop_item_id"].as_str().unwrap();
        let revision = context["revision_id"].as_str().unwrap();
        let master_pem = master["master_public_key_pem"].as_str().unwrap();
        let alternate_pem = master["same_role_master_public_key_pem"].as_str().unwrap();
        let author_context = &author["expected_context"];
        let author_pem = author["public_key_pem"].as_str().unwrap();
        let mut master_bytes = decode(&master, "envelope_base64");
        let bundle = format!("{}{}", master_pem, alternate_pem);
        assert!(verify(&master_bytes, &bundle, app, item, revision).is_ok(), "installed bounded bundle selects the envelope key by header key ID");
        let alternate_bytes = decode(&master, "same_role_envelope_base64");
        assert_eq!(select_trusted_pem(&alternate_bytes, &bundle).unwrap().trim(), alternate_pem.trim(), "retired key ID resolves from the installed bundle");
        assert!(verify(&master_bytes, &format!("{}{}", master_pem, master_pem), app, item, revision).is_err(), "duplicate trusted key IDs are rejected");
        assert!(verify(&master_bytes, alternate_pem, app, item, revision).is_err(), "unknown envelope signer is rejected");
        assert!(verify(&master_bytes, &format!("{}not-a-public-key", master_pem), app, item, revision).is_err(), "malformed trusted bundle is rejected");
        assert!(verify(&master_bytes, &"x".repeat(8193), app, item, revision).is_err(), "oversized trusted bundle is rejected");
        let author_bytes = decode(&author, "envelope_base64");
        let (mut verified, verified_author) = verify_pair(&master_bytes, master_pem, &author_bytes,
            author_pem, app, item, revision).unwrap();
        assert_eq!(verified.claims["revision_id"], revision);
        assert_eq!(verified_author.claims["app_id"].as_u64(), Some(author_context["app_id"].as_u64().unwrap()));
        assert!(verify_pair(&master_bytes, master_pem, &author_bytes, author_pem,
            app, item, "33333333-3333-4333-8333-333333333333").is_err());
        assert!(verify(&decode(&master, "same_role_envelope_base64"),
            master["same_role_master_public_key_pem"].as_str().unwrap(), app, item, revision).is_err());
        let wrong_digest = decode(&master, "wrong_author_digest_envelope_base64");
        assert!(verify(&wrong_digest, master_pem, app, item, revision).is_ok());
        assert!(verify_pair(&wrong_digest, master_pem, &author_bytes, author_pem,
            app, item, revision).is_err());
        assert!(verify(&decode(&master, "noncanonical_envelope_base64"), master_pem,
            app, item, revision).is_err());

        master_bytes[44] ^= 1;
        assert_eq!(verified.claims["revision_id"], revision);
        assert_eq!(verified.canonical_body, decode(&master, "body_base64"));
        verified.claims["revision_id"] = Value::String("33333333-3333-4333-8333-333333333333".into());
        assert_eq!(verified.canonical_body, decode(&master, "body_base64"));
    }
}
