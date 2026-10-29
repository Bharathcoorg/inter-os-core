//! Pure `no_std` RSA signature verification (RFC 8017) over a bounded modulus.
//!
//! Provides the two paddings a public HTTPS chain actually presents: PKCS#1
//! v1.5 with SHA-256 (`sha256WithRSAEncryption` on a certificate, and the TLS
//! 1.3 `rsa_pkcs1_sha256` code point) and RSA-PSS with MGF1-SHA256 (the TLS
//! 1.3 `rsa_pss_rsae_sha256` code point). Verification only — this device
//! holds no RSA private key and never signs.
//!
//! Zero third-party dependencies, zero heap allocations. The arithmetic is
//! sized by the modulus actually presented, up to [`RSA_MAX_LIMBS`], so a
//! 2048-bit leaf costs a quarter of what a 4096-bit root costs rather than
//! paying for the ceiling on every certificate.

use super::sha256::{Sha256, SHA256_DIGEST_LENGTH};

/// Limbs in the largest modulus this module will verify: 4096-bit.
///
/// WHY 4096: the RSA roots still installed in public trust stores — ISRG Root
/// X1 among them — are 4096-bit. A ceiling below that refuses an anchor whose
/// own key is longer than the verifier can hold, which reaches the user as
/// "this site is not trusted" for a site every other browser can reach.
pub const RSA_MAX_LIMBS: usize = 64;

/// Smallest modulus accepted: 2048-bit.
///
/// WHY a floor: RSA below 2048 bits is factorable with work an ordinary
/// attacker can buy, and public CAs stopped issuing it. A short modulus
/// arriving on the wire is therefore either a mis-issued certificate or a
/// deliberate truncation, and neither deserves a verified channel.
pub const RSA_MIN_LIMBS: usize = 32;

/// The 19 bytes that precede a SHA-256 digest inside a PKCS#1 v1.5
/// `DigestInfo` (RFC 8017 Section 9.2, Note 1).
const SHA256_DIGEST_INFO_PREFIX: [u8; 19] = [
    0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05,
    0x00, 0x04, 0x20,
];

/// The trailing byte of an encoded PSS block (RFC 8017 Section 9.1.1).
const PSS_TRAILER: u8 = 0xbc;

/// The salt length TLS 1.3 signers conventionally use: as long as the digest.
const PSS_DEFAULT_SALT_LEN: usize = SHA256_DIGEST_LENGTH;

/// A modulus with the width its own value needs, not the module's ceiling.
struct Modulus {
    /// Active limbs — `value[..limbs]` is the modulus.
    limbs: usize,
    value: [u64; RSA_MAX_LIMBS],
    /// Significant byte length of the modulus, which is the length of an
    /// encoded message.
    bytes: usize,
    /// Bit length of the modulus, which fixes how many leftmost bits of a PSS
    /// block carry no information.
    bits: u32,
}

impl Modulus {
    fn as_limbs(&self) -> &[u64] {
        &self.value[..self.limbs]
    }
}

/// Compares two equal-length limb sequences, most-significant limb first.
fn compare(a: &[u64], b: &[u64]) -> core::cmp::Ordering {
    let mut index = a.len();
    while index > 0 {
        index -= 1;
        if a[index] != b[index] {
            return if a[index] > b[index] {
                core::cmp::Ordering::Greater
            } else {
                core::cmp::Ordering::Less
            };
        }
    }
    core::cmp::Ordering::Equal
}

/// `a -= b` over equal-length limbs, returning the borrow out of the top limb.
fn subtract(a: &mut [u64], b: &[u64]) -> u64 {
    let mut borrow = 0u64;
    for (slot, subtrahend) in a.iter_mut().zip(b.iter()) {
        let (difference, underflow) = slot.overflowing_sub(*subtrahend);
        let (difference, underflow2) = difference.overflowing_sub(borrow);
        *slot = difference;
        borrow = underflow as u64 + underflow2 as u64;
    }
    borrow
}

/// Reads a big-endian unsigned integer into limbs, reporting the width its
/// value actually needs. `None` for zero and for anything wider than the
/// module can hold.
fn parse_be(bytes: &[u8]) -> Option<([u64; RSA_MAX_LIMBS], usize, usize)> {
    let first_significant = bytes.iter().position(|&byte| byte != 0)?;
    let value_bytes = &bytes[first_significant..];
    if value_bytes.len() > RSA_MAX_LIMBS * 8 {
        return None;
    }
    let limbs = (value_bytes.len() + 7) / 8;
    let mut value = [0u64; RSA_MAX_LIMBS];
    for (offset, byte) in value_bytes.iter().rev().enumerate() {
        value[offset / 8] |= (*byte as u64) << (8 * (offset % 8));
    }
    Some((value, limbs, value_bytes.len()))
}

