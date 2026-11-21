// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Pure `#![no_std]` Cryptographic Primitives for APK Signature Verification
//!
//! Implements:
//! - Multiprecision modular arithmetic for RSA (up to 4096-bit).
//! - RSASSA-PKCS1-v1_5 verification with SHA-256.
//! - NIST P-256 (secp256r1) elliptic curve arithmetic and ECDSA verification with SHA-256.
//! - X.509 SubjectPublicKeyInfo (SPKI) ASN.1 DER parser.

use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedPublicKey {
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    EcdsaP256 {
        x: [u8; 32],
        y: [u8; 32],
    },
}

// -----------------------------------------------------------------------------
// Multiprecision Unsigned Integer (BigUint)
// -----------------------------------------------------------------------------

/// Little-endian limb representation (u64 limbs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BigUint {
    pub limbs: Vec<u64>,
}

impl BigUint {
    pub const fn zero() -> Self {
        Self { limbs: Vec::new() }
    }

    pub fn from_u64(val: u64) -> Self {
        if val == 0 {
            Self::zero()
        } else {
            let mut limbs = Vec::with_capacity(1);
            limbs.push(val);
            Self { limbs }
        }
    }

    pub fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut limbs = Vec::new();
        let mut i = bytes.len();
        while i > 0 {
            let start = i.saturating_sub(8);
            let mut limb = 0u64;
            for &b in &bytes[start..i] {
                limb = (limb << 8) | (b as u64);
            }
            limbs.push(limb);
            i = start;
        }
        let mut res = Self { limbs };
        res.trim();
        res
    }

    pub fn to_be_bytes(&self, target_len: usize) -> Vec<u8> {
        let mut out = alloc::vec![0u8; target_len];
        let mut byte_idx = target_len;
        for &limb in &self.limbs {
            let mut l = limb;
            for _ in 0..8 {
                if byte_idx == 0 {
                    break;
                }
                byte_idx -= 1;
                out[byte_idx] = (l & 0xFF) as u8;
                l >>= 8;
            }
            if byte_idx == 0 {
                break;
            }
        }
        out
    }

    pub fn is_zero(&self) -> bool {
        self.limbs.is_empty() || self.limbs.iter().all(|&l| l == 0)
    }

    pub fn trim(&mut self) {
        while let Some(&0) = self.limbs.last() {
            self.limbs.pop();
        }
    }

    pub fn cmp_val(&self, other: &Self) -> core::cmp::Ordering {
        if self.limbs.len() != other.limbs.len() {
            return self.limbs.len().cmp(&other.limbs.len());
        }
        for (a, b) in self.limbs.iter().rev().zip(other.limbs.iter().rev()) {
            if a != b {
                return a.cmp(b);
            }
        }
        core::cmp::Ordering::Equal
    }

    pub fn add_val(&self, other: &Self) -> Self {
        let max_len = core::cmp::max(self.limbs.len(), other.limbs.len());
        let mut limbs = Vec::with_capacity(max_len + 1);
        let mut carry = 0u64;
        for i in 0..max_len {
            let a = self.limbs.get(i).copied().unwrap_or(0);
            let b = other.limbs.get(i).copied().unwrap_or(0);
            let sum1 = a.wrapping_add(carry);
            let c1 = (sum1 < a) as u64;
            let sum2 = sum1.wrapping_add(b);
            let c2 = (sum2 < sum1) as u64;
            limbs.push(sum2);
            carry = c1 + c2;
        }
        if carry > 0 {
            limbs.push(carry);
        }
        let mut res = Self { limbs };
        res.trim();
        res
    }

    pub fn sub_val(&self, other: &Self) -> Option<Self> {
        if self.cmp_val(other) == core::cmp::Ordering::Less {
            return None;
        }
        let mut limbs = Vec::with_capacity(self.limbs.len());
        let mut borrow = 0u64;
        for i in 0..self.limbs.len() {
            let a = self.limbs[i];
            let b = other.limbs.get(i).copied().unwrap_or(0);
            let sub1 = a.wrapping_sub(borrow);
            let b1 = (sub1 > a) as u64;
            let sub2 = sub1.wrapping_sub(b);
            let b2 = (sub2 > sub1) as u64;
            limbs.push(sub2);
            borrow = b1 + b2;
        }
        let mut res = Self { limbs };
        res.trim();
        Some(res)
    }

    pub fn mul_val(&self, other: &Self) -> Self {
        if self.is_zero() || other.is_zero() {
            return Self::zero();
        }
        let n = self.limbs.len();
        let m = other.limbs.len();
        let mut res = alloc::vec![0u64; n + m];

        for i in 0..n {
            let mut carry = 0u128;
            let a = self.limbs[i] as u128;
            for j in 0..m {
                let b = other.limbs[j] as u128;
                let cur = res[i + j] as u128 + a * b + carry;
                res[i + j] = cur as u64;
                carry = cur >> 64;
            }
            let mut k = i + m;
            while carry > 0 && k < res.len() {
                let cur = res[k] as u128 + carry;
                res[k] = cur as u64;
                carry = cur >> 64;
                k += 1;
            }
        }
        let mut r = Self { limbs: res };
        r.trim();
        r
    }

    pub fn bit_len(&self) -> usize {
        if self.is_zero() {
            return 0;
        }
        let top = *self.limbs.last().unwrap();
        (self.limbs.len() - 1) * 64 + (64 - top.leading_zeros() as usize)
    }

    pub fn test_bit(&self, bit: usize) -> bool {
        let limb_idx = bit / 64;
        let bit_idx = bit % 64;
        if limb_idx >= self.limbs.len() {
            false
        } else {
            (self.limbs[limb_idx] & (1u64 << bit_idx)) != 0
        }
    }

    pub fn shl1(&mut self) {
        let mut carry = 0u64;
        for l in self.limbs.iter_mut() {
            let next_carry = *l >> 63;
            *l = (*l << 1) | carry;
            carry = next_carry;
        }
        if carry != 0 {
            self.limbs.push(carry);
        }
    }

    pub fn div_rem(&self, divisor: &Self) -> Option<(Self, Self)> {
        if divisor.is_zero() {
            return None;
        }
        if self.cmp_val(divisor) == core::cmp::Ordering::Less {
            return Some((Self::zero(), self.clone()));
        }

        let mut quotient = Self::zero();
        let mut remainder = Self::zero();

        for bit in (0..self.bit_len()).rev() {
            remainder.shl1();
            if self.test_bit(bit) {
                if remainder.limbs.is_empty() {
                    remainder.limbs.push(1);
                } else {
                    remainder.limbs[0] |= 1;
                }
            }
            remainder.trim();

            if remainder.cmp_val(divisor) != core::cmp::Ordering::Less {
                remainder = remainder.sub_val(divisor).unwrap();
                let q_limb = bit / 64;
                let q_bit = bit % 64;
                while quotient.limbs.len() <= q_limb {
                    quotient.limbs.push(0);
                }
                quotient.limbs[q_limb] |= 1u64 << q_bit;
            }
        }
        quotient.trim();
        remainder.trim();
        Some((quotient, remainder))
    }

    pub fn rem_val(&self, divisor: &Self) -> Option<Self> {
        self.div_rem(divisor).map(|(_, r)| r)
    }

    pub fn mod_pow(&self, exp: &Self, modulus: &Self) -> Option<Self> {
        if modulus.is_zero() {
            return None;
        }
        let mut base = self.rem_val(modulus)?;
        let mut res = Self::from_u64(1).rem_val(modulus)?;

        let exp_bits = exp.bit_len();
        for bit in 0..exp_bits {
            if exp.test_bit(bit) {
                res = res.mul_val(&base).rem_val(modulus)?;
            }
            base = base.mul_val(&base).rem_val(modulus)?;
        }
        Some(res)
    }
}

