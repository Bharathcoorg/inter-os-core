// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Pure `no_std` SHA-384 (FIPS PUB 180-4, Section 5.3.5).
//!
//! SHA-384 is SHA-512 run with a different initial state and read to 48 bytes
//! instead of 64 — the compression function is byte for byte the same one. It
//! lives here rather than beside the SHA-512 inside `tls::ed25519` because that
//! one is the Ed25519 verifier's own helper, returns the full 64 bytes, and is
//! pinned by tests of its own; giving the ECDSA P-384 path a hash by editing the
//! Ed25519 path would put a curve change through a signature scheme that is
//! already working. The cost is two tables of round constants, and both are
//! checked against the same published vectors, so a transcription slip in either
//! fails a test rather than silently disagreeing with the other.
//!
//! Zero third-party dependencies, zero heap allocations.

/// The 80 round constants: the first 64 bits of the fractional parts of the
/// cube roots of the first 80 primes.
const K: [u64; 80] = [
    0x428a2f98d728ae22, 0x7137449123ef65cd, 0xb5c0fbcfec4d3b2f, 0xe9b5dba58189dbbc,
    0x3956c25bf348b538, 0x59f111f1b605d019, 0x923f82a4af194f9b, 0xab1c5ed5da6d8118,
    0xd807aa98a3030242, 0x12835b0145706fbe, 0x243185be4ee4b28c, 0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f, 0x80deb1fe3b1696b1, 0x9bdc06a725c71235, 0xc19bf174cf692694,
    0xe49b69c19ef14ad2, 0xefbe4786384f25e3, 0x0fc19dc68b8cd5b5, 0x240ca1cc77ac9c65,
    0x2de92c6f592b0275, 0x4a7484aa6ea6e483, 0x5cb0a9dcbd41fbd4, 0x76f988da831153b5,
    0x983e5152ee66dfab, 0xa831c66d2db43210, 0xb00327c898fb213f, 0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2, 0xd5a79147930aa725, 0x06ca6351e003826f, 0x142929670a0e6e70,
    0x27b70a8546d22ffc, 0x2e1b21385c26c926, 0x4d2c6dfc5ac42aed, 0x53380d139d95b3df,
    0x650a73548baf63de, 0x766a0abb3c77b2a8, 0x81c2c92e47edaee6, 0x92722c851482353b,
    0xa2bfe8a14cf10364, 0xa81a664bbc423001, 0xc24b8b70d0f89791, 0xc76c51a30654be30,
    0xd192e819d6ef5218, 0xd69906245565a910, 0xf40e35855771202a, 0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8, 0x1e376c085141ab53, 0x2748774cdf8eeb99, 0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63, 0x4ed8aa4ae3418acb, 0x5b9cca4f7763e373, 0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc, 0x78a5636f43172f60, 0x84c87814a1f0ab72, 0x8cc702081a6439ec,
    0x90befffa23631e28, 0xa4506cebde82bde9, 0xbef9a3f7b2c67915, 0xc67178f2e372532b,
    0xca273eceea26619c, 0xd186b8c721c0c207, 0xeada7dd6cde0eb1e, 0xf57d4f7fee6ed178,
    0x06f067aa72176fba, 0x0a637dc5a2c898a6, 0x113f9804bef90dae, 0x1b710b35131c471b,
    0x28db77f523047d84, 0x32caab7b40c72493, 0x3c9ebe0a15c9bebc, 0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6, 0x597f299cfc657e2a, 0x5fcb6fab3ad6faec, 0x6c44198c4a475817,
];

/// The initial hash values for SHA-384, which FIPS 180-4 Section 5.3.5 takes as
/// the leading 64 bits of the fractional part of the square root of 23, 29, 31,
/// 37, 41, 43, 47 and 53 — the eight primes after the ones SHA-512 starts from.
/// That is the whole difference between the two starting states, and the reason
/// the vectors below are SHA-384's own: a single wrong word here yields a digest
/// that is deterministic, well-formed and useless, which is what the
/// published-vector tests are for.
const IV: [u64; 8] = [
    0xcbbb9d5dc1059ed8,
    0x629a292a367cd507,
    0x9159015a3070dd17,
    0x152fecd8f70e5939,
    0x67332667ffc00b31,
    0x8eb44a8768581511,
    0xdb0c2e0d64f98fa7,
    0x47b5481dbefa4fa4,
];

#[derive(Clone, Copy)]
pub struct Sha384 {
    state: [u64; 8],
    buffer: [u8; 128],
    buf_len: usize,
    total_len: u128,
}