/// Writes the low `out.len()` bytes of a value as big-endian bytes.
fn write_be(value: &[u64], out: &mut [u8]) {
    let length = out.len();
    for (index, byte) in out.iter_mut().enumerate() {
        let from_end = length - 1 - index;
        *byte = (value[from_end / 8] >> (8 * (from_end % 8))) as u8;
    }
}

/// Schoolbook multiply into the double-width buffer `product`, whose length is
/// twice the operand width.
fn multiply_wide(a: &[u64], b: &[u64], product: &mut [u64]) {
    let limbs = a.len();
    for i in 0..limbs {
        if a[i] == 0 {
            continue;
        }
        let mut carry = 0u128;
        for j in 0..limbs {
            let cell = product[i + j] as u128 + (a[i] as u128) * (b[j] as u128) + carry;
            product[i + j] = cell as u64;
            carry = cell >> 64;
        }
        let mut index = i + limbs;
        let mut overflow = carry as u64;
        while overflow != 0 && index < product.len() {
            let cell = product[index] as u128 + overflow as u128;
            product[index] = cell as u64;
            overflow = (cell >> 64) as u64;
            index += 1;
        }
    }
}

/// Reduces a double-width product modulo `n`.
///
/// WHY restoring division rather than Montgomery or a fast reduction: the
/// modulus here is a number the *peer* chose, so no algebraic shortcut applies
/// to it. One shift-and-subtract loop over the bits of the product is the only
/// step that is correct for every modulus, and at the 4096-bit ceiling it
/// costs about half a million word operations — well inside what a handshake
/// can spend.
fn reduce_mod(product: &[u64], n: &Modulus) -> [u64; RSA_MAX_LIMBS] {
    let limbs = n.limbs;
    let mut top_bit = product.len() * 64;
    while top_bit > 0 {
        if (product[(top_bit - 1) / 64] >> ((top_bit - 1) % 64)) & 1 == 1 {
            break;
        }
        top_bit -= 1;
    }

    // The loop invariant keeps the remainder below 2n, so one spare limb above
    // the modulus's own width is enough to hold what shifts out.
    let mut remainder = [0u64; RSA_MAX_LIMBS + 1];
    for bit in (0..top_bit).rev() {
        let mut carry = (product[bit / 64] >> (bit % 64)) & 1;
        for slot in remainder.iter_mut().take(limbs + 1) {
            let next_carry = *slot >> 63;
            *slot = (*slot << 1) | carry;
            carry = next_carry;
        }
        let spare = remainder[limbs];
        let head = &mut remainder[..limbs];
        let at_or_above = spare != 0 || compare(head, n.as_limbs()).is_ge();
        let mut borrowed = 0u64;
        if at_or_above {
            // A borrow out of the subtraction can only land in the spare limb,
            // and only when that limb held a one — which is exactly the case
            // `at_or_above` reached with `head < n`.
            borrowed = subtract(head, n.as_limbs());
        }
        if borrowed != 0 {
            remainder[limbs] -= 1;
        }
    }

    let mut reduced = [0u64; RSA_MAX_LIMBS];
    reduced[..limbs].copy_from_slice(&remainder[..limbs]);
    reduced
}

/// `a * b mod n`.
fn multiply_mod(a: &[u64], b: &[u64], n: &Modulus) -> [u64; RSA_MAX_LIMBS] {
    let limbs = n.limbs;
    let mut product = [0u64; RSA_MAX_LIMBS * 2];
    multiply_wide(&a[..limbs], &b[..limbs], &mut product[..limbs * 2]);
    reduce_mod(&product[..limbs * 2], n)
}

/// `base ** exponent mod n` by square-and-multiply over the exponent's bits.
fn power_mod(base: &[u64], exponent: u32, n: &Modulus) -> [u64; RSA_MAX_LIMBS] {
    let mut result = [0u64; RSA_MAX_LIMBS];
    result[0] = 1;
    if exponent == 0 {
        return result;
    }
    let top = 31 - exponent.leading_zeros();
    for bit in (0..=top).rev() {
        result = multiply_mod(&result, &result, n);
        if (exponent >> bit) & 1 == 1 {
            result = multiply_mod(&result, base, n);
        }
    }
    result
}

