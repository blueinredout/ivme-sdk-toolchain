//! Verification of signed author publication claims. This proves signed claims
//! only; Steam ownership, entitlement, key binding, approval, and artifacts are
//! established by other layers.

use crate::signed_envelope;
use serde_json::{Number, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

const FIELDS: [&str; 24] = [
    "schema_version",
    "app_id",
    "submission_id",
    "expected_previous_revision_id",
    "workshop_item_id",
    "publisher_profile_id",
    "author_key_id",
    "level_uuid",
    "content_hash_version",
    "content_hash",
    "package_sha256",
    "package_size",
    "evidence_sha256",
    "evidence_size",
    "ruleset_id",
    "ruleset_fingerprint",
    "simulation_target_id",
    "simulation_contract_id",
    "timeline_contract_id",
    "game_mode",
    "bronze_time_ms",
    "silver_time_ms",
    "gold_time_ms",
    "claimed_author_time_ms",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorReleaseError {
    InvalidEnvelope,
    InvalidClaims,
}

/// Owned validated claims plus the SHA-256 digest of the complete signed envelope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedAuthorRelease {
    pub claims: Value,
    pub release_sha256: String,
}

/// Verify kind-1 claims against a trusted key and externally supplied identity context.
pub fn verify(
    bytes: &[u8],
    trusted_pem: &str,
    expected_app: u32,
    expected_publisher: &str,
    expected_item: &str,
) -> Result<VerifiedAuthorRelease, AuthorReleaseError> {
    if expected_app == 0 || !steam_id(expected_publisher) || !steam_id(expected_item) {
        return Err(AuthorReleaseError::InvalidClaims);
    }
    let envelope = signed_envelope::verify(bytes, trusted_pem, 1)
        .map_err(|_| AuthorReleaseError::InvalidEnvelope)?;
    let claims = validate_claims(&envelope.body)?;
    if claims["app_id"].as_u64() != Some(expected_app as u64)
        || claims["publisher_profile_id"].as_str() != Some(expected_publisher)
        || claims["workshop_item_id"].as_str() != Some(expected_item)
        || claims["author_key_id"].as_str() != Some(&hex(&envelope.key_id))
    {
        return Err(AuthorReleaseError::InvalidClaims);
    }
    let release_sha256 = hex(&Sha256::digest(bytes));
    Ok(VerifiedAuthorRelease {
        claims,
        release_sha256,
    })
}

/// Validate claims and encode their canonical compact ordinal-sorted JSON body.
/// This does not authenticate context, an envelope, or approval.
pub fn canonical_body(claims: &Value) -> Result<Vec<u8>, AuthorReleaseError> {
    let sorted = validate_value(claims)?;
    serde_json::to_vec(&sorted).map_err(|_| AuthorReleaseError::InvalidClaims)
}

fn validate_claims(body: &[u8]) -> Result<Value, AuthorReleaseError> {
    let parsed: Value =
        serde_json::from_slice(body).map_err(|_| AuthorReleaseError::InvalidClaims)?;
    if canonical_body(&parsed)? != body {
        return Err(AuthorReleaseError::InvalidClaims);
    }
    Ok(parsed)
}

fn validate_value(value: &Value) -> Result<BTreeMap<String, Value>, AuthorReleaseError> {
    let object = value.as_object().ok_or(AuthorReleaseError::InvalidClaims)?;
    if object.len() != FIELDS.len() || FIELDS.iter().any(|field| !object.contains_key(*field)) {
        return Err(AuthorReleaseError::InvalidClaims);
    }
    let mut sorted = BTreeMap::<String, Value>::new();
    for field in FIELDS {
        let value = &object[field];
        let normalized = match field {
            "schema_version" | "content_hash_version" => {
                Value::Number(Number::from(uint(value, 1, 1)?))
            }
            "app_id" => Value::Number(Number::from(uint(value, 1, u32::MAX as u64)?)),
            "package_size" => Value::Number(Number::from(uint(value, 1, 67_108_864)?)),
            "evidence_size" => Value::Number(Number::from(uint(value, 1, 22_000_000)?)),
            "bronze_time_ms" | "silver_time_ms" | "gold_time_ms" | "claimed_author_time_ms" => {
                Value::Number(Number::from(uint(value, 1, i32::MAX as u64)?))
            }
            _ => {
                let s = value.as_str().ok_or(AuthorReleaseError::InvalidClaims)?;
                if !s.is_ascii() {
                    return Err(AuthorReleaseError::InvalidClaims);
                }
                match field {
                    "submission_id" | "level_uuid" => {
                        if !canonical_uuid(s) {
                            return Err(AuthorReleaseError::InvalidClaims);
                        }
                    }
                    "expected_previous_revision_id" => {
                        if !s.is_empty() && !canonical_uuid(s) {
                            return Err(AuthorReleaseError::InvalidClaims);
                        }
                    }
                    "workshop_item_id" | "publisher_profile_id" => {
                        if !steam_id(s) {
                            return Err(AuthorReleaseError::InvalidClaims);
                        }
                    }
                    "author_key_id"
                    | "content_hash"
                    | "package_sha256"
                    | "evidence_sha256"
                    | "ruleset_fingerprint" => {
                        if !lower_hex_32(s) {
                            return Err(AuthorReleaseError::InvalidClaims);
                        }
                    }
                    "ruleset_id" | "simulation_target_id" => {
                        if s.is_empty()
                            || s.trim() != s
                            || s.len() > 128
                            || !s.bytes().all(|b| (32..=126).contains(&b))
                        {
                            return Err(AuthorReleaseError::InvalidClaims);
                        }
                    }
                    "simulation_contract_id" => {
                        if s != "ivme-author-sim-v1" {
                            return Err(AuthorReleaseError::InvalidClaims);
                        }
                    }
                    "timeline_contract_id" => {
                        if s != "ivme-author-timeline-v1" {
                            return Err(AuthorReleaseError::InvalidClaims);
                        }
                    }
                    "game_mode" => {
                        if s != "std" {
                            return Err(AuthorReleaseError::InvalidClaims);
                        }
                    }
                    _ => return Err(AuthorReleaseError::InvalidClaims),
                }
                Value::String(s.to_owned())
            }
        };
        sorted.insert(field.to_owned(), normalized);
    }
    if !(sorted["claimed_author_time_ms"].as_u64().unwrap()
        <= sorted["gold_time_ms"].as_u64().unwrap()
        && sorted["gold_time_ms"].as_u64().unwrap() < sorted["silver_time_ms"].as_u64().unwrap()
        && sorted["silver_time_ms"].as_u64().unwrap() < sorted["bronze_time_ms"].as_u64().unwrap())
    {
        return Err(AuthorReleaseError::InvalidClaims);
    }
    Ok(sorted)
}

fn uint(value: &Value, min: u64, max: u64) -> Result<u64, AuthorReleaseError> {
    let n = value.as_u64().ok_or(AuthorReleaseError::InvalidClaims)?;
    if !(min..=max).contains(&n) {
        return Err(AuthorReleaseError::InvalidClaims);
    }
    Ok(n)
}

fn canonical_uuid(s: &str) -> bool {
    Uuid::parse_str(s)
        .map(|id| id.to_string() == s)
        .unwrap_or(false)
}

fn steam_id(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('0')
        && s.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<u64>().map(|n| n != 0).unwrap_or(false)
}

fn lower_hex_32(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 15) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn golden_context_and_rejections() {
        let vector: Value = serde_json::from_str(include_str!(
            "../../contracts/author-release-v1-vector.json"
        ))
        .unwrap();
        let decode = |field: &str| {
            base64::engine::general_purpose::STANDARD
                .decode(vector[field].as_str().unwrap())
                .unwrap()
        };
        let context = &vector["expected_context"];
        let app = context["app_id"].as_u64().unwrap() as u32;
        let publisher = context["publisher_profile_id"].as_str().unwrap();
        let item = context["workshop_item_id"].as_str().unwrap();
        let pem = vector["public_key_pem"].as_str().unwrap();
        let mut bytes = decode("envelope_base64");
        let valid = verify(&bytes, pem, app, publisher, item).unwrap();
        assert_eq!(valid.release_sha256, vector["release_sha256"]);
        assert_eq!(
            canonical_body(&valid.claims).unwrap(),
            decode("body_base64")
        );
        for (expected_app, expected_publisher, expected_item) in [
            (app + 1, publisher, item),
            (app, "76561198000000002", item),
            (app, publisher, "1234567891"),
        ] {
            assert!(verify(&bytes, pem, expected_app, expected_publisher, expected_item).is_err());
        }
        for field in [
            "wrong_key_binding_envelope_base64",
            "invalid_medals_envelope_base64",
            "noncanonical_envelope_base64",
        ] {
            assert!(verify(&decode(field), pem, app, publisher, item).is_err());
        }
        bytes[44] ^= 1;
        assert_eq!(valid.claims["claimed_author_time_ms"].as_u64(), Some(2692));
    }
}
