// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Pure Rust, Allocation-Free ChaCha20-Poly1305 AEAD (RFC 8439)
//!
//! Implements authenticated encryption with associated data (AEAD) as specified
//! in IETF RFC 8439:
//! - 256-bit key
//! - 96-bit nonce
//! - 128-bit Poly1305 authentication tag
//! - Constant-time operations preventing side-channel leakage
//! - Zero heap allocations (`#![no_std]`)
//! - Strict fail-closed verification: ciphertext is never decrypted if tag check fails

use super::{constant_time_eq, zeroize_slice};

/// ChaCha20-Poly1305 AEAD Errors
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AeadError {
    /// Authentication tag mismatch or corrupted ciphertext.
    IntegrityViolation,
    /// Plaintext or ciphertext length exceeds allowable bound.
    PayloadTooLarge,
}

/// Constant-time quarter-round transformation.
#[inline(always)]
fn quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(16);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(12);

    state[a] = state[a].wrapping_add(state[b]);
    state[d] ^= state[a];
    state[d] = state[d].rotate_left(8);

    state[c] = state[c].wrapping_add(state[d]);
    state[b] ^= state[c];
    state[b] = state[b].rotate_left(7);
}

/// Computes a single 64-byte ChaCha20 block.
pub fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12], block: &mut [u8; 64]) {
    let initial = [
        0x61707865u32, 0x3320646e, 0x79622d32, 0x6b206574,
        u32::from_le_bytes([key[0], key[1], key[2], key[3]]),
        u32::from_le_bytes([key[4], key[5], key[6], key[7]]),
        u32::from_le_bytes([key[8], key[9], key[10], key[11]]),
        u32::from_le_bytes([key[12], key[13], key[14], key[15]]),
        u32::from_le_bytes([key[16], key[17], key[18], key[19]]),
        u32::from_le_bytes([key[20], key[21], key[22], key[23]]),
        u32::from_le_bytes([key[24], key[25], key[26], key[27]]),
        u32::from_le_bytes([key[28], key[29], key[30], key[31]]),
        counter,
        u32::from_le_bytes([nonce[0], nonce[1], nonce[2], nonce[3]]),
        u32::from_le_bytes([nonce[4], nonce[5], nonce[6], nonce[7]]),
        u32::from_le_bytes([nonce[8], nonce[9], nonce[10], nonce[11]]),
    ];

    let mut state = initial;
    for _ in 0..10 {
        // Column round
        quarter_round(&mut state, 0, 4, 8, 12);
        quarter_round(&mut state, 1, 5, 9, 13);
        quarter_round(&mut state, 2, 6, 10, 14);
        quarter_round(&mut state, 3, 7, 11, 15);
        // Diagonal round
        quarter_round(&mut state, 0, 5, 10, 15);
        quarter_round(&mut state, 1, 6, 11, 12);
        quarter_round(&mut state, 2, 7, 8, 13);
        quarter_round(&mut state, 3, 4, 9, 14);
    }

    for i in 0..16 {
        let val = state[i].wrapping_add(initial[i]);
        block[i * 4..(i + 1) * 4].copy_from_slice(&val.to_le_bytes());
    }
}

/// XORs `buffer` in-place with the ChaCha20 keystream starting at `counter`.
pub fn chacha20_xor(key: &[u8; 32], mut counter: u32, nonce: &[u8; 12], buffer: &mut [u8]) {
    let mut block = [0u8; 64];
    for chunk in buffer.chunks_mut(64) {
        chacha20_block(key, counter, nonce, &mut block);
        for (b, k) in chunk.iter_mut().zip(block.iter()) {
            *b ^= *k;
        }
        counter = counter.wrapping_add(1);
    }
    zeroize_slice(&mut block);
}

/// Constant-time Poly1305 MAC accumulator in 26-bit limbs.
struct Poly1305 {
    r0: u32,
    r1: u32,
    r2: u32,
    r3: u32,
    r4: u32,
    s1: u32,
    s2: u32,
    s3: u32,
    s4: u32,
    h0: u32,
    h1: u32,
    h2: u32,
    h3: u32,
    h4: u32,
    pad: [u32; 4],
}