// -----------------------------------------------------------------------------
// RSA PKCS#1 v1.5 Verification
// -----------------------------------------------------------------------------

/// SHA-256 DigestInfo prefix for PKCS#1 v1.5 signature padding.
pub const SHA256_DIGEST_INFO: [u8; 19] = [
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20,
];

/// Verifies an RSASSA-PKCS1-v1_5 signature with SHA-256 digest over `signed_data_hash`.
pub fn verify_rsa_pkcs1_v1_5(n_bytes: &[u8], e_bytes: &[u8], sig_bytes: &[u8], digest: &[u8]) -> bool {
    if digest.len() != 32 || sig_bytes.is_empty() || n_bytes.is_empty() || e_bytes.is_empty() {
        return false;
    }
    let k = n_bytes.len();
    if sig_bytes.len() != k || k < 64 {
        return false;
    }

    let n = BigUint::from_be_bytes(n_bytes);
    let e = BigUint::from_be_bytes(e_bytes);
    let s = BigUint::from_be_bytes(sig_bytes);

    if s.cmp_val(&n) != core::cmp::Ordering::Less {
        return false;
    }

    let m = match s.mod_pow(&e, &n) {
        Some(m) => m,
        None => return false,
    };

    let em = m.to_be_bytes(k);

    // Format: 0x00 0x01 [0xFF; PS_LEN >= 8] 0x00 [DigestInfo 19 bytes] [32-byte hash]
    if em[0] != 0x00 || em[1] != 0x01 {
        return false;
    }

    let t_len = SHA256_DIGEST_INFO.len() + 32;
    if k < t_len + 11 {
        return false;
    }

    let mut sep_idx = 2;
    while sep_idx < k && em[sep_idx] == 0xFF {
        sep_idx += 1;
    }

    // Must have at least 8 bytes of 0xFF
    if sep_idx < 10 || sep_idx >= k || em[sep_idx] != 0x00 {
        return false;
    }

    let t = &em[sep_idx + 1..];
    if t.len() != t_len {
        return false;
    }

    if &t[..SHA256_DIGEST_INFO.len()] != &SHA256_DIGEST_INFO {
        return false;
    }

    if &t[SHA256_DIGEST_INFO.len()..] != digest {
        return false;
    }

    true
}

