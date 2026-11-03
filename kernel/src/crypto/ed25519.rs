// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Ed25519 package-signature verification and publisher trust root.

use ed25519_dalek::{Signature, VerifyingKey};

pub const PUBLISHER_ID: &str = "interos_official";
pub const PUBLIC_KEY_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 64;

fn decode_hex_key(s: &str) -> Option<[u8; PUBLIC_KEY_LEN]> {
    if s.len() != PUBLIC_KEY_LEN * 2 { return None; }
    let bytes = s.as_bytes();
    let mut out = [0u8; PUBLIC_KEY_LEN];
    let mut i = 0;
    while i < PUBLIC_KEY_LEN {
        let hi = hex_nibble(bytes[i * 2])?;
        let lo = hex_nibble(bytes[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
        i += 1;
    }
    Some(out)
}

fn hex_nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

pub fn trusted_public_key(publisher_id: &str) -> Option<[u8; PUBLIC_KEY_LEN]> {
    if publisher_id != PUBLISHER_ID { return None; }
    option_env!("INTEROS_TRUSTED_PUBLISHER_KEY_HEX").and_then(decode_hex_key)
}

pub fn verify_rapp_signature(data: &[u8], publisher_id: &str, signature_bytes: &[u8; SIGNATURE_LEN]) -> bool {
    let Some(public_key_bytes) = trusted_public_key(publisher_id) else { return false; };
    let Ok(public_key) = VerifyingKey::from_bytes(&public_key_bytes) else { return false; };
    let signature = Signature::from_bytes(signature_bytes);
    let mut signed = alloc::vec::Vec::with_capacity(data.len());
    signed.extend_from_slice(data);
    if signed.len() < 0xB4 { return false; }
    signed[0x70..0xB0].fill(0);
    public_key.verify_strict(&signed, &signature).is_ok()
}

pub fn is_unsigned_signature(signature: &[u8; SIGNATURE_LEN]) -> bool {
    signature.iter().all(|b| *b == 0)
}

pub fn verify_detached_signature(data: &[u8], publisher_id: &str, signature_bytes: &[u8; SIGNATURE_LEN]) -> bool {
    let Some(public_key_bytes) = trusted_public_key(publisher_id) else { return false; };
    let Ok(public_key) = VerifyingKey::from_bytes(&public_key_bytes) else { return false; };
    let signature = Signature::from_bytes(signature_bytes);
    public_key.verify_strict(data, &signature).is_ok()
}
