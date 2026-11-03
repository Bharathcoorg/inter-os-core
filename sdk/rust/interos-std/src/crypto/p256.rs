// SPDX-License-Identifier: AGPL-3.0-only
// SPDX-FileCopyrightText: 2026 Bharath B R <https://github.com/Bharathcoorg>

//! Pure `no_std` ECDSA over the NIST P-256 curve (SEC 2, FIPS PUB 186-5).
//!
//! Verification only: the device holds no signing key. SHA-256 is the hash the
//! `ecdsa_secp256r1_sha256` TLS scheme and the `ecdsa-with-SHA256` certificate
//! algorithm name, so both paths land here.
//!
//! Zero third-party dependencies, zero heap allocations. Field and scalar
//! arithmetic share one 256-bit Montgomery engine, parameterised by the prime
//! `p` for coordinates and the order `n` for scalars. Curve parameters are
//! written in ordinary form and moved into the Montgomery domain where they
//! are used, so a transcription of `b` or of the generator reads back against
//! the published document.

use super::sha256::Sha256;

/// Limbs in a P-256 element: four `u64`s, little-endian.
type U4 = [u64; 4];

/// The field prime, `2^256 - 2^224 + 2^192 + 2^96 - 1`.
const P: U4 = [
    0xffffffffffffffff,
    0x00000000ffffffff,
    0x0000000000000000,
    0xffffffff00000001,
];

/// The group order.
const N: U4 = [
    0xf3b9cac2fc632551,
    0xbce6faada7179e84,
    0xffffffffffffffff,
    0xffffffff00000000,
];

/// Curve coefficient `b`, in ordinary form.
const B: U4 = [
    0x3bce3c3e27d2604b,
    0x651d06b0cc53b0f6,
    0xb3ebbd55769886bc,
    0x5ac635d8aa3a93e7,
];

/// The generator's x-coordinate, in ordinary form.
const GX: U4 = [
    0xf4a13945d898c296,
    0x77037d812deb33a0,
    0xf8bce6e563a440f2,
    0x6b17d1f2e12c4247,
];

/// The generator's y-coordinate, in ordinary form.
const GY: U4 = [
    0xcbb6406837bf51f5,
    0x2bce33576b315ece,
    0x8ee7eb4a7c0f9e16,
    0x4fe342e2fe1a7f9b,
];

/// A modulus with the Montgomery constants derived from it at compile time.
#[derive(Clone, Copy)]
struct Modulus {
    m: U4,
    /// `-m^-1 mod 2^64`, the word that zeroes a column during reduction.
    n0inv: u64,
    /// `2^512 mod m`, which moves an ordinary value into the Montgomery domain.
    r2: U4,
}

const fn at_or_above(a: &U4, b: &U4) -> bool {
    let mut index = 4;
    while index > 0 {
        index -= 1;
        if a[index] != b[index] {
            return a[index] > b[index];
        }
    }
    true
}

/// `a - b` over four limbs, letting the result wrap.
const fn subtract_unchecked(a: &U4, b: &U4) -> U4 {
    let mut difference = [0u64; 4];
    let mut borrow = 0u64;
    let mut index = 0;
    while index < 4 {
        let cell = (a[index] as u128)
            .wrapping_sub(b[index] as u128)
            .wrapping_sub(borrow as u128);
        difference[index] = cell as u64;
        borrow = ((cell >> 127) & 1) as u64;
        index += 1;
    }
    difference
}

/// `a + b` over four limbs, with the carry out of the top limb.
const fn add_unchecked(a: &U4, b: &U4) -> (U4, u64) {
    let mut sum = [0u64; 4];
    let mut carry = 0u64;
    let mut index = 0;
    while index < 4 {
        let cell = a[index] as u128 + b[index] as u128 + carry as u128;
        sum[index] = cell as u64;
        carry = (cell >> 64) as u64;
        index += 1;
    }
    (sum, carry)
}

/// `a + b mod m`.
const fn add_mod(a: &U4, b: &U4, m: &U4) -> U4 {
    let (sum, carried) = add_unchecked(a, b);
    // A carry out means the true sum is 2^256 + sum, which is above m, and the
    // wrapped subtraction below leaves exactly `true sum - m`.
    if carried != 0 || at_or_above(&sum, m) {
        subtract_unchecked(&sum, m)
    } else {
        sum
    }
}