// -----------------------------------------------------------------------------
// NIST P-256 (secp256r1) ECDSA Verification
// -----------------------------------------------------------------------------

/// P-256 prime modulus: 2^256 - 2^224 + 2^192 + 2^96 - 1
const P256_P_BYTES: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
];

/// P-256 curve order n:
const P256_N_BYTES: [u8; 32] = [
    0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0xbc, 0xe6, 0xfa, 0xad, 0xa7, 0x17, 0x9e, 0x84, 0xf3, 0xb9, 0xca, 0xc2, 0xfc, 0x63, 0x25, 0x51,
];

/// P-256 base point Gx
const P256_GX_BYTES: [u8; 32] = [
    0x6b, 0x17, 0xd1, 0xf2, 0xe1, 0x2c, 0x42, 0x47, 0xf8, 0xbc, 0xe6, 0xe5, 0x63, 0xa4, 0x40, 0xf2,
    0x77, 0x03, 0x7d, 0x81, 0x2d, 0xeb, 0x33, 0xa0, 0xf4, 0xa1, 0x39, 0x45, 0xd8, 0x98, 0xc2, 0x96,
];

/// P-256 base point Gy
const P256_GY_BYTES: [u8; 32] = [
    0x4f, 0xe3, 0x42, 0xe2, 0xfe, 0x1a, 0x7f, 0x9b, 0x8e, 0xe7, 0xeb, 0x4a, 0x7c, 0x0f, 0x9e, 0x16,
    0x2b, 0xce, 0x33, 0x57, 0x6b, 0x31, 0x5e, 0xce, 0xcb, 0xb6, 0x40, 0x68, 0x37, 0xbf, 0x51, 0xf5,
];

