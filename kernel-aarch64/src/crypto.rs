//! Inter OS — AArch64 Sovereign Hardware Cryptographic Enclave & DID Keystore
//!
//! Provides bare-metal cryptographic primitives, decentralized identity derivation,
//! and RFC 4231 HMAC-SHA256 attestation in pure `no_std` Rust.
//!
//! Non-Negotiable Principle #1: User owns the machine.
//! Non-Negotiable Principle #17: Private state remains protected; public assertions use cryptographic proofs.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// SHA-256 digest length in bytes.
pub const SHA256_DIGEST_LENGTH: usize = 32;

/// SHA-256 block size in bytes.
pub const SHA256_BLOCK_SIZE: usize = 64;

/// Initial hash values (first 32 bits of fractional parts of square roots of first 8 primes).
const H_INIT: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// Round constants (first 32 bits of fractional parts of cube roots of first 64 primes).
const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

#[inline(always)]
fn rotr(x: u32, n: u32) -> u32 {
    (x >> n) | (x << (32 - n))
}

#[inline(always)]
fn ch(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (!x & z)
}

#[inline(always)]
fn maj(x: u32, y: u32, z: u32) -> u32 {
    (x & y) ^ (x & z) ^ (y & z)
}

#[inline(always)]
fn sigma0(x: u32) -> u32 {
    rotr(x, 2) ^ rotr(x, 13) ^ rotr(x, 22)
}

#[inline(always)]
fn sigma1(x: u32) -> u32 {
    rotr(x, 6) ^ rotr(x, 11) ^ rotr(x, 25)
}

#[inline(always)]
fn gamma0(x: u32) -> u32 {
    rotr(x, 7) ^ rotr(x, 18) ^ (x >> 3)
}

#[inline(always)]
fn gamma1(x: u32) -> u32 {
    rotr(x, 17) ^ rotr(x, 19) ^ (x >> 10)
}

/// Constant-time 32-byte array equality comparison preventing timing attacks.
#[inline(never)]
pub fn constant_time_eq_32(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

/// SHA-256 state machine conforming to FIPS PUB 180-4.
#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    buffer: [u8; SHA256_BLOCK_SIZE],
    buffer_len: usize,
    total_bytes: u64,
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            h: H_INIT,
            buffer: [0u8; SHA256_BLOCK_SIZE],
            buffer_len: 0,
            total_bytes: 0,
        }
    }

    pub fn update(&mut self, data: &[u8]) {
        self.total_bytes += data.len() as u64;
        let mut offset = 0;

        if self.buffer_len > 0 {
            let needed = SHA256_BLOCK_SIZE - self.buffer_len;
            if data.len() >= needed {
                self.buffer[self.buffer_len..SHA256_BLOCK_SIZE]
                    .copy_from_slice(&data[..needed]);
                self.process_block(&self.buffer.clone());
                self.buffer_len = 0;
                offset = needed;
            } else {
                self.buffer[self.buffer_len..self.buffer_len + data.len()]
                    .copy_from_slice(data);
                self.buffer_len += data.len();
                return;
            }
        }

        while offset + SHA256_BLOCK_SIZE <= data.len() {
            let mut chunk = [0u8; SHA256_BLOCK_SIZE];
            chunk.copy_from_slice(&data[offset..offset + SHA256_BLOCK_SIZE]);
            self.process_block(&chunk);
            offset += SHA256_BLOCK_SIZE;
        }

        let remaining = data.len() - offset;
        if remaining > 0 {
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    pub fn finalize(mut self) -> [u8; SHA256_DIGEST_LENGTH] {
        let total_bits = self.total_bytes * 8;

        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;

        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..SHA256_BLOCK_SIZE].fill(0);
            self.process_block(&self.buffer.clone());
            self.buffer_len = 0;
        }

        self.buffer[self.buffer_len..56].fill(0);

        let len_bytes = total_bits.to_be_bytes();
        self.buffer[56..64].copy_from_slice(&len_bytes);

        self.process_block(&self.buffer.clone());

        let mut digest = [0u8; SHA256_DIGEST_LENGTH];
        for (i, &word) in self.h.iter().enumerate() {
            let bytes = word.to_be_bytes();
            digest[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
        }

        digest
    }

    pub fn digest(data: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut hasher = Self::new();
        hasher.update(data);
        hasher.finalize()
    }

    fn process_block(&mut self, block: &[u8; SHA256_BLOCK_SIZE]) {
        let mut w = [0u32; 64];

        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }

        for i in 16..64 {
            let s0 = gamma0(w[i - 15]);
            let s1 = gamma1(w[i - 2]);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = self.h[0];
        let mut b = self.h[1];
        let mut c = self.h[2];
        let mut d = self.h[3];
        let mut e = self.h[4];
        let mut f = self.h[5];
        let mut g = self.h[6];
        let mut h = self.h[7];

        for i in 0..64 {
            let t1 = h
                .wrapping_add(sigma1(e))
                .wrapping_add(ch(e, f, g))
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let t2 = sigma0(a).wrapping_add(maj(a, b, c));

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }

        self.h[0] = self.h[0].wrapping_add(a);
        self.h[1] = self.h[1].wrapping_add(b);
        self.h[2] = self.h[2].wrapping_add(c);
        self.h[3] = self.h[3].wrapping_add(d);
        self.h[4] = self.h[4].wrapping_add(e);
        self.h[5] = self.h[5].wrapping_add(f);
        self.h[6] = self.h[6].wrapping_add(g);
        self.h[7] = self.h[7].wrapping_add(h);
    }
}

