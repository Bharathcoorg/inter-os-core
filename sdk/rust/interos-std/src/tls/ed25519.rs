// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Pure `#![no_std]` Ed25519 RFC 8032 Signature Verification
//!
//! Provides sovereign on-device Ed25519 signature verification:
//! - SHA-512 (FIPS PUB 180-4 / RFC 6234)
//! - Radix-2^51 field arithmetic over GF(2^255 - 19)
//! - Twisted Edwards curve point decompression, addition, and scalar multiplication
//! - RFC 8032 Section 5.1.7 signature verification
//!
//! Pure Rust, zero third-party dependencies, zero heap allocations.

use crate::crypto::x25519::Fe;
use super::TlsError;

// =========================================================================
// 1. SHA-512 (FIPS PUB 180-4)
// =========================================================================

const SHA512_K: [u64; 80] = [
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

#[derive(Clone, Copy)]
pub struct Sha512 {
    state: [u64; 8],
    buffer: [u8; 128],
    buf_len: usize,
    total_len: u128,
}

impl Sha512 {
    pub const fn new() -> Self {
        Self {
            state: [
                0x6a09e667f3bcc908,
                0xbb67ae8584caa73b,
                0x3c6ef372fe94f82b,
                0xa54ff53a5f1d36f1,
                0x510e527fade682d1,
                0x9b05688c2b3e6c1f,
                0x1f83d9abfb41bd6b,
                0x5be0cd19137e2179,
            ],
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
                let buf = self.buffer;
                self.process_block(&buf);
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
                .wrapping_add(SHA512_K[i])
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

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }

    pub fn finalize(mut self) -> [u8; 64] {
        let bit_len = self.total_len * 8;
        self.buffer[self.buf_len] = 0x80;
        self.buf_len += 1;

        if self.buf_len > 112 {
            for i in self.buf_len..128 {
                self.buffer[i] = 0;
            }
            let buf = self.buffer;
            self.process_block(&buf);
            self.buf_len = 0;
        }

        for i in self.buf_len..112 {
            self.buffer[i] = 0;
        }

        // 128-bit big-endian length
        self.buffer[112..120].copy_from_slice(&0u64.to_be_bytes());
        self.buffer[120..128].copy_from_slice(&(bit_len as u64).to_be_bytes());
        let buf = self.buffer;
        self.process_block(&buf);

        let mut out = [0u8; 64];
        for i in 0..8 {
            out[i * 8..(i + 1) * 8].copy_from_slice(&self.state[i].to_be_bytes());
        }
        out
    }

    pub fn digest(data: &[u8]) -> [u8; 64] {
        let mut hasher = Self::new();
        hasher.update(data);
        hasher.finalize()
    }
}

// =========================================================================
// 2. Curve25519 & Ed25519 Group Arithmetic
// =========================================================================

// Curve constants
// d = -121665 / 121666 mod 2^255 - 19
const D_BYTES: [u8; 32] = [
    0xa3, 0x78, 0x59, 0x13, 0xca, 0x4d, 0xeb, 0x75, 0xab, 0xd8, 0x41, 0x41, 0x4d, 0x0a, 0x70, 0x00,
    0x98, 0xe8, 0x79, 0x77, 0x79, 0x40, 0xc7, 0x8c, 0x73, 0xfe, 0x6f, 0x2b, 0xee, 0x6c, 0x03, 0x52,
];

// I = sqrt(-1) mod 2^255 - 19
const I_BYTES: [u8; 32] = [
    0xb0, 0xa0, 0x0e, 0x4a, 0x27, 0x1b, 0xee, 0xc4, 0x78, 0xe4, 0x2f, 0xad, 0x06, 0x18, 0x43, 0x2f,
    0xa7, 0xd7, 0xfb, 0x3d, 0x99, 0x00, 0x4d, 0x2b, 0x0b, 0xdf, 0xc1, 0x4f, 0x80, 0x24, 0x83, 0x2b,
];

// Ed25519 Base Point B = (xB, yB)
const BASE_X_BYTES: [u8; 32] = [
    26, 213, 37, 143, 96, 45, 86, 201, 178, 167, 37, 149, 96, 199, 44, 105, 92, 220, 214, 253, 49,
    226, 164, 192, 254, 83, 110, 205, 211, 54, 105, 33,
];

const BASE_Y_BYTES: [u8; 32] = [
    0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
];

// Group order L = 2^252 + 27742317777372353535851937790883648493
const L_LIMBS: [u64; 4] = [
    0x5812631a5cf5d3ed,
    0x14def9dea2f79cd6,
    0x0000000000000000,
    0x1000000000000000,
];

/// Compares two 256-bit little-endian numbers: returns true if a >= b.
fn gte_256(a: &[u64; 4], b: &[u64; 4]) -> bool {
    for i in (0..4).rev() {
        if a[i] > b[i] {
            return true;
        }
        if a[i] < b[i] {
            return false;
        }
    }
    true
}

/// Subtracts b from a: a = a - b (assuming a >= b).
fn sub_256(a: &mut [u64; 4], b: &[u64; 4]) {
    let mut borrow = 0u64;
    for i in 0..4 {
        let (diff1, b1) = a[i].overflowing_sub(b[i]);
        let (diff2, b2) = diff1.overflowing_sub(borrow);
        a[i] = diff2;
        borrow = (b1 as u64) + (b2 as u64);
    }
}

/// Reduces a 512-bit little-endian integer modulo L.
fn reduce_mod_l(h: &[u8; 64]) -> [u8; 32] {
    let mut rem = [0u64; 4];
    for bit_idx in (0..512).rev() {
        let byte = h[bit_idx / 8];
        let bit = ((byte >> (bit_idx % 8)) & 1) as u64;

        let mut carry = bit;
        for limb in &mut rem {
            let next_carry = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = next_carry;
        }

        if carry != 0 || gte_256(&rem, &L_LIMBS) {
            sub_256(&mut rem, &L_LIMBS);
        }
    }

    let mut out = [0u8; 32];
    for i in 0..4 {
        out[i * 8..(i + 1) * 8].copy_from_slice(&rem[i].to_le_bytes());
    }
    out
}

/// Checks that S < L.
fn check_scalar_lt_l(s: &[u8; 32]) -> bool {
    let mut s_limbs = [0u64; 4];
    for i in 0..4 {
        let mut arr = [0u8; 8];
        arr.copy_from_slice(&s[i * 8..(i + 1) * 8]);
        s_limbs[i] = u64::from_le_bytes(arr);
    }
    // Strict inequality: not (s >= L)
    !gte_256(&s_limbs, &L_LIMBS)
}

/// Extended Twisted Edwards Point (X : Y : Z : T) where x = X/Z, y = Y/Z, xy = T/Z.
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub x: Fe,
    pub y: Fe,
    pub z: Fe,
    pub t: Fe,
}

impl Point {
    pub fn identity() -> Self {
        Self {
            x: Fe::zero(),
            y: Fe::one(),
            z: Fe::one(),
            t: Fe::zero(),
        }
    }

    pub fn base() -> Self {
        let x = Fe::from_bytes(&BASE_X_BYTES);
        let y = Fe::from_bytes(&BASE_Y_BYTES);
        let z = Fe::one();
        let t = x.mul(&y);
        Self { x, y, z, t }
    }

    /// Point addition: P3 = self + other.
    pub fn add(&self, other: &Point) -> Point {
        let d = Fe::from_bytes(&D_BYTES);
        let d2 = d.add(&d);

        let a = self.y.sub(&self.x).mul(&other.y.sub(&other.x));
        let b = self.y.add(&self.x).mul(&other.y.add(&other.x));
        let c = self.t.mul(&other.t).mul(&d2);
        let d_val = self.z.mul(&other.z).mul_scalar(2);
        let e = b.sub(&a);
        let f = d_val.sub(&c);
        let g = d_val.add(&c);
        let h = b.add(&a);

        Point {
            x: e.mul(&f),
            y: g.mul(&h),
            z: f.mul(&g),
            t: e.mul(&h),
        }
    }

    /// Point doubling: P3 = 2 * self.
    pub fn double(&self) -> Point {
        let a = self.x.sq();
        let b = self.y.sq();
        let c = self.z.sq().mul_scalar(2);
        let d = Fe::zero().sub(&a);
        let e = self.x.add(&self.y).sq().sub(&a).sub(&b);
        let g = d.add(&b);
        let f = g.sub(&c);
        let h = d.sub(&b);

        Point {
            x: e.mul(&f),
            y: g.mul(&h),
            z: f.mul(&g),
            t: e.mul(&h),
        }
    }

    /// Scalar multiplication by a 256-bit scalar: R = [scalar] self.
    pub fn scalar_mul(&self, scalar: &[u8; 32]) -> Point {
        let mut r = Point::identity();
        for bit_idx in (0..256).rev() {
            r = r.double();
            let byte = scalar[bit_idx / 8];
            if ((byte >> (bit_idx % 8)) & 1) == 1 {
                r = r.add(self);
            }
        }
        r
    }

    /// Checks if two projective points represent the same affine point.
    pub fn equals(&self, other: &Point) -> bool {
        let x1z2 = self.x.mul(&other.z);
        let x2z1 = other.x.mul(&self.z);
        let y1z2 = self.y.mul(&other.z);
        let y2z1 = other.y.mul(&self.z);
        x1z2.to_bytes() == x2z1.to_bytes() && y1z2.to_bytes() == y2z1.to_bytes()
    }
}

/// Computes a^((p-5)/8) mod 2^255 - 19.
fn pow_p_minus_5_div_8(a: &Fe) -> Fe {
    // (p-5)/8 = 2^252 - 3
    // Compute a^(2^250 - 1) using the same addition chain as inversion:
    let mut z2 = a.sq();
    z2 = z2.mul(a); // 2^2 - 1

    let mut z4 = z2.sq();
    z4 = z4.sq();
    z4 = z4.mul(&z2); // 2^4 - 1

    let mut z8 = z4;
    for _ in 0..4 { z8 = z8.sq(); }
    z8 = z8.mul(&z4); // 2^8 - 1

    let mut z16 = z8;
    for _ in 0..8 { z16 = z16.sq(); }
    z16 = z16.mul(&z8); // 2^16 - 1

    let mut z32 = z16;
    for _ in 0..16 { z32 = z32.sq(); }
    z32 = z32.mul(&z16); // 2^32 - 1

    let mut z64 = z32;
    for _ in 0..32 { z64 = z64.sq(); }
    z64 = z64.mul(&z32); // 2^64 - 1

    let mut z128 = z64;
    for _ in 0..64 { z128 = z128.sq(); }
    z128 = z128.mul(&z64); // 2^128 - 1

    // Build 2^250 - 1
    let mut t = z128;
    for _ in 0..64 { t = t.sq(); }
    t = t.mul(&z64); // 128 + 64 = 192 ones

    for _ in 0..32 { t = t.sq(); }
    t = t.mul(&z32); // 192 + 32 = 224 ones

    for _ in 0..16 { t = t.sq(); }
    t = t.mul(&z16); // 224 + 16 = 240 ones

    for _ in 0..8 { t = t.sq(); }
    t = t.mul(&z8); // 240 + 8 = 248 ones

    for _ in 0..2 { t = t.sq(); }
    t = t.mul(&z2); // 248 + 2 = 250 ones

    // (2^250 - 1) * 4 + 1 = 2^252 - 3
    t = t.sq();
    t = t.sq();
    t.mul(a)
}

/// Decompresses a 32-byte Ed25519 point according to RFC 8032 Section 5.1.3.
pub fn decompress_point(bytes: &[u8; 32]) -> Result<Point, TlsError> {
    let x_0 = (bytes[31] >> 7) & 1;

    let mut y_bytes = *bytes;
    y_bytes[31] &= 0x7f;
    let y = Fe::from_bytes(&y_bytes);

    let d = Fe::from_bytes(&D_BYTES);
    let i_val = Fe::from_bytes(&I_BYTES);

    let u = y.sq().sub(&Fe::one());
    let v = d.mul(&y.sq()).add(&Fe::one());

    // Candidate root x = (u * v^3) * (u * v^7)^((p-5)/8)
    let v2 = v.sq();
    let v3 = v2.mul(&v);
    let v7 = v3.sq().mul(&v);
    let uv7 = u.mul(&v7);
    let mut x = u.mul(&v3).mul(&pow_p_minus_5_div_8(&uv7));

    let vx2 = v.mul(&x.sq());
    if vx2.to_bytes() == u.to_bytes() {
        // x is square root
    } else if vx2.to_bytes() == Fe::zero().sub(&u).to_bytes() {
        x = x.mul(&i_val);
    } else {
        return Err(TlsError::InvalidSignature);
    }

    if (x.to_bytes()[0] & 1) != x_0 {
        x = Fe::zero().sub(&x);
    }

    let z = Fe::one();
    let t = x.mul(&y);
    Ok(Point { x, y, z, t })
}

/// Verifies a detached Ed25519 signature according to RFC 8032 Section 5.1.7.
///
/// Inputs:
/// - `public_key`: 32 bytes Ed25519 public key
/// - `message`: arbitrary byte slice
/// - `signature`: 64 bytes (R || S)
pub fn verify_ed25519(public_key: &[u8], message: &[u8], signature: &[u8]) -> Result<(), TlsError> {
    if public_key.len() != 32 || signature.len() != 64 {
        return Err(TlsError::InvalidSignature);
    }

    let mut pk_bytes = [0u8; 32];
    pk_bytes.copy_from_slice(public_key);

    let mut r_bytes = [0u8; 32];
    r_bytes.copy_from_slice(&signature[..32]);

    let mut s_bytes = [0u8; 32];
    s_bytes.copy_from_slice(&signature[32..]);

    // Check S < L
    if !check_scalar_lt_l(&s_bytes) {
        return Err(TlsError::InvalidSignature);
    }

    // Decode public key point A and signature point R
    let point_a = decompress_point(&pk_bytes)?;
    let point_r = decompress_point(&r_bytes)?;

    // Compute k = SHA-512(R || A || M) mod L
    let mut hasher = Sha512::new();
    hasher.update(&r_bytes);
    hasher.update(&pk_bytes);
    hasher.update(message);
    let hash = hasher.finalize();
    let k = reduce_mod_l(&hash);

    // Check [S] B == R + [k] A
    let base = Point::base();
    let sb = base.scalar_mul(&s_bytes);
    let ka = point_a.scalar_mul(&k);
    let r_plus_ka = point_r.add(&ka);

    if sb.equals(&r_plus_ka) {
        Ok(())
    } else {
        Err(TlsError::InvalidSignature)
    }
}
