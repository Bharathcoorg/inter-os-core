// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Pure `no_std` RFC 7748 X25519 Montgomery Curve Key Exchange
//!
//! Implements Montgomery curve25519 scalar multiplication:
//!   y^2 = x^3 + 486662 x^2 + x  (mod 2^255 - 19)
//! using 5x51-bit radix-2^51 limb arithmetic.
//! Constant-time, zero heap allocations, zero third-party dependencies.

/// Standard X25519 base point u = 9.
pub const BASE_POINT: [u8; 32] = [
    9, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0,
];

/// Field element in GF(2^255 - 19) represented by 5 limbs of 51 bits.
/// Value = sum_{i=0..4} limb[i] * 2^(51 * i).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fe(pub [u64; 5]);

impl Fe {
    pub const fn zero() -> Self {
        Fe([0, 0, 0, 0, 0])
    }

    pub const fn one() -> Self {
        Fe([1, 0, 0, 0, 0])
    }

    /// Decodes a 32-byte little-endian array into a field element,
    /// masking the top bit according to RFC 7748 Section 5.
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        let mut b = *bytes;
        b[31] &= 0x7f;

        let load64 = |s: &[u8]| -> u64 {
            let mut arr = [0u8; 8];
            arr.copy_from_slice(s);
            u64::from_le_bytes(arr)
        };

        let mask51 = (1u64 << 51) - 1;
        let w0 = load64(&b[0..8]);
        let w1 = load64(&b[6..14]) >> 3;
        let w2 = load64(&b[12..20]) >> 6;
        let w3 = load64(&b[19..27]) >> 1;
        let w4 = load64(&b[24..32]) >> 12;