/// `a - b mod m`.
const fn sub_mod(a: &U4, b: &U4, m: &U4) -> U4 {
    let difference = subtract_unchecked(a, b);
    if at_or_above(a, b) {
        difference
    } else {
        // The wrapped difference is `2^256 + a - b`; adding m and dropping the
        // carry out leaves `a - b + m`, which is the answer below m.
        add_unchecked(&difference, m).0
    }
}

/// `2^doublings mod m`, starting from one and doubling in the ordinary domain.
const fn power_of_two_mod(m: &U4, doublings: u32) -> U4 {
    let mut acc: U4 = [1, 0, 0, 0];
    let mut index = 0;
    while index < doublings {
        acc = add_mod(&acc, &acc, m);
        index += 1;
    }
    acc
}

/// `-m^-1 mod 2^64`, by Newton's iteration.
///
/// WHY computed rather than written down: this is the one constant a
/// transcription slip turns into arithmetic that looks plausible and is wrong,
/// and it is four lines to derive.
const fn n0inv(m0: u64) -> u64 {
    // For an odd m0, m0 itself is already an inverse correct to three bits.
    let mut inv = m0;
    let mut round = 0;
    while round < 6 {
        inv = inv.wrapping_mul(2u64.wrapping_sub(m0.wrapping_mul(inv)));
        round += 1;
    }
    0u64.wrapping_sub(inv)
}

const fn modulus(m: U4) -> Modulus {
    Modulus {
        m,
        n0inv: n0inv(m[0]),
        r2: power_of_two_mod(&m, 512),
    }
}

/// The coordinate field.
const FP: Modulus = modulus(P);
/// The scalar field.
const FN: Modulus = modulus(N);

/// A Jacobian point: `(X, Y, Z)` standing for the affine `(X/Z², Y/Z³)`, with
/// `Z = 0` meaning the point at infinity.
#[derive(Clone, Copy)]
struct Point {
    x: U4,
    y: U4,
    z: U4,
}

/// An affine point, whose `Z` is one — the shape a certificate's public key
/// and the scalar multiplication's addends take.
#[derive(Clone, Copy)]
struct Affine {
    x: U4,
    y: U4,
}

const INFINITY: Point = Point {
    x: [0; 4],
    y: [0; 4],
    z: [0; 4],
};

/// `a * b` over the whole 512-bit product.
fn multiply_wide(a: &U4, b: &U4) -> [u64; 8] {
    let mut product = [0u64; 8];
    let mut i = 0;
    while i < 4 {
        let mut carry = 0u64;
        let mut j = 0;
        while j < 4 {
            let cell = product[i + j] as u128 + (a[i] as u128) * (b[j] as u128) + carry as u128;
            product[i + j] = cell as u64;
            carry = (cell >> 64) as u64;
            j += 1;
        }
        product[i + 4] = carry;
        i += 1;
    }
    product
}

/// Montgomery reduction: `(t + (t * n0inv mod 2^256) * m) / 2^256`.
fn montgomery_reduce(t: &mut [u64; 8], m: &U4, n0inv: u64) -> U4 {
    let mut overflow = 0u64;
    let mut i = 0;
    while i < 4 {
        let k = t[i].wrapping_mul(n0inv);
        let mut carry = 0u64;
        let mut j = 0;
        while j < 4 {
            let cell = t[i + j] as u128 + (k as u128) * (m[j] as u128) + carry as u128;
            t[i + j] = cell as u64;
            carry = (cell >> 64) as u64;
            j += 1;
        }
        // The column that just zeroed cannot absorb the carry, so it walks up
        // through the high half; whatever leaves the top is counted in
        // `overflow`, which the final correction reads.
        let mut index = i + 4;
        while index < 8 && carry != 0 {
            let cell = t[index] as u128 + carry as u128;
            t[index] = cell as u64;
            carry = (cell >> 64) as u64;
            index += 1;
        }
        overflow += carry;
        i += 1;
    }

    let mut result = [t[4], t[5], t[6], t[7]];
    // For inputs below m the reduced value is below 2m, so one correction
    // finishes it — and an `overflow` of one is itself proof of being above m,
    // because the true value is 2^256 + result and result is then below m.
    if overflow != 0 || at_or_above(&result, m) {
        result = subtract_unchecked(&result, m);
    }
    result
}

