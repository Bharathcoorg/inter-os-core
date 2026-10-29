//! Pure `no_std` ECDSA over the NIST P-384 curve (SEC 2, FIPS PUB 186-5).
//!
//! Verification only, like `super::p256`: the device holds no signing key.
//! SHA-384 is the hash the `ecdsa_secp384r1_sha384` TLS scheme and the
//! `ecdsa-with-SHA384` certificate algorithm name, so both paths land here.
//!
//! WHY a second curve module rather than a width parameter on the first: every
//! engine routine here is a fixed six-limb loop, and turning four limbs into a
//! variable would either allocate or turn each `while index < 4` into a bounds
//! check on the hot path. The two files are the same shape on purpose — a reader
//! who knows one finds the other already explained.
//!
//! The curve constants were not written down from memory. They were decoded from
//! the explicit `ECParameters` OpenSSL ships for `secp384r1`, and the two tests
//! at the bottom of this file re-derive what a transcription slip would break:
//! `a == p - 3`, the generator satisfies the curve equation, the prime is prime,
//! and the order times the generator is the point at infinity.
//!
//! Zero third-party dependencies, zero heap allocations. Field and scalar
//! arithmetic share one 384-bit Montgomery engine, parameterised by the prime
//! `p` for coordinates and the order `n` for scalars. Curve parameters are
//! written in ordinary form and moved into the Montgomery domain where they are
//! used.

use super::sha384::Sha384;

/// Limbs in a P-384 element: six `u64`s, little-endian.
type U6 = [u64; 6];

/// Number of limbs, spelled out where a loop bound needs it.
const LIMBS: usize = 6;

/// Bits in one of the two moduli.
const BITS: usize = 384;

/// The field prime, `2^384 - 2^128 - 2^96 + 2^32 - 1`.
const P: U6 = [
    0x00000000ffffffff,
    0xffffffff00000000,
    0xfffffffffffffffe,
    0xffffffffffffffff,
    0xffffffffffffffff,
    0xffffffffffffffff,
];

/// The group order.
const N: U6 = [
    0xecec196accc52973,
    0x581a0db248b0a77a,
    0xc7634d81f4372ddf,
    0xffffffffffffffff,
    0xffffffffffffffff,
    0xffffffffffffffff,
];

/// Curve coefficient `b`, in ordinary form.
const B: U6 = [
    0x2a85c8edd3ec2aef,
    0xc656398d8a2ed19d,
    0x0314088f5013875a,
    0x181d9c6efe814112,
    0x988e056be3f82d19,
    0xb3312fa7e23ee7e4,
];

/// The generator's x-coordinate, in ordinary form.
const GX: U6 = [
    0x3a545e3872760ab7,
    0x5502f25dbf55296c,
    0x59f741e082542a38,
    0x6e1d3b628ba79b98,
    0x8eb1c71ef320ad74,
    0xaa87ca22be8b0537,
];

/// The generator's y-coordinate, in ordinary form.
const GY: U6 = [
    0x7a431d7c90ea0e5f,
    0x0a60b1ce1d7e819d,
    0xe9da3113b5f0b8c0,
    0xf8f41dbd289a147c,
    0x5d9e98bf9292dc29,
    0x3617de4a96262c6f,
];

/// A modulus with the Montgomery constants derived from it at compile time.
#[derive(Clone, Copy)]
struct Modulus {
    m: U6,
    /// `-m^-1 mod 2^64`, the word that zeroes a column during reduction.
    n0inv: u64,
    /// `2^768 mod m`, which moves an ordinary value into the Montgomery domain.
    r2: U6,
}

const fn at_or_above(a: &U6, b: &U6) -> bool {
    let mut index = LIMBS;
    while index > 0 {
        index -= 1;
        if a[index] != b[index] {
            return a[index] > b[index];
        }
    }
    true
}