impl Poly1305 {
    fn new(key: &[u8; 32]) -> Self {
        // Clamp r
        let t0 = u32::from_le_bytes([key[0], key[1], key[2], key[3]]) & 0x0FFF_FFFF;
        let t1 = u32::from_le_bytes([key[4], key[5], key[6], key[7]]) & 0x0FFF_FFFC;
        let t2 = u32::from_le_bytes([key[8], key[9], key[10], key[11]]) & 0x0FFF_FFFC;
        let t3 = u32::from_le_bytes([key[12], key[13], key[14], key[15]]) & 0x0FFF_FFFC;

        let r0 = t0 & 0x03FF_FFFF;
        let r1 = ((t0 >> 26) | (t1 << 6)) & 0x03FF_FFFF;
        let r2 = ((t1 >> 20) | (t2 << 12)) & 0x03FF_FFFF;
        let r3 = ((t2 >> 14) | (t3 << 18)) & 0x03FF_FFFF;
        let r4 = (t3 >> 8) & 0x03FF_FFFF;

        let s1 = r1 * 5;
        let s2 = r2 * 5;
        let s3 = r3 * 5;
        let s4 = r4 * 5;

        let pad = [
            u32::from_le_bytes([key[16], key[17], key[18], key[19]]),
            u32::from_le_bytes([key[20], key[21], key[22], key[23]]),
            u32::from_le_bytes([key[24], key[25], key[26], key[27]]),
            u32::from_le_bytes([key[28], key[29], key[30], key[31]]),
        ];

        Self {
            r0, r1, r2, r3, r4,
            s1, s2, s3, s4,
            h0: 0, h1: 0, h2: 0, h3: 0, h4: 0,
            pad,
        }
    }

    fn blocks(&mut self, m: &[u8], is_final_partial: bool) {
        let mut offset = 0;
        while offset < m.len() {
            let chunk_len = (m.len() - offset).min(16);
            let chunk = &m[offset..offset + chunk_len];

            let mut block = [0u8; 16];
            block[..chunk_len].copy_from_slice(chunk);
            let hibit = if is_final_partial && chunk_len < 16 {
                block[chunk_len] = 0x01;
                0u32
            } else {
                1u32 << 24
            };

            let t0 = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
            let t1 = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
            let t2 = u32::from_le_bytes([block[8], block[9], block[10], block[11]]);
            let t3 = u32::from_le_bytes([block[12], block[13], block[14], block[15]]);

            self.h0 += t0 & 0x03FF_FFFF;
            self.h1 += ((t0 >> 26) | (t1 << 6)) & 0x03FF_FFFF;
            self.h2 += ((t1 >> 20) | (t2 << 12)) & 0x03FF_FFFF;
            self.h3 += ((t2 >> 14) | (t3 << 18)) & 0x03FF_FFFF;
            self.h4 += ((t3 >> 8) & 0x03FF_FFFF) | hibit;

            // Multiply h by r (modulo 2^130 - 5)
            let h0 = self.h0 as u64;
            let h1 = self.h1 as u64;
            let h2 = self.h2 as u64;
            let h3 = self.h3 as u64;
            let h4 = self.h4 as u64;

            let d0 = h0 * (self.r0 as u64) + h1 * (self.s4 as u64) + h2 * (self.s3 as u64) + h3 * (self.s2 as u64) + h4 * (self.s1 as u64);
            let d1 = h0 * (self.r1 as u64) + h1 * (self.r0 as u64) + h2 * (self.s4 as u64) + h3 * (self.s3 as u64) + h4 * (self.s2 as u64);
            let d2 = h0 * (self.r2 as u64) + h1 * (self.r1 as u64) + h2 * (self.r0 as u64) + h3 * (self.s4 as u64) + h4 * (self.s3 as u64);
            let d3 = h0 * (self.r3 as u64) + h1 * (self.r2 as u64) + h2 * (self.r1 as u64) + h3 * (self.r0 as u64) + h4 * (self.s4 as u64);
            let d4 = h0 * (self.r4 as u64) + h1 * (self.r3 as u64) + h2 * (self.r2 as u64) + h3 * (self.r1 as u64) + h4 * (self.r0 as u64);

            // Carry propagation
            let mut c: u64 = d0 >> 26;
            self.h0 = (d0 as u32) & 0x03FF_FFFF;

            let d1 = d1 + c;
            c = d1 >> 26;
            self.h1 = (d1 as u32) & 0x03FF_FFFF;

            let d2 = d2 + c;
            c = d2 >> 26;
            self.h2 = (d2 as u32) & 0x03FF_FFFF;

            let d3 = d3 + c;
            c = d3 >> 26;
            self.h3 = (d3 as u32) & 0x03FF_FFFF;

            let d4 = d4 + c;
            c = d4 >> 26;
            self.h4 = (d4 as u32) & 0x03FF_FFFF;

            self.h0 += (c * 5) as u32;
            let c0 = (self.h0 >> 26) as u32;
            self.h0 &= 0x03FF_FFFF;
            self.h1 += c0;

            offset += chunk_len;
        }
    }