/// Montgomery multiplication: `a * b * 2^-256 mod m`, for `a, b < m`.
fn mont_mul(a: &U4, b: &U4, md: &Modulus) -> U4 {
    let mut wide = multiply_wide(a, b);
    montgomery_reduce(&mut wide, &md.m, md.n0inv)
}

/// Moves an ordinary residue into the Montgomery domain.
fn to_mont(value: &U4, md: &Modulus) -> U4 {
    mont_mul(value, &md.r2, md)
}

/// Moves a Montgomery residue back to ordinary representation.
fn from_mont(value: &U4, md: &Modulus) -> U4 {
    mont_mul(value, &[1, 0, 0, 0], md)
}

/// The Montgomery domain's one, `2^256 mod m`.
const fn mont_one(md: &Modulus) -> U4 {
    power_of_two_mod(&md.m, 256)
}

fn add(a: &U4, b: &U4, md: &Modulus) -> U4 {
    add_mod(a, b, &md.m)
}

fn sub(a: &U4, b: &U4, md: &Modulus) -> U4 {
    sub_mod(a, b, &md.m)
}

fn double(value: &U4, md: &Modulus) -> U4 {
    add(value, value, md)
}

fn is_zero(value: &U4) -> bool {
    value[0] | value[1] | value[2] | value[3] == 0
}

/// Reads a 32-byte big-endian scalar into limbs, reduced modulo the order.
fn scalar_from_be(bytes: &[u8; 32]) -> U4 {
    let mut limbs = [0u64; 4];
    for (index, byte) in bytes.iter().rev().enumerate() {
        limbs[index / 8] |= (*byte as u64) << (8 * (index % 8));
    }
    // One conditional subtraction suffices: the order is above 2^255, so any
    // 256-bit value is below twice it.
    if at_or_above(&limbs, &N) {
        subtract_unchecked(&limbs, &N)
    } else {
        limbs
    }
}

/// The multiplicative inverse of a Montgomery-domain element, by Fermat's
/// `x^(m-2)`.
///
/// WHY exponentiation rather than the binary Euclidean algorithm: the chain
/// `p` and the chain `n` are different fixed sequences of steps, and a slip in
/// one yields a value that is not an inverse and never looks wrong. This reads
/// the same modulus every other operation in this file already uses, so the
/// only thing that can be wrong here is the multiplication it is built from —
/// which the tests pin against real signatures.
fn invert(value: &U4, md: &Modulus) -> U4 {
    let exponent = sub_mod(&md.m, &[2, 0, 0, 0], &md.m);
    let mut result = mont_one(md);
    let mut bit_index = 256;
    while bit_index > 0 {
        bit_index -= 1;
        result = mont_mul(&result, &result, md);
        if (exponent[bit_index / 64] >> (bit_index % 64)) & 1 == 1 {
            result = mont_mul(&result, value, md);
        }
    }
    result
}

/// Point doubling in Jacobian coordinates, specialised to `a = -3`.
fn double_point(point: &Point) -> Point {
    if is_zero(&point.y) {
        return INFINITY;
    }
    let a = mont_mul(&point.x, &point.x, &FP);
    let b = mont_mul(&point.y, &point.y, &FP);
    let c = mont_mul(&b, &b, &FP);
    let sum = add(&point.x, &b, &FP);
    let d = double(
        &sub(&sub(&mont_mul(&sum, &sum, &FP), &a, &FP), &c, &FP),
        &FP,
    );
    let z2 = mont_mul(&point.z, &point.z, &FP);
    let z4 = mont_mul(&z2, &z2, &FP);
    // E = 3*X1² + a*Z1⁴, and a = -3, so E = 3*(X1² - Z1⁴).
    let three_a = add(&double(&a, &FP), &a, &FP);
    let three_z4 = add(&double(&z4, &FP), &z4, &FP);
    let e = sub(&three_a, &three_z4, &FP);
    let f = mont_mul(&e, &e, &FP);
    let x3 = sub(&f, &double(&d, &FP), &FP);
    let c2 = double(&c, &FP);
    let c4 = double(&c2, &FP);
    let eight_c = double(&c4, &FP);
    let y3 = sub(&mont_mul(&e, &sub(&d, &x3, &FP), &FP), &eight_c, &FP);
    let z3 = double(&mont_mul(&point.y, &point.z, &FP), &FP);
    Point { x: x3, y: y3, z: z3 }
}