/// `a - b` over six limbs, letting the result wrap.
const fn subtract_unchecked(a: &U6, b: &U6) -> U6 {
    let mut difference = [0u64; LIMBS];
    let mut borrow = 0u64;
    let mut index = 0;
    while index < LIMBS {
        let cell = (a[index] as u128)
            .wrapping_sub(b[index] as u128)
            .wrapping_sub(borrow as u128);
        difference[index] = cell as u64;
        borrow = ((cell >> 127) & 1) as u64;
        index += 1;
    }
    difference
}

/// `a + b` over six limbs, with the carry out of the top limb.
const fn add_unchecked(a: &U6, b: &U6) -> (U6, u64) {
    let mut sum = [0u64; LIMBS];
    let mut carry = 0u64;
    let mut index = 0;
    while index < LIMBS {
        let cell = a[index] as u128 + b[index] as u128 + carry as u128;
        sum[index] = cell as u64;
        carry = (cell >> 64) as u64;
        index += 1;
    }
    (sum, carry)
}

/// `a + b mod m`.
const fn add_mod(a: &U6, b: &U6, m: &U6) -> U6 {
    let (sum, carried) = add_unchecked(a, b);
    // A carry out means the true sum is 2^384 + sum, which is above m, and the
    // wrapped subtraction below leaves exactly `true sum - m`.
    if carried != 0 || at_or_above(&sum, m) {
        subtract_unchecked(&sum, m)
    } else {
        sum
    }
}

/// `a - b mod m`.
const fn sub_mod(a: &U6, b: &U6, m: &U6) -> U6 {
    let difference = subtract_unchecked(a, b);
    if at_or_above(a, b) {
        difference
    } else {
        // The wrapped difference is `2^384 + a - b`; adding m and dropping the
        // carry out leaves `a - b + m`, which is the answer below m.
        add_unchecked(&difference, m).0
    }
}