/// Recovers the encoded message from an RSA signature.
///
/// `None` for anything the arithmetic itself refuses: a modulus outside the
/// accepted size range, an exponent this module will not spend the time on, a
/// signature that is not a number below the modulus, or a signature longer
/// than the modulus can encode.
fn recover(
    n_bytes: &[u8],
    e_bytes: &[u8],
    signature: &[u8],
) -> Option<([u64; RSA_MAX_LIMBS], Modulus)> {
    let (value, limbs, bytes) = parse_be(n_bytes)?;
    if limbs < RSA_MIN_LIMBS {
        return None;
    }
    // Counted in limbs: `(bytes - 1) * 8 + 64` over-counts a modulus whose top
    // limb is full, and the PSS block's leftmost-bit mask is the difference.
    let bits = (limbs as u32 - 1) * 64 + (64 - value[limbs - 1].leading_zeros());
    let modulus = Modulus {
        limbs,
        value,
        bytes,
        bits,
    };

    let (exponent, exponent_limbs, _) = parse_be(e_bytes)?;
    // WHY a 32-bit public exponent: every certificate a public CA issues uses
    // 3 or 65537, and the exponentiation cost grows with the exponent's own
    // bit count. Accepting a peer-chosen 2048-bit exponent would let a hostile
    // server multiply this device's work by a hundred for a signature that
    // then fails anyway.
    if exponent_limbs != 1
        || exponent[0] < 3
        || exponent[0] % 2 == 0
        || exponent[0] > u64::from(u32::MAX)
    {
        return None;
    }

    if signature.len() > modulus.bytes {
        return None;
    }
    let (signature_value, signature_limbs, _) = parse_be(signature)?;
    if signature_limbs > modulus.limbs
        || compare(&signature_value[..signature_limbs], modulus.as_limbs()).is_ge()
    {
        return None;
    }

    let recovered = power_mod(&signature_value, exponent[0] as u32, &modulus);
    Some((recovered, modulus))
}

/// Verifies an RSASSA-PKCS1-v1_5 signature over SHA-256 (RFC 8017 Section 8.2.2).
///
/// `modulus` and `exponent` are the big-endian unsigned integers exactly as
/// the certificate carries them, and `message` is hashed here. True only when
/// the recovered block is byte-for-byte the padding this scheme defines for a
/// SHA-256 digest.
pub fn verify_pkcs1_sha256(
    modulus: &[u8],
    exponent: &[u8],
    signature: &[u8],
    message: &[u8],
) -> bool {
    let Some((recovered, key)) = recover(modulus, exponent, signature) else {
        return false;
    };
    let mut buffer = [0u8; RSA_MAX_LIMBS * 8];
    let encoded = &mut buffer[..key.bytes];
    write_be(&recovered, encoded);

    let digest = Sha256::digest(message);
    let digest_info_len = SHA256_DIGEST_INFO_PREFIX.len() + digest.len();
    // 0x00 0x01, at least eight 0xFF, a separator, and the DigestInfo.
    if encoded.len() < digest_info_len + 11 {
        return false;
    }
    if encoded[0] != 0x00 || encoded[1] != 0x01 {
        return false;
    }
    // The block is 0x00, 0x01, the padding, one 0x00 separator, the DigestInfo:
    // three fixed bytes on top of the padding, not two.
    let padding_len = encoded.len() - digest_info_len - 3;
    if encoded[2..2 + padding_len].iter().any(|&byte| byte != 0xff) {
        return false;
    }
    if encoded[2 + padding_len] != 0x00 {
        return false;
    }
    let digest_info = &encoded[3 + padding_len..];
    if digest_info[..SHA256_DIGEST_INFO_PREFIX.len()] != SHA256_DIGEST_INFO_PREFIX {
        return false;
    }
    ct_equal(&digest_info[SHA256_DIGEST_INFO_PREFIX.len()..], &digest)
}