/// Adds an affine point to a Jacobian one — the cheaper mixed formula, which
/// is what the scalar multiplication spends most of its additions on.
fn add_mixed(point: &Point, other: &Affine) -> Point {
    if is_zero(&point.z) {
        return Point {
            x: other.x,
            y: other.y,
            z: mont_one(&FP),
        };
    }
    let z1z1 = mont_mul(&point.z, &point.z, &FP);
    let u2 = mont_mul(&other.x, &z1z1, &FP);
    let s2 = mont_mul(&other.y, &mont_mul(&z1z1, &point.z, &FP), &FP);
    let h = sub(&u2, &point.x, &FP);
    let r = sub(&s2, &point.y, &FP);
    if is_zero(&h) {
        return if is_zero(&r) {
            double_point(point)
        } else {
            INFINITY
        };
    }
    let h2 = mont_mul(&h, &h, &FP);
    let h3 = mont_mul(&h2, &h, &FP);
    let v = mont_mul(&point.x, &h2, &FP);
    let x3 = sub(&sub(&mont_mul(&r, &r, &FP), &h3, &FP), &double(&v, &FP), &FP);
    let y3 = sub(
        &mont_mul(&r, &sub(&v, &x3, &FP), &FP),
        &mont_mul(&point.y, &h3, &FP),
        &FP,
    );
    let z3 = mont_mul(&point.z, &h, &FP);
    Point { x: x3, y: y3, z: z3 }
}

/// Adds two Jacobian points.
fn add_points(a: &Point, b: &Point) -> Point {
    if is_zero(&a.z) {
        return *b;
    }
    if is_zero(&b.z) {
        return *a;
    }
    let a_z2 = mont_mul(&a.z, &a.z, &FP);
    let b_z2 = mont_mul(&b.z, &b.z, &FP);
    let u1 = mont_mul(&a.x, &b_z2, &FP);
    let u2 = mont_mul(&b.x, &a_z2, &FP);
    let s1 = mont_mul(&a.y, &mont_mul(&b_z2, &b.z, &FP), &FP);
    let s2 = mont_mul(&b.y, &mont_mul(&a_z2, &a.z, &FP), &FP);
    let h = sub(&u2, &u1, &FP);
    let r = sub(&s2, &s1, &FP);
    if is_zero(&h) {
        return if is_zero(&r) { double_point(a) } else { INFINITY };
    }
    let h2 = mont_mul(&h, &h, &FP);
    let h3 = mont_mul(&h2, &h, &FP);
    let v = mont_mul(&u1, &h2, &FP);
    let x3 = sub(&sub(&mont_mul(&r, &r, &FP), &h3, &FP), &double(&v, &FP), &FP);
    let y3 = sub(
        &mont_mul(&r, &sub(&v, &x3, &FP), &FP),
        &mont_mul(&s1, &h3, &FP),
        &FP,
    );
    // Z3 = Z1*Z2*H: the `2*` some references carry belongs to the formulas that
    // also double `r` and `V`, and mixing the two moves the point.
    let z3 = mont_mul(&mont_mul(&a.z, &b.z, &FP), &h, &FP);
    Point { x: x3, y: y3, z: z3 }
}

/// `scalar * base` by double-and-add from the most significant bit.
fn multiply(base: &Affine, scalar: &U4) -> Point {
    let mut accumulator = INFINITY;
    let mut started = false;
    let mut bit_index = 256;
    while bit_index > 0 {
        bit_index -= 1;
        if started {
            accumulator = double_point(&accumulator);
        }
        if (scalar[bit_index / 64] >> (bit_index % 64)) & 1 == 1 {
            accumulator = if started {
                add_mixed(&accumulator, base)
            } else {
                Point {
                    x: base.x,
                    y: base.y,
                    z: mont_one(&FP),
                }
            };
            started = true;
        }
    }
    accumulator
}