    fn finish(mut self, out: &mut [u8; 16]) {
        // Fully carry to ensure h < 2^130
        let mut c = self.h1 >> 26; self.h1 &= 0x03FF_FFFF; self.h2 += c;
        c = self.h2 >> 26; self.h2 &= 0x03FF_FFFF; self.h3 += c;
        c = self.h3 >> 26; self.h3 &= 0x03FF_FFFF; self.h4 += c;
        c = self.h4 >> 26; self.h4 &= 0x03FF_FFFF; self.h0 += c * 5;
        c = self.h0 >> 26; self.h0 &= 0x03FF_FFFF; self.h1 += c;

        // Compute h - (2^130 - 5)
        let mut g0 = self.h0.wrapping_add(5);
        c = g0 >> 26; g0 &= 0x03FF_FFFF;
        let mut g1 = self.h1.wrapping_add(c);
        c = g1 >> 26; g1 &= 0x03FF_FFFF;
        let mut g2 = self.h2.wrapping_add(c);
        c = g2 >> 26; g2 &= 0x03FF_FFFF;
        let mut g3 = self.h3.wrapping_add(c);
        c = g3 >> 26; g3 &= 0x03FF_FFFF;
        let g4 = self.h4.wrapping_add(c).wrapping_sub(1 << 26);

        // Constant-time select: if borrowed (g4 < 0), keep h; otherwise select g (h - (2^130 - 5))
        let borrow = g4 >> 31;
        let mask = borrow.wrapping_neg(); // 0xFFFF_FFFF if borrowed (keep h), 0x0000_0000 if not (select g)

        self.h0 = (self.h0 & mask) | (g0 & !mask);
        self.h1 = (self.h1 & mask) | (g1 & !mask);
        self.h2 = (self.h2 & mask) | (g2 & !mask);
        self.h3 = (self.h3 & mask) | (g3 & !mask);
        self.h4 = (self.h4 & mask) | (g4 & !mask);

        // Recombine 26-bit limbs into 32-bit words
        let f0 = self.h0 | (self.h1 << 26);
        let f1 = (self.h1 >> 6) | (self.h2 << 20);
        let f2 = (self.h2 >> 12) | (self.h3 << 14);
        let f3 = (self.h3 >> 18) | (self.h4 << 8);

        // Add pad (mod 2^128)
        let mut carry: u64;
        let r0 = (f0 as u64) + (self.pad[0] as u64);
        carry = r0 >> 32;
        let r1 = (f1 as u64) + (self.pad[1] as u64) + carry;
        carry = r1 >> 32;
        let r2 = (f2 as u64) + (self.pad[2] as u64) + carry;
        carry = r2 >> 32;
        let r3 = (f3 as u64) + (self.pad[3] as u64) + carry;

        out[0..4].copy_from_slice(&(r0 as u32).to_le_bytes());
        out[4..8].copy_from_slice(&(r1 as u32).to_le_bytes());
        out[8..12].copy_from_slice(&(r2 as u32).to_le_bytes());
        out[12..16].copy_from_slice(&(r3 as u32).to_le_bytes());
    }
}

