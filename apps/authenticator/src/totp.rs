//! RFC 6238 TOTP primitives for the InterOS Sovereign Authenticator.
//!
//! Self-contained so the code that runs on the device is the same code that was
//! checked against the RFC test vectors. Nothing here fabricates a code: every
//! digit comes out of an HMAC-SHA1 over a caller-supplied secret and a
//! caller-supplied Unix time step.

/// Decoded shared secret, capped at what a vault sector record can hold.
pub const MAX_SECRET: usize = 64;

/// Decodes an RFC 4648 base32 secret (the alphabet every authenticator app
/// prints as `otpauth://`).
///
/// `=` padding and whitespace are ignored; any other character outside the
/// base32 alphabet makes the whole decode fail, so a mistyped secret can never
/// be silently accepted as a different one.
pub fn base32_decode(input: &[u8], out: &mut [u8; MAX_SECRET]) -> Result<usize, ()> {
    let mut acc: u32 = 0;
    let mut bits: u32 = 0;
    let mut n = 0usize;
    for &c in input {
        if c == b'=' || c == b' ' || c == b'-' || c == b'\n' || c == b'\r' || c == b'\t' {
            continue;
        }
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a',
            b'2'..=b'7' => c - b'2' + 26,
            _ => return Err(()),
        } as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            if n >= MAX_SECRET {
                return Err(());
            }
            out[n] = ((acc >> bits) & 0xFF) as u8;
            n += 1;
        }
    }
    // A whole number of bytes, or at most 4 leftover bits that are padding.
    if bits >= 5 {
        return Err(());
    }
    if n == 0 {
        return Err(());
    }
    Ok(n)
}

/// SHA-1 (FIPS 180-4) over one message, written as eight big-endian words.
fn sha1(msg: &[u8], out: &mut [u32; 5]) {
    let bit_len = (msg.len() as u64).wrapping_mul(8);

    let mut padded = [0u8; 128];
    let mut padded_len = msg.len() + 1;
    while padded_len % 64 != 56 {
        padded_len += 1;
    }
    if padded_len > padded.len() {
        // A TOTP counter block is 8 bytes and a key is at most 64, so this
        // branch is unreachable for every caller; fail closed rather than
        // truncate.
        *out = [0; 5];
        return;
    }
    // `padded_len` is chosen so the 8 length bytes end exactly on a block
    // boundary: the length occupies bytes 56..64 of the final block.
    let length_at = padded_len;
    padded[..msg.len()].copy_from_slice(msg);
    padded[msg.len()] = 0x80;
    padded[length_at..length_at + 8].copy_from_slice(&bit_len.to_be_bytes());
    let blocks = (length_at + 8) / 64;

    let (mut h0, mut h1, mut h2, mut h3, mut h4) =
        (0x67452301u32, 0xEFCDAB89u32, 0x98BADCFEu32, 0x10325476u32, 0xC3D2E1F0u32);

    for block in padded[..blocks * 64].chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            *word = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    out[0] = h0;
    out[1] = h1;
    out[2] = h2;
    out[3] = h3;
    out[4] = h4;
}

/// HMAC-SHA1 (RFC 2104) over `msg` with `key`, written as 20 big-endian bytes.
pub fn hmac_sha1(key: &[u8], msg: &[u8], out: &mut [u8; 20]) {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        let mut digest = [0u32; 5];
        sha1(key, &mut digest);
        for i in 0..5 {
            block[i * 4..i * 4 + 4].copy_from_slice(&digest[i].to_be_bytes());
        }
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5Cu8; 64];
    for i in 0..64 {
        ipad[i] ^= block[i];
        opad[i] ^= block[i];
    }

    let mut inner = [0u8; 64 + 128];
    let inner_len = 64 + msg.len();
    inner[..64].copy_from_slice(&ipad);
    inner[64..inner_len].copy_from_slice(msg);
    let mut inner_digest = [0u32; 5];
    sha1(&inner[..inner_len], &mut inner_digest);

    let mut outer = [0u8; 64 + 20];
    outer[..64].copy_from_slice(&opad);
    for i in 0..5 {
        outer[64 + i * 4..64 + i * 4 + 4].copy_from_slice(&inner_digest[i].to_be_bytes());
    }
    let mut outer_digest = [0u32; 5];
    sha1(&outer, &mut outer_digest);

    for i in 0..5 {
        out[i * 4..i * 4 + 4].copy_from_slice(&outer_digest[i].to_be_bytes());
    }
}

/// Computes an RFC 6238 TOTP for `secret` at Unix time `unix_secs`, with a
/// `digits`-digit code. `digits` must be 6 or 8.
///
/// Returns the code, or `None` if the parameters are outside the RFC's range.
pub fn totp(secret: &[u8], unix_secs: i64, step_secs: u64, digits: u8) -> Option<u32> {
    if secret.is_empty() || (digits != 6 && digits != 8) || step_secs == 0 {
        return None;
    }
    if unix_secs < 0 {
        return None;
    }
    let counter = (unix_secs as u64) / step_secs;
    let mut msg = [0u8; 8];
    msg.copy_from_slice(&counter.to_be_bytes());

    let mut mac = [0u8; 20];
    hmac_sha1(secret, &msg, &mut mac);

    let offset = (mac[19] & 0x0F) as usize;
    let bin = ((mac[offset] as u32 & 0x7F) << 24)
        | ((mac[offset + 1] as u32) << 16)
        | ((mac[offset + 2] as u32) << 8)
        | (mac[offset + 3] as u32);
    let modulus = if digits == 6 { 1_000_000u32 } else { 100_000_000u32 };
    Some(bin % modulus)
}

/// Seconds remaining in the current TOTP step. Derived from the same Unix time
/// the code was computed from, so the countdown and the code always agree.
pub fn seconds_remaining(unix_secs: i64, step_secs: u64) -> u8 {
    if step_secs == 0 || unix_secs < 0 {
        return 0;
    }
    let rem = (unix_secs as u64) % step_secs;
    (step_secs - rem) as u8
}