        Fe([
            w0 & mask51,
            w1 & mask51,
            w2 & mask51,
            w3 & mask51,
            w4 & mask51,
        ])
    }

    /// Carry propagation across 51-bit limbs.
    #[inline(always)]
    pub fn carry(&mut self) {
        let mask51 = (1u64 << 51) - 1;
        self.0[1] += self.0[0] >> 51;
        self.0[0] &= mask51;

        self.0[2] += self.0[1] >> 51;
        self.0[1] &= mask51;

        self.0[3] += self.0[2] >> 51;
        self.0[2] &= mask51;

        self.0[4] += self.0[3] >> 51;
        self.0[3] &= mask51;

        self.0[0] += (self.0[4] >> 51) * 19;
        self.0[4] &= mask51;

        self.0[1] += self.0[0] >> 51;
        self.0[0] &= mask51;
    }

    /// Encodes this field element to a canonical 32-byte little-endian array.
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut t = *self;
        t.carry();
        t.carry();

        // Check if t >= 2^255 - 19 by adding 19:
        let mut m = t;
        m.0[0] += 19;
        let mask51 = (1u64 << 51) - 1;
        m.0[1] += m.0[0] >> 51;
        m.0[0] &= mask51;

        m.0[2] += m.0[1] >> 51;
        m.0[1] &= mask51;

        m.0[3] += m.0[2] >> 51;
        m.0[2] &= mask51;

        m.0[4] += m.0[3] >> 51;
        m.0[3] &= mask51;

        let over = m.0[4] >> 51;
        let mask = 0u64.wrapping_sub(over);
        m.0[4] &= mask51;

        let r0 = (t.0[0] & !mask) | (m.0[0] & mask);
        let r1 = (t.0[1] & !mask) | (m.0[1] & mask);
        let r2 = (t.0[2] & !mask) | (m.0[2] & mask);
        let r3 = (t.0[3] & !mask) | (m.0[3] & mask);
        let r4 = (t.0[4] & !mask) | (m.0[4] & mask);

        let mut out = [0u8; 32];
        let w0 = r0 | (r1 << 51);
        let w1 = (r1 >> 13) | (r2 << 38);
        let w2 = (r2 >> 26) | (r3 << 25);
        let w3 = (r3 >> 39) | (r4 << 12);

        out[0..8].copy_from_slice(&w0.to_le_bytes());
        out[8..16].copy_from_slice(&w1.to_le_bytes());
        out[16..24].copy_from_slice(&w2.to_le_bytes());
        out[24..32].copy_from_slice(&w3.to_le_bytes());
        out
    }

    /// Constant-time conditional swap.
    #[inline(always)]
    pub fn cswap(swap: u64, a: &mut Fe, b: &mut Fe) {
        let mask = 0u64.wrapping_sub(swap);
        for i in 0..5 {
            let x = mask & (a.0[i] ^ b.0[i]);
            a.0[i] ^= x;
            b.0[i] ^= x;
        }
    }

    /// Field addition.
    #[inline(always)]
    pub fn add(&self, rhs: &Fe) -> Fe {
        let mut res = Fe([
            self.0[0] + rhs.0[0],
            self.0[1] + rhs.0[1],
            self.0[2] + rhs.0[2],
            self.0[3] + rhs.0[3],
            self.0[4] + rhs.0[4],
        ]);
        res.carry();
        res
    }

    /// Field subtraction modulo 2^255 - 19.
    #[inline(always)]
    pub fn sub(&self, rhs: &Fe) -> Fe {
        // Add 4 * p = 4 * (2^255 - 19) to completely prevent underflow
        // 4 * (2^51 - 19) = 2^53 - 76 = 0x1fffffffffffb4
        // 4 * (2^51 - 1)  = 2^53 - 4  = 0x1ffffffffffffc
        let p0 = 0x1fffffffffffb4u64;
        let p1 = 0x1ffffffffffffcu64;
        let mut res = Fe([
            (self.0[0] + p0) - rhs.0[0],
            (self.0[1] + p1) - rhs.0[1],
            (self.0[2] + p1) - rhs.0[2],
            (self.0[3] + p1) - rhs.0[3],
            (self.0[4] + p1) - rhs.0[4],
        ]);
        res.carry();
        res
    }

    /// Field multiplication modulo 2^255 - 19.
    pub fn mul(&self, rhs: &Fe) -> Fe {
        let a0 = self.0[0] as u128;
        let a1 = self.0[1] as u128;
        let a2 = self.0[2] as u128;
        let a3 = self.0[3] as u128;
        let a4 = self.0[4] as u128;

        let b0 = rhs.0[0] as u128;
        let b1 = rhs.0[1] as u128;
        let b2 = rhs.0[2] as u128;
        let b3 = rhs.0[3] as u128;
        let b4 = rhs.0[4] as u128;

        let b1_19 = b1 * 19;
        let b2_19 = b2 * 19;
        let b3_19 = b3 * 19;
        let b4_19 = b4 * 19;

        let c0 = a0 * b0 + a1 * b4_19 + a2 * b3_19 + a3 * b2_19 + a4 * b1_19;
        let c1 = a0 * b1 + a1 * b0    + a2 * b4_19 + a3 * b3_19 + a4 * b2_19;
        let c2 = a0 * b2 + a1 * b1    + a2 * b0    + a3 * b4_19 + a4 * b3_19;
        let c3 = a0 * b3 + a1 * b2    + a2 * b1    + a3 * b0    + a4 * b4_19;
        let c4 = a0 * b4 + a1 * b3    + a2 * b2    + a3 * b1    + a4 * b0;

        let mask51 = (1u128 << 51) - 1;

        let carry0 = c0 >> 51;
        let mut r0 = (c0 & mask51) as u64;

        let c1 = c1 + carry0;
        let carry1 = c1 >> 51;
        let mut r1 = (c1 & mask51) as u64;

        let c2 = c2 + carry1;
        let carry2 = c2 >> 51;
        let r2 = (c2 & mask51) as u64;

        let c3 = c3 + carry2;
        let carry3 = c3 >> 51;
        let r3 = (c3 & mask51) as u64;

        let c4 = c4 + carry3;
        let carry4 = c4 >> 51;
        let r4 = (c4 & mask51) as u64;

        let c0_rem = (r0 as u128) + carry4 * 19;
        r0 = (c0_rem & mask51) as u64;
        r1 += (c0_rem >> 51) as u64;

        let mut res = Fe([r0, r1, r2, r3, r4]);
        res.carry();
        res
    }

    /// Field squaring.
    #[inline(always)]
    pub fn sq(&self) -> Fe {
        self.mul(self)
    }

    /// Multiplies by a small scalar.
    pub fn mul_scalar(&self, s: u64) -> Fe {
        let s = s as u128;
        let mask51 = (1u128 << 51) - 1;

        let c0 = (self.0[0] as u128) * s;
        let carry0 = c0 >> 51;
        let mut r0 = (c0 & mask51) as u64;

        let c1 = (self.0[1] as u128) * s + carry0;
        let carry1 = c1 >> 51;
        let mut r1 = (c1 & mask51) as u64;

        let c2 = (self.0[2] as u128) * s + carry1;
        let carry2 = c2 >> 51;
        let r2 = (c2 & mask51) as u64;

        let c3 = (self.0[3] as u128) * s + carry2;
        let carry3 = c3 >> 51;
        let r3 = (c3 & mask51) as u64;

        let c4 = (self.0[4] as u128) * s + carry3;
        let carry4 = c4 >> 51;
        let r4 = (c4 & mask51) as u64;

        let c0_rem = (r0 as u128) + carry4 * 19;
        r0 = (c0_rem & mask51) as u64;
        r1 += (c0_rem >> 51) as u64;

        let mut res = Fe([r0, r1, r2, r3, r4]);
        res.carry();
        res
    }

    /// Modular inversion using Fermat's Little Theorem: a^(p-2) = a^(2^255 - 21).
    pub fn invert(&self) -> Fe {
        let mut z2 = self.sq();
        z2 = z2.mul(self); // 2^2 - 1

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

        // Final 5 bits: 0, 1, 0, 1, 1 (11)
        t = t.sq();             // bit 4 = 0
        t = t.sq().mul(self);   // bit 3 = 1
        t = t.sq();             // bit 2 = 0
        t = t.sq().mul(self);   // bit 1 = 1
        t = t.sq().mul(self);   // bit 0 = 1

        t
    }
}