/// The generator as an affine point in the Montgomery domain.
fn generator() -> Affine {
    Affine {
        x: to_mont(&GX, &FP),
        y: to_mont(&GY, &FP),
    }
}

/// The affine x-coordinate of a Jacobian point.
fn affine_x(point: &Point) -> Option<U4> {
    if is_zero(&point.z) {
        return None;
    }
    let z_inv = invert(&point.z, &FP);
    let z_inv2 = mont_mul(&z_inv, &z_inv, &FP);
    Some(mont_mul(&point.x, &z_inv2, &FP))
}

/// True when the affine Montgomery pair satisfies `y² = x³ - 3x + b`.
fn on_curve(x: &U4, y: &U4) -> bool {
    let b_mont = to_mont(&B, &FP);
    let x2 = mont_mul(x, x, &FP);
    let x3 = mont_mul(&x2, x, &FP);
    let three_x = add(&double(x, &FP), x, &FP);
    let right = add(&sub(&x3, &three_x, &FP), &b_mont, &FP);
    mont_mul(y, y, &FP) == right
}

/// Reads an uncompressed P-256 point (`0x04 || X || Y`) as an affine curve
/// point, refusing coordinates outside the field and points off the curve.
fn point_from_spki(public_key: &[u8]) -> Option<Affine> {
    if public_key.len() != 65 || public_key[0] != 0x04 {
        return None;
    }
    let x = limbs_from_be(&public_key[1..33]);
    let y = limbs_from_be(&public_key[33..65]);
    if at_or_above(&x, &P) || at_or_above(&y, &P) {
        return None;
    }
    let x_mont = to_mont(&x, &FP);
    let y_mont = to_mont(&y, &FP);
    if !on_curve(&x_mont, &y_mont) {
        return None;
    }
    Some(Affine {
        x: x_mont,
        y: y_mont,
    })
}

/// Big-endian bytes to four little-endian limbs.
fn limbs_from_be(bytes: &[u8]) -> U4 {
    let mut limbs = [0u64; 4];
    let length = bytes.len();
    for (index, byte) in bytes.iter().enumerate() {
        let from_end = length - 1 - index;
        limbs[from_end / 8] |= (*byte as u64) << (8 * (from_end % 8));
    }
    limbs
}

/// Verifies an ECDSA P-256 signature over SHA-256 of `message`.
///
/// `public_key` is the 65-byte uncompressed point a certificate carries as its
/// `subjectPublicKey`. `signature` is the 64-byte `r || s` pair with each half
/// big-endian — what the DER sequence inside a certificate, or inside a TLS
/// `CertificateVerify`, decodes to.
///
/// WHY a plain `bool` rather than an error: every refusal here is the same
/// answer with a different reason, and the caller already names that reason in
/// TLS terms.
pub fn verify(public_key: &[u8], message: &[u8], signature: &[u8]) -> bool {
    let Some(point) = point_from_spki(public_key) else {
        return false;
    };
    if signature.len() != 64 {
        return false;
    }
    let r = limbs_from_be(&signature[..32]);
    let s = limbs_from_be(&signature[32..]);
    // Both halves have to be proper non-zero scalars. A zero `r` or `s` is a
    // forgery shape the curve arithmetic would happily carry through to an
    // answer if it were let in.
    if is_zero(&r) || is_zero(&s) || at_or_above(&r, &N) || at_or_above(&s, &N) {
        return false;
    }

    let digest = Sha256::digest(message);
    let e = scalar_from_be(&digest);

    let w = invert(&to_mont(&s, &FN), &FN);
    let u1 = from_mont(&mont_mul(&to_mont(&e, &FN), &w, &FN), &FN);
    let u2 = from_mont(&mont_mul(&to_mont(&r, &FN), &w, &FN), &FN);

    let first = multiply(&generator(), &u1);
    let second = multiply(&point, &u2);
    let summed = add_points(&first, &second);
    let Some(x) = affine_x(&summed) else {
        return false;
    };
    let x_plain = from_mont(&x, &FP);
    // The test is `x mod n == r`, and a field element exceeds the order by less
    // than one subtraction.
    let reduced = if at_or_above(&x_plain, &N) {
        subtract_unchecked(&x_plain, &N)
    } else {
        x_plain
    };
    reduced == r
}