/// Computes SHA-256 digest of byte slice directly.
pub fn sha256(data: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
    Sha256::digest(data)
}

/// HMAC-SHA256 engine conforming to RFC 2104 and RFC 4231.
pub struct HmacSha256;

impl HmacSha256 {
    pub fn mac(key: &[u8], message: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        let mut k = [0u8; SHA256_BLOCK_SIZE];

        if key.len() > SHA256_BLOCK_SIZE {
            let key_digest = Sha256::digest(key);
            k[..SHA256_DIGEST_LENGTH].copy_from_slice(&key_digest);
        } else {
            k[..key.len()].copy_from_slice(key);
        }

        let mut k_ipad = [0u8; SHA256_BLOCK_SIZE];
        let mut k_opad = [0u8; SHA256_BLOCK_SIZE];

        for i in 0..SHA256_BLOCK_SIZE {
            k_ipad[i] = k[i] ^ 0x36;
            k_opad[i] = k[i] ^ 0x5C;
        }

        let mut inner_input = Vec::with_capacity(SHA256_BLOCK_SIZE + message.len());
        inner_input.extend_from_slice(&k_ipad);
        inner_input.extend_from_slice(message);
        let inner_digest = Sha256::digest(&inner_input);

        let mut outer_input = Vec::with_capacity(SHA256_BLOCK_SIZE + SHA256_DIGEST_LENGTH);
        outer_input.extend_from_slice(&k_opad);
        outer_input.extend_from_slice(&inner_digest);
        Sha256::digest(&outer_input)
    }

    pub fn verify(key: &[u8], message: &[u8], expected_tag: &[u8; SHA256_DIGEST_LENGTH]) -> bool {
        let actual_tag = Self::mac(key, message);
        constant_time_eq_32(&actual_tag, expected_tag)
    }
}

/// Strongly-typed Sovereign Decentralized Identifier (`did:inter:<fingerprint>`).
#[derive(Clone, PartialEq, Eq)]
pub struct SovereignDid {
    raw: String,
    fingerprint: [u8; 16],
}

impl SovereignDid {
    pub fn from_pubkey_hash(hash: &[u8; 32]) -> Self {
        let mut fingerprint = [0u8; 16];
        fingerprint.copy_from_slice(&hash[..16]);

        let mut hex_str = String::with_capacity(32);
        for byte in &fingerprint {
            use core::fmt::Write;
            let _ = write!(hex_str, "{:02x}", byte);
        }

        let raw = format!("did:inter:{}", hex_str);
        Self { raw, fingerprint }
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    pub fn fingerprint(&self) -> &[u8; 16] {
        &self.fingerprint
    }
}

impl fmt::Display for SovereignDid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.raw)
    }
}

impl fmt::Debug for SovereignDid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SovereignDid({})", self.raw)
    }
}

/// Cryptographically attested capability delegation token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationToken {
    pub subject_did: SovereignDid,
    pub task_id: u64,
    pub capability_mask: u32,
    pub expiry_ticks: u64,
    pub signature: [u8; SHA256_DIGEST_LENGTH],
}

/// Sovereign Hardware Identity Vault holding private device credentials.
pub struct IdentityVault {
    master_seed: [u8; 32],
    public_key: [u8; 32],
    device_did: SovereignDid,
    tokens_issued: u64,
}

impl IdentityVault {
    pub fn new(seed: [u8; 32]) -> Self {
        let public_key = Sha256::digest(&seed);
        let device_did = SovereignDid::from_pubkey_hash(&public_key);
        Self {
            master_seed: seed,
            public_key,
            device_did,
            tokens_issued: 0,
        }
    }

    pub fn device_did(&self) -> &SovereignDid {
        &self.device_did
    }

    pub fn public_key(&self) -> &[u8; 32] {
        &self.public_key
    }

    pub fn sign(&self, message: &[u8]) -> [u8; SHA256_DIGEST_LENGTH] {
        HmacSha256::mac(&self.master_seed, message)
    }

    pub fn verify_signature(&self, message: &[u8], signature: &[u8; SHA256_DIGEST_LENGTH]) -> bool {
        HmacSha256::verify(&self.master_seed, message, signature)
    }

