//! Pure `no_std` RFC 8439 ChaCha20 Stream Cipher, Poly1305 Authenticator,
//! and ChaCha20-Poly1305 AEAD Construction.
//!
//! Provides the primary symmetric cipher suite for TLS 1.3:
//!   TLS_CHACHA20_POLY1305_SHA256 (0x1303)
//! Constant-time, zero heap allocations, zero third-party dependencies.

/// ChaCha20 key size in bytes.
pub const KEY_LEN: usize = 32;

/// ChaCha20 nonce size in bytes (RFC 8439).
pub const NONCE_LEN: usize = 12;

/// Poly1305 authentication tag size in bytes.
pub const TAG_LEN: usize = 16;

#[inline(always)]
fn qr(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

/// Generates a single 64-byte ChaCha20 block.
pub fn chacha20_block(key: &[u8; KEY_LEN], counter: u32, nonce: &[u8; NONCE_LEN]) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[0] = 0x61707865; // "expa"
    state[1] = 0x3320646e; // "nd 3"
    state[2] = 0x79622d32; // "2-by"
    state[3] = 0x6b206574; // "te k"

    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes([
            key[i * 4],
            key[i * 4 + 1],
            key[i * 4 + 2],
            key[i * 4 + 3],
        ]);
    }

    state[12] = counter;

    for i in 0..3 {
        state[13 + i] = u32::from_le_bytes([
            nonce[i * 4],
            nonce[i * 4 + 1],
            nonce[i * 4 + 2],
            nonce[i * 4 + 3],
        ]);
    }

    let mut working = state;

    for _ in 0..10 {
        // Column rounds
        qr(&mut working, 0, 4, 8, 12);
        qr(&mut working, 1, 5, 9, 13);
        qr(&mut working, 2, 6, 10, 14);
        qr(&mut working, 3, 7, 11, 15);
        // Diagonal rounds
        qr(&mut working, 0, 5, 10, 15);
        qr(&mut working, 1, 6, 11, 12);
        qr(&mut working, 2, 7, 8, 13);
        qr(&mut working, 3, 4, 9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let word = working[i].wrapping_add(state[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

/// Encrypts/decrypts data in-place by XORing with ChaCha20 keystream.
pub fn chacha20_xor(
    key: &[u8; KEY_LEN],
    mut counter: u32,
    nonce: &[u8; NONCE_LEN],
    data: &mut [u8],
) {
    let mut offset = 0;
    while offset < data.len() {
        let block = chacha20_block(key, counter, nonce);
        let chunk_len = (data.len() - offset).min(64);
        for i in 0..chunk_len {
            data[offset + i] ^= block[i];
        }
        offset += chunk_len;
        counter = counter.wrapping_add(1);
    }
}

/// Streaming Poly1305 MAC authenticator (RFC 8439 Section 2.5).
pub struct Poly1305 {
    r: [u32; 5],
    h: [u32; 5],
    s_bytes: [u8; 16],
    buffer: [u8; 16],
    buffer_len: usize,
}

impl Poly1305 {
    /// Initializes a Poly1305 session with a 32-byte one-time key.
    pub fn new(key: &[u8; 32]) -> Self {
        let mut r_bytes = [0u8; 16];
        r_bytes.copy_from_slice(&key[0..16]);
        r_bytes[3] &= 15;
        r_bytes[7] &= 15;
        r_bytes[11] &= 15;
        r_bytes[15] &= 15;
        r_bytes[4] &= 252;
        r_bytes[8] &= 252;
        r_bytes[12] &= 252;

        let r0 = u32::from_le_bytes([r_bytes[0], r_bytes[1], r_bytes[2], r_bytes[3]]) & 0x3ffffff;
        let r1 = (u32::from_le_bytes([r_bytes[3], r_bytes[4], r_bytes[5], r_bytes[6]]) >> 2) & 0x3ffffff;
        let r2 = (u32::from_le_bytes([r_bytes[6], r_bytes[7], r_bytes[8], r_bytes[9]]) >> 4) & 0x3ffffff;
        let r3 = (u32::from_le_bytes([r_bytes[9], r_bytes[10], r_bytes[11], r_bytes[12]]) >> 6) & 0x3ffffff;
        let r4 = u32::from_le_bytes([r_bytes[13], r_bytes[14], r_bytes[15], 0]) & 0x3ffffff;

        let mut s_bytes = [0u8; 16];
        s_bytes.copy_from_slice(&key[16..32]);

        Self {
            r: [r0, r1, r2, r3, r4],
            h: [0, 0, 0, 0, 0],
            s_bytes,
            buffer: [0u8; 16],
            buffer_len: 0,
        }
    }

    fn process_block(&mut self, block: &[u8], is_final_block: bool) {
        let mut b = [0u8; 17];
        b[..block.len()].copy_from_slice(block);
        b[block.len()] = 0x01;

        let mask = (1u32 << 26) - 1;
        let w0 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) & mask;
        let w1 = (u32::from_le_bytes([b[3], b[4], b[5], b[6]]) >> 2) & mask;
        let w2 = (u32::from_le_bytes([b[6], b[7], b[8], b[9]]) >> 4) & mask;
        let w3 = (u32::from_le_bytes([b[9], b[10], b[11], b[12]]) >> 6) & mask;
        let w4 = u32::from_le_bytes([b[13], b[14], b[15], b[16]]) & mask;

        let h0 = self.h[0] + w0;
        let h1 = self.h[1] + w1;
        let h2 = self.h[2] + w2;
        let h3 = self.h[3] + w3;
        let h4 = self.h[4] + w4;

        let r0 = self.r[0] as u64;
        let r1 = self.r[1] as u64;
        let r2 = self.r[2] as u64;
        let r3 = self.r[3] as u64;
        let r4 = self.r[4] as u64;

        let s1 = r1 * 5;
        let s2 = r2 * 5;
        let s3 = r3 * 5;
        let s4 = r4 * 5;

        let d0 = (h0 as u64) * r0 + (h1 as u64) * s4 + (h2 as u64) * s3 + (h3 as u64) * s2 + (h4 as u64) * s1;
        let d1 = (h0 as u64) * r1 + (h1 as u64) * r0 + (h2 as u64) * s4 + (h3 as u64) * s3 + (h4 as u64) * s2;
        let d2 = (h0 as u64) * r2 + (h1 as u64) * r1 + (h2 as u64) * r0 + (h3 as u64) * s4 + (h4 as u64) * s3;
        let d3 = (h0 as u64) * r3 + (h1 as u64) * r2 + (h2 as u64) * r1 + (h3 as u64) * r0 + (h4 as u64) * s4;
        let d4 = (h0 as u64) * r4 + (h1 as u64) * r3 + (h2 as u64) * r2 + (h3 as u64) * r1 + (h4 as u64) * r0;

        let mask64 = (1u64 << 26) - 1;
        let c0 = d0 >> 26;
        let mut nh0 = (d0 & mask64) as u32;

        let d1 = d1 + c0;
        let c1 = d1 >> 26;
        let mut nh1 = (d1 & mask64) as u32;

        let d2 = d2 + c1;
        let c2 = d2 >> 26;
        let nh2 = (d2 & mask64) as u32;

        let d3 = d3 + c2;
        let c3 = d3 >> 26;
        let nh3 = (d3 & mask64) as u32;

        let d4 = d4 + c3;
        let c4 = d4 >> 26;
        let nh4 = (d4 & mask64) as u32;

        let d0_rem = (nh0 as u64) + c4 * 5;
        nh0 = (d0_rem & mask64) as u32;
        nh1 += (d0_rem >> 26) as u32;

        self.h[0] = nh0;
        self.h[1] = nh1;
        self.h[2] = nh2;
        self.h[3] = nh3;
        self.h[4] = nh4;
        let _ = is_final_block;
    }

    /// Feeds data into Poly1305.
    pub fn update(&mut self, data: &[u8]) {
        let mut offset = 0;

        if self.buffer_len > 0 {
            let needed = 16 - self.buffer_len;
            if data.len() >= needed {
                self.buffer[self.buffer_len..16].copy_from_slice(&data[..needed]);
                let buf = self.buffer;
                self.process_block(&buf, false);
                self.buffer_len = 0;
                offset = needed;
            } else {
                self.buffer[self.buffer_len..self.buffer_len + data.len()].copy_from_slice(data);
                self.buffer_len += data.len();
                return;
            }
        }

        while offset + 16 <= data.len() {
            self.process_block(&data[offset..offset + 16], false);
            offset += 16;
        }

        let remaining = data.len() - offset;
        if remaining > 0 {
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    /// Finalizes and computes the 16-byte Poly1305 authentication tag.
    pub fn finalize(mut self) -> [u8; TAG_LEN] {
        if self.buffer_len > 0 {
            let mut remaining = [0u8; 16];
            remaining[..self.buffer_len].copy_from_slice(&self.buffer[..self.buffer_len]);
            let len = self.buffer_len;
            self.process_block(&remaining[..len], true);
        }

        // Full carry
        let mask = (1u32 << 26) - 1;
        let mut c = self.h[0] >> 26; self.h[0] &= mask;
        self.h[1] += c; c = self.h[1] >> 26; self.h[1] &= mask;
        self.h[2] += c; c = self.h[2] >> 26; self.h[2] &= mask;
        self.h[3] += c; c = self.h[3] >> 26; self.h[3] &= mask;
        self.h[4] += c; c = self.h[4] >> 26; self.h[4] &= mask;
        self.h[0] += c * 5;
        c = self.h[0] >> 26; self.h[0] &= mask;
        self.h[1] += c;

        // Compute g = h + 5 to test if h >= 2^130 - 5
        let g0 = self.h[0] + 5;
        let mut c = g0 >> 26;
        let g0 = g0 & mask;
        let g1 = self.h[1] + c;
        c = g1 >> 26;
        let g1 = g1 & mask;
        let g2 = self.h[2] + c;
        c = g2 >> 26;
        let g2 = g2 & mask;
        let g3 = self.h[3] + c;
        c = g3 >> 26;
        let g3 = g3 & mask;
        let g4 = self.h[4] + c;

        // If g4 has bit 26 set, then h >= 2^130 - 5, so select g; else h.
        let over = (g4 >> 26) != 0;
        let g4 = g4 & mask;

        let (h0, h1, h2, h3, h4) = if over {
            (g0, g1, g2, g3, g4)
        } else {
            (self.h[0], self.h[1], self.h[2], self.h[3], self.h[4])
        };

        let w0 = (h0 | (h1 << 26)) as u64;
        let w1 = ((h1 >> 6) | (h2 << 20)) as u64;
        let w2 = ((h2 >> 12) | (h3 << 14)) as u64;
        let w3 = ((h3 >> 18) | (h4 << 8)) as u64;

        let mut h_bytes = [0u8; 16];
        h_bytes[0..4].copy_from_slice(&(w0 as u32).to_le_bytes());
        h_bytes[4..8].copy_from_slice(&(w1 as u32).to_le_bytes());
        h_bytes[8..12].copy_from_slice(&(w2 as u32).to_le_bytes());
        h_bytes[12..16].copy_from_slice(&(w3 as u32).to_le_bytes());

        let h_int = u128::from_le_bytes(h_bytes);
        let s_int = u128::from_le_bytes(self.s_bytes);
        let tag_int = h_int.wrapping_add(s_int);
        tag_int.to_le_bytes()
    }

    /// Computes Poly1305 MAC over `message` in one shot.
    pub fn compute(key: &[u8; 32], message: &[u8]) -> [u8; TAG_LEN] {
        let mut poly = Self::new(key);
        poly.update(message);
        poly.finalize()
    }
}

/// ChaCha20-Poly1305 AEAD Authenticated Encryption (RFC 8439 Section 2.8).
pub struct ChaCha20Poly1305;

impl ChaCha20Poly1305 {
    /// Encrypts `plaintext` with associated data `aad`, writing the ciphertext into `ciphertext`
    /// and the 16-byte authentication tag into `tag`.
    ///
    /// `ciphertext` length must equal `plaintext.len()`.
    pub fn encrypt(
        key: &[u8; KEY_LEN],
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        plaintext: &[u8],
        ciphertext: &mut [u8],
        tag: &mut [u8; TAG_LEN],
    ) {
        assert_eq!(ciphertext.len(), plaintext.len());

        // 1. Generate Poly1305 one-time key from ChaCha20 counter 0
        let otk_block = chacha20_block(key, 0, nonce);
        let mut otk = [0u8; 32];
        otk.copy_from_slice(&otk_block[0..32]);

        // 2. Encrypt plaintext starting at counter 1
        ciphertext.copy_from_slice(plaintext);
        chacha20_xor(key, 1, nonce, ciphertext);

        // 3. Authenticate with Poly1305
        let mut poly = Poly1305::new(&otk);
        poly.update(aad);
        let pad_aad = (16 - (aad.len() % 16)) % 16;
        if pad_aad > 0 {
            poly.update(&[0u8; 16][..pad_aad]);
        }

        poly.update(ciphertext);
        let pad_ct = (16 - (ciphertext.len() % 16)) % 16;
        if pad_ct > 0 {
            poly.update(&[0u8; 16][..pad_ct]);
        }

        let aad_len = (aad.len() as u64).to_le_bytes();
        let ct_len = (ciphertext.len() as u64).to_le_bytes();
        poly.update(&aad_len);
        poly.update(&ct_len);

        *tag = poly.finalize();
    }

    /// Decrypts `ciphertext` with associated data `aad` and authentication tag `tag`.
    ///
    /// Returns `Ok(())` on success writing plaintext to `plaintext`, or `Err(())` if authentication fails.
    /// `plaintext` length must equal `ciphertext.len()`.
    pub fn decrypt(
        key: &[u8; KEY_LEN],
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        ciphertext: &[u8],
        tag: &[u8; TAG_LEN],
        plaintext: &mut [u8],
    ) -> Result<(), ()> {
        if plaintext.len() != ciphertext.len() {
            return Err(());
        }

        // 1. Generate Poly1305 one-time key
        let otk_block = chacha20_block(key, 0, nonce);
        let mut otk = [0u8; 32];
        otk.copy_from_slice(&otk_block[0..32]);

        // 2. Compute expected tag
        let mut poly = Poly1305::new(&otk);
        poly.update(aad);
        let pad_aad = (16 - (aad.len() % 16)) % 16;
        if pad_aad > 0 {
            poly.update(&[0u8; 16][..pad_aad]);
        }

        poly.update(ciphertext);
        let pad_ct = (16 - (ciphertext.len() % 16)) % 16;
        if pad_ct > 0 {
            poly.update(&[0u8; 16][..pad_ct]);
        }

        let aad_len = (aad.len() as u64).to_le_bytes();
        let ct_len = (ciphertext.len() as u64).to_le_bytes();
        poly.update(&aad_len);
        poly.update(&ct_len);

        let expected_tag = poly.finalize();

        // 3. Constant-time verification
        let mut diff = 0u8;
        for i in 0..TAG_LEN {
            diff |= expected_tag[i] ^ tag[i];
        }
        if diff != 0 {
            return Err(());
        }

        // 4. Decrypt ciphertext
        plaintext.copy_from_slice(ciphertext);
        chacha20_xor(key, 1, nonce, plaintext);
        Ok(())
    }
}