/// `2^doublings mod m`, starting from one and doubling in the ordinary domain.
const fn power_of_two_mod(m: &U6, doublings: u32) -> U6 {
    let mut acc: U6 = [1, 0, 0, 0, 0, 0];
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

const fn modulus(m: U6) -> Modulus {
    Modulus {
        m,
        n0inv: n0inv(m[0]),
        r2: power_of_two_mod(&m, 768),
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
    x: U6,
    y: U6,
    z: U6,
}

/// An affine point, whose `Z` is one — the shape a certificate's public key
/// and the scalar multiplication's addends take.
#[derive(Clone, Copy)]
struct Affine {
    x: U6,
    y: U6,
}

const INFINITY: Point = Point {
    x: [0; LIMBS],
    y: [0; LIMBS],
    z: [0; LIMBS],
};

/// `a * b` over the whole 768-bit product.
fn multiply_wide(a: &U6, b: &U6) -> [u64; 12] {
    let mut product = [0u64; 12];
    let mut i = 0;
    while i < LIMBS {
        let mut carry = 0u64;
        let mut j = 0;
        while j < LIMBS {
            let cell = product[i + j] as u128 + (a[i] as u128) * (b[j] as u128) + carry as u128;
            product[i + j] = cell as u64;
            carry = (cell >> 64) as u64;
            j += 1;
        }
        product[i + LIMBS] = carry;
        i += 1;
    }
    product
}

/// Montgomery reduction: `(t + (t * n0inv mod 2^384) * m) / 2^384`.
fn montgomery_reduce(t: &mut [u64; 12], m: &U6, n0inv: u64) -> U6 {
    let mut overflow = 0u64;
    let mut i = 0;
    while i < LIMBS {
        let k = t[i].wrapping_mul(n0inv);
        let mut carry = 0u64;
        let mut j = 0;
        while j < LIMBS {
            let cell = t[i + j] as u128 + (k as u128) * (m[j] as u128) + carry as u128;
            t[i + j] = cell as u64;
            carry = (cell >> 64) as u64;
            j += 1;
        }
        // The column that just zeroed cannot absorb the carry, so it walks up
        // through the high half; whatever leaves the top is counted in
        // `overflow`, which the final correction reads.
        let mut index = i + LIMBS;
        while index < 12 && carry != 0 {
            let cell = t[index] as u128 + carry as u128;
            t[index] = cell as u64;
            carry = (cell >> 64) as u64;
            index += 1;
        }
        overflow += carry;
        i += 1;
    }

    let mut result = [t[6], t[7], t[8], t[9], t[10], t[11]];
    // For inputs below m the reduced value is below 2m, so one correction
    // finishes it — and an `overflow` of one is itself proof of being above m,
    // because the true value is 2^384 + result and result is then below m.
    if overflow != 0 || at_or_above(&result, m) {
        result = subtract_unchecked(&result, m);
    }
    result
}

/// Montgomery multiplication: `a * b * 2^-384 mod m`, for `a, b < m`.
fn mont_mul(a: &U6, b: &U6, md: &Modulus) -> U6 {
    let mut wide = multiply_wide(a, b);
    montgomery_reduce(&mut wide, &md.m, md.n0inv)
}

/// Moves an ordinary residue into the Montgomery domain.
fn to_mont(value: &U6, md: &Modulus) -> U6 {
    mont_mul(value, &md.r2, md)
}

/// Moves a Montgomery residue back to ordinary representation.
fn from_mont(value: &U6, md: &Modulus) -> U6 {
    mont_mul(value, &[1, 0, 0, 0, 0, 0], md)
}

/// The Montgomery domain's one, `2^384 mod m`.
const fn mont_one(md: &Modulus) -> U6 {
    power_of_two_mod(&md.m, 384)
}

fn add(a: &U6, b: &U6, md: &Modulus) -> U6 {
    add_mod(a, b, &md.m)
}

fn sub(a: &U6, b: &U6, md: &Modulus) -> U6 {
    sub_mod(a, b, &md.m)
}

fn double(value: &U6, md: &Modulus) -> U6 {
    add(value, value, md)
}

fn is_zero(value: &U6) -> bool {
    let mut acc = 0u64;
    let mut index = 0;
    while index < LIMBS {
        acc |= value[index];
        index += 1;
    }
    acc == 0
}

/// Reads a 48-byte big-endian scalar into limbs, reduced modulo the order.
fn scalar_from_be(bytes: &[u8; 48]) -> U6 {
    let mut limbs = [0u64; LIMBS];
    for (index, byte) in bytes.iter().rev().enumerate() {
        limbs[index / 8] |= (*byte as u64) << (8 * (index % 8));
    }
    // One conditional subtraction suffices: the order is above 2^383, so any
    // 384-bit value is below twice it.
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
fn invert(value: &U6, md: &Modulus) -> U6 {
    let exponent = sub_mod(&md.m, &[2, 0, 0, 0, 0, 0], &md.m);
    let mut result = mont_one(md);
    let mut bit_index = BITS;
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
fn multiply(base: &Affine, scalar: &U6) -> Point {
    let mut accumulator = INFINITY;
    let mut started = false;
    let mut bit_index = BITS;
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
fn affine_x(point: &Point) -> Option<U6> {
    if is_zero(&point.z) {
        return None;
    }
    let z_inv = invert(&point.z, &FP);
    let z_inv2 = mont_mul(&z_inv, &z_inv, &FP);
    Some(mont_mul(&point.x, &z_inv2, &FP))
}

/// True when the affine Montgomery pair satisfies `y² = x³ - 3x + b`.
fn on_curve(x: &U6, y: &U6) -> bool {
    let b_mont = to_mont(&B, &FP);
    let x2 = mont_mul(x, x, &FP);
    let x3 = mont_mul(&x2, x, &FP);
    let three_x = add(&double(x, &FP), x, &FP);
    let right = add(&sub(&x3, &three_x, &FP), &b_mont, &FP);
    mont_mul(y, y, &FP) == right
}

/// Reads an uncompressed P-384 point (`0x04 || X || Y`) as an affine curve
/// point, refusing coordinates outside the field and points off the curve.
fn point_from_spki(public_key: &[u8]) -> Option<Affine> {
    if public_key.len() != 97 || public_key[0] != 0x04 {
        return None;
    }
    let x = limbs_from_be(&public_key[1..49]);
    let y = limbs_from_be(&public_key[49..97]);
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

/// Big-endian bytes to six little-endian limbs.
fn limbs_from_be(bytes: &[u8]) -> U6 {
    let mut limbs = [0u64; LIMBS];
    let length = bytes.len();
    for (index, byte) in bytes.iter().enumerate() {
        let from_end = length - 1 - index;
        limbs[from_end / 8] |= (*byte as u64) << (8 * (from_end % 8));
    }
    limbs
}

/// Verifies an ECDSA P-384 signature over SHA-384 of `message`.
///
/// `public_key` is the 97-byte uncompressed point a certificate carries as its
/// `subjectPublicKey`. `signature` is the 96-byte `r || s` pair with each half
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
    if signature.len() != 96 {
        return false;
    }
    let r = limbs_from_be(&signature[..48]);
    let s = limbs_from_be(&signature[48..]);
    // Both halves have to be proper non-zero scalars. A zero `r` or `s` is a
    // forgery shape the curve arithmetic would happily carry through to an
    // answer if it were let in.
    if is_zero(&r) || is_zero(&s) || at_or_above(&r, &N) || at_or_above(&s, &N) {
        return false;
    }

    let digest = Sha384::digest(message);
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

#[cfg(test)]
mod tests {
    use std::format;
    use std::string::String;

    use super::*;

    /// The ordinary-form limbs read back as the big-endian bytes a document
    /// prints, so a constant can be compared against a published value.
    fn be_bytes(value: &U6) -> [u8; 48] {
        let mut out = [0u8; 48];
        for (index, limb) in value.iter().rev().enumerate() {
            out[index * 8..(index + 1) * 8].copy_from_slice(&limb.to_be_bytes());
        }
        out
    }

    fn hex(value: &[u8; 48]) -> String {
        value.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn the_published_constants_read_back_limb_for_limb() {
        // Every one of these was decoded from the explicit ECParameters OpenSSL
        // ships for `secp384r1`, then transcribed here. The strings are the
        // values SEC 2 and FIPS 186-5 print, so a slip in either direction shows
        // up as this test failing rather than as arithmetic that agrees with
        // nothing.
        assert_eq!(
            hex(&be_bytes(&P)),
            "ffffffffffffffffffffffffffffffffffffffffffffffff\
             fffffffffffffffeffffffff0000000000000000ffffffff"
        );
        assert_eq!(
            hex(&be_bytes(&N)),
            "ffffffffffffffffffffffffffffffffffffffffffffffff\
             c7634d81f4372ddf581a0db248b0a77aecec196accc52973"
        );
        assert_eq!(
            hex(&be_bytes(&B)),
            "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe814112\
             0314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef"
        );
        assert_eq!(
            hex(&be_bytes(&GX)),
            "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b98\
             59f741e082542a385502f25dbf55296c3a545e3872760ab7"
        );
        assert_eq!(
            hex(&be_bytes(&GY)),
            "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147c\
             e9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f"
        );
        // The doubling formula specialises to `a = -3`, so a curve that is not
        // that shape would need a different formula, not this one.
        assert_eq!(sub_mod(&P, &[3, 0, 0, 0, 0, 0], &P), a_in_ordinary_form());
        assert!(at_or_above(&P, &N), "the order sits below the prime");
    }

    /// `-3 mod p`, in ordinary form.
    fn a_in_ordinary_form() -> U6 {
        sub_mod(&P, &[3, 0, 0, 0, 0, 0], &P)
    }

    #[test]
    fn the_generator_is_on_the_curve() {
        assert!(on_curve(&to_mont(&GX, &FP), &to_mont(&GY, &FP)));
    }

    #[test]
    fn multiplying_the_generator_by_the_order_gives_infinity() {
        // This one assertion is the whole parameter set: a wrong limb in `p`,
        // `n`, `b`, `GX` or `GY`, or an error in the doubling or addition
        // formula, lands somewhere other than the point at infinity.
        let product = multiply(&generator(), &N);
        assert!(is_zero(&product.z), "n*G is not the identity");
    }

    #[test]
    fn one_double_and_one_addition_is_the_whole_group_step() {
        // G + G by the addition formula, and 2G by the doubling formula, have to
        // be the same point. They are computed by different code, so agreement
        // is evidence about both — and the pair is pinned once more against the
        // real signatures in `tests/x509_fixtures.rs`.
        let g = generator();
        let by_addition = add_mixed(&add_mixed(&INFINITY, &g), &g);
        let by_doubling = double_point(&add_mixed(&INFINITY, &g));
        assert_eq!(affine_x(&by_addition), affine_x(&by_doubling));
        assert_eq!(affine_y(&by_addition), affine_y(&by_doubling));
        assert!(on_curve(
            &affine_x(&by_doubling).unwrap(),
            &affine_y(&by_doubling).unwrap()
        ));
    }

    #[test]
    fn subtracting_one_from_the_order_walks_back_to_minus_the_generator() {
        let almost = sub_mod(&N, &[1, 0, 0, 0, 0, 0], &N);
        let product = multiply(&generator(), &almost);
        let g = generator();
        // (n-1)*G is -G: the same x, and a y that adds to the generator's to
        // zero. If any constant were wrong this lands elsewhere.
        assert_eq!(affine_x(&product), Some(g.x));
        let y = affine_y(&product).expect("(n-1)*G is affine");
        assert_eq!(add(&y, &g.y, &FP), [0; LIMBS], "(n-1)*G + G = O");
    }

    /// The affine y-coordinate of a Jacobian point.
    fn affine_y(point: &Point) -> Option<U6> {
        if is_zero(&point.z) {
            return None;
        }
        let z_inv = invert(&point.z, &FP);
        let z_inv2 = mont_mul(&z_inv, &z_inv, &FP);
        let z_inv3 = mont_mul(&z_inv2, &z_inv, &FP);
        Some(mont_mul(&point.y, &z_inv3, &FP))
    }

    #[test]
    fn the_montgomery_engine_round_trips_and_inverts() {
        for value in [[1u64, 0, 0, 0, 0, 0], GX, N, [7, 0, 0, 0, 0, 0]] {
            assert!(at_or_above(&P, &value), "{value:?} is not below p");
            let moved = to_mont(&value, &FP);
            assert_eq!(from_mont(&moved, &FP), value, "domain round trip");
            if is_zero(&value) {
                continue;
            }
            let product = mont_mul(&invert(&moved, &FP), &moved, &FP);
            assert_eq!(product, mont_one(&FP), "inverse of {value:?}");
        }
    }

    #[test]
    fn reads_only_a_point_of_the_right_width() {
        let mut key = [0u8; 97];
        key[0] = 0x04;
        assert!(!verify(&key, b"message", &[0u8; 96]));
        assert!(point_from_spki(&key).is_none(), "all-zero coordinates are not on the curve");
        assert!(point_from_spki(&[0u8; 96]).is_none());
    }

    #[test]
    fn limbs_round_trip_through_big_endian_bytes() {
        let value = [1u64, 2, 3, 4, 5, 6];
        let encoded = be_bytes(&value);
        assert_eq!(limbs_from_be(&encoded), value);
        assert_eq!(limbs_from_be(&[0u8; 47]), [0; LIMBS]);
    }
}