/// Computes X25519 scalar multiplication according to RFC 7748.
pub fn x25519(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    let mut k = *scalar;
    k[0] &= 248;
    k[31] &= 127;
    k[31] |= 64;

    let u = Fe::from_bytes(point);
    let x1 = u;
    let mut x2 = Fe::one();
    let mut z2 = Fe::zero();
    let mut x3 = u;
    let mut z3 = Fe::one();
    let mut swap = 0u64;

    for i in (0..255).rev() {
        let byte = k[i / 8];
        let bit = ((byte >> (i % 8)) & 1) as u64;
        swap ^= bit;
        Fe::cswap(swap, &mut x2, &mut x3);
        Fe::cswap(swap, &mut z2, &mut z3);
        swap = bit;

        let a = x2.add(&z2);
        let aa = a.sq();
        let b = x2.sub(&z2);
        let bb = b.sq();
        let e = aa.sub(&bb);
        let c = x3.add(&z3);
        let d = x3.sub(&z3);
        let da = d.mul(&a);
        let cb = c.mul(&b);

        let da_plus_cb = da.add(&cb);
        x3 = da_plus_cb.sq();
        let da_minus_cb = da.sub(&cb);
        z3 = x1.mul(&da_minus_cb.sq());

        x2 = aa.mul(&bb);
        z2 = e.mul(&aa.add(&e.mul_scalar(121665)));
    }

    Fe::cswap(swap, &mut x2, &mut x3);
    Fe::cswap(swap, &mut z2, &mut z3);

    let result = x2.mul(&z2.invert());
    result.to_bytes()
}

/// Computes X25519 scalar multiplication with the standard base point u = 9.
pub fn x25519_base(scalar: &[u8; 32]) -> [u8; 32] {
    x25519(scalar, &BASE_POINT)
}