impl Drop for Poly1305 {
    fn drop(&mut self) {
        unsafe {
            core::ptr::write_volatile(&mut self.r0, 0);
            core::ptr::write_volatile(&mut self.r1, 0);
            core::ptr::write_volatile(&mut self.r2, 0);
            core::ptr::write_volatile(&mut self.r3, 0);
            core::ptr::write_volatile(&mut self.r4, 0);
            core::ptr::write_volatile(&mut self.s1, 0);
            core::ptr::write_volatile(&mut self.s2, 0);
            core::ptr::write_volatile(&mut self.s3, 0);
            core::ptr::write_volatile(&mut self.s4, 0);
            core::ptr::write_volatile(&mut self.h0, 0);
            core::ptr::write_volatile(&mut self.h1, 0);
            core::ptr::write_volatile(&mut self.h2, 0);
            core::ptr::write_volatile(&mut self.h3, 0);
            core::ptr::write_volatile(&mut self.h4, 0);
            for word in self.pad.iter_mut() {
                core::ptr::write_volatile(word, 0);
            }
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
}

/// Maximum AEAD payload length (64 MiB) supported under microkernel buffer constraints.
pub const MAX_PAYLOAD_SIZE: usize = 64 * 1024 * 1024;

/// ChaCha20-Poly1305 AEAD (RFC 8439).
pub struct ChaCha20Poly1305;

impl ChaCha20Poly1305 {
    /// Encrypts `buffer` in-place and computes the 16-byte Poly1305 tag.
    ///
    /// # Arguments
    /// - `key`: 256-bit symmetric key.
    /// - `nonce`: 96-bit unique nonce.
    /// - `aad`: Additional Authenticated Data (unencrypted header).
    /// - `buffer`: In-place plaintext to be encrypted into ciphertext.
    /// - `tag`: Output 16-byte authentication tag.
    pub fn seal(
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut [u8],
        tag: &mut [u8; 16],
    ) -> Result<(), AeadError> {
        if buffer.len() > MAX_PAYLOAD_SIZE || aad.len() > MAX_PAYLOAD_SIZE {
            return Err(AeadError::PayloadTooLarge);
        }

        // 1. Generate one-time Poly1305 subkey (counter = 0)
        let mut otk_block = [0u8; 64];
        chacha20_block(key, 0, nonce, &mut otk_block);
        let mut otk = [0u8; 32];
        otk.copy_from_slice(&otk_block[..32]);
        zeroize_slice(&mut otk_block);

        // 2. Encrypt plaintext in-place (counter = 1)
        chacha20_xor(key, 1, nonce, buffer);

        // 3. Compute Poly1305 tag over AAD || pad(AAD) || Ciphertext || pad(Ciphertext) || len(AAD) || len(Ciphertext)
        let mut poly = Poly1305::new(&otk);
        zeroize_slice(&mut otk);

        // Feed AAD in 16-byte blocks with zero-padding
        if !aad.is_empty() {
            let full_blocks = (aad.len() / 16) * 16;
            if full_blocks > 0 {
                poly.blocks(&aad[..full_blocks], false);
            }
            let rem = aad.len() % 16;
            if rem > 0 {
                let mut pad_block = [0u8; 16];
                pad_block[..rem].copy_from_slice(&aad[full_blocks..]);
                // For RFC 8439 padding, partial AAD block is padded with zeros and has 16-byte hibit set
                poly.blocks(&pad_block, false);
            }
        }

        // Feed Ciphertext in 16-byte blocks with zero-padding
        if !buffer.is_empty() {
            let full_blocks = (buffer.len() / 16) * 16;
            if full_blocks > 0 {
                poly.blocks(&buffer[..full_blocks], false);
            }
            let rem = buffer.len() % 16;
            if rem > 0 {
                let mut pad_block = [0u8; 16];
                pad_block[..rem].copy_from_slice(&buffer[full_blocks..]);
                poly.blocks(&pad_block, false);
            }
        }

        // Feed 8-byte LE len(AAD) and 8-byte LE len(Ciphertext)
        let mut lens = [0u8; 16];
        lens[..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
        lens[8..16].copy_from_slice(&(buffer.len() as u64).to_le_bytes());
        poly.blocks(&lens, false);

        poly.finish(tag);
        Ok(())
    }

    /// Verifies the 16-byte tag, and only decrypts `buffer` in-place if verification succeeds.
    ///
    /// # Arguments
    /// - `key`: 256-bit symmetric key.
    /// - `nonce`: 96-bit unique nonce.
    /// - `aad`: Additional Authenticated Data.
    /// - `buffer`: In-place ciphertext to be decrypted into plaintext upon success.
    /// - `tag`: Expected 16-byte authentication tag.
    pub fn open(
        key: &[u8; 32],
        nonce: &[u8; 12],
        aad: &[u8],
        buffer: &mut [u8],
        tag: &[u8; 16],
    ) -> Result<(), AeadError> {
        if buffer.len() > MAX_PAYLOAD_SIZE || aad.len() > MAX_PAYLOAD_SIZE {
            return Err(AeadError::PayloadTooLarge);
        }

        // 1. Generate one-time Poly1305 subkey (counter = 0)
        let mut otk_block = [0u8; 64];
        chacha20_block(key, 0, nonce, &mut otk_block);
        let mut otk = [0u8; 32];
        otk.copy_from_slice(&otk_block[..32]);
        zeroize_slice(&mut otk_block);

        // 2. Compute expected Poly1305 tag over Ciphertext
        let mut poly = Poly1305::new(&otk);
        zeroize_slice(&mut otk);

        if !aad.is_empty() {
            let full_blocks = (aad.len() / 16) * 16;
            if full_blocks > 0 {
                poly.blocks(&aad[..full_blocks], false);
            }
            let rem = aad.len() % 16;
            if rem > 0 {
                let mut pad_block = [0u8; 16];
                pad_block[..rem].copy_from_slice(&aad[full_blocks..]);
                poly.blocks(&pad_block, false);
            }
        }

        if !buffer.is_empty() {
            let full_blocks = (buffer.len() / 16) * 16;
            if full_blocks > 0 {
                poly.blocks(&buffer[..full_blocks], false);
            }
            let rem = buffer.len() % 16;
            if rem > 0 {
                let mut pad_block = [0u8; 16];
                pad_block[..rem].copy_from_slice(&buffer[full_blocks..]);
                poly.blocks(&pad_block, false);
            }
        }

        let mut lens = [0u8; 16];
        lens[..8].copy_from_slice(&(aad.len() as u64).to_le_bytes());
        lens[8..16].copy_from_slice(&(buffer.len() as u64).to_le_bytes());
        poly.blocks(&lens, false);

        let mut computed_tag = [0u8; 16];
        poly.finish(&mut computed_tag);

        // 3. Constant-time tag check
        if !constant_time_eq(&computed_tag, tag) {
            zeroize_slice(&mut computed_tag);
            return Err(AeadError::IntegrityViolation);
        }
        zeroize_slice(&mut computed_tag);

        // 4. Decrypt in-place (counter = 1)
        chacha20_xor(key, 1, nonce, buffer);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rfc8439_test_vector() {
        // Official RFC 8439 Section 2.8.2 Test Vector
        let key: [u8; 32] = [
            0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
            0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f,
            0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
            0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x9f,
        ];
        let nonce: [u8; 12] = [
            0x07, 0x00, 0x00, 0x00,
            0x40, 0x41, 0x42, 0x43,
            0x44, 0x45, 0x46, 0x47,
        ];
        let aad: [u8; 12] = [
            0x50, 0x51, 0x52, 0x53,
            0xc0, 0xc1, 0xc2, 0xc3,
            0xc4, 0xc5, 0xc6, 0xc7,
        ];
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let mut buffer = *plaintext;

        let expected_ciphertext: [u8; 114] = [
            0xd3, 0x1a, 0x8d, 0x34, 0x64, 0x8e, 0x60, 0xdb, 0x7b, 0x86, 0xaf, 0xbc, 0x53, 0xef, 0x7e, 0xc2,
            0xa4, 0xad, 0xed, 0x51, 0x29, 0x6e, 0x08, 0xfe, 0xa9, 0xe2, 0xb5, 0xa7, 0x36, 0xee, 0x62, 0xd6,
            0x3d, 0xbe, 0xa4, 0x5e, 0x8c, 0xa9, 0x67, 0x12, 0x82, 0xfa, 0xfb, 0x69, 0xda, 0x92, 0x72, 0x8b,
            0x1a, 0x71, 0xde, 0x0a, 0x9e, 0x06, 0x0b, 0x29, 0x05, 0xd6, 0xa5, 0xb6, 0x7e, 0xcd, 0x3b, 0x36,
            0x92, 0xdd, 0xbd, 0x7f, 0x2d, 0x77, 0x8b, 0x8c, 0x98, 0x03, 0xae, 0xe3, 0x28, 0x09, 0x1b, 0x58,
            0xfa, 0xb3, 0x24, 0xe4, 0xfa, 0xd6, 0x75, 0x94, 0x55, 0x85, 0x80, 0x8b, 0x48, 0x31, 0xd7, 0xbc,
            0x3f, 0xf4, 0xde, 0xf0, 0x8e, 0x4b, 0x7a, 0x9d, 0xe5, 0x76, 0xd2, 0x65, 0x86, 0xce, 0xc6, 0x4b,
            0x61, 0x16,
        ];
        let expected_tag: [u8; 16] = [
            0x1a, 0xe1, 0x0b, 0x59, 0x4f, 0x09, 0xe2, 0x6a,
            0x7e, 0x90, 0x2e, 0xcb, 0xd0, 0x60, 0x06, 0x91,
        ];

        let mut tag = [0u8; 16];
        ChaCha20Poly1305::seal(&key, &nonce, &aad, &mut buffer, &mut tag).expect("seal failed");

        assert_eq!(&buffer[..], &expected_ciphertext[..], "Ciphertext does not match RFC 8439 vector");
        assert_eq!(&tag, &expected_tag, "Tag does not match RFC 8439 vector");

        // Now test decryption (open)
        ChaCha20Poly1305::open(&key, &nonce, &aad, &mut buffer, &tag).expect("open failed");
        assert_eq!(&buffer[..], plaintext, "Decrypted text does not match original plaintext");

        // Test corruption / tamper resistance
        buffer[0] ^= 0x01;
        assert_eq!(
            ChaCha20Poly1305::open(&key, &nonce, &aad, &mut buffer, &tag),
            Err(AeadError::IntegrityViolation)
        );
    }

    #[test]
    fn test_rfc8439_empty_payload() {
        let key = [0x42u8; 32];
        let nonce = [0x24u8; 12];
        let aad = b"Additional Authenticated Header Data";
        let mut payload = [0u8; 0];
        let mut tag = [0u8; 16];

        ChaCha20Poly1305::seal(&key, &nonce, aad, &mut payload, &mut tag).expect("seal empty payload failed");
        assert_ne!(tag, [0u8; 16]);

        ChaCha20Poly1305::open(&key, &nonce, aad, &mut payload, &tag).expect("open empty payload failed");

        let mut bad_tag = tag;
        bad_tag[0] ^= 0xFF;
        assert_eq!(
            ChaCha20Poly1305::open(&key, &nonce, aad, &mut payload, &bad_tag),
            Err(AeadError::IntegrityViolation)
        );
    }

    #[test]
    fn test_rfc8439_no_aad() {
        let key = [0x33u8; 32];
        let nonce = [0x77u8; 12];
        let aad = b"";
        let plaintext = b"Sensitive microkernel payload without extra AAD";
        let mut buffer = *plaintext;
        let mut tag = [0u8; 16];

        ChaCha20Poly1305::seal(&key, &nonce, aad, &mut buffer, &mut tag).expect("seal failed");
        assert_ne!(&buffer, plaintext);

        ChaCha20Poly1305::open(&key, &nonce, aad, &mut buffer, &tag).expect("open failed");
        assert_eq!(&buffer, plaintext);

        let wrong_key = [0x55u8; 32];
        assert_eq!(
            ChaCha20Poly1305::open(&wrong_key, &nonce, aad, &mut buffer, &tag),
            Err(AeadError::IntegrityViolation)
        );
    }
}
