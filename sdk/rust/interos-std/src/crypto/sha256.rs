//! Pure `no_std` Implementation of Standard SHA-256 (FIPS PUB 180-4)
//!
//! Provides deterministic cryptographic hashing for TLS 1.3 transcript hashes,
//! HMAC-SHA256, and HKDF. Zero third-party dependencies, zero allocations.

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

/// SHA-256 state machine.
#[derive(Clone, Copy)]
pub struct Sha256 {
    h: [u32; 8],
    buffer: [u8; SHA256_BLOCK_SIZE],
    buffer_len: usize,
    total_bytes: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// Creates a new SHA-256 hasher initialized to standard constants.
    pub const fn new() -> Self {
        Self {
            h: H_INIT,
            buffer: [0u8; SHA256_BLOCK_SIZE],
            buffer_len: 0,
            total_bytes: 0,
        }
    }

    /// Appends arbitrary bytes to the hashing state.
    pub fn update(&mut self, data: &[u8]) {
        self.total_bytes += data.len() as u64;
        let mut offset = 0;

        // If buffer already has bytes, fill it first
        if self.buffer_len > 0 {
            let needed = SHA256_BLOCK_SIZE - self.buffer_len;
            if data.len() >= needed {
                self.buffer[self.buffer_len..SHA256_BLOCK_SIZE]
                    .copy_from_slice(&data[..needed]);
                let buf = self.buffer;
                self.process_block(&buf);
                self.buffer_len = 0;
                offset = needed;
            } else {
                self.buffer[self.buffer_len..self.buffer_len + data.len()]
                    .copy_from_slice(data);
                self.buffer_len += data.len();
                return;
            }
        }

        // Process 64-byte chunks directly
        while offset + SHA256_BLOCK_SIZE <= data.len() {
            let mut chunk = [0u8; SHA256_BLOCK_SIZE];
            chunk.copy_from_slice(&data[offset..offset + SHA256_BLOCK_SIZE]);
            self.process_block(&chunk);
            offset += SHA256_BLOCK_SIZE;
        }

        // Store remaining bytes
        let remaining = data.len() - offset;
        if remaining > 0 {
            self.buffer[..remaining].copy_from_slice(&data[offset..]);
            self.buffer_len = remaining;
        }
    }

    /// Computes and returns the final 32-byte digest.
    pub fn finalize(mut self) -> [u8; SHA256_DIGEST_LENGTH] {
        let total_bits = self.total_bytes * 8;

        // Append '1' bit (0x80 byte)
        self.buffer[self.buffer_len] = 0x80;
        self.buffer_len += 1;

        // If not enough room for 8-byte length at end, pad with zeros and process
        if self.buffer_len > 56 {
            self.buffer[self.buffer_len..SHA256_BLOCK_SIZE].fill(0);
            let buf = self.buffer;
            self.process_block(&buf);
            self.buffer_len = 0;
        }

        // Pad with zeros up to 56 bytes
        self.buffer[self.buffer_len..56].fill(0);

        // Append 64-bit length in big-endian
        let len_bytes = total_bits.to_be_bytes();
        self.buffer[56..64].copy_from_slice(&len_bytes);

        let buf = self.buffer;
        self.process_block(&buf);

        // Produce 32-byte digest
        let mut digest = [0u8; SHA256_DIGEST_LENGTH];
        for (i, &word) in self.h.iter().enumerate() {
            let bytes = word.to_be_bytes();
            digest[i * 4..(i + 1) * 4].copy_from_slice(&bytes);
        }

        digest
    }

    /// Helper for one-shot hashing.
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