/// P-256 parameter b:
const P256_B_BYTES: [u8; 32] = [
    0x5a, 0xc6, 0x35, 0xd8, 0xaa, 0x3a, 0x93, 0xe7, 0xb3, 0xeb, 0xbd, 0x55, 0x76, 0x98, 0x86, 0xbc,
    0x65, 0x1d, 0x06, 0xb0, 0xcc, 0x53, 0xb0, 0xf6, 0x3b, 0xce, 0x3c, 0x3e, 0x27, 0xd2, 0x60, 0x4b,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AffinePoint {
    pub x: BigUint,
    pub y: BigUint,
    pub is_infinity: bool,
}

impl AffinePoint {
    pub fn infinity() -> Self {
        Self {
            x: BigUint::zero(),
            y: BigUint::zero(),
            is_infinity: true,
        }
    }
}

fn mod_add(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
    let sum = a.add_val(b);
    if sum.cmp_val(m) != core::cmp::Ordering::Less {
        sum.sub_val(m).unwrap()
    } else {
        sum
    }
}

fn mod_sub(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
    if a.cmp_val(b) != core::cmp::Ordering::Less {
        a.sub_val(b).unwrap()
    } else {
        a.add_val(m).sub_val(b).unwrap()
    }
}

fn mod_mul(a: &BigUint, b: &BigUint, m: &BigUint) -> BigUint {
    a.mul_val(b).rem_val(m).unwrap_or(BigUint::zero())
}

fn mod_inv(a: &BigUint, m: &BigUint) -> Option<BigUint> {
    if a.is_zero() {
        return None;
    }
    // By Fermat's Little Theorem for prime m: a^(m-2) mod m
    let two = BigUint::from_u64(2);
    let exp = m.sub_val(&two)?;
    a.mod_pow(&exp, m)
}

#[derive(Clone)]
struct JacobianPoint {
    x: BigUint,
    y: BigUint,
    z: BigUint,
}

impl JacobianPoint {
    fn infinity() -> Self {
        Self {
            x: BigUint::from_u64(1),
            y: BigUint::from_u64(1),
            z: BigUint::zero(),
        }
    }

    fn from_affine(pt: &AffinePoint) -> Self {
        if pt.is_infinity {
            Self::infinity()
        } else {
            Self {
                x: pt.x.clone(),
                y: pt.y.clone(),
                z: BigUint::from_u64(1),
            }
        }
    }

    fn to_affine(&self, p: &BigUint) -> AffinePoint {
        if self.z.is_zero() {
            return AffinePoint::infinity();
        }
        let z_inv = match mod_inv(&self.z, p) {
            Some(inv) => inv,
            None => return AffinePoint::infinity(),
        };
        let z2 = mod_mul(&z_inv, &z_inv, p);
        let z3 = mod_mul(&z2, &z_inv, p);
        let x = mod_mul(&self.x, &z2, p);
        let y = mod_mul(&self.y, &z3, p);
        AffinePoint {
            x,
            y,
            is_infinity: false,
        }
    }

    // Point doubling on y^2 = x^3 - 3x + b (NIST P-256)
    fn double(&self, p: &BigUint) -> Self {
        if self.z.is_zero() || self.y.is_zero() {
            return Self::infinity();
        }
        // delta = Z^2
        // gamma = Y^2
        // beta = X * gamma
        // alpha = 3 * (X - delta) * (X + delta)
        let delta = mod_mul(&self.z, &self.z, p);
        let gamma = mod_mul(&self.y, &self.y, p);
        let beta = mod_mul(&self.x, &gamma, p);

        let t1 = mod_sub(&self.x, &delta, p);
        let t2 = mod_add(&self.x, &delta, p);
        let t3 = mod_mul(&t1, &t2, p);
        let alpha = mod_mul(&BigUint::from_u64(3), &t3, p);

        let alpha_sq = mod_mul(&alpha, &alpha, p);
        let eight_beta = mod_mul(&BigUint::from_u64(8), &beta, p);
        let x3 = mod_sub(&alpha_sq, &eight_beta, p);

        let two = BigUint::from_u64(2);
        let z3 = mod_mul(&mod_mul(&two, &self.y, p), &self.z, p);

        let four_beta = mod_mul(&BigUint::from_u64(4), &beta, p);
        let diff = mod_sub(&four_beta, &x3, p);
        let y3_term1 = mod_mul(&alpha, &diff, p);
        let gamma_sq = mod_mul(&gamma, &gamma, p);
        let eight_gamma_sq = mod_mul(&BigUint::from_u64(8), &gamma_sq, p);
        let y3 = mod_sub(&y3_term1, &eight_gamma_sq, p);

        Self { x: x3, y: y3, z: z3 }
    }

    // Mixed addition: Jacobian + Affine
    fn add_affine(&self, pt: &AffinePoint, p: &BigUint) -> Self {
        if pt.is_infinity {
            return self.clone();
        }
        if self.z.is_zero() {
            return Self::from_affine(pt);
        }
        // U1 = X1, S1 = Y1
        // U2 = X2 * Z1^2
        // S2 = Y2 * Z1^3
        let z1_sq = mod_mul(&self.z, &self.z, p);
        let z1_cu = mod_mul(&z1_sq, &self.z, p);
        let u2 = mod_mul(&pt.x, &z1_sq, p);
        let s2 = mod_mul(&pt.y, &z1_cu, p);

        if self.x == u2 {
            if self.y == s2 {
                return self.double(p);
            } else {
                return Self::infinity();
            }
        }

        // H = U2 - X1
        // R = S2 - Y1
        let h = mod_sub(&u2, &self.x, p);
        let r = mod_sub(&s2, &self.y, p);

        let h_sq = mod_mul(&h, &h, p);
        let h_cu = mod_mul(&h_sq, &h, p);
        let u1_h_sq = mod_mul(&self.x, &h_sq, p);

        // X3 = R^2 - H^3 - 2 * U1 * H^2
        let r_sq = mod_mul(&r, &r, p);
        let two_u1_h_sq = mod_mul(&BigUint::from_u64(2), &u1_h_sq, p);
        let x3_temp = mod_sub(&r_sq, &h_cu, p);
        let x3 = mod_sub(&x3_temp, &two_u1_h_sq, p);

        // Y3 = R * (U1 * H^2 - X3) - Y1 * H^3
        let y3_diff = mod_sub(&u1_h_sq, &x3, p);
        let y3_term1 = mod_mul(&r, &y3_diff, p);
        let y3_term2 = mod_mul(&self.y, &h_cu, p);
        let y3 = mod_sub(&y3_term1, &y3_term2, p);

        // Z3 = Z1 * H
        let z3 = mod_mul(&self.z, &h, p);

        Self { x: x3, y: y3, z: z3 }
    }
}

fn point_mul(pt: &AffinePoint, scalar: &BigUint, p: &BigUint) -> AffinePoint {
    if pt.is_infinity || scalar.is_zero() {
        return AffinePoint::infinity();
    }
    let mut res = JacobianPoint::infinity();
    let bits = scalar.bit_len();
    for bit in (0..bits).rev() {
        res = res.double(p);
        if scalar.test_bit(bit) {
            res = res.add_affine(pt, p);
        }
    }
    res.to_affine(p)
}

fn point_add(p1: &AffinePoint, p2: &AffinePoint, p: &BigUint) -> AffinePoint {
    JacobianPoint::from_affine(p1).add_affine(p2, p).to_affine(p)
}

/// Verifies an ECDSA signature (r, s) on NIST P-256 with SHA-256 digest.
pub fn verify_ecdsa_p256(
    pub_x: &[u8],
    pub_y: &[u8],
    sig_r: &[u8],
    sig_s: &[u8],
    digest: &[u8],
) -> bool {
    if pub_x.len() != 32 || pub_y.len() != 32 || sig_r.is_empty() || sig_s.is_empty() || digest.len() != 32 {
        return false;
    }

    let p = BigUint::from_be_bytes(&P256_P_BYTES);
    let n = BigUint::from_be_bytes(&P256_N_BYTES);

    let r = BigUint::from_be_bytes(sig_r);
    let s = BigUint::from_be_bytes(sig_s);

    if r.is_zero() || r.cmp_val(&n) != core::cmp::Ordering::Less {
        return false;
    }
    if s.is_zero() || s.cmp_val(&n) != core::cmp::Ordering::Less {
        return false;
    }

    let qx = BigUint::from_be_bytes(pub_x);
    let qy = BigUint::from_be_bytes(pub_y);

    if qx.cmp_val(&p) != core::cmp::Ordering::Less || qy.cmp_val(&p) != core::cmp::Ordering::Less {
        return false;
    }

    // Verify Q lies on curve: y^2 = x^3 - 3x + b mod p
    let y_sq = mod_mul(&qy, &qy, &p);
    let x_cu = mod_mul(&mod_mul(&qx, &qx, &p), &qx, &p);
    let three_x = mod_mul(&BigUint::from_u64(3), &qx, &p);
    let rhs_temp = mod_sub(&x_cu, &three_x, &p);
    let b = BigUint::from_be_bytes(&P256_B_BYTES);
    let rhs = mod_add(&rhs_temp, &b, &p);
    if y_sq != rhs {
        return false;
    }

    let q = AffinePoint {
        x: qx,
        y: qy,
        is_infinity: false,
    };

    let gx = BigUint::from_be_bytes(&P256_GX_BYTES);
    let gy = BigUint::from_be_bytes(&P256_GY_BYTES);
    let g = AffinePoint {
        x: gx,
        y: gy,
        is_infinity: false,
    };

    let w = match mod_inv(&s, &n) {
        Some(w) => w,
        None => return false,
    };

    let e = BigUint::from_be_bytes(digest);
    let u1 = mod_mul(&e, &w, &n);
    let u2 = mod_mul(&r, &w, &n);

    let pt1 = point_mul(&g, &u1, &p);
    let pt2 = point_mul(&q, &u2, &p);
    let pt_r = point_add(&pt1, &pt2, &p);

    if pt_r.is_infinity {
        return false;
    }

    let rx_mod_n = pt_r.x.rem_val(&n).unwrap_or(BigUint::zero());
    rx_mod_n == r
}

/// Signs a 32-byte digest using ECDSA P-256 with given private key and non-zero k. Returns (r, s).
pub fn sign_ecdsa_p256(priv_d: &[u8; 32], k_bytes: &[u8; 32], digest: &[u8; 32]) -> Option<([u8; 32], [u8; 32])> {
    let p = BigUint::from_be_bytes(&P256_P_BYTES);
    let n = BigUint::from_be_bytes(&P256_N_BYTES);
    let gx = BigUint::from_be_bytes(&P256_GX_BYTES);
    let gy = BigUint::from_be_bytes(&P256_GY_BYTES);
    let g = AffinePoint { x: gx, y: gy, is_infinity: false };

    let d = BigUint::from_be_bytes(priv_d);
    let k = BigUint::from_be_bytes(k_bytes);
    let k_inv = mod_inv(&k, &n)?;

    let r_pt = point_mul(&g, &k, &p);
    let r = r_pt.x.rem_val(&n)?;
    if r.is_zero() { return None; }

    let e = BigUint::from_be_bytes(digest);
    let dr = mod_mul(&d, &r, &n);
    let e_plus_dr = mod_add(&e, &dr, &n);
    let s = mod_mul(&k_inv, &e_plus_dr, &n);
    if s.is_zero() { return None; }

    let mut r_out = [0u8; 32];
    let mut s_out = [0u8; 32];
    r_out.copy_from_slice(&r.to_be_bytes(32));
    s_out.copy_from_slice(&s.to_be_bytes(32));
    Some((r_out, s_out))
}

/// Derives public key (x, y) from ECDSA P-256 private key.
pub fn public_key_from_private_p256(priv_d: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let p = BigUint::from_be_bytes(&P256_P_BYTES);
    let gx = BigUint::from_be_bytes(&P256_GX_BYTES);
    let gy = BigUint::from_be_bytes(&P256_GY_BYTES);
    let g = AffinePoint { x: gx, y: gy, is_infinity: false };
    let d = BigUint::from_be_bytes(priv_d);
    let q = point_mul(&g, &d, &p);
    let mut x_out = [0u8; 32];
    let mut y_out = [0u8; 32];
    x_out.copy_from_slice(&q.x.to_be_bytes(32));
    y_out.copy_from_slice(&q.y.to_be_bytes(32));
    (x_out, y_out)
}


// -----------------------------------------------------------------------------
// SubjectPublicKeyInfo (SPKI) Parser
// -----------------------------------------------------------------------------

/// Parses ASN.1 DER length. Returns (length, bytes_consumed).
fn parse_der_len(bytes: &[u8]) -> Option<(usize, usize)> {
    if bytes.is_empty() {
        return None;
    }
    let first = bytes[0];
    if first < 0x80 {
        Some((first as usize, 1))
    } else {
        let count = (first & 0x7F) as usize;
        if count == 0 || count > 4 || bytes.len() < 1 + count {
            return None;
        }
        let mut len = 0usize;
        for i in 0..count {
            len = (len << 8) | (bytes[1 + i] as usize);
        }
        Some((len, 1 + count))
    }
}

/// Parses an X.509 SubjectPublicKeyInfo (SPKI) structure and extracts RSA or ECDSA parameters.
pub fn parse_spki(spki_bytes: &[u8]) -> Option<ParsedPublicKey> {
    if spki_bytes.len() < 16 || spki_bytes[0] != 0x30 {
        return None;
    }

    // Check for RSA encryption OID: 1.2.840.113549.1.1.1 (06 09 2a 86 48 86 f7 0d 01 01 01)
    let rsa_oid = [0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    // Check for EC public key OID: 1.2.840.10045.2.1 (06 07 2a 86 48 ce 3d 02 01)
    let ec_oid = [0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];

    if let Some(pos) = spki_bytes.windows(rsa_oid.len()).position(|w| w == rsa_oid) {
        // Locate BIT STRING: 0x03
        let bitstring_pos = spki_bytes[pos..].iter().position(|&b| b == 0x03)?;
        let abs_bs = pos + bitstring_pos;
        let (_, len_bytes) = parse_der_len(&spki_bytes[abs_bs + 1..])?;
        let payload = &spki_bytes[abs_bs + 1 + len_bytes..];
        if payload.is_empty() || payload[0] != 0x00 {
            return None; // 0 unused bits
        }
        let rsa_key_bytes = &payload[1..];
        if rsa_key_bytes.is_empty() || rsa_key_bytes[0] != 0x30 {
            return None;
        }
        // Parse RSAPublicKey: SEQUENCE { modulus INTEGER, publicExponent INTEGER }
        let (_, rsa_seq_len) = parse_der_len(&rsa_key_bytes[1..])?;
        let mut cur = 1 + rsa_seq_len;
        if cur >= rsa_key_bytes.len() || rsa_key_bytes[cur] != 0x02 {
            return None;
        }
        cur += 1;
        let (mod_len, mod_len_b) = parse_der_len(&rsa_key_bytes[cur..])?;
        cur += mod_len_b;
        let mut mod_bytes = &rsa_key_bytes[cur..cur + mod_len];
        cur += mod_len;
        // Strip leading 0x00 if present
        if !mod_bytes.is_empty() && mod_bytes[0] == 0x00 {
            mod_bytes = &mod_bytes[1..];
        }

        if cur >= rsa_key_bytes.len() || rsa_key_bytes[cur] != 0x02 {
            return None;
        }
        cur += 1;
        let (exp_len, exp_len_b) = parse_der_len(&rsa_key_bytes[cur..])?;
        cur += exp_len_b;
        let mut exp_bytes = &rsa_key_bytes[cur..cur + exp_len];
        if !exp_bytes.is_empty() && exp_bytes[0] == 0x00 {
            exp_bytes = &exp_bytes[1..];
        }

        return Some(ParsedPublicKey::Rsa {
            n: mod_bytes.to_vec(),
            e: exp_bytes.to_vec(),
        });
    }

    if spki_bytes.windows(ec_oid.len()).any(|w| w == ec_oid) {
        // Look for BIT STRING with uncompressed point: 0x03, 0x42, 0x00, 0x04
        let bitstring_marker = [0x03, 0x42, 0x00, 0x04];
        if let Some(pos) = spki_bytes.windows(bitstring_marker.len()).position(|w| w == bitstring_marker) {
            let payload = &spki_bytes[pos + bitstring_marker.len()..];
            if payload.len() >= 64 {
                let mut x = [0u8; 32];
                let mut y = [0u8; 32];
                x.copy_from_slice(&payload[..32]);
                y.copy_from_slice(&payload[32..64]);
                return Some(ParsedPublicKey::EcdsaP256 { x, y });
            }
        }
    }

    None
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_biguint_add_mul_pow() {
        let a = BigUint::from_u64(12345);
        let b = BigUint::from_u64(6789);
        let c = a.add_val(&b);
        assert_eq!(c, BigUint::from_u64(12345 + 6789));

        let prod = a.mul_val(&b);
        assert_eq!(prod, BigUint::from_u64(12345 * 6789));

        let base = BigUint::from_u64(3);
        let exp = BigUint::from_u64(5);
        let m = BigUint::from_u64(100);
        let p = base.mod_pow(&exp, &m).unwrap();
        assert_eq!(p, BigUint::from_u64(43)); // 3^5 = 243 = 43 mod 100
    }

    #[test]
    fn test_ecdsa_p256_known_vector() {
        // NIST P-256 test vector
        let gx = P256_GX_BYTES;
        let gy = P256_GY_BYTES;
        let d_bytes = [
            0xc9, 0xaf, 0xa9, 0xd8, 0x45, 0xba, 0x75, 0x16, 0x6b, 0x5c, 0x21, 0x36, 0x2a, 0x37, 0x77, 0x52,
            0x19, 0xf2, 0x84, 0xf7, 0x1e, 0x07, 0x1f, 0x10, 0x18, 0x7f, 0x33, 0xe4, 0x7c, 0x78, 0xe1, 0xcd,
        ];
        let p = BigUint::from_be_bytes(&P256_P_BYTES);
        let d = BigUint::from_be_bytes(&d_bytes);
        let g = AffinePoint {
            x: BigUint::from_be_bytes(&gx),
            y: BigUint::from_be_bytes(&gy),
            is_infinity: false,
        };
        let q = point_mul(&g, &d, &p);
        let qx = q.x.to_be_bytes(32);
        let qy = q.y.to_be_bytes(32);

        // Digest
        let digest = [
            0xdc, 0xbd, 0x85, 0x16, 0xf3, 0x3e, 0x36, 0x14, 0xe4, 0x40, 0xe9, 0x68, 0xa5, 0xf0, 0x42, 0x97,
            0x7f, 0x43, 0x31, 0x4a, 0x37, 0x6b, 0x9b, 0x18, 0x24, 0x43, 0x4d, 0x05, 0x14, 0x53, 0x57, 0x34,
        ];

        // Random k for signing
        let k_bytes = [
            0xae, 0xa0, 0xf3, 0x60, 0x82, 0x88, 0x76, 0x3c, 0xfb, 0x81, 0x26, 0x65, 0x43, 0x10, 0xe5, 0xb5,
            0x5b, 0x45, 0x57, 0x55, 0x18, 0x97, 0x44, 0x62, 0xad, 0x18, 0x56, 0x60, 0xcb, 0x73, 0xc9, 0xac,
        ];
        let k = BigUint::from_be_bytes(&k_bytes);
        let n = BigUint::from_be_bytes(&P256_N_BYTES);
        let k_inv = mod_inv(&k, &n).unwrap();

        let r_pt = point_mul(&g, &k, &p);
        let r = r_pt.x.rem_val(&n).unwrap();
        let e = BigUint::from_be_bytes(&digest);
        let dr = mod_mul(&d, &r, &n);
        let e_plus_dr = mod_add(&e, &dr, &n);
        let s = mod_mul(&k_inv, &e_plus_dr, &n);

        let r_bytes = r.to_be_bytes(32);
        let s_bytes = s.to_be_bytes(32);

        // Valid signature must verify
        assert!(verify_ecdsa_p256(&qx, &qy, &r_bytes, &s_bytes, &digest));

        // Tampered signature must fail
        let mut tampered_s = s_bytes.clone();
        tampered_s[0] ^= 0xFF;
        assert!(!verify_ecdsa_p256(&qx, &qy, &r_bytes, &tampered_s, &digest));

        // Tampered digest must fail
        let mut tampered_digest = digest;
        tampered_digest[0] ^= 0x01;
        assert!(!verify_ecdsa_p256(&qx, &qy, &r_bytes, &s_bytes, &tampered_digest));
    }
}