    pub fn issue_delegation_token(
        &mut self,
        task_id: u64,
        capability_mask: u32,
        duration_ticks: u64,
        current_ticks: u64,
    ) -> DelegationToken {
        let expiry_ticks = current_ticks.saturating_add(duration_ticks);

        let mut payload = Vec::with_capacity(8 + 4 + 8 + self.device_did.as_str().len());
        payload.extend_from_slice(&task_id.to_le_bytes());
        payload.extend_from_slice(&capability_mask.to_le_bytes());
        payload.extend_from_slice(&expiry_ticks.to_le_bytes());
        payload.extend_from_slice(self.device_did.as_str().as_bytes());

        let signature = self.sign(&payload);
        self.tokens_issued = self.tokens_issued.saturating_add(1);

        DelegationToken {
            subject_did: self.device_did.clone(),
            task_id,
            capability_mask,
            expiry_ticks,
            signature,
        }
    }

    pub fn verify_delegation_token(&self, token: &DelegationToken, current_ticks: u64) -> bool {
        if current_ticks > token.expiry_ticks {
            return false;
        }

        let mut payload = Vec::with_capacity(8 + 4 + 8 + token.subject_did.as_str().len());
        payload.extend_from_slice(&token.task_id.to_le_bytes());
        payload.extend_from_slice(&token.capability_mask.to_le_bytes());
        payload.extend_from_slice(&token.expiry_ticks.to_le_bytes());
        payload.extend_from_slice(token.subject_did.as_str().as_bytes());

        self.verify_signature(&payload, &token.signature)
    }
}

/// Verifies RFC 4231 HMAC-SHA256 test vectors and crypto correctness.
pub fn verify_crypto_subsystem() {
    // 1. SHA-256 test vector for "abc"
    let digest_abc = Sha256::digest(b"abc");
    let expected_abc = [
        0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea,
        0x41, 0x41, 0x40, 0xde, 0x5d, 0xae, 0x22, 0x23,
        0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c,
        0xb4, 0x10, 0xff, 0x61, 0xf2, 0x00, 0x15, 0xad,
    ];
    assert!(constant_time_eq_32(&digest_abc, &expected_abc), "SHA-256 'abc' test vector failed");

    // 2. RFC 4231 Test Case 1:
    // Key: 20 bytes of 0x0b
    // Data: "Hi There"
    let key1 = [0x0bu8; 20];
    let data1 = b"Hi There";
    let tag1 = HmacSha256::mac(&key1, data1);
    let expected_tag1 = [
        0xb0, 0x34, 0x4c, 0x61, 0xd8, 0xdb, 0x38, 0x53,
        0x5c, 0xa8, 0xaf, 0xce, 0xaf, 0x0b, 0xf1, 0x2b,
        0x88, 0x1d, 0xc2, 0x00, 0xc9, 0x83, 0x3d, 0xa7,
        0x26, 0xe9, 0x37, 0x6c, 0x2e, 0x32, 0xcf, 0xf7,
    ];
    assert!(constant_time_eq_32(&tag1, &expected_tag1), "RFC 4231 Test Case 1 failed");

    // 3. RFC 4231 Test Case 2:
    // Key: "Jefe"
    // Data: "what do ya want for nothing?"
    let key2 = b"Jefe";
    let data2 = b"what do ya want for nothing?";
    let tag2 = HmacSha256::mac(key2, data2);
    let expected_tag2 = [
        0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e,
        0x6a, 0x04, 0x24, 0x26, 0x08, 0x95, 0x75, 0xc7,
        0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83,
        0x9d, 0xec, 0x58, 0xb9, 0x64, 0xec, 0x38, 0x43,
    ];
    assert!(constant_time_eq_32(&tag2, &expected_tag2), "RFC 4231 Test Case 2 failed");
}


/// Verifies a detached Ed25519 signature using a caller-supplied trusted
/// publisher public key. OTA trust decisions must remain asymmetric.
pub fn verify_ed25519_signature(
    message: &[u8],
    public_key: &[u8; 32],
    signature: &[u8; 64],
) -> bool {
    use ed25519_dalek::{Signature, VerifyingKey};

    let Ok(key) = VerifyingKey::from_bytes(public_key) else {
        return false;
    };
    let sig = Signature::from_bytes(signature);
    key.verify_strict(message, &sig).is_ok()
}

/// Signs a message with an Ed25519 private key. Only an offline publisher or
/// update-signing service holds such a key; the kernel only ever verifies.
pub fn sign_ed25519_signature(secret_key: &[u8; 32], message: &[u8]) -> [u8; 64] {
    use ed25519_dalek::{Signer, SigningKey};

    let signing_key = SigningKey::from_bytes(secret_key);
    signing_key.sign(message).to_bytes()
}

/// Derives the Ed25519 public key matching a signing key.
pub fn ed25519_public_key(secret_key: &[u8; 32]) -> [u8; 32] {
    use ed25519_dalek::{SigningKey, VerifyingKey};

    let signing_key = SigningKey::from_bytes(secret_key);
    VerifyingKey::from(&signing_key).to_bytes()
}
