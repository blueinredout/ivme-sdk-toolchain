//! Frozen IVME signed-envelope v1 framing. This module performs cryptography
//! only; callers remain responsible for trust and typed-body authorization.

use rsa::{pkcs1::DecodeRsaPrivateKey, pkcs8::DecodePrivateKey, pkcs8::DecodePublicKey};
use rsa::{pkcs8::EncodePublicKey, traits::PublicKeyParts, PrivateKey, PublicKey};
use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"IVME-SIGNED-ENVELOPE-V1\0";
const HEADER_BYTES: usize = 44;
const SIGNATURE_BYTES: usize = 384;
const MAX_BODY_BYTES: usize = 65_536;
const MAX_PUBLIC_PEM_BYTES: usize = 2_048;
const MAX_PRIVATE_PEM_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvelopeError {
    InvalidEnvelope,
    InvalidKey,
    InvalidSignature,
    SigningFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedEnvelope {
    pub body: Vec<u8>,
    pub key_id: [u8; 32],
}

/// Verify an envelope against a separately trusted public PEM and expected kind.
pub fn verify(
    bytes: &[u8],
    trusted_public_pem: &str,
    expected_kind: u8,
) -> Result<VerifiedEnvelope, EnvelopeError> {
    if !(1..=3).contains(&expected_kind)
        || bytes.len() < HEADER_BYTES + 1 + SIGNATURE_BYTES
        || bytes.len() > HEADER_BYTES + MAX_BODY_BYTES + SIGNATURE_BYTES
        || &bytes[..4] != b"IVSG"
        || bytes[4] != 1
        || bytes[5] != expected_kind
        || bytes[6] != 0
        || bytes[7] != 0
    {
        return Err(EnvelopeError::InvalidEnvelope);
    }
    let body_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    if !(1..=MAX_BODY_BYTES).contains(&body_len)
        || bytes.len() != HEADER_BYTES + body_len + SIGNATURE_BYTES
    {
        return Err(EnvelopeError::InvalidEnvelope);
    }

    let public_key = parse_public_key(trusted_public_pem)?;
    let spki = public_key
        .to_public_key_der()
        .map_err(|_| EnvelopeError::InvalidKey)?;
    let key_id: [u8; 32] = Sha256::digest(spki.as_bytes()).into();
    if bytes[12..44] != key_id {
        return Err(EnvelopeError::InvalidKey);
    }

    let mut message = Vec::with_capacity(DOMAIN.len() + HEADER_BYTES + body_len);
    message.extend_from_slice(DOMAIN);
    message.extend_from_slice(&bytes[..HEADER_BYTES + body_len]);
    public_key
        .pkcs1v15_verify::<rsa::sha2::Sha256>(&message, &bytes[HEADER_BYTES + body_len..])
        .map_err(|_| EnvelopeError::InvalidSignature)?;

    Ok(VerifiedEnvelope {
        body: bytes[HEADER_BYTES..HEADER_BYTES + body_len].to_vec(),
        key_id,
    })
}

/// Sign a body with an RSA-3072 PKCS#1 or PKCS#8 private PEM.
pub fn sign(kind: u8, body: &[u8], private_pem: &str) -> Result<Vec<u8>, EnvelopeError> {
    if !(1..=3).contains(&kind) || body.is_empty() || body.len() > MAX_BODY_BYTES {
        return Err(EnvelopeError::InvalidEnvelope);
    }
    let private_key = parse_private_key(private_pem)?;
    if private_key.n().bit_width() != 3072 {
        return Err(EnvelopeError::InvalidKey);
    }
    let public_key = PublicKey::from(&private_key);
    let spki = public_key
        .to_public_key_der()
        .map_err(|_| EnvelopeError::InvalidKey)?;
    let key_id = Sha256::digest(spki.as_bytes());

    let mut envelope = Vec::with_capacity(HEADER_BYTES + body.len() + SIGNATURE_BYTES);
    envelope.extend_from_slice(b"IVSG");
    envelope.extend_from_slice(&[1, kind, 0, 0]);
    envelope.extend_from_slice(&(body.len() as u32).to_le_bytes());
    envelope.extend_from_slice(&key_id);
    envelope.extend_from_slice(body);

    let mut message = Vec::with_capacity(DOMAIN.len() + envelope.len());
    message.extend_from_slice(DOMAIN);
    message.extend_from_slice(&envelope);
    let signature = private_key
        .pkcs1v15_sign::<rsa::sha2::Sha256>(&message)
        .map_err(|_| EnvelopeError::SigningFailed)?;
    if signature.len() != SIGNATURE_BYTES {
        return Err(EnvelopeError::SigningFailed);
    }
    envelope.extend_from_slice(&signature);
    Ok(envelope)
}

fn parse_public_key(pem: &str) -> Result<PublicKey, EnvelopeError> {
    if pem.len() > MAX_PUBLIC_PEM_BYTES
        || !pem.is_ascii()
        || !pem.starts_with("-----BEGIN PUBLIC KEY-----")
        || !pem.trim_end().ends_with("-----END PUBLIC KEY-----")
    {
        return Err(EnvelopeError::InvalidKey);
    }
    let key = PublicKey::from_public_key_pem(pem).map_err(|_| EnvelopeError::InvalidKey)?;
    if key.n().bit_width() != 3072 {
        return Err(EnvelopeError::InvalidKey);
    }
    Ok(key)
}

fn parse_private_key(pem: &str) -> Result<PrivateKey, EnvelopeError> {
    if pem.len() > MAX_PRIVATE_PEM_BYTES || !pem.is_ascii() {
        return Err(EnvelopeError::InvalidKey);
    }
    let key = if pem.starts_with("-----BEGIN PRIVATE KEY-----") {
        PrivateKey::from_pkcs8_pem(pem).map_err(|_| EnvelopeError::InvalidKey)?
    } else if pem.starts_with("-----BEGIN RSA PRIVATE KEY-----") {
        PrivateKey::from_pkcs1_pem(pem).map_err(|_| EnvelopeError::InvalidKey)?
    } else {
        return Err(EnvelopeError::InvalidKey);
    };
    key.validate().map_err(|_| EnvelopeError::InvalidKey)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::Value;

    #[test]
    fn independent_golden_verifies_and_rejects_framing_mutations() {
        let vector: Value = serde_json::from_str(include_str!(
            "../../contracts/signed-envelope-v1-vector.json"
        ))
        .unwrap();
        let decode = |field: &str| {
            base64::engine::general_purpose::STANDARD
                .decode(vector[field].as_str().unwrap())
                .unwrap()
        };
        let mut envelope = decode("envelope_base64");
        let body = decode("body_base64");
        let public_pem = vector["public_key_pem"].as_str().unwrap();
        let verified = verify(&envelope, public_pem, 1).unwrap();
        assert_eq!(verified.body, body);
        let key_id_hex: String = verified
            .key_id
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(key_id_hex, vector["key_id"]);
        envelope[HEADER_BYTES] ^= 1;
        assert_eq!(
            verified.body, body,
            "verified body must be detached from input"
        );
        envelope[HEADER_BYTES] ^= 1;

        let mut changed = envelope.clone();
        changed[HEADER_BYTES] ^= 1;
        assert_eq!(
            verify(&changed, public_pem, 1),
            Err(EnvelopeError::InvalidSignature)
        );
        assert_eq!(
            verify(&envelope, public_pem, 2),
            Err(EnvelopeError::InvalidEnvelope)
        );
        assert_eq!(
            verify(&envelope[..envelope.len() - 1], public_pem, 1),
            Err(EnvelopeError::InvalidEnvelope)
        );
        let mut bad_header = envelope;
        bad_header[6] = 1;
        assert_eq!(
            verify(&bad_header, public_pem, 1),
            Err(EnvelopeError::InvalidEnvelope)
        );
    }
}