/// Verifies an RSASSA-PSS signature over SHA-256 with MGF1-SHA256
/// (RFC 8017 Section 8.1.2) for either conventional salt length.
///
/// WHY two salt lengths are tried: a signature does not carry its salt length,
/// it is only agreed out of band. Signers following the TLS 1.3 convention use
/// a salt as long as the digest; signers reading RFC 8017's "the salt fills
/// what is left" use the remaining room. Refusing the second rejects real
/// certificates over a choice the specification leaves open, while trying
/// arbitrary lengths would let a peer steer how much padding this code parses.
pub fn verify_pss_sha256(
    modulus: &[u8],
    exponent: &[u8],
    signature: &[u8],
    message: &[u8],
) -> bool {
    let Some((recovered, key)) = recover(modulus, exponent, signature) else {
        return false;
    };
    let mut buffer = [0u8; RSA_MAX_LIMBS * 8];
    let encoded = &mut buffer[..key.bytes];
    write_be(&recovered, encoded);

    let digest = Sha256::digest(message);
    let em_len = encoded.len();
    if em_len < SHA256_DIGEST_LENGTH + PSS_DEFAULT_SALT_LEN + 2 {
        return false;
    }
    // emBits is modBits - 1 (RFC 8017 Section 9.1.1), so the leftmost bits
    // beyond it are the ones the padding must leave zero.
    let leftmost_unused = em_len as u32 * 8 - (key.bits - 1);
    let full_salt_room = em_len - 2 * SHA256_DIGEST_LENGTH - 2;
    if verify_pss_encoded(encoded, &digest, PSS_DEFAULT_SALT_LEN, leftmost_unused) {
        return true;
    }
    full_salt_room != PSS_DEFAULT_SALT_LEN
        && verify_pss_encoded(encoded, &digest, full_salt_room, leftmost_unused)
}

/// EMSA-PSS-VERIFY for one named salt length (RFC 8017 Section 9.1.2).
fn verify_pss_encoded(
    encoded: &[u8],
    digest: &[u8],
    salt_len: usize,
    leftmost_unused: u32,
) -> bool {
    let hash_len = SHA256_DIGEST_LENGTH;
    if encoded.len() < hash_len + salt_len + 2 {
        return false;
    }
    if encoded[encoded.len() - 1] != PSS_TRAILER {
        return false;
    }

    // The block is maskedDB, then the hash of the padded message, then 0xbc.
    let masked_len = encoded.len() - hash_len - 1;
    let (masked, tail) = encoded.split_at(masked_len);
    let recovered_hash = &tail[..hash_len];

    let mut mask = [0u8; RSA_MAX_LIMBS * 8];
    let mask = &mut mask[..masked_len];
    // MGF1 runs on the recovered hash, not on the masked bytes: seeding it with
    // the masked data block unmask to something that looks like padding and is
    // not, and no amount of checking downstream notices.
    mgf1_sha256(recovered_hash, mask);

    let mut block = [0u8; RSA_MAX_LIMBS * 8];
    let data_block = &mut block[..masked_len];
    for index in 0..masked_len {
        data_block[index] = masked[index] ^ mask[index];
    }
    clear_left_bits(data_block, leftmost_unused);

    // maskedDB is zero padding, one 0x01 separator, then the salt.
    let padding_len = data_block.len() - salt_len - 1;
    if data_block[..padding_len].iter().any(|&byte| byte != 0x00) {
        return false;
    }
    if data_block[padding_len] != 0x01 {
        return false;
    }
    let salt = &data_block[padding_len + 1..];

    let mut hasher = Sha256::new();
    hasher.update(&[0u8; 8]);
    hasher.update(digest);
    hasher.update(salt);
    ct_equal(&hasher.finalize(), recovered_hash)
}

/// Clears the `count` leftmost bits of a big-endian buffer.
fn clear_left_bits(buffer: &mut [u8], count: u32) {
    let mut remaining = count;
    let mut index = 0usize;
    while index < buffer.len() && remaining >= 8 {
        buffer[index] = 0;
        remaining -= 8;
        index += 1;
    }
    if index < buffer.len() && remaining > 0 {
        buffer[index] &= 0xff >> remaining;
    }
}

/// MGF1 with SHA-256 as its hash, over a caller-named length
/// (RFC 8017 Section B.2.1).
fn mgf1_sha256(seed: &[u8], out: &mut [u8]) {
    let mut produced = 0usize;
    let mut counter = 0u32;
    while produced < out.len() {
        let mut hasher = Sha256::new();
        hasher.update(seed);
        hasher.update(&counter.to_be_bytes());
        let block = hasher.finalize();
        let take = (out.len() - produced).min(block.len());
        out[produced..produced + take].copy_from_slice(&block[..take]);
        produced += take;
        counter += 1;
    }
}

/// Equality that does not branch on the first differing byte.
fn ct_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0u8;
    for index in 0..a.len() {
        difference |= a[index] ^ b[index];
    }
    difference == 0
}