impl Sha384 {
    pub const fn new() -> Self {
        Self {
            state: IV,
            buffer: [0u8; 128],
            buf_len: 0,
            total_len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len += data.len() as u128;

        if self.buf_len > 0 {
            let space = 128 - self.buf_len;
            let take = space.min(data.len());
            self.buffer[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 128 {
                let block = self.buffer;
                self.process_block(&block);
                self.buf_len = 0;
            }
        }

        while data.len() >= 128 {
            let mut block = [0u8; 128];
            block.copy_from_slice(&data[..128]);
            self.process_block(&block);
            data = &data[128..];
        }

        if !data.is_empty() {
            self.buffer[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    fn process_block(&mut self, block: &[u8; 128]) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            let offset = i * 8;
            w[i] = u64::from_be_bytes([
                block[offset], block[offset + 1], block[offset + 2], block[offset + 3],
                block[offset + 4], block[offset + 5], block[offset + 6], block[offset + 7],
            ]);
        }

        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }

        let mut a = self.state[0];
        let mut b = self.state[1];
        let mut c = self.state[2];
        let mut d = self.state[3];
        let mut e = self.state[4];
        let mut f = self.state[5];
        let mut g = self.state[6];
        let mut h = self.state[7];

        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (state, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *state = state.wrapping_add(value);
        }
    }

    /// Pads, runs the final block, and returns the 48-byte digest — the first
    /// twelve of the eight state words, which is where SHA-384 and SHA-512
    /// part ways.
    pub fn finalize(mut self) -> [u8; 48] {
        let bit_len = self.total_len * 8;
        self.buffer[self.buf_len] = 0x80;
        self.buf_len += 1;

        if self.buf_len > 112 {
            for i in self.buf_len..128 {
                self.buffer[i] = 0;
            }
            let block = self.buffer;
            self.process_block(&block);
            self.buf_len = 0;
        }

        for i in self.buf_len..112 {
            self.buffer[i] = 0;
        }

        // The length field is 128 bits; a message this side of 2^64 bits never
        // uses the high half, which is written as zero rather than assumed.
        self.buffer[112..120].copy_from_slice(&0u64.to_be_bytes());
        self.buffer[120..128].copy_from_slice(&(bit_len as u64).to_be_bytes());
        let block = self.buffer;
        self.process_block(&block);

        let mut out = [0u8; 48];
        for i in 0..6 {
            out[i * 8..(i + 1) * 8].copy_from_slice(&self.state[i].to_be_bytes());
        }
        out
    }

    pub fn digest(data: &[u8]) -> [u8; 48] {
        let mut hasher = Self::new();
        hasher.update(data);
        hasher.finalize()
    }
}

#[cfg(test)]
mod tests {
    use super::Sha384;

    fn hex(value: &[u8; 48]) -> std::string::String {
        value.iter().map(|byte| std::format!("{byte:02x}")).collect()
    }

    #[test]
    fn matches_the_fips_vector_for_abc() {
        assert_eq!(
            hex(&Sha384::digest(b"abc")),
            "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded163\
             1a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"
        );
    }

    #[test]
    fn pads_across_the_block_boundary() {
        // The three cases that separate a correct padding rule from one that
        // happens to work on short messages: an empty input, a message that
        // fills a block exactly, and one byte past it — which needs a second
        // block for the length field alone.
        assert_eq!(
            hex(&Sha384::digest(b"")),
            "38b060a751ac96384cd9327eb1b1e36a21fdb71114be0743\
             4c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"
        );
        assert_eq!(
            hex(&Sha384::digest(&[b'a'; 127])),
            "9bd06b1763c2cf7aef40e795dc65bc96d59c41b537f3ad72\
             ebdefd485476b5717c1aeb37c327fe9c1831b12b9efd08ae"
        );
        assert_eq!(
            hex(&Sha384::digest(&[b'a'; 128])),
            "edb12730a366098b3b2beac75a3bef1b0969b15c48e2163c\
             23d96994f8d1bef760c7e27f3c464d3829f56c0d53808b0b"
        );
        assert_eq!(
            hex(&Sha384::digest(&[b'a'; 129])),
            "39b6f5a7b0e781dbc419f72e49b30eaac10f2c98c4403bc6\
             10da31067fd1b48f324138c8615d2b496d08d73d5e865326"
        );
    }

    #[test]
    fn feeding_the_same_bytes_in_pieces_gives_the_same_digest() {
        // Certificate bodies arrive in records, so `update` is called on
        // fragments far more often than on a whole message.
        let message: std::vec::Vec<u8> = (0u16..900).map(|n| n as u8).collect();
        let whole = Sha384::digest(&message);
        let mut hasher = Sha384::new();
        for chunk in message.chunks(64) {
            hasher.update(chunk);
        }
        assert_eq!(hasher.finalize(), whole);
    }
}
